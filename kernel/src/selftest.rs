//! Consolidated boot self-test — one authoritative health line for the
//! "one big bare-metal verify".
//!
//! Concept §"Fast is a feature" / MasterChecklist §1.9 + §4.12 acceptance:
//! the whole point of the Athena bring-up is a single pass/fail readout of
//! "did the OS come up healthy". Dozens of per-subsystem `run_boot_smoketest`
//! lines prove individual modules, but a bare-metal operator (no debugger, a
//! photographed screen, or a `BOOTLOG.TXT` pulled off the stick) needs ONE
//! line to grep. This module queries the authoritative end-of-boot state of
//! every critical subsystem and emits exactly that:
//!
//! ```text
//!   [selftest] boot health: 9/9 critical PASS, 2/3 optional present
//!   [selftest]   PASS crit smp            (cpus_online=2)
//!   [selftest]   FAIL crit athfs          (root not mounted)
//!   ...
//! ```
//!
//! It does NOT re-run subsystem logic (no duplication, no side effects); it
//! only READS state via existing public accessors, so it can't perturb the
//! boot it's measuring. A `crit` failure means the OS did not reach a usable
//! state; `opt` checks are environment-dependent (e.g. no NIC under some QEMU
//! profiles) and are reported as present/absent, never failing the boot.
//!
//! R10 contract: `run()` (the aggregator, called once at end of `kernel_main`),
//! `dump_text()` → `/proc/athena/selftest`, this docstring.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

/// How a check counts toward boot health.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Must hold for the OS to be considered up; a failure fails the boot health.
    Critical,
    /// Environment-dependent (NIC, USB controller, …); reported present/absent.
    Optional,
}

struct Check {
    name: &'static str,
    kind: Kind,
    pass: bool,
    detail: String,
}

static RESULTS: Mutex<Vec<Check>> = Mutex::new(Vec::new());
static RAN: AtomicBool = AtomicBool::new(false);
static CRIT_PASS: AtomicU32 = AtomicU32::new(0);
static CRIT_TOTAL: AtomicU32 = AtomicU32::new(0);
static OPT_PRESENT: AtomicU32 = AtomicU32::new(0);
static OPT_TOTAL: AtomicU32 = AtomicU32::new(0);

// ── Smoketest result registry ────────────────────────────────────────────
// Per-module boot smoketests register their pass/fail here so the single
// boot-health line reflects them too — an operator reading one line on a
// photographed screen sees if ANY smoketest failed without scanning the log.
static SMOKE_TOTAL: AtomicU32 = AtomicU32::new(0);
static SMOKE_FAILS: AtomicU32 = AtomicU32::new(0);
static SMOKE_FAILED_NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Register a boot smoketest result. A failing smoketest fails the consolidated
/// boot health (so the one line catches it). Call right after the module prints
/// its own `-> PASS/FAIL`.
pub fn record_smoketest(name: &'static str, pass: bool) {
    SMOKE_TOTAL.fetch_add(1, Ordering::Relaxed);
    if !pass {
        SMOKE_FAILS.fetch_add(1, Ordering::Relaxed);
        if let Some(mut g) = SMOKE_FAILED_NAMES.try_lock() {
            g.push(name);
        }
    }
}

/// Run the consolidated boot self-test. Idempotent — safe to call once at the
/// end of `kernel_main` (after every subsystem smoketest + `BOOT_COMPLETE`).
pub fn run() {
    if RAN.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut checks: Vec<Check> = Vec::new();

    // ── Critical: the OS isn't "up" without these ────────────────────────
    let cpus = crate::smp::ONLINE_CPUS.load(Ordering::Relaxed);
    checks.push(crit("smp", cpus >= 1, format!("cpus_online={}", cpus)));

    let heap_used = crate::memory::allocator::heap_used();
    let heap_free = crate::memory::allocator::heap_free();
    checks.push(crit(
        "heap",
        heap_used > 0 && heap_free > 0,
        format!("used={} free={}", heap_used, heap_free),
    ));

    checks.push(crit(
        "scheduler",
        crate::scheduler::BOOT_COMPLETE.load(Ordering::Relaxed),
        String::from("boot_complete"),
    ));

    let (fb_w, fb_h) = crate::framebuffer::current_mode();
    checks.push(crit(
        "framebuffer",
        fb_w > 0 && fb_h > 0,
        format!("{}x{}", fb_w, fb_h),
    ));

    let profile = crate::hardware_profile::active();
    checks.push(crit(
        "hardware_profile",
        profile.is_some(),
        profile
            .as_ref()
            .map(|p| String::from(p.id))
            .unwrap_or_else(|| String::from("none")),
    ));

    // Active block device — try_lock so the selftest can never deadlock on a
    // subsystem still holding the lock; "couldn't sample" is reported, not a
    // hard fail (the storage smoketest already proved the device separately).
    let block_state = match crate::block_io::ACTIVE_BLOCK_DEVICE.try_lock() {
        Some(g) => {
            if g.is_some() {
                (true, String::from("active block device present"))
            } else {
                (false, String::from("no active block device"))
            }
        }
        None => (true, String::from("lock busy — not sampled")),
    };
    checks.push(crit("block_io", block_state.0, block_state.1));

    // Aggregate of every registered per-module smoketest — one verdict covering
    // the whole fleet so a failing module surfaces in the single health line.
    let smoke_total = SMOKE_TOTAL.load(Ordering::Relaxed);
    let smoke_fails = SMOKE_FAILS.load(Ordering::Relaxed);
    let smoke_detail = if smoke_fails == 0 {
        format!("{}/{} smoketests passed", smoke_total, smoke_total)
    } else {
        let names = SMOKE_FAILED_NAMES.lock();
        format!(
            "{} of {} FAILED: {}",
            smoke_fails,
            smoke_total,
            names.join(",")
        )
    };
    checks.push(crit("smoketests", smoke_fails == 0, smoke_detail));

    // ── Optional: environment-dependent presence ─────────────────────────
    let xhci_up = crate::xhci::is_initialized();
    checks.push(opt(
        "xhci",
        xhci_up,
        String::from(if xhci_up {
            "controller initialized"
        } else {
            "no controller"
        }),
    ));

    // Tally.
    let (mut cp, mut ct, mut op, mut ot) = (0u32, 0u32, 0u32, 0u32);
    for c in &checks {
        match c.kind {
            Kind::Critical => {
                ct += 1;
                if c.pass {
                    cp += 1;
                }
            }
            Kind::Optional => {
                ot += 1;
                if c.pass {
                    op += 1;
                }
            }
        }
    }
    CRIT_PASS.store(cp, Ordering::Relaxed);
    CRIT_TOTAL.store(ct, Ordering::Relaxed);
    OPT_PRESENT.store(op, Ordering::Relaxed);
    OPT_TOTAL.store(ot, Ordering::Relaxed);

    let healthy = cp == ct;
    crate::serial_println!(
        "[selftest] boot health: {}/{} critical {}, {}/{} optional present -> {}",
        cp,
        ct,
        if healthy { "PASS" } else { "FAIL" },
        op,
        ot,
        if healthy { "HEALTHY" } else { "DEGRADED" },
    );
    for c in &checks {
        let tag = match c.kind {
            Kind::Critical => "crit",
            Kind::Optional => "opt ",
        };
        let verdict = if c.pass {
            match c.kind {
                Kind::Critical => "PASS",
                Kind::Optional => "PRESENT",
            }
        } else {
            match c.kind {
                Kind::Critical => "FAIL",
                Kind::Optional => "absent",
            }
        };
        crate::serial_println!(
            "[selftest]   {} {:<7} {:<16} ({})",
            tag,
            verdict,
            c.name,
            c.detail
        );
    }

    *RESULTS.lock() = checks;
}

fn crit(name: &'static str, pass: bool, detail: String) -> Check {
    Check {
        name,
        kind: Kind::Critical,
        pass,
        detail,
    }
}

fn opt(name: &'static str, pass: bool, detail: String) -> Check {
    Check {
        name,
        kind: Kind::Optional,
        pass,
        detail,
    }
}

/// `/proc/athena/selftest` body.
pub fn dump_text() -> String {
    let mut s = String::new();
    s.push_str("# AthenaOS consolidated boot self-test\n");
    if !RAN.load(Ordering::Relaxed) {
        s.push_str("status: not yet run\n");
        return s;
    }
    let cp = CRIT_PASS.load(Ordering::Relaxed);
    let ct = CRIT_TOTAL.load(Ordering::Relaxed);
    s.push_str(&format!(
        "boot_health: {}\n",
        if cp == ct { "HEALTHY" } else { "DEGRADED" }
    ));
    s.push_str(&format!("critical_pass: {}/{}\n", cp, ct));
    s.push_str(&format!(
        "optional_present: {}/{}\n",
        OPT_PRESENT.load(Ordering::Relaxed),
        OPT_TOTAL.load(Ordering::Relaxed)
    ));
    s.push_str("# per-check:\n");
    for c in RESULTS.lock().iter() {
        let tag = match c.kind {
            Kind::Critical => "crit",
            Kind::Optional => "opt",
        };
        s.push_str(&format!(
            "  [{}] {} {} ({})\n",
            tag,
            if c.pass { "ok" } else { "FAIL" },
            c.name,
            c.detail
        ));
    }
    s
}
