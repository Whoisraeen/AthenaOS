// Link the merged real-amdgpu object graph (HOST build of m4c-link.sh) into the
// runner. Mirrors amdgpud/build.rs, but expects the x86_64-unknown-linux-gnu
// object (m4c-link.sh WITHOUT FREESTANDING=1).
use std::path::PathBuf;

fn main() {
    let obj = std::env::var_os("RAE_AMDGPU_BRINGUP_OBJ")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("m4-obj/amdgpu-bringup.o"))
        })
        .expect("set RAE_AMDGPU_BRINGUP_OBJ or HOME to locate amdgpu-bringup.o");

    println!("cargo:rerun-if-env-changed=RAE_AMDGPU_BRINGUP_OBJ");
    println!("cargo:rerun-if-changed={}", obj.display());

    assert!(
        obj.exists(),
        "amdgpu-bringup.o not found at {}.\n  Build it first (HOST mode):  bash linuxkpi-drm/m4c-link.sh",
        obj.display()
    );

    println!("cargo:rustc-link-arg={}", obj.display());
    // Same vtable-reloc discipline as the iron link (see amdgpud/build.rs);
    // harmless on host, keeps the two links maximally alike.
    println!("cargo:rustc-link-arg=-Bsymbolic");
    // The C graph and the shim's printf/string shadows preempt libc's dynamic
    // symbols — intended (running the REAL shim code is the point).
    println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
}
