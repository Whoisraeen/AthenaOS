# Spec: Kernel heap freelist hardening (DMA-UAF-corrupts-freelist class)

Status: SPEC ONLY — no kernel code written. Author: athena-researcher, 2026-07-04.
Hand-off: athena-kernel.

## Concept promise served

> "**Driver isolation:** Every driver runs in its own protection domain with IOMMU
> enforcement. A bad GPU driver crashes a service, not the kernel." (LEGACY_GAMING_CONCEPT.md §Kernel Architecture, line 29)

> "4. **Security by default, not by friction.**" (§Principles, line 13) and
> "Memory safety where bugs are catastrophic" / "No GC, no UB, full control" (§Language table, lines 40–41)

The concrete failure this spec defends: a userspace/DMA-capable driver (or any kernel
subsystem) issues a **wild write into a heap chunk that has already been freed**. In the
current allocator that freed chunk's first 16 bytes ARE the freelist node (`Hole{size,
next}`). Corrupting `next` turns the *next* `alloc()` into "hand out an attacker/garbage
pointer," and corrupting `size` mislinks the hole graph on the next `dealloc()` merge.
Today that is a silent, cross-subsystem, non-deterministic memory-corruption crash — the
exact opposite of "a bad driver crashes a service, not the kernel." This is the class the
USB-MSC `DmaPage` leak workaround (`25d8afa`) was papering over, and the class
MasterChecklist flags as the next always-on hardening target after the buddy double-free guard.

## Already in the tree (verify-before-implement)

- `kernel/src/memory/buddy.rs` — the **physical** frame allocator already has an
  **always-on, default-build** double-free guard + free-list-node validation. Study these
  as the pattern to mirror at the heap layer:
  - `free_block` rejects a double-free via the per-frame bitmap authority (`buddy.rs:112`), WARNs, returns. Status `[x]` iron-proven.
  - `block_on_free_list` (`buddy.rs:183`) validates a node's `next`/`prev` are page-aligned + in-range + back-linked before trusting it — this is the intrusive-freelist-pointer-validation idea, already shipping for physical frames.
  - `order_counts` (`buddy.rs:271`) caps traversal + validates each node before `phys_to_virt` so a corrupt list can't panic the `/proc/athena/buddy` dump.
  - FAIL-able boot smoketest at `buddy.rs:298–394`: double-free-guard proof (`rejected_second_free -> PASS/FAIL`) + range-check proof. **This is the template the heap smoketest must match.**
- `kernel/src/memory/allocator.rs` — the **kernel heap**. `linked_list_allocator::LockedHeap` v0.10 behind `OomAwareHeap: GlobalAlloc`. HEAP_START `0xFFFF_9999_0000_0000`, HEAP_SIZE 128 MiB. **No freelist hardening today, and no `run_boot_smoketest()` at all** (buddy has one; the heap does not). This is the gap.
  - KASAN (`kasan_rt`, shadow + 512-slot quarantine) and KFENCE (guard-page sampler) already exist but are `#[cfg(feature = "kasan")]` / `#[cfg(feature = "kfence")]` — **compiled out of the default build**. They are UAF/OOB *detectors under test*, not always-on freelist-integrity guards. This spec does NOT touch them; it adds an always-on layer that ships in every build.
- `kernel/src/hardening.rs` — reports KASLR/SMEP/SMAP/CFI honestly; `/proc/athena/hardening`. `EntropySource` (rdrand/rdseed/tsc) lives here. New freelist-guard status will get its own `/proc/athena/heap_guard` endpoint (do not overload the hardening dump).
- Entropy available at `init_heap` time: `memory::aslr_random()` (`memory.rs:1287`, TSC + xorshift mix) and rdrand (`cpu_features` detects it at `cpu_features.rs:268`). `init_heap` runs before the crypto entropy pool is up, so the cookie must be sourced from rdrand-with-TSC-fallback, NOT the CSPRNG.
- `kernel/Cargo.toml:21` and `components/ath_linuxkpi/Cargo.toml:31` both pin `linked_list_allocator = "0.10"`. A `[patch.crates-io]` vendor hardens **both** consumers at once (bonus: the LinuxKPI userspace-driver shim's heap gets the same guard).
- Vendoring precedent: `components/vendored/aml/` (the AML crate) is already vendored via `[patch.crates-io]` (pitfall #11). Mirror that exactly.

## Prior art & OSS verdict

- **Linux `CONFIG_SLAB_FREELIST_HARDENED`** (SLUB `mm/slub.c` `freelist_ptr_encode`) — the free-object `next` pointer is stored XOR-obfuscated: `enc = ptr ^ s->random ^ swab(&location)`. The `swab(&location)` term ties the ciphertext to the *slot address*, so a valid encoded pointer copied to a different slot decodes to garbage. On unlink the decoded pointer is range/alignment-checked (`CONFIG_SLAB_FREELIST_RANDOM` is a separate init-order feature; the *hardened* one is the encode+check). Verdict: **GPLv2 — study only 📖**, do not copy code. The *technique* (per-heap random cookie ⊕ location-tied encode + validate-on-unlink) is what we adopt.
- **glibc `malloc` "safe-linking"** (2.32+, `PROTECT_PTR`/`REVEAL_PTR` on tcache/fastbin `fd`) — `enc = (loc >> 12) ^ ptr`; validates 16-byte alignment on unlink (`malloc_printerr("misaligned")`). Same shape, pointer-only, no separate random cookie. Verdict: **LGPL — study only 📖**. Confirms the "shift the storage address in, check alignment out" minimal variant works in production at scale.
- **glibc/Windows chunk canaries** (the classic `size`/prev-in-use magic; Windows LFH `_HEAP_ENTRY` cookie + encoded header) — a per-chunk header magic validated on free, catches *linear overflow of an adjacent live chunk*. Verdict: 📖 technique reference. Note the mismatch: this defends **live-chunk header** corruption, not the **freed-chunk link-pointer** follow which is our actual class.
- **`linked_list_allocator` v0.10** (phil-opp) — the crate we already ship. Singly-linked intrusive `HoleList`: `struct Hole { size: usize, next: Option<NonNull<Hole>> }`, headers written *inside* free memory; `allocate_first_fit` walks `Cursor{prev,current}` first-fit; `deallocate` reconstructs a `Hole` at the freed ptr and address-order-inserts + merges adjacent holes. **License: MIT / Apache-2.0 — permissive, already in use → vendorable ➕** (same class as the vendored aml crate).
- **AthenaOS `buddy.rs`** — our own always-on intrusive-freelist validator for physical frames (see above). In-tree prior art; the heap guard should read like its sibling.

## Design

### Options considered

| # | Approach | Capability | Boot/hot-path cost | Risk / kLOC |
|---|---|---|---|---|
| 1 | Replace with a slab / segregated-freelist allocator + XOR-encoded next + range validation (full SLUB-hardened model) | Highest; also kills fragmentation | Rewrites the single most load-bearing unsafe component; must re-prove no regression | **High** — large new unsafe surface, big kLOC, disproportionate boot-stability risk for the hardening goal |
| 2 | Per-alloc header+footer canary, validated on free (glibc/Windows chunk-magic) | Catches adjacent-chunk *overflow* + double-free | **Per-alloc bytes + pointer relocation** (must offset every returned ptr, fix alignment); measurable on hot path | Medium; and it defends the *wrong* class — the live-chunk header, not the freed-chunk link follow |
| 3 | Vendor `linked_list_allocator`, add range+alignment validation on every `Hole.next` deref (unlink/walk/merge) | Catches a corrupted link the moment it is followed | ~O(1) per hop, in-place, **zero extra memory**, no pointer relocation | **Low** — surgical, mirrors buddy.rs |
| 4 | **Hybrid = Option 3 + XOR-obfuscate `next` with a boot-random, location-tied cookie (the useful half of Option 1's hardening applied to the existing hole list)** | Option 3 detection **plus** a wild data write can't forge a valid-looking link, and a copied link decodes wrong | Same O(1)-per-hop; one XOR added per deref; zero extra memory | **Low** — the recommended path |

### RECOMMENDATION: Option 4 (hybrid)

Vendor `linked_list_allocator` (Option 3 mechanism) and, on top, encode the intrusive
`next` field the CONFIG_SLAB_FREELIST_HARDENED way. This buys Windows/macOS-grade
freelist integrity **without** the boot-stability risk of rewriting the allocator, at
~O(1) per hop and **no per-allocation memory or layout overhead** (dealloc stays simple —
no returned-pointer relocation, unlike Option 2). Highest capability-per-kLOC; it targets
*exactly* the DMA-UAF-corrupts-freelist class and nothing speculative.

Explicitly rejected: Option 1 now (adopt a hardened slab later as a *perf* play if
fragmentation demands it — hardening does not require it; rule 4/12: no mass without a
feature, boot stays fast). Option 2 (defends the wrong class, adds hot-path cost).

### Data structures (in the vendored `src/hole.rs`)

Unchanged on-disk layout — `Hole` stays 16 bytes `{ size: usize, next_enc: usize }`.
The only change: the `next` field is stored **encoded**.

```
// Boot-random, installed once by init_heap. 0 = "not yet installed" → encode/decode
// are identity so the very first holes built during HoleList::init are still walkable.
static FREELIST_COOKIE: AtomicU64  // set once; rdrand, TSC-fallback

// Location-tied XOR encode (mirrors SLUB freelist_ptr_encode):
//   store slot = &hole.next  (the address the ciphertext lives at)
fn encode(ptr: usize, slot: *const usize) -> usize
    = ptr ^ COOKIE ^ (slot as usize).rotate_left(32)
fn decode(enc: usize, slot: *const usize) -> usize   // symmetric

// Pure predicate — the whole detector, host-KAT-able:
fn validate_link(enc: usize, slot: *const usize,
                 bottom: usize, top: usize) -> Result<Option<NonNull<Hole>>, HeapCorruption>
//   let p = decode(enc, slot);
//   p == 0                        -> Ok(None)              // list tail
//   p % align_of::<Hole>() != 0   -> Err(Misaligned)       // 8/16-byte align
//   p < bottom || p+16 > top      -> Err(OutOfRange)       // must land in the heap span
//   else                          -> Ok(Some(NonNull(p)))

enum HeapCorruption { Misaligned, OutOfRange }
struct GuardStats { cookie_installed: bool, validations: AtomicU64, corruptions: AtomicU64,
                    last: Option<{enc, decoded, slot, kind}> }
```

`HoleList` gains `bottom`/`top` bounds (0.10 already tracks `bottom`/`top`) and a
`set_freelist_cookie(u64)`. Every site in 0.10 that reads or writes `hole.next` —
`Cursor::next`/`current` advance in `allocate_first_fit`, the insert/merge in
`deallocate`, `extend`, and the `HoleList::first` head — routes through
`encode`/`decode` + `validate_link`.

### Where the validation hook goes

Exactly the deref sites the allocator already touches — no new traversal:
1. **alloc walk** (`Cursor` advance, first-fit): before following `current.next`, `validate_link`. This is the primary catch — the corrupted freed chunk is detected the instant the next allocation reaches it.
2. **dealloc insert/merge**: validate each neighbour link touched while address-order-inserting and coalescing adjacent holes.
3. **`extend`/init**: encode the head link with the (possibly still-zero) cookie.

Cost = one XOR + three integer compares per hop already being walked. No extra hops, no
extra memory, no lock changes (the existing `LockedHeap` spinlock still serializes).

### Failure action (graded, fail-closed on the live path)

- **Live allocator path, `validate_link` → Err:** the intrusive freelist has no
  authoritative side-table to fall back to (unlike buddy.rs, where the per-frame bitmap is
  ground truth) — once a link is corrupt the structure is untrustworthy and any further
  `alloc` may hand out a wild pointer. So the live path is **fail-closed: `panic!`** with a
  dedicated, greppable line (`[heap] CORRUPTION …`) after bumping `corruptions` + recording
  `last`. This matches Windows `KeBugCheck` on pool-metadata corruption and macOS's
  `zalloc`/kalloc corruption panics — a detected heap-metadata corruption is
  non-recoverable and continuing is a privilege-escalation / silent-data-loss risk. The
  panic path already dumps a backtrace (`panic.rs` + force-frame-pointers) and persists to
  BOOTLOG.TXT/netlog, so the offending `{enc, decoded, slot}` reaches iron.
- **Detector-under-test path:** `validate_link` is a **pure predicate returning
  `Result`**, so the boot smoketest and host KAT can feed it a deliberately corrupted
  encoding and assert `Err` **without** killing boot. The live path is a thin
  `.unwrap_or_else(|e| panic_corruption(e))` over the same predicate. This is what makes the
  test FAIL-able while the production behaviour stays fail-closed.
- Why not quarantine (KASAN's model) or WARN-and-continue (buddy's model): quarantine needs
  a poisoned side region we don't have on the always-on path; WARN-and-continue is only safe
  when a side-table lets you *reject the specific bad op* (buddy has the bitmap; the
  intrusive hole list does not). Fail-closed is the honest choice here.

### Always-on cost budget

- Per `alloc`/`dealloc`: +1 XOR +≤3 compares per hole hopped (first-fit typically hops a
  handful). Target **< ~50 cycles added per allocation** on average; **zero** added bytes
  per allocation; **zero** added heap traversals.
- Whole-boot: must stay inside `[BOOT-BENCH]` — assert **no new `[boot] WARN`** and the
  BOOT-BENCH total does not regress beyond measurement noise (budget ≤ +20 ms over the
  whole boot; realistically single-digit ms). SCHED_BODY latency budgets are untouched
  because steady-state game/audio/compositor threads do not allocate on the hot path
  (kernel rule 2), and the per-alloc delta is far below one audio period / frame.
- The cookie install (`init_heap`) is one rdrand + one atomic store — one-time.

### Security model / threat coverage

- **Detects:** a wild/DMA write into a freed chunk that stomps `next` (random bytes decode
  to a misaligned/out-of-range pointer → caught on the next alloc that reaches it); a
  freed-chunk link copied/replayed from another slot (location-tied encode → decodes wrong →
  caught); a double-free that relinks a hole into an inconsistent graph (the merge-path
  validation catches the bogus neighbour link).
- **Cookie secrecy:** the XOR cookie is a boot-random not exported through any
  `/proc`/syscall (the `/proc/athena/heap_guard` dump masks it — prints only
  `cookie_installed: true`, never the value), so a data-only wild write cannot forge a valid
  ciphertext without an infoleak of both the cookie and the target slot address.
- **Not covered (out of scope, honest):** a *live-chunk* linear overflow that never touches
  a freed link (that is KASAN/KFENCE's job, and the redzone/guard-page detectors already
  exist feature-gated); an attacker with a full cookie+slot infoleak. This is a
  metadata-integrity guard, not a full allocator sanitizer — reported as such in the procfs
  dump (mirror hardening.rs's honesty discipline).

## Interface needs (NEEDS-INTERFACE)

**None.** Purely internal to the kernel heap. No new syscall, no `ath_abi` change, no
`ath_driver_api` change, no ABI bump. (State this explicitly so athena-architect is not
pulled in.)

## File-by-file plan

- `components/vendored/linked_list_allocator/` — **new**: vendor v0.10 source verbatim
  (keep MIT/Apache LICENSE headers), mirroring `components/vendored/aml/`. Edit
  `src/hole.rs`: add `FREELIST_COOKIE`, `encode`/`decode`/`validate_link`/`HeapCorruption`/
  `GuardStats`; route every `hole.next` read/write through encode/validate; add
  `HoleList::set_freelist_cookie` + `pub` stat accessors + a `panic_corruption`. Add
  `#[cfg(test)]` unit tests (crate uses std in tests) for the pure predicate.
- Root `Cargo.toml` — add `[patch.crates-io] linked_list_allocator = { path =
  "components/vendored/linked_list_allocator" }` (hardens both the kernel and
  `ath_linuxkpi`).
- `kernel/src/memory/allocator.rs` — in `init_heap`, after `HEAP_INNER.lock().init(...)`,
  call `set_freelist_cookie(rdrand64_or_tsc())` and print the init marker. Add a **new**
  `pub fn run_boot_smoketest()` (the heap currently has none). Add pub accessors that
  forward the vendored crate's `GuardStats` for procfs. Add the Concept docstring quote.
- `kernel/src/main.rs` — call `memory::allocator::run_boot_smoketest()` in the smoketest
  tier, next to `buddy::run_boot_smoketest()`.
- `kernel/src/procfs.rs` — add `proc_athena_heap_guard()`; register `("heap_guard",
  proc_athena_heap_guard)` in the `ENTRIES` table (~line 1531), the match arm (~line 1138),
  and the index listing (~line 1025). Format mirrors `proc_athena_buddy`.
- Host KAT — the encode/decode/validate predicate is pure; test it in the vendored crate
  (`cargo test -p linked_list_allocator`). Heed the no_std host-test gotcha: run it
  per-crate, never `cargo test --workspace`.

## Acceptance criteria (the exact proof)

- **Host KAT** (layer ①, cheapest — run first): `cargo test -p linked_list_allocator`
  asserts: (a) `decode(encode(p,slot),slot) == p` round-trips for many p/slot; (b) flipping
  any single bit of an encoded word makes `validate_link` return `Err` (probabilistically
  ~certain — a random 64-bit value decoding to an aligned in-range Hole is astronomically
  unlikely); (c) an encoding built for `slot_a` fed at `slot_b` decodes ≠ original and
  fails; (d) `p == 0` → `Ok(None)`.
- **Boot smoketest** (layer ②) MUST print, and MUST be able to print FAIL:
  `[heap-guard] smoketest: encode_roundtrip=true rejected_corrupt_next=true validations=<N> -> PASS`
  Built by: construct a synthetic `[Hole; 2]` in a static scratch buffer, encode a valid
  link (assert decode+validate PASS), then **deliberately corrupt the encoded `next` word**
  and assert `validate_link` returns `Err` (`rejected_corrupt_next=true`). Reverting the
  validator flips this to `-> FAIL` — that is the false-green guard. (Does **not** corrupt
  the live heap; operates on the scratch buffer.)
- **Init marker** MUST show:
  `[heap-guard] freelist hardening ON: cookie=installed encode=xor+loc-tie validate=range+align`
- **`/proc/athena/heap_guard`** MUST report: `cookie_installed: true`, `encoding:
  xor+location-tie`, `failure_action: panic (fail-closed)`, `validations: <n>`,
  `corruptions: 0` (and `last_corruption:` only if ever non-zero). Never prints the cookie value.
- **Real-corruption line** (only if it ever fires in the field):
  `[heap] CORRUPTION: hole@<addr> enc=<hex> decoded=<addr> (out-of-range|misaligned) -> PANIC`
- **Boot-time guard**: no new `[boot] WARN`; `[BOOT-BENCH]` total not regressed beyond noise.
- **Docstring** on the vendored hardening + `allocator.rs` MUST quote the Concept promise above.
- **QEMU proof** (`[~]`): the init marker + smoketest `-> PASS` present in
  `$env:TEMP\athena-serial.log`, `System successfully booted`, 0 PANIC.
- **Iron proof** (`[x]`): the same two lines present in a committed `logs/bootlog-*.txt`
  Athena transcript. Because the guard is always-on, every future iron boot re-proves it.

## Handoff

- **Implementer: athena-kernel.**
- **Unblocks / advances checklist lines:** the "Latent kernel bugs to clear before
  production ready" section (the DMA-UAF / freed-chunk-corruption hardening target); lets the
  USB-MSC `DmaPage` leak *workaround* (`25d8afa`) be revisited as a real fix rather than a
  guard; strengthens the "Driver crash ≠ system crash" North-Star contract; feeds Phase 4
  (production-ready kernel) hardening.
- **Sequencing:** (1) vendor + `[patch.crates-io]` and host-KAT the predicate FIRST (no
  kernel boot needed — cheapest proof); (2) wire the cookie install + init marker in
  `init_heap`; (3) add `run_boot_smoketest` + main.rs call + procfs endpoint; (4) QEMU boot
  → `[~]`; (5) fold into the next Athena flash bundle → `[x]`. **No interface commit
  required** (NEEDS-INTERFACE: none), so this does not depend on or block athena-architect.

## R10 4-artifact checklist (for athena-kernel)

1. **`init()` from `kernel_main`** — cookie install + `[heap-guard] … ON` marker in
   `init_heap` (reached from the memory-init tier of `kernel_main`).
2. **`run_boot_smoketest()`** — new in `allocator.rs`, called from `main.rs`, FAIL-able
   (the deliberate-corruption assertion above).
3. **procfs line** — `/proc/athena/heap_guard`.
4. **Concept docstring** — quote §Kernel Architecture line 29 ("a bad GPU driver crashes a
   service, not the kernel") + §Principles line 13 ("Security by default, not by friction").
