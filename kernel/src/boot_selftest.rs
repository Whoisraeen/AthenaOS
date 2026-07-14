//! Deferred boot self-test sweep (ADR 0006 — boot-time gate).
//!
//! AthenaOS_Concept §"Fast is a feature": boot under 6 s (target 3 s). The boot
//! critical path is dozens of synchronous correctness smoketests run serially
//! on CPU0 *before* the "System successfully booted." marker. Many of those are
//! userspace-feature correctness checks (theme/vibe/wallpaper/RGB/widgets/
//! search/game-profile/wireguard) that prove a subsystem works but gate nothing
//! the OS needs to be considered "up" — they do not feed the boot-health 7/7
//! aggregation and they spawn no thread the kernel depends on.
//!
//! This module batches that clearly-non-critical set into [`run_deferred`],
//! called AFTER the success marker so the marker (and the 6 s gate measured at
//! `record_boot_complete()`) no longer wait on them. Every test STILL prints
//! its own `-> PASS/FAIL` (R10 rule 16: a test that cannot FAIL is a false
//! green) — the only change is *when* it runs, not *whether* it can fail.
//!
//! The deferred set was extended (boot-time live-fix #1) to also carry the
//! heavy *pure-compute* feature smoketests that dominated the old
//! `[tier1-prof] modules=5574ms` bucket and the Tier-7/8 smoketest blocks:
//! notify (the ~24-surface toast storm), wm_policy / window_chrome / login_ui /
//! athshell-terminal, aer, and athbridge. **Two safety gates govern what may
//! be deferred:**
//!  1. *No real init.* Smoketests that double as init (xHCI HID arm/drain,
//!     watchdogs, thread spawns, the athgfx text-engine build, shell_runner's
//!     embedded control_panel::init) stay inline — deferring them bricks boot
//!     or breaks devices.
//!  2. *No masked-context perturbation.* The sweep runs inside the marker's
//!     `without_interrupts` block after BOOT_COMPLETE; a deferred test that
//!     touches block I/O, RAM disks, real IPC channels, or the swap/soak/
//!     installer paths lets CPU0 get preempted into the runqueue mid-sweep (the
//!     HID drain thread was observed scheduling right after a deferred `swap`
//!     test, after which the sweep continuation starved → `[PANIC] no next
//!     task`). So oom / swap / soak / installer(×6) / virtio_gpu stay inline.
//! edid + compress also stay inline (they feed the boot-health `smoketests`
//! aggregate that selftest::run() computes before this sweep).
//!
//! Reversal (ADR 0006 "How to reverse"): move any call here back ahead of the
//! marker in `kernel_main`.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Set once `run_deferred` has executed, so a second invocation is a no-op
/// (the sweep is idempotent — the underlying smoketests already are).
static RAN: AtomicBool = AtomicBool::new(false);
/// Number of deferred smoketests dispatched on the last sweep — surfaced via
/// `/proc/athena/boot` so an operator can confirm the sweep ran.
static LAST_COUNT: AtomicU32 = AtomicU32::new(0);
/// Set by [`init`] to confirm the deferred-sweep orchestration was armed on the
/// critical path before any feature smoketest could run early.
static ARMED: AtomicBool = AtomicBool::new(false);

/// Arm the deferred-sweep orchestration (ADR 0006). Called from `kernel_main`
/// on the boot critical path, BEFORE the success marker, alongside the other
/// module `init()`s. Kept deliberately lightweight (a couple of atomic stores +
/// one log line) so it never moves the 6 s boot-time gate; the *heavy* feature
/// smoketests it governs run later via [`run_deferred`], post-marker.
///
/// Concretely it resets the sweep state to a known-armed baseline (`RAN=false`,
/// `LAST_COUNT=0`) and records that the orchestration is live, so the
/// critical-path [`run_boot_smoketest`] can assert the gate is armed and
/// `/proc/athena/boot` reports a coherent pre-sweep state.
pub fn init() {
    RAN.store(false, Ordering::SeqCst);
    LAST_COUNT.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    crate::serial_println!("[boot-selftest] init: deferred-sweep orchestration armed (ADR 0006)");
}

/// Run the deferred, non-critical feature smoketests. Call ONCE, after the
/// "System successfully booted." marker has printed (so it never gates the
/// boot-time measurement) and after `BOOT_COMPLETE` (CPU0 preemptible).
///
/// Each entry calls the subsystem's existing `run_boot_smoketest()` — which
/// emits its own `-> PASS/FAIL` line — exactly as it did inline pre-ADR-0006.
/// The deferral changes ordering only; it preserves every assertion.
pub fn run_deferred() {
    if RAN.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::serial_println!("[boot-selftest] deferred sweep START (post-marker, ADR 0006)");
    let mut count: u32 = 0;

    // ── Tier 1 deferred: desktop widgets (Rainmeter-equivalent) ──────────
    crate::widgets::run_boot_smoketest();
    count += 1;

    // ── Tier 1 deferred: pure UI-correctness smoketests (no init; the
    //    subsystem init()s already ran on the critical path). Each verified
    //    pure-test (creates+tears down its own surfaces / restores any state
    //    it flips) before deferral — they gate no boot-health check and spawn
    //    no thread the OS depends on. notify's smoketest is the single largest
    //    line-item in the old `modules=5574ms` bucket (~24 compositor surfaces
    //    posted+expired). ──────────────────────────────────────────────────
    crate::notify::run_boot_smoketest(); // posts synthetic toasts then expires all (visible==0)
    count += 1;
    crate::wm_policy::run_boot_smoketest(); // creates 3 surfaces, applies tile, restores MODE, closes
    count += 1;
    crate::window_chrome::run_boot_smoketest(); // token/glyph reads only
    count += 1;
    crate::login_ui::run_boot_smoketest(); // token reads only
    count += 1;
    {
        // athshell VT100/ANSI terminal parser coverage (pure decode test).
        let (text, cup, sgr, ed) = athshell::terminal::run_smoketest();
        let pass = text && cup && sgr && ed;
        crate::serial_println!(
            "[athshell] terminal smoketest: text={} cup={} sgr={} erase={} -> {}",
            text,
            cup,
            sgr,
            ed,
            if pass { "PASS" } else { "FAIL" },
        );
    }
    count += 1;
    {
        // AthMind affect engine (Layer A) P1 proof — pure compute (decay/
        // event update law + bounds invariants), allocates only the dump
        // line; no init, no I/O, so it is safe for the post-marker masked
        // sweep per the ADR 0006 gates above. xtask's post-boot drain keeps
        // the VM alive past the marker, so this line lands in the CI serial
        // log (spec docs/superpowers/specs/2026-07-14-athena-affect-arc-design.md §8 P1).
        let (pass, line) = athmind::affect::run_smoketest();
        crate::serial_println!("{} -> {}", line, if pass { "PASS" } else { "FAIL" });
    }
    count += 1;

    // ── Tier 7 deferred: aer (pure counter read). ────────────────────────
    crate::aer::run_boot_smoketest(); // reads registration counters
    count += 1;

    // SWEEP-CONTEXT SAFETY (observed 2026-06-22): run_deferred executes inside
    // the marker's `without_interrupts` block AFTER BOOT_COMPLETE. Any deferred
    // test that touches block I/O, RAM disks, real IPC channels, or the swap/
    // soak/installer paths perturbs that masked context and lets CPU0 be
    // preempted into the runqueue mid-sweep (the HID drain thread was observed
    // scheduling right after the `swap` smoketest, after which the sweep
    // continuation starved → `[PANIC] no next task`). So oom / swap / soak /
    // installer(×6) / virtio_gpu are deliberately LEFT inline on the critical
    // path — only pure-compute / token / surface / parser smoketests (which do
    // not perturb the masked context) are deferred. edid + compress also stay
    // inline (they feed the boot-health `smoketests` aggregate that
    // selftest::run() computes before this sweep).

    // ── Tier 8 deferred: pure feature-correctness smoketests. ────────────
    crate::athbridge_boot::run_boot_smoketest(); // throwaway DLL registry + embedded PE parse
    count += 1;

    // ── Tier 8 deferred: userspace-feature correctness ───────────────────
    crate::search_index::run_boot_smoketest();
    count += 1;
    crate::game_profile::run_boot_smoketest();
    count += 1;
    crate::rgb::run_boot_smoketest();
    count += 1;
    crate::theme_engine::run_boot_smoketest();
    count += 1;
    crate::wireguard::run_boot_smoketest();
    count += 1;
    crate::live_wallpaper::run_boot_smoketest();
    count += 1;
    crate::vibe_mode::run_boot_smoketest();
    count += 1;
    // Dynamic-cohesion proofs (one live accent re-skins every surface; the
    // SYS_THEME_GET value separate apps read equals the live accent). These
    // depend on theme_engine + vibe_mode being initialized, which happens on
    // the critical path; only the *check* is deferred.
    crate::theme_engine::run_accent_cohesion_smoketest();
    count += 1;
    crate::theme_engine::run_theme_get_smoketest();
    count += 1;
    crate::theme_engine::run_register_smoketest();
    count += 1;
    crate::fast_boot::run_splash_smoketest();
    count += 1;

    // ── Phase 2.4: S3 suspend→resume ROUND-TRIP — deliberately LAST. ─────
    // Hypervisor-gated (bare metal: prints skipped, never sleeps). It
    // INIT-parks the APs (they stay offline afterwards — matching post-boot
    // reality, where every service thread is BSP-pinned) and runs entirely
    // inside this masked sweep context: the RTC wake is a platform-level
    // event, not a serviced IRQ, and the LAPIC-timer liveness check reads
    // TMCCT instead of waiting for a tick — so the sweep-context safety
    // rules above hold. If this test ever wedges, everything before it has
    // already printed, and the boot marker landed pre-sweep.
    crate::suspend::run_s3_roundtrip_smoketest();
    count += 1;

    LAST_COUNT.store(count, Ordering::Relaxed);
    crate::serial_println!(
        "[boot-selftest] deferred sweep DONE: {} feature smoketests run post-marker",
        count
    );
}

/// True once the deferred sweep has completed (for `/proc/athena/boot`).
pub fn ran() -> bool {
    RAN.load(Ordering::Relaxed)
}

/// Number of deferred smoketests dispatched (0 until the sweep runs).
pub fn deferred_count() -> u32 {
    LAST_COUNT.load(Ordering::Relaxed)
}

/// Boot smoketest for the deferred-sweep machinery itself. Proves the registry
/// is wired and prints PASS/FAIL (R10). Runs on the critical path (cheap: a
/// couple of atomic loads + one log line) so the 7/7 health line can see it;
/// the *heavy* feature smoketests it schedules run later via `run_deferred`.
pub fn run_boot_smoketest() {
    // Before the sweep runs, init() must have armed the orchestration and the
    // sweep must not have executed yet (ran()==false, count==0) — proves the
    // gate is armed and nothing accidentally ran the deferred set early.
    let armed = ARMED.load(Ordering::Relaxed) && !ran() && deferred_count() == 0;
    crate::serial_println!(
        "[boot-selftest] smoketest: deferred-sweep armed={} (runs post-marker) -> {}",
        armed,
        if armed { "PASS" } else { "FAIL" }
    );
    crate::selftest::record_smoketest("boot_selftest", armed);
}
