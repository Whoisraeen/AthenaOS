# RedoxFS upstream (R08 extraction)

Source reference: `redox_reference_upstream/redoxfs` (clone locally, gitignored).

- Upstream: https://gitlab.redox-os.org/redox-os/redoxfs
- License: MIT (`LICENSE` in this directory)
- AthenaOS destination: `components/raefs` on-disk format layer (CoW foundation per `LEGACY_GAMING_CONCEPT.md`)

Integration is incremental: AthFS user API stays in `src/lib.rs`; on-disk btree/COW will adapt
from RedoxFS in checklist-gated slices. Do not depend on `redox_syscall` / scheme IPC.

**Slice 1 (landed):** `src/redoxfs_adapter/tree.rs` — `TreePtr`, `TreeList`, 4-level `Tree`
(packed 4096-byte nodes, MIT-adapted from upstream `tree.rs`).
