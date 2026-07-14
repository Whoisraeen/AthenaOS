# Spec: RaeFS data-integrity / per-block checksums (detect-only)

## Concept promise served

> "**CoW with snapshots** — instant rollback, time-machine-style backups, atomic system
> updates that never half-apply" (§File System: RaeFS, line 66)

> "a system that **resists ransomware structurally** ... where malware infections are
> bounded, and where you can run untrusted software without fear." (§Security Model, line 142)

A CoW filesystem whose blocks can silently bit-rot or be tampered with is not "instant
rollback you can trust" — a corrupt snapshot restores corruption. The integrity contract
is the missing half of the RaeFS promise: **every block carries a strong checksum, and a
read that does not verify returns an error instead of silently serving corrupt or tampered
bytes.** This is the ZFS/Btrfs "checksum everything" property adapted to RaeFS's CoW +
extent-B-tree + XTS-encrypt + LZ4-compress layout.

## Already in the tree (verify-before-implement)

All paths are `kernel/src/raefs.rs` unless noted. Status is the real ladder ([x] iron / [~] QEMU / [ ] none).

- **Data write path** `write_data_block(block_idx, data)` (line 4390) — compress (LZ4, header byte
  `BLOCK_HDR_COMPRESSED=0x01` at `raw[0]`, len in `raw[1..3]`) → **then** XTS-encrypt
  (`encrypt_data_block`, line 3305) → `write_block`. **[x]** (iron, compression+FDE proven).
- **Data read path** `read_data_block(block_idx, buf)` (line 4331) — `read_block` → **decrypt**
  (`decrypt_data_block`, line 3348) → **decompress** (if header byte present). No verification today.
  **[x]**.
- **Raw block I/O** `read_block`/`write_block` (lines 1015/1032) — 8×512B sectors per 4 KiB block via
  `ACTIVE_BLOCK_DEVICE` at `ROOT_PARTITION_LBA + block_idx*8`. All disk traffic funnels here.
  **Writes route through `block_io::safe_mode_guard_write` at the trait** (per CLAUDE.md §9). **[x]**.
- **Extent / B-tree leaf** `BTreeLeafEntry` (line 118, 32 bytes): `logical_start:u64,
  physical_block:u64, length_blocks:u32, flags:u32`. Flags `EXTENT_FLAG_ENCRYPTED=1<<0`,
  `_COMPRESSED=1<<1`, `_GAME=1<<2` (lines 125-127). **The 32-byte entry is full** — no spare
  field for a 32-bit checksum without growing it (would re-pack the `BTreeNode` union arity 127). **[x]**.
- **CoW divergence (journaled)** `cow_diverge_extent_journaled` (line 2090) — allocate→`journal_begin`
  →`write_data_block`→`insert_extent`→`write_inode`→`journal_commit`→`dec_refcount`/`free`.
  This is the 2026-06-17 crash-consistency primitive; the checksum write must ride **inside** this
  envelope, never as a separate post-commit step. **[x]** (`run_cow_journal_crash_smoketest`, line 665).
- **Superblock** `Superblock` (line 28), exactly 1 block (`const _: assert size == BLOCK_SIZE`, line 75).
  Has `reserved: [u8; BLOCK_SIZE - 176]` tail; the 176-byte head is fully accounted (last fields
  `block_bitmap_blocks`/`refcount_blocks`, the multi-block-bitmap "Landmine-1" addition). Legacy-zero
  fields are normalised on mount by `normalise_region_counts` (line 1911). **[x]**.
- **fsck** `fsck_integrity` (line 2436, bitmap↔refcount span-safe walk), `fsck_btree_integrity`
  (line 2570), `fsck_orphan_inode_cleanup` (line 2501). No data-checksum scrub exists. **[~]**.
- **R10 smoketests** `RaeFS::run_boot_smoketest` (line 1255), `run_cow_journal_crash_smoketest`
  (line 665), `run_compression_flag_smoketest` (line 4452); all use `with_ram_raefs` (line 480) for a
  throwaway RAM volume in safe-mode. `/proc/raeen/raefs` via `proc_dump_text` (line 215). **[x]**.
- **Checksum primitives already in-tree:** `fatfs_esp::crc32_ieee` (line 871) — IEEE CRC32, table-free,
  used for GPT. `crypto::Blake2b256/512` (lines 53-54) — strong but heavy. **No CRC32C, no xxHash.**
- **A NON-integrity "checksum"** exists at line 5441 (versioned-config `DiskVersionEntry.checksum`) —
  a trivial additive sum over block pointers, **not** a data-integrity check. Do not conflate.

**Delta to build:** a per-physical-block strong checksum store, computed over the *on-disk
ciphertext*, written atomically with every block write, verified on every read (fail-loud), plus a
`fsck_scrub_checksums` pass and its R10 proof. Nothing else in the list is rebuilt.

## Prior art & OSS verdict

- **ZFS** — checksum (fletcher4 default, SHA-256 optional) stored in the **block pointer** of the
  *parent*, not beside the data → self-validating Merkle tree; verify-on-read returns EIO + triggers
  repair from a mirror. Mechanism we adapt: checksum-in-parent-metadata, fail-loud read. License CDDL —
  📖 **study/isolate**, do not vendor.
- **Btrfs** — separate **checksum tree** (CRC32C default, xxHash/SHA-256/BLAKE2 selectable), one tree
  keyed by logical byte offset, 4-byte CRC per 4 KiB. Verify-on-read → EIO; `btrfs scrub` walks every
  block. This is the closest model to RaeFS's separate-metadata-region layout. GPL-2 — 📖 **isolate**
  (pattern only, our impl is independent Rust).
- **CRC32C (Castagnoli)** — 32-bit, table-driven (or `crc32c` HW insn on SSE4.2). **Our kernel is
  soft-float / no guaranteed SSE (CLAUDE.md §10 #?, kernel-soft-float memory)** → must be the
  software table form (1 KiB lookup table or slice-by-8). Detects all bit-rot up to Hamming distance,
  burst errors, single/double-bit flips. ➕ **implement natively** (trivial pure-`no_std` logic).
- **xxHash3 / XXH64** — faster than CRC32C in software, 64-bit, *not* cryptographic. Crate `xxhash-rust`
  is MIT/permissive ➕ but adds a dependency for a hot path; the table-CRC32C is simpler to host-KAT
  against the public CRC32C vectors and small enough to inline. **Decision: CRC32C, software table.**
- **BLAKE2s-128 (truncated)** — cryptographically strong (resists *malicious* tampering, not just
  bit-rot). We already ship BLAKE2b (`crypto.rs`). Strong but ~10× the per-block cost of CRC32C on a
  soft-float core. **Reserved as an opt-in "paranoid" mode** for metadata only (follow-up), not the
  default — see Design §Algorithm.

**Verdict summary:** no new crate. Implement **CRC32C in software** as `raefs` internal logic
(host-KAT against RFC 3720 / iSCSI CRC32C public vectors). Respects Concept §R7 (no Linux lineage —
this is an independent CRC implementation, not a Btrfs port).

## Design

### Algorithm

**CRC32C (Castagnoli, poly 0x1EDC6F41, reflected, init 0xFFFFFFFF, xorout 0xFFFFFFFF)**, computed in
software via a 256-entry `u32` table built `const` at compile time. Rationale:

- Bit-rot / silent corruption is the dominant threat the Concept names ("never half-apply", "resists
  ransomware structurally" = *detect* the tamper). CRC32C catches all 1-bit and 2-bit errors, all odd
  bit-error counts, all burst errors ≤ 32 bits, and ~1−2⁻³² of random corruption — sufficient for the
  **detection** goal of this spec.
- Soft-float safe: pure integer table lookups, zero FP, zero SSE. Allocation-free.
- A determined attacker *can* forge a CRC32C, but on an **encrypted** volume the checksum is computed
  over ciphertext (see ordering) — to forge it an attacker must already control the XTS keystream,
  which is the encryption threat model, not the integrity one. For volumes that demand
  tamper-*resistance* not just tamper-*detection*, a `CKSUM_ALG_BLAKE2S128` value is reserved in the
  superblock (follow-up; the on-disk slot is 8 bytes so it holds a 128-bit truncation).

The checksum field is stored as **`u64`**: low 32 bits = CRC32C, high 32 bits = `0` for CRC32C
(room for a 64-bit hash later without a format bump). A reserved-zero `u64` also lets
`normalise_region_counts`-style legacy handling treat an all-zero entry as "unknown/not-yet-checksummed".

### Where the checksum lives — a dedicated **checksum region** (NOT in the leaf entry, NOT in the block)

Three candidates were weighed:

1. *Inline in `BTreeLeafEntry`* — rejected: the 32-byte entry is full; growing it re-packs the
   `BTreeNode` union (127→fewer entries) = on-disk B-tree format break + every extent path touched.
   Also does not cover metadata blocks (bitmaps, inode table, B-tree nodes themselves) which have no
   extent.
2. *In the existing per-block compression header* (`raw[0..3]`) — rejected: that header lives in the
   *plaintext* and is consumed by decompression; it cannot cover the ciphertext, and incompressible/raw
   blocks have no header. Also unusable for metadata blocks.
3. **A contiguous checksum table keyed by physical block index** — **chosen.** One `u64` per block,
   `BLOCK_SIZE/8 = 512` entries per checksum-table block, spanning `div_ceil(total_blocks, 512)` blocks,
   laid out in the reserved metadata region exactly like the multi-block bitmap/refcount runs
   ("Landmine-1" pattern). This covers **every** block uniformly — data, inode table, B-tree nodes,
   bitmaps — with one mechanism, keyed the same way refcounts already are (`block_id`).

   This mirrors Btrfs's separate checksum tree, simplified to a flat array because RaeFS already
   addresses everything by physical block index (same access pattern as `read_refcount`, line 1955).

**New superblock fields (format change — see Handoff):**
```
pub checksum_block:    u64,  // first block of the checksum table run (0 on legacy volumes)
pub checksum_blocks:   u64,  // run length = div_ceil(total_blocks, BLOCK_SIZE/8); 0 on legacy
pub checksum_alg:      u8,   // 0 = none/legacy, 1 = CRC32C (default), 2 = BLAKE2S128 (reserved)
pub checksum_enabled:  u8,   // mount-time gate, mirrors compression_enabled/encrypted
pub _pad_cksum:        [u8; 6],
```
These consume 24 bytes; the `reserved` tail shrinks `BLOCK_SIZE-176` → `BLOCK_SIZE-200`, and the
`const _: assert size == BLOCK_SIZE` (line 75) must be updated to match. Like the bitmap/refcount
fields, **a legacy superblock reads these as 0**; `normalise_region_counts` is extended (or a sibling
`normalise_checksum_region`) to detect `checksum_block == 0` and treat the volume as
**checksum-absent → verify-on-read is a no-op (returns Ok), scrub reports `unverified`**, never a
false FAIL. mkfs/format lays down the region and sets `checksum_enabled=1, checksum_alg=1`.

### Ordering vs encryption + compression — **checksum-the-ciphertext (encrypt-then-checksum)**

The checksum is computed over the **exact bytes written to disk** = the post-compression,
post-encryption buffer. Equivalently, on read it is verified **before** decrypt/decompress.

```
WRITE (extends write_data_block, line 4390):
    plaintext --LZ4--> compressed (header) --XTS-encrypt--> to_write    [unchanged today]
    cksum = crc32c(&to_write)                                           [NEW]
    write_block(block_idx, &to_write)                                   [unchanged]
    checksum_table[block_idx] = pack(cksum)                             [NEW — same envelope]

READ (extends read_data_block, line 4331):
    read_block(block_idx, &mut raw)                                     [unchanged]
    expected = checksum_table[block_idx]                                [NEW]
    if checksum_enabled && expected != 0:                               [NEW]
        if crc32c(&raw) != low32(expected): return Err(E_RAEFS_CKSUM)   [NEW — fail loud]
    decrypt(&mut raw); decompress(...)                                  [unchanged]
```

**Why ciphertext, not plaintext:**
- The job is to detect corruption of **what is physically stored**. A flipped bit on the platter/NAND
  corrupts ciphertext; checksumming plaintext would require a decrypt-then-compare that *cannot
  distinguish* "disk corruption" from "wrong key" and would mis-attribute XTS error propagation.
- Checksumming ciphertext also catches tampering by anything that writes the raw device under us
  (the ransomware/structural-resistance angle) **without** needing the key.
- It composes cleanly with the existing pipeline: the checksum brackets the *outermost* on-disk
  representation, so neither compression nor encryption ordering changes. (This is the same choice
  dm-integrity-over-dm-crypt and Btrfs-on-LUKS make: integrity wraps the stored bytes.)

Trade-off accepted: an *authenticated*-encryption-style guarantee (detect ciphertext substitution by
someone who can also forge the CRC) is out of scope — CRC32C is not a MAC. The reserved BLAKE2S128
alg + a per-volume keyed-MAC mode is the named follow-up for tamper-*resistance*; this spec delivers
tamper/bit-rot **detection**, which is what §142 ("resists ... structurally", detect-and-fail) and
§66 ("never half-apply", trustworthy rollback) require.

### Checksum-table accessors (mirror the refcount accessors)

```
fn read_block_checksum(sb, block_id) -> Option<u64>     // bounds-checked, indexes the run
fn write_block_checksum(sb, block_id, c: u64) -> Result<(),()>   // routes through write_block → guard
fn checksum_index_limit(sb) -> u64                      // = checksum_blocks * (BLOCK_SIZE/8)
```
Bounds-checked against `checksum_index_limit` exactly like `read_refcount` (line 1955) so a corrupt
`total_blocks` can never OOB-panic. `block_id >= limit` or `checksum_block == 0` → `None` →
verify-on-read treats as "unknown" → `Ok` (legacy/region-absent), scrub counts as `unverified`.

### Crash-consistency — the checksum write rides the existing journal envelope

The checksum-table update for `new_phys` is part of the **same** CoW transaction as the data write,
ordered so a crash leaves a state `replay_journal` already understands:

- In `cow_diverge_extent_journaled` (line 2090): after `write_data_block(new_phys, …)` succeeds
  (which now also writes `checksum_table[new_phys]`), and **before** `journal_commit`, the inode is
  repointed to `new_phys`. If we crash here, `replay_journal` reverts the extent to `old_phys` — and
  `old_phys`'s checksum-table entry was **never touched** (CoW = new block, old block untouched), so
  the reverted state's checksum is automatically still correct. **No journal schema change is needed**:
  because checksums are keyed by *physical* block and CoW never overwrites a live block in place, the
  new block's checksum is written to a *fresh* table slot that the revert simply orphans (freed with
  the block). The crucial invariant: **`write_data_block` must write the data block AND its checksum
  slot before the inode is repointed** — both are pre-commit, so the journal's existing
  "revert pointer to old_phys" fully covers it.

- For **in-place metadata** that does *not* go through CoW (superblock, bitmaps, inode table when
  `write_inode` overwrites in place): the checksum slot is updated in the same `write_block` wrapper
  used to persist them. A torn write there is already the domain of the bitmap/refcount fsck; the
  checksum simply gives `fsck_scrub_checksums` a second, stronger detector. **Metadata checksums are
  best-effort detect-only in this spec** (a metadata block whose checksum mismatches is reported by
  scrub and made a hard read-error for B-tree nodes via `read_block` callers that opt in — see
  Interface needs). Self-healing of metadata is the follow-up.

### Failure model

- **Data read mismatch** → `read_data_block` returns `Err`; the VFS read syscall surfaces a new
  `E_RAEFS_CKSUM` error constant (alongside `E_RAEFS_EXTENT_FAIL` at line 131). **Never** decrypt/serve.
- **Metadata (B-tree node) read mismatch** → the node read returns `Err`, failing the operation loud
  rather than walking a corrupt tree (matches the existing "fail loud to protect snapshot integrity"
  stance, line 1720).
- **Legacy / region-absent / slot==0** → verification is skipped (Ok); scrub reports `unverified`.
- **Scrub** (`fsck_scrub_checksums`) walks `0..checksum_index_limit`, reads each *allocated* block
  (bitmap bit set), recomputes CRC32C over the raw on-disk bytes, compares to the table; returns
  `ScrubReport { checked, mismatches, unverified }`. Read-only — safe in safe-mode.

### Security model

- Detection of bit-rot and of any agent that rewrites raw device blocks without updating the
  ciphertext checksum (the structural ransomware-resistance angle: a process that bypasses the FS to
  scribble the device is caught on next read).
- CRC32C is **not** a MAC: an attacker who can write both the data block and its checksum-table slot
  (i.e. already has FS-level write through the kernel) is not stopped by CRC32C alone — that threat is
  addressed by capability/sandbox enforcement (RaeShield) plus the reserved keyed-BLAKE2S follow-up.
  This spec's boundary is explicit and matches Concept §142's "bounded" (detect + fail), not "prevent
  a privileged in-kernel write".

## Interface needs (NEEDS-INTERFACE)

Verify-on-read is **internal** to the FS — no syscall required for the core feature. Two optional
surfaces, both deferrable, flagged for raeen-architect (do **not** assign numbers here):

- `NEEDS-INTERFACE:` (optional, follow-up) a `SYS_RAEFS_SCRUB` syscall so userspace (Settings /
  a "Verify disk" UI) can trigger `fsck_scrub_checksums` and read back `ScrubReport`. Not needed for
  the R10 boot proof (the smoketest calls the function directly). Assign only if/when the UI lands.
- `NEEDS-INTERFACE:` (optional) extend the existing RaeFS VFS error surface so a userspace `read()`
  that hits `E_RAEFS_CKSUM` gets a distinct errno rather than a generic EIO. `E_RAEFS_CKSUM` itself
  is an internal `pub const` in `raefs.rs` (pattern of `E_RAEFS_EXTENT_FAIL`, line 131) and needs no
  ABI number; only the errno *mapping* would touch the syscall surface.

No `rae_abi` / `ABI_VERSION` change is required for the core detect+scrub feature.

## File-by-file plan

- `kernel/src/raefs.rs`:
  - Add `CKSUM_ALG_NONE/CRC32C/BLAKE2S128` consts; a `const CRC32C_TABLE: [u32;256]` + `fn crc32c(&[u8])->u32`.
  - `Superblock`: add `checksum_block, checksum_blocks, checksum_alg, checksum_enabled, _pad_cksum`;
    update the `size == BLOCK_SIZE` const-assert and the `reserved` tail length; update every
    `Superblock { … }` literal (mount, format, mkfs, RAM-volume, snapshot-restore paths) to init the
    new fields.
  - Extend `normalise_region_counts` (or add `normalise_checksum_region`) — legacy `checksum_block==0`
    → `checksum_enabled=0` (verify is a no-op, scrub = unverified).
  - mkfs/`format` + `mkfs_raefs` (line ~6027): allocate the checksum-table run in the reserved region,
    zero it, set `checksum_enabled=1, checksum_alg=CRC32C`.
  - Add `read_block_checksum/write_block_checksum/checksum_index_limit` (mirror refcount accessors).
  - `write_data_block` (4390): after `write_block`, `write_block_checksum(block_idx, crc)`.
  - `read_data_block` (4331): after `read_block`, verify; mismatch → `Err`/`E_RAEFS_CKSUM`.
  - Add `fsck_scrub_checksums(&self) -> ScrubReport` + `ScrubReport` struct.
  - Add `run_integrity_smoketest()` (R10) + `INTEGRITY_SELFTEST: AtomicU8`.
  - `proc_dump_text` (215): add the integrity status line.
  - `cow_diverge_extent_journaled` (2090): no schema change; rely on `write_data_block` now writing the
    checksum pre-commit (add a comment asserting the invariant).
- `kernel/src/main.rs`: add `raefs::run_integrity_smoketest();` in the RaeFS smoketest cluster
  (after line 1183, beside the cow-journal test).
- Host KAT: `kernel/src/raefs.rs` `#[cfg(test)]` module (or `tools/`-side harness) for `crc32c`
  against public CRC32C vectors — see Host KAT plan.
- `docs/SYSCALL_TABLE.md`: **only** if the optional `SYS_RAEFS_SCRUB` is later approved (not now).
- `MasterChecklist.md`: add a Phase 5.x "RaeFS data-integrity checksums" row (currently absent).

## Acceptance criteria (the exact proof)

- **Boot log MUST show** a FAIL-able line. The smoketest, on a `with_ram_raefs` throwaway volume:
  writes a known block (computing+storing its checksum), reads it back (`clean_verifies=true`),
  then **flips one byte of the raw on-disk block behind the FS** and asserts `read_data_block`
  returns `Err` (`checksum_detects_flip=true`), then runs `fsck_scrub_checksums` over the volume with
  the corrupt block present and asserts it counts the mismatch (`scrub_found=1`). Any false →
  the line prints `-> FAIL`:
  ```
  [raefs] integrity selftest: checksum_detects_flip=true clean_verifies=true scrub_found=1 -> PASS
  ```
  (It can print FAIL: if verify-on-read silently returned Ok on the flipped block, `checksum_detects_flip`
  is `false` → `-> FAIL`. The byte-flip is the can-this-test-fail guarantee per CLAUDE.md rule 16.)
- **`/proc/raeen/raefs` MUST report** (added to `proc_dump_text`):
  ```
    Integrity:    Enabled (CRC32C, encrypt-then-checksum)
    Integrity Selftest: PASS
    Checksum Blocks: <n> (covers <total_blocks> blocks)
  ```
  (Legacy volume: `Integrity: Disabled (legacy volume — no checksum region)`.)
- **Docstring** on the integrity module/functions MUST quote the Concept promise (§66 + §142 above).
- **No new `[boot] WARN`** / no `[BOOT-BENCH]` regression — the per-block CRC32C is a table lookup over
  4 KiB (≈ negligible vs the existing XTS pass already on that path); the smoketest is RAM-only.

### Exact boot-log lines that prove detection + scrub in QEMU

```
[raefs] Running boot smoketest...
[raefs] integrity selftest: checksum_detects_flip=true clean_verifies=true scrub_found=1 -> PASS
```
and in the end-of-boot `/proc/raeen/raefs` dump:
```
  Integrity:    Enabled (CRC32C, encrypt-then-checksum)
  Integrity Selftest: PASS
```
Plus the unchanged `System successfully booted.` with zero `PANIC`.

## Host KAT plan (pure logic first — CLAUDE.md rule 15)

The CRC32C function is pure `no_std` integer logic → host-KAT **before** QEMU:

1. **CRC32C known-answer vectors** (public, RFC 3720 / iSCSI / Btrfs test vectors):
   - `crc32c(b"") == 0x00000000`
   - `crc32c(b"123456789") == 0xE3069283`
   - `crc32c(&[0x00;32]) == 0x8A9136AA`
   - `crc32c(&[0xFF;32]) == 0x62A8AB43`
   - `crc32c(&[0x00..=0x1F]) == 0x46DD794E` (32 incrementing bytes — iSCSI vector)
   Run with `cargo test -p kernel` behind a `#[cfg(test)]` gate, OR (preferred, avoids the no_std
   host-test "duplicate lang item" trap — MEMORY no-std-workspace-host-test) extracted as a tiny
   `rustc --edition 2021` standalone like the Argon2id/Ed25519 KATs (MEMORY host-kat-under-embargo).
2. **Round-trip + tamper logic** (pure, no block device): a `[u8;4096]` buffer; `c0 = crc32c(&buf)`;
   flip `buf[1234] ^= 1`; assert `crc32c(&buf) != c0` (single-bit flip always detected by CRC32C).
3. **Table self-consistency**: assert the `const` table equals a runtime-generated table (catches a
   typo'd table constant) — generate once in the test, compare 256 entries.

Layer order: ① host KAT (above) → ② boot smoketest (`run_integrity_smoketest`, RAM volume) →
③ QEMU CI (the two log lines) → ④ iron (paused; the design is fully provable at layers ①-③).

## Handoff

- **Implementer: raeen-fs.**
- **On-disk-format change — FLAG:** new `Superblock` fields (`checksum_block`, `checksum_blocks`,
  `checksum_alg`, `checksum_enabled`, `_pad_cksum`) + a new **checksum-table region** in the reserved
  metadata area. This is a mkfs/format-compatibility change of the **same class** as the recent
  multi-block bitmap/refcount work ("Landmine-1"): **legacy volumes (fields == 0) MUST be normalised
  on mount** (`checksum_block==0` ⇒ checksum-absent ⇒ verify-on-read is a no-op, scrub = `unverified`,
  never a false FAIL) so older images keep mounting. mkfs/`mkfs_raefs` lays the region down and enables
  it. Update the `const _: assert size_of::<Superblock>() == BLOCK_SIZE` and the `reserved` tail length
  in the **same** commit as the field additions, or the superblock `ptr::write` overflows the stack
  (the documented Landmine at line 66-69).
- **Unblocks checklist lines:** no existing MasterChecklist item — **add a Phase 5.x row** "RaeFS
  data-integrity / per-block checksums (detect-only)"; this is the next RaeFS hardening iteration after
  per-file encryption keys (`FILE_KEY_SELFTEST`). Strengthens the §66 "never half-apply" / §142
  "resists ransomware structurally" contracts in `Audit.md`.
- **Sequencing:** (1) host-KAT `crc32c`; (2) superblock fields + const-assert + reserved tail + all
  literal initializers + `normalise_*` in ONE commit (format change, must be atomic); (3) accessors +
  write/read-path wiring + `E_RAEFS_CKSUM`; (4) `fsck_scrub_checksums` + `run_integrity_smoketest` +
  main.rs call + proc line. No `rae_abi`/interface commit needed unless the optional `SYS_RAEFS_SCRUB`
  is later approved (separate `[interface]` commit then).
- **Follow-ups (explicitly NOT this spec):** self-healing / redundancy (mirror or parity copies +
  auto-repair-from-good-copy on mismatch); keyed-MAC tamper-*resistance* mode (`CKSUM_ALG_BLAKE2S128`
  with a per-volume key) for the "malicious privileged writer" threat; multi-block snapshot metadata
  must also freeze the checksum region when `create_snapshot` is extended past the current 1-bitmap-block
  gate (line 2155).
