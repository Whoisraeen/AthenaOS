//! AthenaOS VPN — userspace ELF entry shim.
//!
//! All the app (the syscall-free [`vpn::VpnModel`] over the LIVE `raevpn`
//! Noise_IKpsk2 WireGuard handshake, the config model, plus the draw path +
//! syscall wiring) lives in the `vpn` LIBRARY crate (`src/lib.rs`) so a host test
//! can link it and drive the real handshake against a scripted (mock) peer with
//! no kernel and no network. This bin is just the freestanding `_start` that
//! hands control to `vpn::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    vpn::run()
}
