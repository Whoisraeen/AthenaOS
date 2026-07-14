//! AthenaOS Mail — userspace ELF entry shim.
//!
//! All the app (the syscall-free [`mail::MailModel`] over the LIVE `rae_mail`
//! SMTP/IMAP/POP3 + RFC822/MIME engines, the `rae_pim` vCard contacts, and the
//! `rae_kv` local cache, plus the draw path + syscall wiring) lives in the `mail`
//! LIBRARY crate (`src/lib.rs`) so a host test can link it and exercise the real
//! engines against a scripted (mock) transport with no kernel and no network.
//! This bin is just the freestanding `_start` that hands control to `mail::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    mail::run()
}
