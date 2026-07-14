# AthenaOS Boot Status

## M1 proof — 2026-07-13

| Item | Result |
|---|---|
| Command | `cargo run -p xtask --release -- run --release --ci` |
| Kernel | Built successfully (release, `x86_64-unknown-none`) |
| QEMU | BIOS image `kernel.bios.img` |
| CI | **Success** — boot completion marker detected |
| Banner | `AthKernel v0.0.1 — AthenaOS` on serial |
| Marker | `[ OS ] System successfully booted.` (CI waiter) |

Serial transcript copy: [`logs/athena-m1-qemu-serial.log`](../logs/athena-m1-qemu-serial.log)

### Notes / bootstrap breakage

1. **Do not regenerate `Cargo.lock` blindly.** A fresh `cargo` update pulled `x86_64` 0.15.5, which fails to compile against `nightly-2026-05-01` (`Step::forward_overflowing`). Prefer a lockfile known-good with this toolchain until Athena pins its own vetted set.
2. **Debug user-app builds** of `raenet` (browser path) can hit `rustc-LLVM ERROR: Do not know how to split the result of this operator!`. **Release** builds succeed; prefer `--release` for Athena CI.
3. QEMU may warn about hostfwd `tcp::2222-:22` if the port is busy; CI still passed.
4. Many inherited introspection strings still say “RaeenOS” in `/proc`-style dumps; product banner, DMI placeholders, `/system/name`, mDNS, installer labels, and xtask branding were retargeted to AthenaOS. Inherited `rae*` **crate directory names** remain temporarily; Athena-first code lives under `components/ath*`.

### Remotes (do not mix)

| Remote | URL | Push? |
|---|---|---|
| `origin` | https://github.com/Whoisraeen/AthenaOS | Yes — Athena only |
| `upstream-raeenos` | https://github.com/Whoisraeen/RaeenOS | **No** — optional reference only, not a fork parent |
