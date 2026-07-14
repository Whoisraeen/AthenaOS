//! AthenaOS Browser — userspace ELF entry shim.
//!
//! All the app (the navigation model, the load/render path over the LIVE `athweb`
//! engine + the `ath_js` interpreter, the chrome draw, the host-KAT'd
//! [`browser::BrowserModel`]) lives in the `browser` LIBRARY crate (`src/lib.rs`)
//! so a host test can link it and exercise the real engines with no kernel. This
//! bin is just the freestanding `_start` that hands control to `browser::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    browser::run()
}
