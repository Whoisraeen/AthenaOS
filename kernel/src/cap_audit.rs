//! Capability audit — observability for the `capability` subsystem.
//!
//! `RaeenOS_Concept.md` §Security:
//!
//! > "Capability-based permissions — apps request capabilities (file
//! >  access, camera, mic, network), user grants, OS enforces at the
//! >  syscall layer."
//!
//! "OS enforces at the syscall layer" implies the user has to be able to
//! see what's enforced. This module is the lens.
//!
//! Every `grant` / `revoke` / `query` against the capability subsystem
//! records a small fixed-size event in a ring buffer. `cat /proc/raeen/caps`
//! prints the last N events plus running totals. Untrusted code can't
//! delete or alter the log — there's no userspace API to it.
//!
//! Boot smoketest produces a small fake grant/revoke pair so the dump
//! always shows at least one example.
//!
//! Per `kernelchecklist.md` §5.9 acceptance criterion #2:
//! "Capability revocation is observable in /proc/raeen/caps".

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ── Event model ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Grant = 1,
    Revoke = 2,
    Query = 3,
    Use = 4,    // resource actually exercised through this cap
    Denied = 5, // a syscall refused for missing/insufficient cap
}

#[derive(Debug, Clone, Copy)]
pub struct Event {
    pub seq: u64,
    pub kind: EventKind,
    pub actor_task: u64,  // who took the action (granter / revoker / caller)
    pub target_task: u64, // whose table changed (0 if N/A)
    pub cap_flavor: u32,  // matches Cap discriminant
    pub rights: u32,      // raw Rights bitmask
    pub handle: u64,      // the CapHandle in the target's table (or src on grant)
    pub tsc: u64,
}

impl Event {
    fn empty() -> Self {
        Self {
            seq: 0,
            kind: EventKind::Grant,
            actor_task: 0,
            target_task: 0,
            cap_flavor: 0,
            rights: 0,
            handle: 0,
            tsc: 0,
        }
    }
}

// ── Ring buffer ────────────────────────────────────────────────────────

const RING_CAP: usize = 256;

struct Ring {
    events: [Event; RING_CAP],
    /// Write index modulo RING_CAP. Reads use `tail` (oldest valid) implied
    /// by `seq` overflow comparisons.
    head: usize,
    filled: bool,
}

impl Ring {
    const fn new() -> Self {
        const EMPTY: Event = Event {
            seq: 0,
            kind: EventKind::Grant,
            actor_task: 0,
            target_task: 0,
            cap_flavor: 0,
            rights: 0,
            handle: 0,
            tsc: 0,
        };
        Self {
            events: [EMPTY; RING_CAP],
            head: 0,
            filled: false,
        }
    }

    fn push(&mut self, ev: Event) {
        self.events[self.head] = ev;
        self.head = (self.head + 1) % RING_CAP;
        if self.head == 0 {
            self.filled = true;
        }
    }

    /// Iterate from oldest to newest.
    fn iter_oldest_first<F: FnMut(&Event)>(&self, mut f: F) {
        if self.filled {
            for i in 0..RING_CAP {
                let idx = (self.head + i) % RING_CAP;
                f(&self.events[idx]);
            }
        } else {
            for i in 0..self.head {
                f(&self.events[i]);
            }
        }
    }

    fn len(&self) -> usize {
        if self.filled {
            RING_CAP
        } else {
            self.head
        }
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring::new());
static SEQ: AtomicU64 = AtomicU64::new(0);

// Aggregate counters that survive the ring rolling over.
static TOTAL_GRANTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_REVOKES: AtomicU64 = AtomicU64::new(0);
static TOTAL_QUERIES: AtomicU64 = AtomicU64::new(0);
static TOTAL_USES: AtomicU64 = AtomicU64::new(0);
static TOTAL_DENIED: AtomicU64 = AtomicU64::new(0);

// ── Recording API (called from capability.rs) ──────────────────────────

fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn record(kind: EventKind, actor: u64, target: u64, flavor: u32, rights: u32, handle: u64) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let ev = Event {
        seq,
        kind,
        actor_task: actor,
        target_task: target,
        cap_flavor: flavor,
        rights,
        handle,
        tsc: rdtsc(),
    };
    RING.lock().push(ev);

    // Update counters last so the ring is stable when /proc/raeen/caps
    // reads while a recording is in flight.
    match kind {
        EventKind::Grant => {
            TOTAL_GRANTS.fetch_add(1, Ordering::Relaxed);
        }
        EventKind::Revoke => {
            TOTAL_REVOKES.fetch_add(1, Ordering::Relaxed);
        }
        EventKind::Query => {
            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
        }
        EventKind::Use => {
            TOTAL_USES.fetch_add(1, Ordering::Relaxed);
        }
        EventKind::Denied => {
            TOTAL_DENIED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Public helpers used by capability.rs.
pub fn record_grant(actor: u64, target: u64, flavor: u32, rights: u32, handle: u64) {
    record(EventKind::Grant, actor, target, flavor, rights, handle);
}

pub fn record_revoke(actor: u64, target: u64, flavor: u32, rights: u32, handle: u64) {
    record(EventKind::Revoke, actor, target, flavor, rights, handle);
}

pub fn record_query(actor: u64, flavor: u32, rights: u32, handle: u64) {
    record(EventKind::Query, actor, actor, flavor, rights, handle);
}

pub fn record_use(actor: u64, flavor: u32, rights: u32, handle: u64) {
    record(EventKind::Use, actor, actor, flavor, rights, handle);
}

pub fn record_denied(actor: u64, flavor: u32, rights: u32, handle: u64) {
    record(EventKind::Denied, actor, actor, flavor, rights, handle);
}

// ── Aggregate counters ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct AuditTotals {
    pub grants: u64,
    pub revokes: u64,
    pub queries: u64,
    pub uses: u64,
    pub denied: u64,
    pub ring_filled: u64, // 1 if the ring has wrapped at least once
    pub ring_len: u64,
}

pub fn totals() -> AuditTotals {
    let r = RING.lock();
    AuditTotals {
        grants: TOTAL_GRANTS.load(Ordering::Relaxed),
        revokes: TOTAL_REVOKES.load(Ordering::Relaxed),
        queries: TOTAL_QUERIES.load(Ordering::Relaxed),
        uses: TOTAL_USES.load(Ordering::Relaxed),
        denied: TOTAL_DENIED.load(Ordering::Relaxed),
        ring_filled: r.filled as u64,
        ring_len: r.len() as u64,
    }
}

// ── /proc/raeen/caps dump ──────────────────────────────────────────────

fn flavor_label(flavor: u32) -> &'static str {
    match flavor {
        0 => "Channel",
        1 => "Mmio",
        2 => "Irq",
        3 => "Port",
        4 => "Filesystem",
        5 => "Network",
        6 => "Gpu",
        7 => "Audio",
        8 => "Camera",
        9 => "Process",
        10 => "CryptoKey",
        11 => "Hypervisor",
        12 => "Attestation",
        13 => "Debug",
        _ => "?",
    }
}

fn kind_label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Grant => "GRANT",
        EventKind::Revoke => "REVOKE",
        EventKind::Query => "QUERY",
        EventKind::Use => "USE",
        EventKind::Denied => "DENIED",
    }
}

fn rights_label(buf: &mut [u8; 16], rights: u32) -> &str {
    let mut n = 0;
    let bits: &[(u32, u8)] = &[
        (1 << 0, b'r'),
        (1 << 1, b'w'),
        (1 << 2, b'x'),
        (1 << 3, b'M'),
        (1 << 4, b'W'),
        (1 << 5, b'g'),
        (1 << 6, b'R'),
    ];
    for (mask, ch) in bits {
        if rights & mask != 0 {
            buf[n] = *ch;
            n += 1;
        }
    }
    if n == 0 {
        buf[0] = b'-';
        n = 1;
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("-")
}

pub fn dump_text() -> String {
    let t = totals();
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "# RaeenOS capability audit\n\
         # totals: grants={} revokes={} queries={} uses={} denied={} (ring {}/{} wrapped={})\n\
         # legend: rights = r(ead) w(rite) x(ec) M(map) W(ait) g(rant) R(evoke)\n\
         # seq  kind   actor  target  flavor       rights   handle\n",
        t.grants,
        t.revokes,
        t.queries,
        t.uses,
        t.denied,
        t.ring_len,
        RING_CAP,
        t.ring_filled,
    ));
    let r = RING.lock();
    let mut rights_buf = [0u8; 16];
    r.iter_oldest_first(|ev| {
        let rights_str = rights_label(&mut rights_buf, ev.rights);
        out.push_str(&alloc::format!(
            "{:>5} {:<7} {:>5} {:>5}  {:<11}  {:<7}  0x{:x}\n",
            ev.seq,
            kind_label(ev.kind),
            ev.actor_task,
            ev.target_task,
            flavor_label(ev.cap_flavor),
            rights_str,
            ev.handle,
        ));
    });
    out
}

// ── Boot init + smoketest ──────────────────────────────────────────────

pub fn init() {
    crate::serial_println!(
        "[ OK ] Capability audit ring initialized (capacity {} events)",
        RING_CAP,
    );
}

pub fn run_boot_smoketest() {
    // Synthesize a small audit trace so /proc/raeen/caps is never empty
    // on a freshly-booted system. Real apps will dwarf these once they
    // start grabbing IRQ/MMIO/IPC caps.
    record_grant(0, 1, /*Channel*/ 0, /*r|w|W|g*/ 0b0110011, 0x1001);
    record_grant(0, 1, /*Mmio*/ 1, /*r|w|M*/ 0b0001011, 0x1002);
    record_grant(0, 1, /*Irq*/ 2, /*W*/ 0b0010000, 0x1003);
    record_use(1, /*Channel*/ 0, 0b0000001, 0x1001);
    record_query(1, /*Mmio*/ 1, 0b0001011, 0x1002);
    record_revoke(0, 1, /*Irq*/ 2, 0b0010000, 0x1003);
    record_denied(2, /*Process*/ 9, 0b0100000, 0xDEAD);
    let t = totals();
    crate::serial_println!(
        "[cap_audit] smoketest: {} events recorded (grants={} revokes={} denied={})",
        t.ring_len,
        t.grants,
        t.revokes,
        t.denied,
    );
}
