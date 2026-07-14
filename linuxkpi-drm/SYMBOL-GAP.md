# M4 link target — the exact amdgpu kernel-symbol surface (trace-extracted)

**The definitive "what to implement" list, extracted from the real `amdgpu.ko` on
Athena** (`nm -u amdgpu.ko` → its undefined symbols = every kernel function it imports).
No guessing, no header-grind blind spots: if every symbol here resolves to a
`raeen_linuxkpi` export with correct semantics, amdgpu **links**.

## The numbers (kernel 7.0.12, gfx11/Phoenix)

| | count | file |
|---|---|---|
| amdgpu needs (undefined symbols) | **1124** | `amdgpu-undefined-symbols.txt` |
| already provided by `raeen_linuxkpi` | **182** | `amdgpu-symbols-covered.txt` |
| **GAP (to implement)** | **942** | `amdgpu-symbol-gap.txt` |

## The gap, categorized (implement in this order)

| # | bucket | count | strategy |
|---|---|---|---|
| **A** | `*_noprof` allocator aliases | 14 | **cheap** — kernel 7.0 alloc-profiling variants; alias `__kmalloc_noprof`→`__kmalloc` etc. |
| **E** | misc kernel helpers (string/bitmap/math/printk/bsearch/sort/crc/hex/uuid…) | ~592 | mostly **small** pure functions; many are 1–5 line wrappers or libc-equivalents |
| **C** | core LinuxKPI primitives (dma_fence / workqueue / xarray / idr / wait / timer / completion / kref) | 50 | **real work** — these get CALLED; extend the existing `raeen_linuxkpi` facades |
| **D** | ttm / mm / buddy / sg / dma-buf / dma-resv | 71 | **real work** — GPU memory mgmt; MES-relevant |
| **B** | display / DC (`drm_atomic_*`, `drm_dp_*`, crtc/plane/connector/encoder/bridge) | 223 | **link-only stubs** — the MES subset never calls these; an honest `-ENOSYS`/abort-if-called stub resolves the link (RaeenOS uses its own compositor, not DRM/KMS) |

**Honest framing:** the extraction is the easy 10% — it *scopes* the work precisely. The
90% is implementing buckets C/D (and the non-trivial parts of E) with **correct
semantics** (real `dma_fence` signalling, real workqueues), not just resolving the name.
Per rule 9, the bucket-B stubs return a real error or abort — never silent success.

## Reconciliation with the header track

The shims under `include/linux/*.h` *declare* these; `components/raeen_linuxkpi` *defines*
them. Struct layouts must agree (the `work@offset 24`, `mutex`, `timer`, `dma_fence`
debt). When adding a struct-based primitive, match the layout the header shim uses.

## Regenerate (when the kernel/driver version changes)

```bash
# on Athena:
zstd -dkf /lib/modules/$(uname -r)/kernel/drivers/gpu/drm/amd/amdgpu/amdgpu.ko.zst -o /tmp/amdgpu.ko
nm -u /tmp/amdgpu.ko | awk '{print $2}' | sort -u   # the 1124
# vs raeen_linuxkpi exports:
grep -rhoE 'pub (unsafe )?extern "C" fn [a-z_0-9]+' components/raeen_linuxkpi/src/ | sed -E 's/.*fn //' | sort -u
```
