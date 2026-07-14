// build.rs — link the REAL amdgpu object set into amdgpud (M5).
//
// OFF by default: with the `real_amdgpu_init` feature disabled, this does nothing,
// so the daemon builds anywhere without the vendored GPL amdgpu source.
//
// With the feature ON (the bare-metal run build), it links the pre-built
// `amdgpu-bringup.o` — the 75 real amdgpu/drm_sched/TTM/SMU objects + the daemon
// entry (bringup_entry.o) + the off-path WEAK stubs, relocatable-linked into one
// freestanding object by `linuxkpi-drm/m4c-link.sh`. Its remaining undefined
// symbols are the LinuxKPI surface, which the daemon already provides via its
// `ath_linuxkpi` crate dependency.
//
// We deliberately do NOT run the C build from here: it is a heavy external GPL
// artifact (gcc against the vendored kernel tree), not a cargo dependency, and a
// nested cargo invocation inside the daemon's own build races the workspace locks
// (m4-link.sh's `set -e` then exits early on the lock hiccup, leaving a stale
// object). The object is produced explicitly by the M5 build step:
//
//     FREESTANDING=1 bash linuxkpi-drm/m4c-link.sh     # -> $HOME/m4-obj/amdgpu-bringup.o
//
// (xtask wires this ahead of the daemon build for the Athena image.)
// See linuxkpi-drm/M5-BAREMETAL-PLAN.md.

use std::path::PathBuf;

fn main() {
    // Default build (feature off) — nothing to do, no GPL-source dependency.
    if std::env::var_os("CARGO_FEATURE_REAL_AMDGPU_INIT").is_none() {
        return;
    }

    // The merged object; override with RAE_AMDGPU_BRINGUP_OBJ, else $HOME/m4-obj.
    let obj = std::env::var_os("RAE_AMDGPU_BRINGUP_OBJ")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("m4-obj/amdgpu-bringup.o"))
        })
        .expect("real_amdgpu_init: set RAE_AMDGPU_BRINGUP_OBJ or HOME to locate amdgpu-bringup.o");

    println!("cargo:rerun-if-env-changed=RAE_AMDGPU_BRINGUP_OBJ");
    println!("cargo:rerun-if-changed={}", obj.display());

    assert!(
        obj.exists(),
        "real_amdgpu_init: {} not found.\n  \
         Build it first:  FREESTANDING=1 bash linuxkpi-drm/m4c-link.sh\n  \
         (or point RAE_AMDGPU_BRINGUP_OBJ at the merged object).",
        obj.display()
    );

    // Link the merged amdgpu object into the daemon; its LinuxKPI undefs resolve
    // against the daemon's ath_linuxkpi crate.
    println!("cargo:rustc-link-arg={}", obj.display());

    // RELOC FIX (the 0x77 fault): -Bsymbolic binds every intra-daemon reference to
    // its local definition, so amdgpu's global-symbol .data.rel.ro vtable slots
    // (`X_funcs.op = x_op`) emit R_X86_64_RELATIVE at this final link instead of an
    // interposable, symbol-based R_X86_64_64. The AthenaOS ELF loader
    // (kernel/src/elf.rs) applies ONLY R_X86_64_RELATIVE and skips symbol-based
    // relocs, so without this an amdgpu vtable slot stays null and the first
    // `nbio.funcs->set_reg_remap()` on the Phoenix init path jumps through 0
    // (observed on Athena as a jump to 0x77). Proven off-target: a global-symbol
    // fn-pointer vtable goes 3xRELATIVE+1xR_X86_64_64 -> 4xRELATIVE with this flag.
    println!("cargo:rustc-link-arg=-Bsymbolic");
}
