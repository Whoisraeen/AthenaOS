//! RaeenOS Calendar & Contacts — userspace ELF entry shim.
//!
//! All the app (the PIM model, the draw path, the syscall wiring, and the
//! host-KAT'd [`calendar::PimModel`]) lives in the `calendar` LIBRARY crate
//! (`src/lib.rs`) so a host test can link it and exercise the LIVE `rae_pim`
//! engine (iCalendar + vCard parse, RRULE expand, timezone math) with no kernel.
//! This bin is just the freestanding `_start` that hands control to
//! `calendar::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    calendar::run()
}
