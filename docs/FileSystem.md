# AthFS Architecture and User-Experience Specification

AthenaOS uses the custom **AthFS** filesystem to deliver a tree that is
human-readable, self-cleaning, crash-safe, and structurally enforces security
and gaming priorities. This document is both the **UX contract** (what the user
sees) and the **architecture plan** (how AthFS delivers it), with an honest
**implementation-status** map to the code in `kernel/src/athfs.rs` and friends.

> Conventions: `[x]` shipped + proven · `[~]` implemented, QEMU-verified ·
> `[ ]` planned. AthFS follows the no-Linux-clone rule (§CLAUDE.md R7): no ext4,
> no FUSE; internal volumes are AthFS-native, foreign media is handled by
> purpose-built read paths.

---

## Part I — The Namespace (what the user sees)

### The Apex root

AthenaOS abandons drive letters (`C:\`, `D:\`). The root is **`PC`** / **`Apex`**
(`/`). It contains exactly five canonical directories and **no** hidden system
files or untracked dumps at the root:

| Path | Role | Backing |
|---|---|---|
| `/System` | Immutable core (kernel, drivers, frameworks) | A/B CoW slots, signed |
| `/Apps` | `.app` bundles (drag-and-drop, zero-trace) | Per-bundle subtree |
| `/Users` | Per-user `Documents/Downloads/Media/Projects` + `Configs` + `Data` | Encrypted subtree |
| `/Games` | High-performance tier (game-aware extents, tiered routing) | Contiguous extents, tiered |
| `/Vaults` | Dynamic mount point for secondary/external drives (by name, not letter) | AthFS-native or foreign-FS bridge |

> **✅ Done:** `athfs::format()` now seeds exactly the Apex tree (`/System /Apps
> /Users /Games /Vaults`, inodes 2–6) **and writes real named `DirEntry` records**
> into the root directory block (block 9), so the tree is navigable by name — not
> synthesized from a hidden manifest. Verified: `format_smoketest: … dirs=true
> apex_root=true -> PASS` (first entry confirmed `System→inode 2`, free-block
> accounting correct). `/var/crash` + `/var/log` are flat-named artifacts, not a
> root entry, so the "exactly five" rule holds. `[~]` *(Remaining: the in-RAM
> bootstrap `AthFS::format() -> Self` used at live mount doesn't seed the named
> tree yet — separate, lower-risk follow-up.)*

### `/System` — the immutable core

* **Immutability:** read-only to users and apps under standard operation.
* **A/B atomic updates:** an update is downloaded into the *inactive* `/System`
  slot (CoW, no disruption to the running slot), a snapshot is taken, and the
  bootloader is pointed at the new slot on reboot. **Boot-health fallback:** if
  the new slot doesn't reach userspace within N seconds, the slot is marked bad
  and the previous slot is restored automatically. (Ties to MasterChecklist
  Phase 3.6 "atomic kernel updates"; installer writes slot A today, B-slot +
  health rollback `[ ]`.)
* **Signed, not necessarily encrypted:** `/System` integrity comes from a
  signature chain (Phase 3.7 secure boot), not confidentiality — it must be
  verifiable, not secret.

### `/Apps` — the drag-and-drop vault

* **Bundle paradigm:** every app is a `.app` bundle — a self-contained subtree
  presented as one icon. **Defined layout:**
  ```
  Photos.app/
    RaeManifest.toml      # declared capabilities + sandbox level + data-bucket id
    bin/photos.elf        # entry executable
    lib/                  # private dependencies (no shared system libs leak in)
    assets/               # icons, resources
  ```
* **Zero-trace uninstall:** no central registry. Install = move into `/Apps`;
  uninstall = delete the bundle. All binaries/deps/assets are sandboxed inside.
* **Manifest-driven sandbox:** `RaeManifest.toml` declares the app's sandbox
  level (Trusted/AppSandbox/Strict — see `sandbox.rs`) and per-capability grants
  (Network, Camera, `/Users/Documents`, …). Absent/unknown apps default to
  `AppSandbox`. (Phase 9; classifier `[~]`, full manifest parse `[ ]`.)

### `/Users` — the personal sandbox

`/Users/[User]/` holds `Documents`, `Downloads`, `Media`, `Projects`, plus:

* **`/Users/[User]/Configs`** — versioned `.rcfg` files. AthFS natively versions
  every config write, so the Settings app exposes a **History slider** for
  one-click rollback (whole-snapshot **and** per-setting). Backed by the
  `config_registry` generation journal. `[~]`
* **`/Users/[User]/Data`** — **per-app data buckets**: each app gets exactly one
  folder for caches/saves, isolated from every other app's bucket and encrypted
  with a per-app key. Deleting an app prompts for data retention. `[~]`

### `/Games` — the high-performance tier

* **Game-aware extents:** any folder under `/Games` triggers contiguous block
  pre-allocation + large-sequential-read optimization on NVMe (`game_install_hint`,
  syscall 99). `[~]`
* **Sequential prefetch:** read patterns matching a game's extents prefetch the
  next window (`prefetch_extent`). `[~]`
* **Tiered routing:** `/Games` is one logical library across physical drives;
  hot games are promoted to NVMe, dormant ones demoted to SATA/HDD. `[~]`

### `/Vaults` — external & secondary drives

Secondary/USB drives mount by **name**, not letter: drive "Archive" → `/Vaults/Archive`.

* **AthFS-native drives:** mounted directly, full feature set.
* **Foreign media (FAT32/exFAT, the common USB-stick case):** handled by a
  **purpose-built, read-first bridge** (`fatfs_esp.rs` already parses FAT32 BPB +
  directory + cluster chains and reads files). Plan: FAT32 read `[~]`; bounded
  FAT32 write to pre-existing files `[~]` (used by the bootlog); exFAT read `[ ]`;
  NTFS read `[ ]` (read-only, no driver port — translation shim only). This keeps
  the no-Linux-clone rule while still letting AthenaOS read the user's existing
  sticks. USB enumeration via the in-kernel MSC driver (`usb_msc.rs`). `[~]`

---

## Part II — AthFS On-Disk Architecture (how the UX is delivered)

The namespace above rides on these layers (all in `kernel/src/athfs.rs` unless noted):

1. **Superblock** (magic `0x5261654653_5321`) — geometry, bitmaps, root inode,
   snapshot/refcount/bucket table pointers, encryption + compression flags. `[~]`
2. **Inodes + extents** — small files use 12 direct blocks; large files migrate
   to a **B-tree of extents** (`BTreeLeafEntry { logical, physical, length,
   flags }`). Extent flags carry `ENCRYPTED` / `COMPRESSED` / `GAME`. `[~]`
3. **Copy-on-write + refcounts** — blocks are never overwritten in place when
   shared; a per-block refcount table enables snapshots and cheap clones. `[~]`
4. **Journal / WAL** — metadata mutations are journaled; a dirty mount replays
   the journal to a consistent state. `fsck` (bitmap/refcount coherence, B-tree
   integrity, orphan-inode reclaim) verifies + repairs. `[~]`
5. **Snapshots** — `create`/`rollback`/`delete` with CoW refcount bumps; exposed
   to userspace as `SYS_ATHFS_SNAPSHOT_CREATE/RESTORE/DELETE` (101–103). `[~]`
6. **Encryption** — XTS-AES-256 per 512-byte sector, tweak = block number
   (`encrypt_data_block`/`decrypt_data_block`). Cipher core proven against the
   FIPS-197 known-answer vector. `[~]`
7. **Compression** — transparent LZ4-style coder with a per-block header + a
   per-extent `COMPRESSED` flag; live ratio accounting at `/proc/athena/athfs`
   (kernel logs compress to ~17× — see Part IV). zstd decoder lives in the
   `components/athfs` userspace half. `[~]`
8. **Tiered storage** — devices classified NVMe / SATA / HDD; access-frequency
   hot/cold migration; game-install hot-pin. `[~]`
9. **Per-app data buckets** — `create_bucket(app_id)` registers an isolated
   subtree root with capability flags (read/write own/shared, create-temp) and a
   block quota; `open_in_bucket(app_id, …)` resolves names only within that
   bucket. `[~]`

### Encryption & key hierarchy (the security spine)

```
Master key  ──(FDE, LUKS-equivalent, passphrase→KDF)──▶  /Users volume
   │
   └─ per-app bucket key = derive(master, app_id, "raebkt")   # FSCRYPT-equivalent
                                          │
                                          ▼
                          /Users/[User]/Data/<app>/ encrypted with its OWN key
```

* **`/System`:** signed (integrity), not encrypted (must boot before any key).
* **`/Users`:** full-volume encryption under a passphrase-derived master key
  (FDE). KDF is HKDF-SHA256 today; **Argon2 (memory-hard) is the planned
  upgrade** for passphrase hardening. `[~]` core / `[ ]` Argon2 + boot-time unlock.
* **Per-app `/Data` buckets:** each derives a distinct key from the master +
  `app_id` (domain-tagged), so a key leak in one bucket can't decrypt another —
  the structural defense behind the Concept's "a malicious app cannot read
  another app's data." Proven: a block encrypted under app A's key does **not**
  decrypt to plaintext under app B's key. `[~]`
* **TPM unseal** at boot (so the user isn't prompted on a trusted machine) — `[ ]`.

### Consistency & durability model

* **Atomicity:** metadata changes are journaled (WAL) and CoW; a power loss
  rolls forward via journal replay on next mount, never leaving a torn tree.
* **Cache flush:** writers that must survive a power-cycle (e.g. the bootlog,
  installer) issue an explicit device cache-sync — data in a controller's DRAM
  is otherwise lost on power-off.
* **fsck:** runs the integrity passes above; a userspace `athfsck` mirrors the
  logic for offline repair (`components/athfs/src/fsck.rs`).

### Capability → path enforcement (who can touch what)

| Path | Default policy | Enforced by |
|---|---|---|
| `/System` | read-only for all; write needs "Unrestricted Mode" | write-lock + `Cap::System{WRITE}` |
| `/Apps/<x>.app` | the app reads its own bundle only | bundle-scoped resolution |
| `/Users/[U]/Data/<app>` | only `<app>` (its bucket) | `Cap::Filesystem{bucket_root}` + bucket caps |
| `/Users/[U]/Documents` | apps need a declared grant | manifest capability + prompt |
| `/Vaults/<drive>` | user-granted per app | capability + Settings "Capabilities" page |

Every privileged path operation routes through `crate::capability` (the single
authority, §R3). The bucket layer proves cross-app denial today; the broader
manifest-driven path grants are Phase 9. `[~]` buckets / `[ ]` manifest grants.

### Snapshots as Time Machine

* **Retention policy:** hourly → daily → weekly thinning (keep N of each), so the
  whole FS has a browsable history, not just configs. `[ ]`
* **Snapshot quota:** snapshots are capped (by count + total CoW bytes) so they
  can't silently fill the drive; oldest-thinnest are reclaimed first. `[ ]`
* **UX:** a global "history slider" in the file browser + per-`.rcfg` slider in
  Settings, both backed by the same snapshot/version machinery.

---

## Part III — OS UX & System Behaviors

### 1. The "Glass Wall" system protection
System data is **visible but enclosed** — no opaque "Access Denied" or hidden
dirs. Viewing `/System` is allowed; a modify attempt raises an *educational*
prompt explaining the CoW-update constraint. **Override Key:** a "System Safety"
page in Settings toggles "Unrestricted Mode" (one warning, no terminal needed),
flipping the write-locks.

### 2. Guided game routing & storage pools
Running an installer/`.app` prompts for a storage target: **Default → `/Games`**
(tiered, optimized) or **Custom → a `/Vaults/` location**. Power users get a
**node-based routing graph** to map the `/Games` node to a specific NVMe, or an
"Archive HDD" to `/Vaults/Mods` — granular data-flow control.

### 3. Unified Control Center
One settings app, human-readable categories (*Appearance, Hardware & Power,
Network, Privacy & Capabilities, Storage Vaults*). A **Capabilities dashboard**
inspects each `.app`'s granted permissions. Every page has a **"Revert Changes"
history slider** because all config is versioned `.rcfg`.

### The `.rcfg` versioned-config format
* **Shape:** a flat key→value tree (`Text`/`Int`/`Bool`/`Bytes`), e.g.
  `/display/refresh_hz = 60`, surfaced at `/proc/athena/config`.
* **Versioning:** every `set` bumps a monotonic *generation* and journals the
  prior value. **Whole-snapshot rollback** (snapshot a generation, roll back all
  writes since) **and per-setting restore** (roll one key back to its value at a
  prior generation, leaving others untouched) are both implemented. `[~]`
* **Persistence:** in-kernel journal today; on-disk `.rcfg` materialization under
  `/Users/[User]/Configs` with reboot round-trip is the next step. `[ ]`

---

## Part IV — Implementation Status (spec → code → state)

| Capability | Code | State |
|---|---|---|
| CoW + per-block refcounts | `athfs.rs` | `[~]` |
| Snapshots (create/rollback/delete) + syscalls 101–103 | `athfs.rs`, `syscall.rs` | `[~]` |
| Journal/WAL replay + fsck (integrity/btree/orphan) | `athfs.rs`, `components/athfs/fsck.rs` | `[~]` |
| `mkfs` / `format()` | `athfs::format` | `[~]` (but seeds Unix tree — see gap) |
| XTS-AES-256 block encryption (FIPS-197 KAT) | `athfs.rs`, `crypto.rs` | `[~]` |
| Per-app bucket keys (FSCRYPT-equiv) | `athfs::bucket_encryption_key` | `[~]` |
| Full-disk encryption (FDE) at boot | `encryption.rs` | `[ ]` (Argon2 + TPM unlock) |
| Compression (LZ4-style + per-extent flag + ratio) | `athfs.rs` | `[~]` (`/var/log` 17.2×) |
| Tiered storage (NVMe/SATA/HDD, hot/cold) | `athfs.rs` | `[~]` |
| Game-aware extents + sequential prefetch | `athfs.rs` | `[~]` |
| Per-app data buckets (isolation + quota + caps) | `athfs.rs`, `data_buckets.rs` | `[~]` |
| Versioned config (whole + per-key restore) | `config_registry.rs` | `[~]` |
| FAT32 foreign-media read/bounded-write (`/Vaults`) | `fatfs_esp.rs`, `usb_msc.rs` | `[~]` |
| Apex root tree (`/System /Apps /Users /Games /Vaults`) | `athfs::format` | `[~]` (mkfs writes named entries; in-RAM bootstrap pending) |
| A/B `/System` atomic update + boot-health rollback | `installer.rs`, bootloader | `[ ]` (slot A only) |
| `.app` bundle + `RaeManifest.toml` parse/enforce | `app_bundle.rs`, `sandbox.rs` | `[~]` classifier / `[ ]` manifest |
| Time-Machine retention + snapshot quota | — | `[ ]` |
| exFAT/NTFS read shim for `/Vaults` | — | `[ ]` |

## Part V — Prioritized improvements (the plan, in order)

1. ~~Align the on-disk root with the spec~~ — **DONE**: `format()` writes the
   Apex tree with named entries (`format_smoketest … apex_root=true`). Follow-up:
   seed the same named tree in the in-RAM bootstrap `AthFS::format()`.
2. **Longer + Unicode names** — AthFS directory entries cap at 55 bytes with no
   true long-filename support; human-readable paths (`Counter-Strike 2`,
   `My Résumé.rcfg`) need an LFN/UTF-8 name scheme. *(Foundational; touches every
   path-facing feature.)*
3. **`/Users` FDE + per-app `/Data` keys at the mount boundary** — wire the
   already-proven per-app key derivation into the bucket open/read/write path so
   `/Data` is transparently encrypted, then layer Argon2 + TPM unlock.
4. **A/B `/System` + boot-health rollback** — the Concept's "one-click rollback"
   for the OS itself (Phase 3.6).
5. **`RaeManifest.toml` parse + path-capability enforcement** — make `/Apps`
   sandboxing manifest-driven (Phase 9).
6. **Time-Machine retention + snapshot quota** — turn the snapshot primitive into
   the browsable history the UX promises.
7. **`/Vaults` foreign-FS breadth** — exFAT read, NTFS read shim, name-based
   automount.
