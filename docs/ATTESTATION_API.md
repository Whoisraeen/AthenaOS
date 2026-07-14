# AthenaOS Attestation API

> Concept §Security — *"You don't need kernel access on our OS; here's a better
> primitive."*

Anti-cheat vendors, enterprise compliance, and remote services historically demand a
ring-0 driver to answer one question: **can I trust the machine and the process I'm
talking to?** AthenaOS answers that question through a **userspace-queryable attestation
API** instead — bounded, audited, crash-isolated, and transparent. A vendor gets the
trust signal without being handed the kernel.

This document is the concrete API reference. For the *why* (the two-tier anti-cheat
posture), see [`ANTICHEAT_STRATEGY.md`](ANTICHEAT_STRATEGY.md); this is the *what* and
*how*.

---

## 1. What the API attests

Three independent evidence sources, each readable from userspace:

| Evidence | Question it answers | Surface |
|---|---|---|
| **Measured boot** (PCRs) | *Did the exact, untampered OS image boot, in order?* | `/proc/raeen/measured_boot`, `kernel::measured_boot`, `rae_crypto::pcr` |
| **Process integrity** | *Is this process's code unmodified — no injected hooks, no debugger, W^X intact?* | `SYS_AC_*` syscalls, `/proc/raeen/anticheat`, `kernel::anticheat` |
| **Kernel self-defense** | *Is the kernel itself intact (syscall table, IDT, `.text`)?* | `kernel::anticheat` self-checks, surfaced in the attestation quote |

A vendor combines all three: *"this unmodified process is running on a kernel that is
intact, which booted an OS image whose measurement matches the known-good build."*

---

## 2. Measured boot — the boot-chain commitment

Measured boot records **what ran** into a TPM 2.0-style bank of SHA-256 Platform
Configuration Registers. The engine is the shared, host-KAT'd
[`rae_crypto::pcr`](../components/rae_crypto/src/pcr.rs) core; the kernel drives it from
[`kernel/src/measured_boot.rs`](../kernel/src/measured_boot.rs).

### The extend operation

```
PCR[i] := SHA256( PCR[i] ‖ measurement )
```

Because each measurement is folded into the running hash, the final PCR is a
cryptographic commitment to the **exact contents AND order** of everything measured.
Tamper with any stage, reorder the chain, or swap the kernel, and the PCR diverges — an
attacker cannot forge a PCR value back to a known-good state.

### What AthenaOS measures today

| PCR | Stage | What it commits | TPM convention |
|---|---|---|---|
| **4** | Kernel / boot manager | the Ed25519-**signed boot manifest** (commits the kernel build + the initramfs hash it signed) | PCR 4 = boot-manager code |
| **8** | OS / userspace | the authentic in-memory `INITRAMFS` (the userspace image that actually booted) | PCR 8 = OS/loader data |

Read the live bank + event log:

```
$ cat /proc/raeen/measured_boot
measured_boot smoketest: PASS
event pcr=4 boot-manifest digest=ae47...
event pcr=8 initramfs    digest=5c78...
PCR[4]=0c2e...70b7
PCR[8]=fb0e...4cfd
```

The PCR values are **reproducible per image** (verified across repeated real-Ryzen
boots) — the property a verifier depends on.

### The quote (the thing a verifier consumes)

```rust
// rae_crypto::pcr::PcrBank
pub fn quote(&self, selection: &[u8], nonce: &[u8]) -> Option<[u8; 32]>
//  => SHA256( PCR[sel0] ‖ PCR[sel1] ‖ … ‖ nonce )
```

A **quote** is `SHA256` over a selected set of PCRs concatenated with a verifier-issued
**nonce**. It is:

- **Deterministic** — a remote verifier recomputes it from the golden PCRs + the nonce
  it issued, and compares.
- **Replay-proof** — the fresh nonce defeats replay of a stale quote captured earlier.

Verify a measured-boot log against a sealed golden PCR with
`rae_crypto::pcr::verify_log(log, pcr_index, &golden)` — it replays the log and returns
`false` on any tamper, reorder, or truncation.

---

## 3. Process-integrity + the attestation session (syscalls)

The per-game attestation session lives in [`kernel/src/anticheat.rs`] and is exposed
through Block 21 of the syscall table ([`SYSCALL_TABLE.md`](SYSCALL_TABLE.md) §284–290):

| # | Syscall | Args | Returns |
|---|---|---|---|
| 284 | `SYS_AC_REQUEST_ATTESTATION` | `game_pid, vendor_id, timestamp` | `session_id` |
| 285 | `SYS_AC_VERIFY_ATTESTATION` | `session_id, …` | `AC_OK` / `AC_ERR_*` |
| 286 | `SYS_AC_REGISTER_GAME` | `pid, …` | `AC_OK` / err |
| 287 | `SYS_AC_UNREGISTER_GAME` | `pid` | `AC_OK` / err |
| 288 | `SYS_AC_REPORT_VIOLATION` | `session_id, code, payload` | `AC_OK` / err |
| 289 | `SYS_AC_QUERY_STATUS` | `session_id` | status word |
| 290 | `SYS_AC_HEARTBEAT` | `session_id, timestamp` | `AC_OK` / err |

While a session is live, the kernel continuously checks the registered process:
code-page hashing, W^X enforcement, stack-canary integrity, and hook/debugger
detection — surfaced via `/proc/raeen/anticheat` and folded into the attestation quote
(`AttestationReport.tpm_quote`).

---

## 4. The remote-verification flow

```
   Vendor server                         AthenaOS (userspace app + kernel)
   ─────────────                         ───────────────────────────────
1. issue fresh nonce  ───────────────▶
2.                                       SYS_AC_REQUEST_ATTESTATION(pid, vendor, ts)
3.                                       kernel gathers: measured-boot PCRs +
                                         process-integrity state + kernel self-check,
                                         then quote = SHA256(PCRs ‖ nonce)
4.        ◀───────────────  signed attestation { quote, report, nonce }
5. recompute expected quote from the
   known-good golden PCRs + the issued
   nonce; compare. Match ⇒ trust.
```

The vendor never runs code in the kernel. The trust decision is made on **their**
server against evidence the kernel produced — that is the "better primitive."

---

## 5. Capability model

Attestation is a privileged operation, gated like everything else:

- A native app requests it through the `SYS_AC_*` syscalls, which check the caller's
  `Cap`.
- A **sandboxed Wasm app** ([`raewasm`](../components/raewasm/)) cannot issue syscalls
  directly; it reaches attestation through a **capability-gated host import** — the
  embedder (AthGuard) binds the import to an attestation `Cap` via the
  [`raewasm::pkg`](../components/raewasm/src/pkg.rs) manifest, and the call traps unless
  the user granted that capability.

No path to attestation bypasses the capability check.

---

## 6. Hardware TPM vs software measurement

| Layer | Status |
|---|---|
| Software measured-boot core (`rae_crypto::pcr`) + kernel `measured_boot` | **Live + host-KAT'd + real-Ryzen verified** (two-stage PCR[4]/PCR[8]) |
| Process-integrity monitors + `SYS_AC_*` session | **Live** (detection logic boot-smoketested: `wx/clean/tamper/canary/hook/ban/attest -> PASS`) |
| Hardware TPM 2.0 (TIS/CRB MMIO) + sealing keys to PCR state | **Iron-gated** — the chip transport + key sealing pend a flash; the software PCRs already match a hardware TPM's values for the same events, so the protocol is unchanged when the chip lands |

The software measurement chain is byte-identical in behavior to a hardware TPM for the
same events. When the TPM chip transport lands, the same quote/verify protocol gains a
hardware root of trust with no API change.

---

## 7. Status summary

- **Built + proven:** measured-boot two-stage PCR chain (real-Ryzen verified), the
  `quote`/`verify_log` primitives (host-KAT'd), the process-integrity session syscalls,
  `/proc/raeen/{measured_boot,anticheat}`, the capability gate.
- **Pending (iron-gated):** hardware-TPM transport + PCR-sealed keys; byte-reproducible
  initramfs builds so the same *source* yields the same golden PCR for remote
  attestation against a published manifest (local sealing only needs the per-image
  stability already proven).
