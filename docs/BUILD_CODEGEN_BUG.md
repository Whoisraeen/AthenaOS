# Kernel build blocker — `offset is not a multiple of 16` — RESOLVED (was a host-build phantom)

**Status:** ✅ RESOLVED 2026-05-29. The real `x86_64-unknown-none` kernel build
was **never broken**. The error only ever appeared when building the kernel for
the **host** target by mistake.

## Root cause

`.cargo/config.toml` does **not** set `build.target`, so a bare:

```sh
cargo build -p kernel --release        # ❌ builds for the HOST (x86_64-pc-windows-msvc)
```

compiles the `no_std`/`no_main` kernel for Windows/MSVC, where it hits an
unrelated host x86 codegen assertion (`offset is not a multiple of 16`) and then
a host link failure (`link.exe` 1561 — a bare-metal kernel can't link as a
Windows exe). **This is not the kernel's real build.**

The correct build always goes through xtask (which passes `--target
x86_64-unknown-none`) and **succeeds**:

```sh
cargo run -p xtask --release -- build --release   # ✅ produces kernel.bios.img / kernel.uefi.img
# equivalently:
cargo build -p kernel --release --target x86_64-unknown-none   # ✅
```

## Lesson / guardrail

**Never build the kernel without `--target x86_64-unknown-none`.** Always use
`cargo run -p xtask -- build [--release]`. Note: a workspace-wide `build.target`
in `.cargo/config.toml` is **not** a safe guardrail here — `xtask` is a host
tool and must build for the host, so forcing the bare-metal target globally
would break it. The guardrail is simply: **always go through xtask** (and
`cargo check` is fine either way — it skips codegen/link).

## Contributing factor (also fixed)

Separately, the interrupt handlers were converted to the **naked → `#[inline(never)]`
inner** pattern (`kernel/src/interrupts.rs`) to keep aligned spills out of the
`x86-interrupt` frame — a genuine robustness improvement that's now in place and
boot-validated.
