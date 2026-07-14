# RaeenOS Update & Recovery Model

A daily-driver OS lives or dies on whether updates are **safe**. The Windows/Linux
failure mode — a half-applied update leaves an unbootable machine — is unacceptable.
RaeenOS's rule: **an update can never brick the box.** A failed or bad update boots
straight back into the last-known-good system, automatically, with no user action.

This is the design for atomic kernel updates (A/B slots), signed update delivery, and
the recovery paths. Tracks MasterChecklist Phase 3.6.

---

## 1. The guarantees

1. **Atomic** — an update is either fully applied or not at all; no partial state is ever
   booted.
2. **Auto-rollback** — if a freshly-updated slot fails to reach a healthy boot, the
   bootloader falls back to the previous slot on the next attempt, automatically.
3. **Signed** — only an update whose signature verifies against the trust anchor is ever
   written or booted (no unsigned/forged kernel boots).
4. **Reversible userspace** — system/app state changes are RaeFS snapshots, so "undo the
   update" restores data too, not just the kernel.

---

## 2. A/B slot model (the kernel/system half)

Two complete system slots on disk, **A** and **B**. One is *active* (running), the other
*inactive* (the update target).

```
update flow:
  1. raeupdate downloads image + detached signature
  2. verify signature against the trust anchor (secure_boot::verify_against_anchor)
  3. write the image into the INACTIVE slot (active slot untouched, still bootable)
  4. mark inactive slot "pending", set boot-attempts = 0, flip the boot pointer
  5. reboot → bootloader tries the pending slot
  6. on a healthy boot, userspace sets the slot "good" (commit)
  7. if N boot attempts fail → bootloader reverts the pointer to the old slot
```

**State per slot:** `{ good | pending | bad }` + a boot-attempt counter, stored where the
bootloader can read AND write it (a small slot-state region, not inside either kernel).

### The current blocker (be honest)
RaeenOS's bootloader today opens a **fixed path** (`kernel-x86_64` at the ESP root, no
config file). A/B requires a bootloader that can (a) read slot state, (b) pick a slot,
(c) decrement the attempt counter and revert on repeated failure. **So A/B is gated on a
slot-aware bootloader** — that's the first real work item here, not the update daemon.
Until then, updates are whole-image replace with a manual recovery USB as the only
fallback. (See the install-spine notes: the fixed-path bootloader is the same constraint
that blocks config-file boot.)

---

## 3. Signed delivery (raeupdate)

- **`raeupdate` daemon** fetches an update bundle (image + detached Ed25519 signature +
  metadata: version, slot, rollback-min).
- **Verification reuses the built trust anchor:** `secure_boot::verify_against_anchor`
  (embedded Ed25519 public key, offline-signed by `tools/raesign`) — the same primitive
  proven by `[secboot] trust-anchor verify -> PASS`. A bad/forged/old image is rejected
  before it's written.
- **Anti-rollback:** metadata carries a minimum version; the updater refuses to install an
  image older than the rollback floor (stops downgrade-to-vulnerable attacks).

---

## 4. Userspace / data rollback (the RaeFS half — already works)

The system half (A/B) handles the kernel + base system. User/app data uses **RaeFS
snapshots**, which are **already proven** (Phase 5, one-click rollback round-trips in
QEMU):

- Before applying an update, raeupdate takes a RaeFS snapshot.
- "Roll back this update" = revert the boot slot **and** restore the pre-update snapshot —
  kernel and data both return to the known-good point.
- Routine "system restore" points are the same mechanism, on a schedule.

This is a genuine advantage over Windows System Restore / mac Time Machine: it's CoW,
instant, and atomic, not a background file copy.

---

## 5. Recovery paths (in order of severity)

| Failure | Recovery | Built? |
|---|---|---|
| Update boots but misbehaves | User picks "roll back" → flip slot + restore snapshot | 🟡 snapshot side done |
| Update fails to reach healthy boot | Bootloader auto-reverts after N attempts | ⬜ needs slot-aware bootloader |
| Both slots bad / corrupt FS | Recovery USB: re-flash + preserve `/Users` `/Vaults` | 🟡 installer exists |
| Secure-boot/anchor failure | Refuse to boot tampered kernel; recovery USB | 🟡 anchor verify done |
| User data only | Restore a RaeFS snapshot from the recovery menu | 🟡 |

**Healthy-boot definition:** userspace reaches a defined "system good" checkpoint (the
boot marker + a settle period) and writes the slot `good`. Anything short of that — panic,
hang, watchdog — counts as a failed attempt and feeds auto-rollback.

---

## 6. Build order

1. **Slot-state region + a slot-aware bootloader** — read/pick/decrement/revert. The
   load-bearing prerequisite; nothing else works without it.
2. **`raeupdate` write-to-inactive-slot + signature verify** (verify primitive exists).
3. **Healthy-boot commit + attempt-counter auto-revert.**
4. **Snapshot-on-update + unified "roll back update" (slot + snapshot).**
5. **Recovery USB menu** (re-flash preserving user vaults).

Each step proven on the ladder in `TESTING_STRATEGY.md`: boot smoketest for slot-state
logic, QEMU for the full flip/revert cycle, iron for the real flash. `[~]` until an
Athena update round-trips; `[x]` only then.
