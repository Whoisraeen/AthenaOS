# AthenaOS Testing & Verification Strategy

How AthenaOS proves a thing works. A `#![no_std]` kernel on a bare-metal target can't
`cargo test`, so we use a layered ladder instead — host KATs for pure logic, boot
smoketests for kernel subsystems, QEMU for integration, and iron for the truth. This
doc is the methodology; CLAUDE.md §9 is the per-commit checklist.

---

## 1. The status ladder (what `[x]` / `[~]` / `[ ]` mean)

Every MasterChecklist item carries exactly one status, and the bar is strict:

| Mark | Means | Bar |
|---|---|---|
| `[x]` | **Proven on real hardware** (Athena) or by iron-equivalent proof | An Athena serial log / hardware capture shows it. Nothing else earns `[x]`. |
| `[~]` | **QEMU-proven only** | Builds + boots in QEMU with the feature's serial marker present. The honest majority of the tree. |
| `[ ]` | **Not started / not proven** | No working code path, or no proof at all. |

**The cardinal rule:** QEMU green is `[~]`, never `[x]`. TCG emulation hides timing,
hardware quirks, real ACPI/firmware, real USB trees, and real GPUs. Promotion to `[x]`
requires the physical machine. (See the ACPI namespace case: 55 devices in QEMU, 0 on
Athena — the exact reason QEMU-green ≠ done.)

---

## 2. The four test layers

### Layer 1 — Host KAT (pure logic, no kernel, no QEMU)
For anything that is **deterministic pure logic** — crypto, parsers, codecs, driver
register state machines, allocator math. The logic lives in a `#![no_std]` module behind
a trait for any hardware access, and a **host harness** runs it on the dev machine via
ordinary `cargo test` / `rustc --edition 2021`. Instant, deterministic, no emulation.

- **Proven examples:** `ath_crypto` (SHA-256/BLAKE2/Argon2id/X25519/Ed25519 KATs),
  `tools/argon2_kat`, `tools/linuxkpi_harness` (36/36 — atomics, MMIO, allocators + a
  mock-GPU bring-up), `ath_amdgpu::bringup` (17 unit + 43/43 harness).
- **The embargo trick:** when QEMU is unavailable, lift the *byte-identical* pure logic
  into a standalone `rustc` host program and check it against a published KAT — this is
  how Argon2id (RFC 9106) and BLAKE2b were validated with no boot.
- **Mock-device pattern (drivers):** model the device as a mock register file behind the
  `Mmio`/`Port` trait; assert the driver walks reset→init→ready→one-operation correctly.
  A native driver reaches "host-KAT green" *before* it touches QEMU (see
  `NATIVE_DRIVER_PLAN.md` §4).

**This is the highest-leverage layer** — it has repeatedly surfaced real latent bugs
(the HMAC-SHA384/512 wrong-digest bug, the terminal monochrome SGR bug) the moment a
first test existed. Add a host KAT to any "it compiles, must work" subsystem.

### Layer 2 — Boot smoketest (the kernel's unit test, R10 contract)
The kernel can't `cargo test`, so each subsystem proves itself **during boot**. Every
kernel module owes the **R10 four-artifact contract**:

1. `init()` called from `kernel_main` in the right order,
2. `run_boot_smoketest()` that exercises the real code path and prints a serial verdict,
3. a `/proc/athena/<module>` procfs line for live state,
4. a Concept docstring.

The smoketest drives the *real* subsystem deterministically and prints
`[<module>] smoketest: a=.. b=.. -> PASS|FAIL`. Examples in-tree:
`[anticheat] smoketest: wx=.. clean=.. -> PASS`, `[rgb] effect smoketest: -> PASS`,
`[bundle] smoketest: reject(...) -> PASS`, `[acpi] run_boot_smoketest: devices=55 -> PASS`.

A subsystem with no `run_boot_smoketest` is a gap, regardless of how clean the code looks.

### Layer 3 — QEMU integration (the CI gate)
The full system boots headless and self-reports.

```
cargo run -p xtask --release -- build --release          # exit 0
ATHENA_SMP=2 cargo run -p xtask --release -- run --release --uefi --ci --disk smoketest
# xtask waits for [ OS ] System successfully booted., drains daemons, exit 0 = booted
```

Verify the serial log (`$env:TEMP\athena-serial.log`, **never** in the repo — OneDrive
would lock it):
- `PANIC` / `KERNEL PAGE FAULT` ⇒ must be absent,
- `System successfully booted.` ⇒ must be present,
- the feature's own marker ⇒ present with the expected value.

Knobs: `ATHENA_SMP=<n>` (1 avoids work-stealing), `ATHENA_ACCEL=whpx`, `--disk=<virtio|nvme|ata|smoketest>`.
Run on **both SMP=1 and SMP=2** for anything touching scheduling/IPI — and don't trust
SMP-green on fewer than ~5 boots (the steal-resume race is intermittent).

### Layer 4 — Iron (Athena) — the only thing that earns `[x]`
Real hardware via the bare-metal flash loop:
- Flash the UEFI image to USB, boot Athena, capture `BOOTLOG.TXT` from the ESP.
- **Self-capturing diagnostics**: the kernel dumps what it can't otherwise get off the
  machine — e.g. the ACPI byte-capture net base64-dumps the raw DSDT/SSDTs into the
  bootlog when the namespace comes up empty, so **one flash** returns the bytes to
  reproduce locally (`extract-acpi-dump.ps1` → `--features embed_test_dsdt` → QEMU).
- Design every iron trip to **maximize what one boot teaches** — bundle the verifies, make
  failures self-dump, so you don't burn flashes blind.

---

## 3. Choosing the right layer

| If the thing is… | Prove it with |
|---|---|
| Deterministic pure logic (crypto, parser, codec, driver state machine) | Layer 1 host KAT — always, first |
| A kernel subsystem with side effects (alloc, IPC, fs, sched, driver attach) | Layer 2 boot smoketest |
| Cross-subsystem behavior (daemon chain, spawn/reap, device claim) | Layer 3 QEMU |
| Anything timing-, firmware-, USB-tree-, or GPU-dependent | Layer 4 iron — no shortcut |

Prefer the cheapest layer that actually proves it. A host KAT that runs in 50 ms beats a
90-second QEMU boot for the same coverage.

---

## 4. Process discipline (the non-negotiables)

- **Build + boot after every kernel change.** Exit 0 + marker + no PANIC, or it didn't
  pass. No "looks right."
- **Report faithfully.** Tests fail → say so with the output. Step skipped → say it. Don't
  mark `[x]` on QEMU; don't mark anything done you didn't see pass.
- **Commit your own files only.** Stage explicit paths and verify the staged set — the repo
  is OneDrive-synced and multi-agent, so the index can mutate mid-operation (a sibling's
  `git add` has swept a staged file into the wrong commit). Commit promptly.
- **Run the gates** before commit: `ATHENA_AGENT=opus`, `scripts/ownership-lock.sh` +
  `scripts/architecture-gate.sh` (the pre-commit hook runs both).
- **No stubs that claim success.** An empty `Ok(())` or a smoketest that can't fail is
  worse than no test — it's a false green. A smoketest must be able to print `FAIL`.

---

## 5. Known measurement gaps

- **Perf telemetry is thin** — boot is well-instrumented; frame/audio/input/scheduler
  latency mostly isn't. A `/proc/athena/perf` histogram surface is the missing piece (see
  `PERFORMANCE_TARGETS.md`).
- **No automated iron CI** — Athena verification is a manual flash loop. Until there's a
  hardware test rig, `[x]` is gated on a human boot.
- **Coverage is uneven** — host-KAT'd subsystems (crypto, amdgpu bring-up, linuxkpi) are
  well-tested; many kernel modules have a boot smoketest but no fault-injection. Adding a
  first real test to an untested subsystem is consistently high-yield.
