// kernel/src/namespaces.rs
//
// *Placeholder* Linux-style namespace subsystem. The original 1372-line
// implementation was deleted in commit b568b6e. main.rs still calls
// `namespaces::init()`. Concept §R7 ("no Linux-clones") suggests we
// won't restore Linux namespace semantics verbatim — sandboxing in
// AthenaOS is expressed through capabilities (see crate::capability).
//
// This shim exists purely to keep the boot path linking. When the
// sandboxing story is decided, replace with the real model.

pub fn init() {
    crate::serial_println!("[namespaces] placeholder online (capabilities used instead)");
}

pub fn run_boot_smoketest() {
    crate::serial_println!("[namespaces] smoketest: stub present");
}
