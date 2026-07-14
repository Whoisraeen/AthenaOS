//! AthenaOS Sync — userspace ELF entry shim.
//!
//! All the app (the syscall-free [`sync::SyncApp`] over the LIVE zero-knowledge
//! `raesync` E2E core, plus the draw path + syscall wiring) lives in the `sync`
//! LIBRARY crate (`src/lib.rs`) so a host test can link it and drive a real
//! enroll/pair/encrypt/merge against a mock peer device with no kernel and no
//! network. This bin is just the freestanding `_start` that hands control to
//! `sync::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    sync::run()
}
