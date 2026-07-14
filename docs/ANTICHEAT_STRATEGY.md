# Anti-Cheat Strategy (PARKED — not an AthenaOS product goal)

> **Athena note:** Competitive-game anti-cheat partnerships are **abandoned** for
> AthenaOS. See [`LEGACY_GAMING_CONCEPT.md`](../LEGACY_GAMING_CONCEPT.md) and
> [`PARKED_GAMING.md`](PARKED_GAMING.md). Attestation primitives in
> `kernel/src/anticheat.rs` may later feed **body/safety attestation**, not EAC/BattlEye.

**Historical bootstrap text follows (do not treat as roadmap).**

**Anti-cheat exists for the game publishers, not the user.** Its purpose is to let
competitive titles — Fortnite, Valorant, Overwatch, Apex, Destiny 2, PUBG, R6
Siege — run on AthenaOS *without* giving cheaters an easy exploit surface, so those
publishers will certify the platform. A embodiment-first OS that can't run these games
has failed at its one job. This is precisely where Linux loses: it refuses kernel
anti-cheat, so the kernel-AC titles are blocked.

AthenaOS solves it with **two tiers**. Tier 1 is the better primitive we offer
everyone; Tier 2 is the pragmatic compatibility path for titles that mandate a
kernel vantage point.

---

## Tier 1 — Userspace hardware-backed attestation  *(default; built)*

The kernel exposes an attestation API that EAC / BattlEye / Vanguard can query from
**user-space, without a ring-0 driver**. Backed by `kernel/src/anticheat.rs`:

- Continuous process-integrity monitoring (code-page hashing, W^X enforcement,
  stack-canary, hook/debugger detection).
- Kernel self-defense (syscall table, IDT, `.text` integrity).
- A signed attestation (TPM-backed where present) the vendor's servers can trust:
  "this process is unmodified, the kernel is intact, secure boot held."
- `SYS_AC_*` syscalls, `/proc/athena/anticheat`, boot smoketest proving the
  detection logic fires (`wx/clean/tamper/canary/hook/ban/attest -> PASS`).

> **Concrete API reference:** [`ATTESTATION_API.md`](ATTESTATION_API.md) — the measured-boot
> PCR model, the `quote(selection, nonce)` primitive, the `SYS_AC_*` session syscalls
> (table 284–290), the remote-verification flow, and the capability gate.

**Preferred** because it's the user-respecting model: no permanent ring-0 code, full
transparency, crash-isolated. We push every vendor here first. But some publishers
will not accept a userspace-only model — hence Tier 2.

---

## Tier 2 — Sanctioned kernel anti-cheat, only for games that require it  *(planned)*

A **defined, signed, per-game kernel anti-cheat module slot** that a certified vendor
ports their detection engine to. It is *not* arbitrary ring-0 code and *not*
boot-resident. The design constraints are the whole point — AthenaOS gives the kernel
access the publisher demands while removing the things users (rightly) hate about
Windows kernel anti-cheat:

| Property | Windows (Vanguard/EAC-kernel) | AthenaOS Tier 2 |
|---|---|---|
| When it loads | At **boot**, always resident | **On game launch only**, unloaded on exit |
| User awareness | Opaque; installs a driver silently | Explicit **consent prompt**, listed + revocable in Settings |
| Scope | Full ring-0, persistent | Bounded kernel API surface, audited, non-persistent |
| Trust chain | Vendor-signed only | Vendor-signed **and** AthenaOS-countersigned (approved-AC registry) |
| Removal | Often survives game uninstall | Uninstalls with the game |

### How it works
1. A game's `RaeManifest` declares `requires_kernel_anticheat = "eac" | "battleye" |
   "vanguard" | …`.
2. On launch, AthGuard raises a **distinct high-privilege consent prompt**
   (`perm_prompt`): *"Fortnite requires Easy Anti-Cheat to run at kernel level. This
   grants it deep system access while the game is running. Allow / Don't run."*
3. On consent, the kernel loads the **countersigned** AC module from the approved-AC
   registry into a constrained kernel context (its own region, W^X + IOMMU enforced on
   it too, a fixed audited API — no free-form kernel writes, no kernel-text
   modification, no cross-reboot persistence).
4. The module uses the **same detection primitives** Tier 1 exposes
   (`MemoryProtectionEngine`: code hashing, hook/debugger detection, module
   enumeration) plus a secure channel to the vendor's userspace + servers.
5. On game exit (or crash), the module is **unloaded** and its grant revoked. Every
   action it took is in the audit log (`/proc/athena/anticheat` + `audit`).

### Why publishers accept it
A real kernel vantage point exists and the platform is certifiable — the thing Linux
can't offer. Vendors already port to consoles' constrained kernels; this is the same
shape of effort against a published **Kernel Anti-Cheat SDK** (the fixed API surface
above).

### Why it's still better for the user than Windows
Load-on-launch (not a 24/7 boot driver), visible and revocable, bounded + audited,
and gone when the game is uninstalled. "Ring-0 on a leash," not a permanent rootkit.

---

## Security posture (Tier 2 is a *deliberate, scoped* exception)

AthenaOS's whole model is capabilities + sandboxing + no arbitrary ring-0. Tier 2 is a
consciously bounded exception, justified only by the game-compatibility need, and
contained by: countersigning (only vetted vendor modules load), per-game + per-launch
scoping, the fixed API surface, W^X/IOMMU enforced on the module, full audit, and
explicit revocable user consent. It is never on by default and never required for
non-AC software.

---

## Settings & user control (see SETTINGS_CATALOG.md §10/§12)

- Per-game: "Kernel anti-cheat: required / loaded / off" + revoke.
- A global list: "Games allowed to load kernel anti-cheat."
- Audit view: what each AC module did this session.
- A hard user-side off switch: refuse kernel AC entirely (those games then won't run —
  the user's informed choice).

---

## Build status

- **Tier 1:** built and QEMU-proven (`anticheat.rs`, detection smoketest PASS).
- **Tier 2:** *strategy only.* Needs: the Kernel Anti-Cheat SDK / module ABI, the
  countersigned approved-AC registry, the constrained module-load mechanism, the
  `requires_kernel_anticheat` manifest field + consent flow, and (the long pole)
  vendor business-development to port EAC/BattlEye/Vanguard. Multi-quarter; gated on
  the platform being attractive enough that publishers engage.

> The kernel-side detection building blocks already exist in `anticheat.rs` — Tier 2
> is primarily an *interface + trust + lifecycle + BD* effort, not a from-scratch
> detection engine.
