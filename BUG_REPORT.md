# AthenaOS Bug Report

**Date:** 2026-06-13
**Auditor:** Opus (Claude Code), static review — no build/boot was run (this box has no toolchain).
**Scope this pass:** `kernel/src/**` (194 files) + first-party components touched along the way. Vendored trees (`components/vendored/*`, `components/kanata_daemon/vendor/*`, `components/raebridge/relibc/*`, `dlmalloc-rs`) were treated as upstream snapshots and **excluded**.

> This is a **point-in-time static audit**. Every finding below was read and reasoned about in source; none were reproduced on hardware or in QEMU. Severities reflect *potential* impact. Where I could not confirm a precondition, the item is filed under **Audit (unconfirmed)** rather than asserted as a bug.

---

## Methodology & coverage (honest)

Two techniques, combined:

1. **Mechanical pattern scans across all 194 kernel files** for documented bug classes:
   - syscall-number dispatch collisions (CLAUDE.md pitfall #1)
   - `allocate_frame()` loops building multi-page buffers (pitfall #7)
   - `unsafe impl Send/Sync` sites
   - stubs (`todo!`/`unimplemented!`/`unreachable!`)
   - lock-across-`switch_context`
   - length-to-`u8`/`u16` truncation
   - integer-division/modulo by a possibly-zero divisor
2. **Deep reads** of ~12 high-risk files (parsers of untrusted input + security + DMA core):
   `edid.rs`, `dhcp.rs`, `sandbox.rs`, `nvme.rs`, `elf.rs`, `acpi_full.rs` (battery), `raefs.rs` (name path), `audio.rs` (DMA), `atombios.rs`, plus `scheduler.rs`/`context.rs` (skimmed).

**Not yet deep-reviewed** (next passes): the bulk of `components/*` (raeaudio, raegfx, raeui, raenet, raeshield, rae_crypto), the `amdgpud`/`i915d`/`raeinstaller`/`xtask` binaries, and large kernel files only pattern-scanned (`compositor.rs`, `usb_core.rs`, `xhci.rs`, `ahci.rs`, `linux_compat.rs`, `ipc.rs`, `scheduler.rs` in depth).

---

## Severity legend

| Mark | Meaning |
|---|---|
| 🔴 High | Memory unsafety / crash / security bypass reachable on a realistic path |
| 🟠 Medium | Real defect with a narrow trigger, or a latent landmine in load-bearing code |
| 🟡 Low | Edge-case / defensive / cosmetic correctness |
| 🔵 Audit | Plausible defect, **precondition unconfirmed** — needs verification |

---

## Summary

| ID | Sev | Area | File | One-liner |
|----|-----|------|------|-----------|
| BUG-001 | 🟠 | Display | `kernel/src/edid.rs` | EDID detailed-timing decode is wrong per spec; smoketest is a self-consistent false green |
| BUG-002 | 🟠 | Storage/DMA | `kernel/src/nvme.rs` | `alloc_dma_frames_mapped` builds a multi-page DMA buffer non-contiguously (latent pitfall #7) |
| BUG-003 | 🟡 | Net | `kernel/src/dhcp.rs` | `lease_time * 7 / 8` overflows `u32` on the RFC-legal infinite lease (`0xFFFFFFFF`) |
| BUG-004 | 🟡 | Security | `kernel/src/sandbox.rs` | Linux network gate denylist omits `sendmsg`/`recvmsg`/`shutdown` (latent sandbox gap) |
| BUG-005 | 🟡 | GPU | `components/raeen_amdgpu/src/atombios.rs` | `parse_rom` accepts a zero master-command-table offset its own comment forbids |
| BUG-006 | 🟡 | Exec | `kernel/src/elf.rs` | Per-page zero erases data when two PT_LOAD segments share a sub-page-aligned page |
| AUDIT-1 | 🔵 | Several | (see below) | Integer div/mod by values that *might* be zero (compositor Hz, cpufreq perf, audio ring size) |

No syscall-number dispatch collisions were found (the historical 19/27 cases are fixed).

---

## Fix status (2026-06-13, Opus)

All findings from this report are now fixed in-tree. Build verified clean
(`cargo run -p xtask --release -- build --release` exit 0). Bit-exact decode
math for BUG-001 host-verified independent of QEMU.

| ID | Status | Fix |
|----|--------|-----|
| BUG-001 | ✅ Fixed | `edid.rs` decode now masks the high nibble to bits [11:8] (`((b58 & 0xF0) << 4) | b56`); `synthesize_from_gop` rewritten to the real VESA layout with a real pixel clock; smoketest now decodes a real 1920×1080 descriptor and can FAIL. Host-checked: real 1080p → 1920×1080, old code → 1816. |
| BUG-002 | ✅ Fixed | `alloc_dma_frames_mapped` now uses `allocate_contiguous_frames(order)` (pitfall #7) and zeroes the full `count*4096` region. |
| BUG-003 | ✅ Fixed | `dhcp.rs` rebind default widened: `((lease_time as u64) * 7 / 8) as u32` — no overflow on the infinite lease. |
| BUG-004 | ✅ Fixed | `sandbox.rs class_of_linux` now gates the full contiguous socket block `41..=55` (covers sendmsg/recvmsg/shutdown + getsockname/getpeername/socketpair/sockopt). |
| BUG-005 | ✅ Fixed | `atombios.rs parse_rom` now also rejects `master_command_table_offset == 0`. |
| BUG-006 | ✅ Fixed | `elf.rs` only full-page-zeroes a freshly mapped frame; a shared page zeroes strictly this segment's bss bytes, preserving the earlier segment's data. |
| AUDIT-1 | ✅ Resolved | compositor (4 sites) and cpufreq (2 sites) were already guarded by `== 0` early-returns — false positives. `AudioRingBuffer::new` now floors `size` at 1 (was the one genuinely reachable div-by-zero / zero-size-alloc); HDA CORB/RIRB sizes are hardware-fixed at 256. |

---

## Detailed findings

### BUG-001 — 🟠 EDID detailed-timing decode is incorrect, and the smoketest can't catch it
**File:** [kernel/src/edid.rs:40](kernel/src/edid.rs:40) (parse), [kernel/src/edid.rs:82](kernel/src/edid.rs:82) (synthesize)

The detailed-timing-descriptor active-pixel extraction does not match the EDID 1.x spec.

```rust
let h_active = ((edid[58] as u16) << 4) | ((edid[56] as u16) >> 4);
let v_active = ((edid[61] as u16) << 4) | ((edid[59] as u16) >> 4);
```

Per the EDID DTD layout (18-byte block at offset 54):
- byte 56 = horizontal-active **low 8 bits**
- byte 58 bits 7:4 = horizontal-active **high 4 bits** (i.e. bits 11:8)

So the correct decode is:
```rust
let h_active = (((edid[58] as u16) & 0xF0) << 4) | (edid[56] as u16);
let v_active = (((edid[61] as u16) & 0xF0) << 4) | (edid[59] as u16);
```
The current code (a) fails to mask byte 58/61 (the blanking high-nibble in bits 3:0 leaks into the result) and (b) shifts byte 56/59 right by 4 instead of using it as the low 8 bits. For 1920×1080 it yields ~1800×… instead of 1920×1080.

**Why the smoketest is green anyway:** `synthesize_from_gop` ([edid.rs:82-85](kernel/src/edid.rs:82)) *encodes* the same non-spec layout (and even stores height in byte 58, the horizontal-upper-nibble slot). `run_boot_smoketest` round-trips synth→parse, so it validates a broken encoder against a broken decoder and always passes — a textbook false green (violates CLAUDE.md rule 16, "a smoketest must be able to print FAIL").

**Impact:** Today introspection-only, so low blast radius — but this is the parser Phase 2.3 modesetting will lean on, and it will mis-decode the captured real panel EDID (`firmware/edid/athena-beelink-elitemini/*`). Medium because it's silently wrong *and* the test green-washes it.

**Fix:** correct the bit math above, and replace the smoketest with a KAT over a **real** captured EDID blob (the `aml_probe`/VFCT pattern) asserting the known 1920×1080 result, so the test can actually FAIL.

---

### BUG-002 — 🟠 NVMe multi-page DMA allocation is not physically contiguous (latent pitfall #7)
**File:** [kernel/src/nvme.rs:38](kernel/src/nvme.rs:38)

```rust
fn alloc_dma_frames_mapped(iommu_domain: Option<u16>, count: usize) -> Result<(u64,u64), BlockError> {
    let first_frame = alloc.allocate_frame()...;        // page 0
    let phys = first_frame.start_address().as_u64();
    ...
    for _ in 1..count {                                 // pages 1..count
        let f = alloc.allocate_frame()...;              // NOT adjacent to `phys`
        ... // zero it, then drop the address on the floor
    }
    if let Some(dom) = iommu_domain {
        let size = (count as u64).saturating_mul(DMA_PAGE);
        crate::iommu::map_dma(dom, phys, phys, size, true, true); // maps [phys, phys+count*4096)
    }
    Ok((phys, virt))                                    // returns only page 0
}
```

This is the exact anti-pattern in CLAUDE.md pitfall #7. The loop pulls `count` independent frames, but the function returns the **first** frame's address and IOMMU-maps `[phys, phys + count*4096)` as a single identity range. Only page 0 actually belongs to the allocation; pages 1..count of that physical range are whatever else lives there (heap, other DMA). A device DMA across the buffer corrupts unrelated memory, while the real allocated frames 2..count are zeroed and leaked.

Every other multi-page DMA site in the kernel correctly uses `allocate_contiguous_frames(order)` — `compositor.rs:1823`, `gpu.rs:578`/`1193`, `linux_compat.rs:727`, `linuxkpi_host.rs:607`. `nvme.rs` is the lone exception.

**Why Medium not High:** currently **latent** — the only caller chain is `alloc_dma_pages()` ([nvme.rs:829](kernel/src/nvme.rs:829)), which has no live callers; all real NVMe queues use the single-page `alloc_dma_frame_mapped` and cap depth at 64 so each ring fits one page ([nvme.rs:2413-2415](kernel/src/nvme.rs:2413)). It detonates the day someone allocates a multi-page queue/PRP list through this helper, and may "work" in QEMU (sequential early allocation) before failing on iron.

**Fix:** route `alloc_dma_frames_mapped` through `crate::memory::allocate_contiguous_frames(order)` (compute `order` from `count.next_power_of_two().trailing_zeros()`), mirroring `gpu.rs`. Either fix it or delete the dead helper so it can't be misused.

---

### BUG-003 — 🟡 DHCP rebind-time computation overflows `u32` on an infinite lease
**File:** [kernel/src/dhcp.rs:655](kernel/src/dhcp.rs:655)

```rust
let rebind_time = pkt.get_option(DhcpOption::RebindTime as u8)
    ...
    .unwrap_or(lease_time * 7 / 8);   // lease_time: u32, attacker/server-controlled
```

`lease_time` comes straight off the wire ([dhcp.rs:624-633](kernel/src/dhcp.rs:624)). RFC 2131 defines `0xFFFFFFFF` as the *infinite* lease, a perfectly legal value a server can send. `0xFFFFFFFF * 7` overflows `u32`:
- release build (no overflow checks): silently wraps → a tiny/garbage rebind time → the client thrashes into rebind almost immediately;
- any build with `overflow-checks = true`: **panics** — a remote DoS from one crafted/honest OFFER.

The sibling `renewal_time` default (`lease_time / 2`, [dhcp.rs:644](kernel/src/dhcp.rs:644)) is safe; only the `* 7` path overflows.

**Fix:** widen before multiplying — `((lease_time as u64) * 7 / 8) as u32` — or reorder as `lease_time / 8 * 7`.

---

### BUG-004 — 🟡 Sandbox Linux network gate misses `sendmsg`/`recvmsg`/`shutdown`
**File:** [kernel/src/sandbox.rs:247](kernel/src/sandbox.rs:247)

```rust
fn class_of_linux(nr: u64) -> Option<GateClass> {
    match nr {
        // socket/connect/accept/sendto/recvfrom/bind/listen
        41..=45 | 49 | 50 => Some(GateClass::Network),
        _ => None,
    }
}
```

The range `41..=45` then `49 | 50` deliberately steps over `46`/`47`/`48` = `sendmsg`/`recvmsg`/`shutdown` — the primary message-based send/receive syscalls. A sandboxed (`AppSandbox`/`Strict`) Linux-ABI task issuing `sendmsg`/`recvmsg` would bypass the network gate entirely. The sandbox *is* wired on spawn (`rae_manifest.rs` → `set_task_level`/`level_for_app`), so this is a live gate, not dead code.

**Why Low (for now):** `linux_syscall.rs` does not yet implement 46/47/48 (grep is empty), so the gap is currently unreachable. But this is a **denylist** in a security boundary — it fails *open* for any number not enumerated. The in-file comment already warns future device/install handlers to register here; it omits the same warning for the network class.

**Fix:** gate by socket-family intent rather than an integer allowlist, or at minimum extend the range to `41..=50` and add a regression note. Prefer fail-closed for unknown socket-class numbers.

---

### BUG-005 — 🟡 ATOMBIOS `parse_rom` accepts a zero command-table offset it claims to reject
**File:** [components/raeen_amdgpu/src/atombios.rs:92](components/raeen_amdgpu/src/atombios.rs:92)

```rust
// A zero data-table offset would be nonsensical; both must land in-image.
if master_data_table_offset == 0
    || master_data_table_offset as usize >= rom.len()
    || master_command_table_offset as usize >= rom.len()
{
    return Err(AtomError::TableOffsetOutOfRange);
}
```

The comment says "both must land in-image," but only `master_data_table_offset` is checked for `== 0`; a `master_command_table_offset == 0` passes. Not a memory-safety issue (0 is in-bounds), but a malformed/garbage VBIOS with a zero command table would be accepted as valid and mislead later command-table decoding.

**Why Low:** the vendored real VBIOS parses fine and the VFCT KATs are solid; this is robustness only. **Fix:** add `|| master_command_table_offset == 0` to match the stated contract, and add a KAT case for it.

---

### BUG-006 — 🟡 ELF loader zeroes a whole page per segment, corrupting shared sub-page-aligned segments
**File:** [kernel/src/elf.rs:208](kernel/src/elf.rs:208)

For each page of each PT_LOAD segment the loader does `core::ptr::write_bytes(frame_ptr, 0, 4096)` then copies the segment's overlap. If two PT_LOAD segments are **not** page-aligned and share a page (e.g. the tail of `.text` and head of `.data`), loading the second reuses the already-mapped frame (the `map_page_in_pml4_fallible` → false branch at [elf.rs:187-192](kernel/src/elf.rs:187)), **re-zeroes the entire page** (erasing the first segment's bytes), and copies back only the second segment's slice — losing the first segment's data in the shared page.

Related: when `map_page_in_pml4_fallible` returns false the code frees the frame and falls through rather than early-returning; it's caught one step later by `pml4_page_ptr(...).ok_or(...)?`, so it's benign but reads oddly.

**Why Low:** mainstream toolchains page-align PT_LOAD (`p_align = 0x1000`), and AthenaOS's own binaries are page-aligned, so this doesn't trigger today. It's a real latent correctness bug for any ELF with sub-page-aligned adjacent segments. **Fix:** only zero the BSS tail (`[file_end, page_end)`) rather than the whole page, or map/zero each page exactly once across all segments.

---

## AUDIT-1 — 🔵 Integer division/modulo by possibly-zero divisors (unconfirmed)

Integer `/` or `%` by zero is a `#DE` fault (kernel crash), unlike float division. The scan surfaced these divisors that I did **not** confirm are always non-zero. Each needs a quick "can this be 0?" check; guard with `.max(1)` or an early return if so.

| Location | Divisor | Reaches zero if… |
|---|---|---|
| [compositor.rs:206](kernel/src/compositor.rs:206), [:211](kernel/src/compositor.rs:211) | `vrr.max_hz` / `vrr.min_hz` | VRR range left unset / zero-initialized |
| [compositor.rs:2933](kernel/src/compositor.rs:2933), [:3104](kernel/src/compositor.rs:3104) | `refresh_hz` / `current_hz` | a panel/EDID reporting 0 Hz flows in (note BUG-001 can compute ~0 Hz) |
| [cpufreq.rs:666](kernel/src/cpufreq.rs:666), [:728](kernel/src/cpufreq.rs:728) | `highest_perf` | CPPC `highest_perf` reads 0 on unsupported/odd silicon |
| [audio.rs:616](kernel/src/audio.rs:616), [:634](kernel/src/audio.rs:634), [:932](kernel/src/audio.rs:932) | `corb.size` / `rirb.size` / ring `size` | codec init skipped/failed but the ring is still serviced |

Note: [compositor.rs:3086](kernel/src/compositor.rs:3086) already uses `.max(1)` on `target_hz`, which suggests the zero case is real for at least one of these.

---

## Verified clean (false positives I checked and dismissed)

Listed so a future pass doesn't re-flag them:

- **AthFS `name_len: name.len() as u8`** ([raefs.rs:2507](kernel/src/raefs.rs:2507) et al.) — guarded everywhere by `name.len() > 55` before the cast (`DirEntry.name` is `[u8; 55]`). Safe.
- **Battery `time_remaining_minutes` div-by-zero** ([acpi_full.rs:2411](kernel/src/acpi_full.rs:2411)) — guarded by `if self.present_rate == 0 || !self.is_discharging() { return None; }` immediately above. `percentage()` likewise guards `full_capacity == 0`. Safe.
- **ELF header / program-header table bounds** ([elf.rs:42-117](kernel/src/elf.rs:42)) — `checked_mul`/`checked_add` + `data.len()` bounds + entry-in-userspace. Well done.
- **HDA audio single-page DMA allocs** ([audio.rs:533](kernel/src/audio.rs:533), 543, 756, 766) — each structure (CORB 1 KiB, RIRB 2 KiB, BDL, data) is ≤1 page, so single-frame `allocate_frame()` is correct (no contiguity requirement). Distinct from BUG-002.
- **ATOMBIOS `parse_vfct` / `parse_rom` bounds** — fully bounds-checked (`get(off..off+n)`, zero-length terminator, `checked_add`); real-table KAT asserts byte-identity. Solid.
- **DHCP `parse_options` / `handle_eth_frame`** — tight per-field length checks before every slice; a malformed frame is rejected, not over-read. Solid.
- **Syscall dispatch** ([syscall.rs](kernel/src/syscall.rs)) — no duplicate numeric match arms.

---

## Recommended next steps

1. Fix BUG-002 (swap to `allocate_contiguous_frames`) and BUG-001 (correct math + real-EDID KAT) — both are cheap and BUG-002 removes a memory-corruption landmine.
2. Resolve AUDIT-1 divisors with `.max(1)`/guards — div-by-zero is a hard kernel crash for a one-line fix.
3. Extend the audit to the unreviewed surface: `components/rae_crypto` (security-critical, host-testable), `raenet`, `raeaudio`, and the large pattern-scanned kernel files (`compositor.rs`, `usb_core.rs`, `xhci.rs`, `ahci.rs`, `linux_compat.rs`).
4. Adopt a lint pass for the recurring classes here: width-before-multiply for wire-derived integers, and `allocate_contiguous_frames` for any DMA buffer > 1 page.
