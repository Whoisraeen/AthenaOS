//! RaeenOS Passwords & Authenticator — userspace ELF entry shim.
//!
//! All the app (vault model, draw path, syscall wiring, the host-KAT'd
//! `VaultModel`) lives in the `passwords` LIBRARY crate (`src/lib.rs`) so a host
//! test can link it and exercise the LIVE rae_keychain + rae_otp engines with no
//! kernel. This bin is just the freestanding `_start` that hands control to
//! `passwords::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    passwords::run()
}
