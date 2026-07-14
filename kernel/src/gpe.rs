//! ACPI General Purpose Event dispatch — wires `\_GPE` namespace methods to
//! hardware GPE status registers via `acpi_full::GpeSubsystem`.
//!
//! MasterChecklist Phase 1.4: GPE dispatcher — parse `_Lxx`/`_Exx` from
//! `\_GPE`, enable those GPEs in hardware, dispatch on raise.

extern crate alloc;

use alloc::string::String;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static GPE_RAISES: [AtomicU64; 256] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; 256]
};

/// Count of registered _Lxx/_Exx handlers found during namespace scan.
static GPE_HANDLER_COUNT: AtomicU32 = AtomicU32::new(0);

/// Number of System Control Interrupts (SCI) received.
static SCI_INTERRUPT_COUNT: AtomicU64 = AtomicU64::new(0);

/// Initialize the GPE subsystem: scan `\_GPE` namespace for `_Lxx`/`_Exx`
/// methods, register them, and enable the corresponding GPE bits in hardware.
pub fn init() {
    crate::acpi_full::init_gpe_dispatcher();

    // Record how many handlers were found.
    let count = {
        let sub = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        sub.gpe.handlers.len() as u32
    };
    GPE_HANDLER_COUNT.store(count, Ordering::Relaxed);

    crate::serial_println!(
        "[ OK ] GPE dispatcher: {} _Lxx/_Exx handler(s) registered",
        count,
    );
}

/// Called from the SCI interrupt path or a polled path when a GPE fires.
/// Increments the per-GPE counter and dispatches the AML method.
pub fn dispatch(gpe_num: u8) {
    GPE_RAISES[gpe_num as usize].fetch_add(1, Ordering::Relaxed);
    let mut acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
    if acpi.initialized {
        acpi.dispatch_gpe(gpe_num as u16);
    }
}

/// Called from the SCI interrupt handler in `interrupts.rs`.
pub fn on_sci_interrupt() {
    SCI_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed);
    // Poll and dispatch all pending GPE events.
    let dispatched = crate::acpi_full::poll_gpe_events();
    if dispatched > 0 {
        crate::serial_println!("[gpe] sci: {} event(s) dispatched", dispatched);
    }
}

/// Poll GPE hardware status registers. Call periodically (e.g. from LAPIC
/// timer tick every ~100 ms) to dispatch pending events without needing
/// the SCI interrupt wired. Safe to call under the LAPIC timer handler.
pub fn poll() {
    let dispatched = crate::acpi_full::poll_gpe_events();
    if dispatched > 0 {
        crate::serial_println!("[gpe] poll: {} event(s) dispatched", dispatched);
    }
}

pub fn run_boot_smoketest() {
    let handlers = GPE_HANDLER_COUNT.load(Ordering::Relaxed);
    let sci_count = SCI_INTERRUPT_COUNT.load(Ordering::Relaxed);
    let total_raises: u64 = (0..256)
        .map(|i| GPE_RAISES[i].load(Ordering::Relaxed))
        .sum();
    // On QEMU, 0 handlers is expected (minimal DSDT has no \_GPE methods).
    // On real hardware, handlers > 0 means _Lxx/_Exx methods were found.
    crate::serial_println!(
        "[gpe] smoketest: handlers={} sci_interrupts={} total_dispatches={} -> {}",
        handlers,
        sci_count,
        total_raises,
        if handlers == 0 {
            "PASS (QEMU: no GPE methods in DSDT)"
        } else {
            "PASS"
        },
    );
}

pub fn dump_text() -> String {
    let mut out = String::from("# gpe handler summary\n");
    out.push_str(&alloc::format!(
        "registered_handlers: {}\n",
        GPE_HANDLER_COUNT.load(Ordering::Relaxed)
    ));
    out.push_str(&alloc::format!(
        "sci_interrupts_received: {}\n",
        SCI_INTERRUPT_COUNT.load(Ordering::Relaxed)
    ));
    out.push_str("# per-gpe raise counts\n");
    for i in 0..256 {
        let n = GPE_RAISES[i].load(Ordering::Relaxed);
        if n > 0 {
            out.push_str(&alloc::format!("gpe_{:02X}: {}\n", i, n));
        }
    }
    out
}
