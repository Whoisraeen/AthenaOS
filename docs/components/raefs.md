# AthFS

Copy-on-write filesystem optimized for a gaming desktop's access patterns.

## Goals (from the concept doc)

- CoW with instant snapshots → atomic system updates, one-click rollback
- Native encryption, hardware-backed where available (TPM 2.0 minimum)
- Tiered storage: NVMe / SATA / spinning rust, automatic hot-data promotion
- Game-aware extents: large sequential-read optimization, "game install" hint that
  pre-allocates contiguous blocks
- Zstd compression by default, transparent
- Per-app data buckets enforced at the FS layer
- Versioned config: every system config file auto-versioned, single-click rollback

## Non-goals

- Distributed FS features (Ceph, GlusterFS territory)
- POSIX permission model fidelity beyond what's needed for Linux app compat

## Layout sketch

```
Superblock (redundant, A/B)
  ↓
Object map (CoW B-tree, root pointer = the only mutable bit on disk)
  ↓
Object types: inode, extent, snapshot, capability-bucket, journal-entry
  ↓
Extent allocator: bitmap with a "contiguous-prefer" hint for game installs
```

The root object pointer in the superblock is what makes CoW atomic: a new
superblock with the new root pointer is the last write of any transaction.

## Open design questions

- Encryption key hierarchy: per-volume? per-bucket? per-file?
- Whether to land on a journaled or fully CoW metadata format
- Native vs. plugin layer for de-dup
