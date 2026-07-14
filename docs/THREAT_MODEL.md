# AthenaOS Threat Model

**Status:** Living document. Authoritative for "who we defend against and where
enforcement actually lives." Pairs with `docs/CAPABILITIES.md` (the capability
contract) and `MasterChecklist.md` Phase 9 (AthGuard roadmap).

**Rule for contributors:** if you change *what is enforced* (add a sandbox gate
class, wire a capability check, land secure boot), update the **Enforcement
status** tables below in the SAME commit. A security doc that describes the
target state in the present tense is worse than no doc — it manufactures false
confidence. Every row here is marked with where it stands TODAY.

Legend for every status cell:
- **ENFORCED** — wired on the live boot path, boot-proven in QEMU (cite the marker).
- **PARTIAL** — mechanism exists and runs, but coverage is incomplete (scope noted).
- **DESIGNED** — contract/format defined, not yet checked end-to-end.
- **PLANNED** — not started; tracked in MasterChecklist.

---

## 1. What AthenaOS protects (assets)

In priority order — this ordering decides what we fix first when defenses conflict.

1. **User data confidentiality + integrity.** A user's documents, saves,
   credentials, and per-app data buckets. The Concept's anti-ransomware promise
   lives here: "apps can't touch other apps' data without explicit permission."
2. **System integrity.** The kernel, boot chain, and system config cannot be
   silently modified by an app, a downloaded bundle, or a malicious driver.
3. **Availability for the foreground task.** A background app (or a buggy
   driver) cannot starve or crash the foreground game/creative workload.
   (Embodiment-first: a dropped frame is a defect, a crash is a failure.)
4. **Attestation integrity.** When AthGuard tells an anti-cheat vendor "this
   process is unmodified," that statement is trustworthy without the vendor
   owning ring 0.

---

## 2. Who we defend against (adversaries) and where the line is

### A. Malicious or compromised application (PRIMARY adversary)
A sideloaded or store app, possibly signed by a real-but-hostile developer, that
runs as a normal user process and tries to read another app's data, claim
hardware, exfiltrate over the network, or escalate to kernel.

- **In scope.** This is the adversary the capability model + AthGuard sandbox +
  data buckets are built for. Defense must hold even for a *signed* app — a valid
  signature proves provenance, not benevolence.
- **Defenses:** capability gating (§4), syscall-edge sandbox (§5), per-app bucket
  isolation + per-app encryption (§6), memory isolation (§3).

### B. Malicious bundle author / supply chain
Someone who ships a tampered app bundle, or tampers with one in transit/at rest.

- **In scope for tamper-detection.** Ed25519 sig → manifest → ELF-hash chain
  (§7) rejects a *modified* signed bundle fail-closed. An *unsigned* bundle runs
  in "unverified developer" posture, never silently blocked (Concept policy).
- **Out of scope (today):** a hostile developer with a *legitimately issued*
  signing identity. We have no developer PKI, certificate revocation, or
  store-review trust tiering yet (PLANNED, §7). A valid signature today only
  means "these bytes match what was signed," not "this developer is vetted."

### C. Compromised / buggy device driver
A userspace driver daemon (GPU, NIC, Wi-Fi) that is exploited or simply wrong,
and tries to DMA into kernel/other-process memory or take down the system.

- **In scope by architecture.** Drivers run in userspace, claim hardware through
  capability-gated syscalls, and (design) live in per-device IOMMU domains so the
  hardware can only DMA into frames the driver was granted. A crashed daemon is
  restarted by the supervisor; it takes down a service, not the kernel.
- **Caveat:** IOMMU *enforcement* on real hardware (VT-d/AMD-Vi root-pointer
  programming) is PARTIAL — the parser + domain creation exist, but end-to-end
  per-device DMA confinement on iron is Phase 4. In QEMU the isolation is
  structural (separate address spaces), not yet silicon-enforced.

### D. Network attacker
Anyone on the wire between AthenaOS and a remote host.

- **In scope:** confidentiality/integrity of traffic we originate via real,
  KAT-proven crypto (X25519, ChaCha20-Poly1305, Ed25519 — §8). Firewall is
  DefaultDeny with per-app rules requiring `Cap::Network`.
- **Out of scope (today):** a full authenticated TLS 1.3 session to arbitrary
  servers (only ClientHello framing + the primitives exist; full handshake is
  PLANNED). Do not treat outbound HTTPS as confidential yet.

### E. Evil-maid / physical attacker (boot-time tampering)
Someone with physical access who modifies the boot image, kernel, or disk while
the machine is off.

- **OUT OF SCOPE TODAY — and this is the model's biggest honest gap.** Secure
  boot (bootloader→kernel→init→compositor measurement) and TPM 2.0 sealing of
  root keys are both PLANNED, not built. **Every defense in this document sits on
  a kernel that is not itself measured.** An attacker who can write the boot
  image or kernel binary bypasses capabilities, sandboxing, signing, and bucket
  encryption wholesale. Full-disk encryption (FDE) is also PLANNED, so data at
  rest is readable by anyone with the disk.
  → Until §9 lands, AthenaOS protects **apps from each other at runtime**, NOT
  **the system from offline tampering.** State this plainly to any user or
  partner; do not imply otherwise.

### F. Malicious local user with admin intent
A user trying to inspect/modify their *own* machine.

- **Explicitly NOT an adversary.** Concept §"The user owns the machine." Debug
  caps, sideloading, and full local control are features. We do not build
  defenses against the machine's owner. (This is the deliberate inverse of the
  iOS lockdown model.)

### G. Side-channel / microarchitectural attacker (Spectre/Meltdown-class)
- **OUT OF SCOPE for v1**, explicitly. No speculative-execution mitigations are
  claimed. Documented here so the omission is a decision, not an oversight.

---

## 3. Trust boundaries (and the invariant everything rests on)

```
  ┌─────────────────────────── Ring 3 (untrusted) ───────────────────────────┐
  │  App (AppSandbox/Strict)   Linux-ABI app   Driver daemon   First-party app │
  └───────────────┬───────────────┬───────────────┬───────────────┬──────────┘
        syscall edge ── AthGuard sandbox gate (§5) ── runs for BOTH ABIs ─────
  ┌───────────────┴───────────────────────────────────────────────────────────┐
  │                         Ring 0 — AthKernel (TCB)                            │
  │  capability authority · scheduler · memory isolation · VFS gate · crypto    │
  └────────────────────────────────────────────────────────────────────────────┘
```

**The load-bearing invariant: per-process memory isolation.** Every layer above
— capabilities, buckets, sandbox levels — assumes Process A literally cannot read
or write Process B's pages. This is the substrate; if it leaks, nothing else
matters.

| Invariant | Status | Evidence / note |
|---|---|---|
| Separate PML4 per task; user pages private | **ENFORCED** | `create_new_pml4` deep-copies PML4→PDPT→PD→**PT** so no two address spaces share a user page table. *This had a hole until 2026-06-10:* the copy stopped at the PD level, so all base-0 PIE apps shared one PT and a child's load overwrote the running parent (proven: user_init #UD fetching a child's `.rodata`). Now boot-clean. |
| W^X on user pages | **PARTIAL** | Linux ELF loader sets `NO_EXECUTE` on non-exec segments (`task.rs`); the native `elf.rs` loader does not yet mark NX per-segment. Unify before claiming W^X. |
| SMEP / SMAP (kernel can't exec/read user pages) | **ENFORCED** | `cpu_features.rs` enables both on BSP+APs; reported in the `[cpu] kernel:` boot line. |
| KASLR | **PARTIAL** | `hardening.rs` + KASLR slide present; entropy quality not audited. |
| User TLS can't point into kernel half | **ENFORCED** | `arch_prctl(ARCH_SET_FS)` + native syscall 126 reject any base ≥ `0x0000_8000_0000_0000` (non-canonical / kernel-half) before writing it (2026-06-10). |

---

## 4. Capability enforcement status (per flavor)

The 14-flavor `Cap` enum (`docs/CAPABILITIES.md`) is the *contract*. Holding a
cap should be necessary to perform the operation. Reality today is uneven — this
table is the honest map. "Mint-gated" = is a user/policy decision required to
*obtain* the cap; "Use-checked" = is the cap actually verified at the syscall
that performs the privileged op.

| Flavor | Use-checked at syscall? | Mint-gated? | Status | Note |
|---|---|---|---|---|
| `Mmio` / `Irq` / `Port` | yes | yes (driver claim path) | **ENFORCED** | userspace driver framework gates claim/DMA. |
| `Network` | yes (firewall rule ops) | partial | **PARTIAL** | `firewall.rs` requires `Cap::Network`+WRITE for rule changes; socket *creation* is gated by the sandbox class, not a per-socket cap check. |
| `Filesystem` | partial | n/a | **PARTIAL** | bucket access is enforced via the VFS gate + AthFS (§6), not via a `Cap::Filesystem` handle check on every open. |
| `Camera` | **no** | **no** | **DESIGNED** | flavor + rights defined; the "requires user prompt" rule is NOT wired — a held Camera cap is not gated by user consent because no UI consumes the prompt queue (§5 prompt gap). Do not advertise camera consent. |
| `Audio` / `Gpu` | partial | via driver | **PARTIAL** | device daemons hold them; no per-call app-side check yet. |
| `Process` | yes | at spawn | **ENFORCED** | derivation + transitive revoke proven. |
| `CryptoKey` | yes | TPM/athshield | **PARTIAL** | minting path depends on TPM sealing (PLANNED). |
| `Attestation` | yes | anti-cheat | **DESIGNED** | syscalls 100–106 exist; no vendor harness. |
| `Hypervisor` / `Debug` | yes | root-only | **DESIGNED** | privileged; not exercised. |

**Derivation/revocation core (narrowing-only grant, subset rights, transitive
revoke, audit ring): ENFORCED** — boot-proven, `/proc/athena/caps`. The gap is
not the algebra; it's that several flavors aren't yet *consulted* at their
operation sites.

---

## 5. Syscall-edge sandbox (AthGuard) — coverage

`kernel/src/sandbox.rs` gates the syscall edge by per-task `SandboxLevel`
(Trusted / AppSandbox / Strict). Trusted (the default) short-circuits on one
atomic load.

| Property | Status | Note |
|---|---|---|
| Gate runs for **native** ABI tasks | **ENFORCED** | `check_syscall` in `syscall_handler_inner`. |
| Gate runs for **Linux** ABI tasks | **ENFORCED (2026-06-10)** | `check_linux_syscall` — *the Linux dispatch used to `return` before the gate, so every sandboxed Linux binary bypassed AthGuard entirely.* Now both ABIs gate before dispatch. Boot proof: `[sandbox] run_boot_smoketest: … linux_gate=true … -> PASS`. |
| Gated syscall classes | **PARTIAL — by design, but the checklist oversold it** | Only **Device/DMA/PCI**, **Network (sockets)**, and **Install** are classed. The ~170 other syscalls (file I/O, IPC, mmap, scheduling, compositor) are **not** sandbox-gated at this edge — a sandboxed app calls them freely. This is a deliberate staged rollout, NOT "every syscall that touches userspace state." Confidentiality for those paths relies on capabilities (§4) + memory isolation (§3), not this gate. |
| Manifest-granted relaxation | **ENFORCED** | an AppSandbox task whose `RaeManifest.toml` declared a class passes that class; Strict never gets grants. |
| Runtime permission **prompt** (user says yes/no live) | **PARTIAL** | kernel queue + syscall surface exist (`perm_prompt.rs`, `perm_syscalls.rs`), but **no compositor UI consumes it**, so no cap is actually gated on live user consent yet. This is why `Camera` is DESIGNED, not ENFORCED. |

**To close §5:** expand `class_of`/`class_of_linux` to cover file + IPC + mmap
classes for Strict tasks, and build the compositor prompt consumer so mint-time
consent becomes real.

---

## 6. Data-at-rest isolation (the anti-ransomware claim)

Three independent layers, each boot-proven 2026-06-10 — a bug in one does not
collapse the boundary:

| Layer | Status | Boot proof |
|---|---|---|
| VFS gate (the path a real read takes) | **ENFORCED** | `data_buckets::run_boot_smoketest: … vfs_deny=true deny_foreign=true allow_owner=true -> PASS` |
| AthFS capability layer | **ENFORCED** | `[athfs] bucket smoketest: isolation=true … cap=true` |
| Per-app encryption keys (FSCRYPT-equiv) | **ENFORCED** | `[athfs] bucket-key selftest: cross_app_unreadable=true -> PASS` |

**Gap:** the end-to-end acceptance test — a real *malicious userspace app*
spawned sandboxed that attempts a foreign-bucket read/write post-boot and reports
the denial on serial — is still PLANNED. The layers are proven by in-kernel
smoketests, not yet by a hostile process exercising the syscall surface. Also:
data at rest is unencrypted without FDE (§9), so these layers protect against a
*running malicious app*, not against someone with the powered-off disk.

---

## 7. Code signing & supply chain

| Property | Status | Note |
|---|---|---|
| Sig → manifest → ELF-hash chain | **ENFORCED** | `rae_manifest::lookup` verifies Ed25519 over the staged manifest, then the ELF SHA-256, with the in-kernel KAT-proven verifier. |
| Tamper fail-close | **ENFORCED** | bad sig / hash mismatch / signed-without-hash rejects the whole bundle (tampered ≠ unsigned). Proof: `reject_tamper=true`. |
| Unsigned = "unverified developer", runs not blocked | **ENFORCED** | Concept policy: clear posture, not punitive. |
| Verified sig as trust root for `sandbox="trusted"` | **ENFORCED** | a valid signature lets a non-first-party app reach Trusted without the allowlist. |
| Developer PKI / per-dev certs / **revocation** | **PLANNED** | the dev key in `keys/` is a single shared trust root for development only (`keys/README.md`). There is no way to revoke a compromised developer or tier store trust. **A valid signature ≠ a vetted developer** (adversary B). |
| Key rotation | **PLANNED** | embedded pubkey is compile-time fixed. |

---

## 8. Cryptographic foundation

All KAT-proven against published vectors (not self-consistency), and every
*unimplemented* primitive is **fail-closed** (`Err(NotSupported)`) so a stub can
never be mistaken for a valid signature or real ciphertext — this is the property
that makes the rest trustworthy.

| Primitive | Status | KAT |
|---|---|---|
| Ed25519 (sign/verify) | **ENFORCED** | RFC 8032; `ed25519 KAT … -> PASS` |
| X25519 (ECDH) | **ENFORCED** | RFC 7748 §5.2/§6.1 |
| ChaCha20-Poly1305 AEAD | **ENFORCED** | RFC 8439 + forgery-reject |
| AES-GCM, HMAC, SHA-256, BLAKE2s | **ENFORCED** | RFC 4231 / published vectors |
| ECDSA P-256 verify | **ENFORCED** | OpenSSL vector |
| RSA-PSS, FF-DH, P-384/P-521 | **PLANNED / fail-closed** | return `NotSupported` until real bignum modexp lands. |
| TLS 1.3 full handshake | **PLANNED** | only ClientHello framing today — outbound TLS is NOT yet confidential. |

---

## 9. The unrooted-chain gap (read this before trusting anything above)

Everything in §3–§8 executes inside a kernel that is **not itself verified at
boot**. The following are all PLANNED:

- Secure boot chain: bootloader → kernel → init → compositor measurement.
- TPM 2.0 sealing of root/bucket keys.
- Full-disk encryption (data at rest).
- A/B atomic update slots with signature verification (tamper-evident updates).

**Consequence, stated bluntly:** an attacker who can write the boot image, the
kernel binary, or the raw disk defeats the entire model. AthenaOS today is a
strong **runtime app-isolation** system and a weak **offline-tamper-resistance**
system. The roadmap closes this (MasterChecklist Phase 9.2/9.3 + Phase 4), but
until it does, do not describe AthenaOS as "secure boot" or "encrypted at rest" to
users or partners.

---

## 10. Defense priority (what to build next, in order)

1. **Secure boot + measured kernel (§9)** — roots everything else; without it the
   rest is defense-in-depth on sand.
2. **Compositor permission-prompt consumer (§5)** — turns DESIGNED caps
   (Camera/mic/etc.) into real mint-time consent.
3. **Expand sandbox gate classes (§5)** — file/IPC/mmap for Strict, so the
   "syscall-layer enforcement" claim becomes true rather than aspirational.
4. **Malicious-app acceptance test (§6)** — prove the anti-ransomware boundary
   with a hostile process, not just in-kernel smoketests.
5. **FDE + TPM sealing (§9)** — protect data at rest.
6. **Developer PKI + revocation (§7)** — make signatures mean "vetted," not just
   "unmodified."

---

## 11. Maintenance contract

When you land security-relevant work, in the SAME commit:
1. Flip the relevant status cell here (and cite the boot marker for ENFORCED).
2. If you added a sandbox class, update both `class_of` and `class_of_linux`.
3. If you added a `Cap` flavor, update `docs/CAPABILITIES.md` AND the §4 table.
4. If a claim here becomes true or false, fix the wording — present tense is
   reserved for what's actually wired.
