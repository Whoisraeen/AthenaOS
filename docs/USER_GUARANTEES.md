# AthenaOS User Guarantees

> LEGACY_GAMING_CONCEPT.md §2: *"The user owns the machine. No forced updates. No telemetry
> without explicit opt-in. No ads in the OS."* — and §"No ads. No data sales. Ever.
> **Burned into the EULA.**"

These are not marketing promises. They are **binding product guarantees** that belong
in the AthenaOS End-User License Agreement, and — uniquely — each one is **enforced by
the architecture**, not just asserted in prose. This document is the canonical text of
those guarantees plus the technical mechanism that makes each one true.

A future legal EULA incorporates this file as its "Your Rights" section verbatim.

---

## 1. No ads in the operating system. Ever.

AthenaOS will never display advertisements in the shell, the Start menu, the lock
screen, Settings, Search, the file manager, or any first-party surface. There is no
ad SDK, no sponsored placement, no "suggested app" that is a paid placement.

**Enforced by:** there is no advertising subsystem in the source tree. The Start
menu / Search surface (`search_index`, the command palette) ranks only the user's
own apps, files, and settings actions — there is no ad inventory to inject.

## 2. No data sales. Ever.

AthenaOS does not collect, broker, or sell user data. There is no data-broker
relationship, no behavioral-profile export, no "anonymized analytics" sold downstream.

**Enforced by:** no telemetry-exfiltration path exists. AthSync (`components/raesync`)
is **end-to-end encrypted** (x25519 + HKDF + ChaCha20-Poly1305) — AthenaOS servers, if
present, see ciphertext only and could not sell what they cannot read. AthID
(`components/raeid`) is **optional**; the OS boots to a full guest desktop with no
account.

## 3. No telemetry without explicit, revocable opt-in.

No usage data leaves the machine unless the user has explicitly opted in, and opt-in
is revocable at any time. The default state is **off**.

**Enforced by:** there is no always-on telemetry daemon. Diagnostic surfaces
(`/proc/raeen/*`, the boot log, netlog) are **local**, developer-facing, and not
transmitted off-box except by an explicit user action (e.g. attaching a bootlog to a
bug report).

## 4. No forced updates. The user controls the schedule.

The OS will never install an update the user did not consent to, and never reboots
into one without permission.

**Enforced by:** `components/raeupdate` gates auto-install on a **user-controlled
policy** — `Disabled` never auto-installs (host-KAT `auto_update_is_user_consent_gated`),
the shipped default `SecurityOnly` installs only Critical/Security fixes, and `All`
is strictly opt-in. Update **channels** (Stable ⊆ Beta ⊆ Nightly) are the user's
choice, defaulting to the most conservative. Every update is Ed25519-signed and
applied atomically to a standby A/B slot with **one-click rollback** — a bad update
can always be undone.

## 5. The user owns the machine.

Privileged operations are the user's to grant, not the OS's to assume. Apps are
sandboxed by default and reach the system only through capabilities the user grants.

**Enforced by:** every privileged op flows through `crate::capability` (the `Cap`
enum). Apps run under per-app encrypted data buckets (one app cannot read another's —
`cross_app_unreadable`, real-Ryzen-verified). Even accessibility tooling needs an
explicit `Cap::Accessibility` grant (no app reads another's UI tree unprompted). The
anti-cheat answer is a **userspace attestation API** (`docs/ATTESTATION_API.md`), not
a vendor kernel driver — see also `docs/ANTICHEAT_STRATEGY.md`.

## 6. No lock-in by friction.

The default shell (AthShell) can be replaced (`kernel::shell_api` — register/switch
shells), the window manager is swappable (float/tile/stack), and the whole look is
user-owned via the theme engine + Vibe Mode. Nothing about leaving is engineered to
be painful.

---

## Status

The **guarantees above are written and architecturally backed today** (the enforcing
mechanisms are built + host-KAT'd / real-Ryzen-verified, as cited). The remaining step
for the checklist item is purely legal/business: incorporating this text into the
shipped EULA when the legal entity + license are finalized — that is a company action,
not an engineering one.
