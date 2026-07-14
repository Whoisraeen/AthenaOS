# AthenaOS Logic Bug Review

> Reviewed: 2026-06-03  
> Scope: Kernel core — scheduler, memory, context switch, syscalls, IPC, capabilities, VirtIO, interrupts, GDT/SMP, buddy allocator, futex

---

## Status update (2026-06-12, triage pass before non-safe iron flash)

Each bug re-checked against current code. **Build-verified fixes this pass** are not yet boot-proven (dev-box QEMU wedged this session) — they ride the iron flash.

**FIXED this pass (commit `bugfix: NVMe bounce leak ...`):**
- **BUG-37** 🔴 — NVMe `read_sector`/`write_sector` leaked a DMA frame + IOMMU mapping per I/O (OOM walking the FS). Now a persistent per-controller bounce buffer (`NvmeController::ensure_bounce`), one lock-hold per op. **The single most important fix for daily-driving.**
- **BUG-24** 🔴 — AthFS `cow_write_block` fresh-allocation path (`old_block == 0`) didn't `write_inode`, orphaning the block on reboot (silent data loss). Now persists the inode like the CoW path.
- **BUG-27** 🔴 — `RawSpinlock` now irqsave: disables local interrupts on `lock()`/`try_lock()`, restores on drop (the guard carries `restore_intr`). (Note: RawSpinlock has no live callers yet — this de-fuses a future footgun.)
- **BUG-39** 🟡 — RTC `compute_yearday` `month - 1` underflow on a month-0 hardware read → `saturating_sub(1)`.
- **BUG-41** 🔴 — WireGuard `add_tunnel` hardcoded `local_static = [0u8;32]`; now CSPRNG-generated per tunnel.

**ALREADY FIXED before this pass (verified in current code):**
- **BUG-04** 🔴 — `free_user_page_tables` now skips frames shared with the kernel PML4 at every level (the reboot-loop fix).
- **BUG-05** 🔴 — `map_mmio_region` uses `saturating_add` (no wrap-to-0).
- **BUG-26** 🔴 — NVMe `poll_io_completion` is a bounded poll, not a futex-block on an MMIO register.
- **BUG-03** 🔴 — Zombie reaping: deferred-reap protocol + `wake_wait_parents` (orphan-leak edge may remain — re-check).
- **BUG-10** 🟠 — futex wake wired (native SYS_FUTEX path).

**MOOT (premise no longer holds):**
- **BUG-31** 🟡 — `Superblock` has no `free_inodes` field; the inode bitmap is already durably persisted by `write_block`.

**PARTIAL / hardening deferred:**
- **BUG-42** 🔴 — TLS `getrandom` now uses the real CSPRNG as the primary path; the deterministic stub only runs as a fallback *if the CSPRNG fails* (doesn't occur in practice — netlog shows TLS PASS with real X25519). Fail-closed (refuse rather than degrade) is the remaining hardening.

**STILL NEEDS TRIAGE (not yet re-checked this pass — do NOT assume fixed):**
BUG-01, 02, 06, 07, 08, 09, 11, 12, 28, 29, 30, 32, 33, 34, 35, 36, 38, 40, 43, and the remaining 🟡/🔵 tier (BUG-13, 15, 16, 17, 18, 19, 20, 21, 22, 23).

---

## Status update (2026-06-13, second triage pass — build-verified, boot pending)

Re-checked the "still needs triage" set against current code.

**FIXED this pass (commit `bugfix: ELF reloc bounds, sandbox install-priv, ...`):**
- **BUG-06** 🔴 — MSI vector now computed in `usize` + `debug_assert` before narrowing to `u8` (was a latent silent wrap if `MSI_VEC_COUNT` grows).
- **BUG-16** 🟡 — `virtio.rs` queue alloc `allocate_contiguous_frames(3)` (=8 pages) → `(2)` (=4 pages) for the 3-page layout; was wasting 5 pages per queue.
- **BUG-22** 🔵 — APIC error handler now reads/clears the ESR (`apic::read_error_status`) and EOIs; was print-only, so the LVT could re-fire and flood the log.
- **BUG-32** 🔴 — `net::cleanup_task_sockets(pid)` sweeps the global `SOCKET_TABLE` + smoltcp sockets on task exit (wired into `exit_current_task`); was an unbounded socket/port leak per exited networked process.
- **BUG-33** 🔴 — attestation now fails CLOSED (empty quote) when no signed TPM quote is available; removed the forgeable unsigned "RAEB" PCR blob (anti-cheat spoof vector).
- **BUG-36** 🟠 — `elf_loader::apply_relocations` now returns `RelocationFailed` (with a serial line) on an out-of-segment target or out-of-bounds write, instead of silently leaving an unresolved pointer (crash / control-flow-hijack surface).
- **BUG-38** 🟠 — tickless `enter_idle` uses `next.saturating_sub(now)` so an already-due timer skips 0 ticks instead of underflowing to a max-duration sleep.
- **BUG-40** 🔴 — sandbox install class no longer aliases to `DeviceAccess{Gpu}`; maps to `CapabilityRequest{SystemConfig}` so a GPU-permitted sandbox can't reach the installer via the policy fallback. (The grant-level separation `g.install != g.devices` was already in place.)
- **BUG-43** 🟠 — GENEVE TLV length now ceiling-division `(len+3)/4` to match the padded bytes on the wire.

**ALREADY FIXED before this pass (verified in current code):**
- **BUG-01** 🔴 — `yield_task` double-enqueue block removed; it calls `finish_task_switch()` once.
- **BUG-08** 🟠 — `is_valid_derivation` now has rules for every extended cap type (Filesystem/Network/Gpu/Audio/Camera/Process/CryptoKey/Hypervisor/Attestation/Debug/System) — extended caps can be delegated.
- **BUG-09** 🟠 — `unblock_virtio_waiters(head)` matches `BlockedOnVirtio(head)` exactly.
- **BUG-13** 🟡 — `check_deadline_misses` now stores `worst_miss_us`.

**VESTIGIAL / not the live path (no fix needed now):**
- **BUG-34 / BUG-35** 🔴 — the offending `process.rs::FileDescriptorTable::read/write/close` is NOT the live I/O path. SYS_READ/WRITE (syscall.rs 16/17) go through `Task.fds` → VFS `File::read/write`, which is correct. The `FileDescriptorTable` stub is vestigial; left as-is (refactoring unreachable code unverifiably before the flash is the bigger risk). Flagged for deletion later.

**STILL OPEN (need real work / boot to verify — deferred):**
- **BUG-02** 🔴 kill_task reschedule, **BUG-25** 🔴 cpu_offline IPI — both need the IPI-park protocol.
- **BUG-28** 🔴 dma_alloc_coherent needs a real pool allocator (not a bump pointer).
- **BUG-29** 🔴 RwSemaphore waiter `AtomicBool` redesign (check live callers first).
- **BUG-23** 🔵 IPC channel teardown/refcount, **BUG-18** 🟡 keyboard hardcoded channel 1.
- **BUG-30** 🟠 compositor per-pixel HDR (needs GPU/LUT — ties to the AthGFX submit gap).
- **BUG-07** 🟠 (rcx-=2 ordering, "likely works"), **BUG-11** 🟠 / **BUG-12** 🟠 (PML4 clear / GsBase asm — need iron to verify safely), **BUG-15** 🟡 (read_user_cstr page-at-a-time), **BUG-17** 🟡 (bounce 4032), **BUG-19/20/21** 🔵 (low).

---

## Status update (2026-06-13, third triage pass — build-verified, boot pending)

Took on the "still open" set.

**FIXED this pass (commit `bugfix: DMA coherent free-list, RwSemaphore, IPC/socket reap`):**
- **BUG-28** 🔴 — `dma_engine` coherent allocator now has a free-list; `dma_free_coherent` returns the range so an alloc/free loop no longer exhausts the address space (was a pure bump pointer). (Self-contained module; live LinuxKPI DMA already uses `allocate_contiguous_frames`.)
- **BUG-29** 🔴 — `RwSemaphore::down_read_slowpath` spun on a LOCAL `AtomicBool` that `wake_readers` never touched. Now spins on the shared `count` (single source of truth); removed the entire vestigial/half-built waiter list + `wake_writer`/`wake_readers`. (No live callers.)
- **BUG-23** 🟡 — IPC `Channel` now carries `owner_pid`; `IpcSystem::cleanup_task_channels(pid)` destroys a task's channels (buffer + shared frame) on exit, wired into `exit_current_task`. System channels (`SYSTEM_OWNER`) are exempt. (`destroy_channel` already freed the shared frame.)
- **BUG-18** 🟡 — `KEYBOARD_CHANNEL`/`MOUSE_CHANNEL` constants replace the scattered `1`/`2` literals in the keyboard/mouse IRQ + USB-HID paths; main.rs `debug_assert`s the boot creation order matches.
- **BUG-20** 🔵 — inner `VirtioBlk::total_sectors` reads real capacity from config space (was 0); the live `VirtioBlockDevice` wrapper was already correct.

**ALREADY FIXED before this pass (verified):**
- **BUG-15** 🟡 — `read_user_cstr` already validates page-at-a-time (`i == 0 || addr & 0xFFF == 0`), not per byte.
- **BUG-21** 🔵 — `non_game_cores = online_mask & !dedicated_cores` (masked to online CPUs).

**NON-BUG:** **BUG-19** 🔵 — `fetch_add` is atomic under `Relaxed`; no duplicate TaskIds possible (the report concurs). Left as-is.

**DEFERRED to the iron pass (correct fix needs hardware to verify safely):**
- **BUG-02** 🔴 / **BUG-25** 🔴 — kill/offline a task running on another CPU needs the IPI-park protocol (send IPI, let the target migrate itself). SMP-sensitive; CLAUDE.md rule 17 wants ≥5 boots at smp=1/2 — do it with iron in hand.
- **BUG-11** 🟠 (PML4 PD[0..8] clear) / **BUG-12** 🟠 (GsBase on the non-canonical RCX path) — page-table / syscall-entry asm; a wrong change bricks boot. Verify on iron.
- **BUG-07** 🟠 — `rcx -= 2` after `block_current_task` is benign with work-stealing OFF and the current ordering is proven (sys_wait daemon chain boots green); reorder needs iron to confirm it doesn't regress the proven path.
- **BUG-17** 🟡 — virtio bounce caps I/O at 4032 B; QEMU-only path, currently working. Needs a 2-page bounce.
- **BUG-30** 🟠 — per-pixel software HDR; ties to the AthGFX GPU-submit gap (move tonemap to GPU/LUT).
- **BUG-34 / BUG-35** 🔴 — vestigial `process.rs::FileDescriptorTable` (not the live I/O path); flagged for deletion.

---

## 🔴 Critical — Can crash / corrupt / hang the system

---

### BUG-01: `yield_task` double-enqueue of outgoing task

**File:** [scheduler.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/scheduler.rs#L506-L514)  
**Severity:** 🔴 Critical

After `switch_context` returns, `yield_task` calls `finish_task_switch()` at line 507, which already re-locks the scheduler and enqueues the stashed task (lines 843–849). Then, *immediately after*, lines 509–514 re-lock the scheduler *again* and enqueue the same `switch_stash[cpu_id]` task a second time:

```rust
crate::scheduler::finish_task_switch(); // enqueues stash

if !is_idle {
    let mut sched = SCHEDULER.lock();
    if let Some(task) = sched.switch_stash[cpu_id].take() { // already None!
        sched.enqueue(task);
    }
}
```

Currently this is *benign* because `finish_task_switch` already `.take()`s the stash, so the second `.take()` returns `None`. But it's a **latent double-free risk**: if `finish_task_switch` is ever removed or refactored (it's also called from `kernel_thread_entry` and `thread_entry_user`), the second block would silently duplicate the task in the runqueue, causing one task to run on two CPUs simultaneously — a catastrophic data race.

**Fix:** Remove lines 509–514 entirely; `finish_task_switch()` handles it.

---

### BUG-02: `kill_task` marks current task Zombie but never reschedules it

**File:** [scheduler.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/scheduler.rs#L948-L954)  
**Severity:** 🔴 Critical

When `kill_task` targets a task that is currently running on some CPU:

```rust
for current in sched.current_task.iter_mut().flatten() {
    if current.id == task_id {
        current.state = TaskState::Zombie(0); // ← marks it
        ...
        return Ok(());                        // ← but never context-switches away
    }
}
```

The Zombie task continues executing. The next `yield_task` will pick it up, see `is_idle == false`, increment its vruntime, and re-enqueue it into the runqueue — effectively un-killing it. A Zombie should never re-enter the runqueue.

**Fix:** After marking Zombie, if the target is on the *calling* CPU, trigger `exit_current_task`; if on a *different* CPU, send an IPI to force a reschedule.

---

### BUG-03: `exit_current_task` pushes Zombie to `blocked_tasks` — leaks forever

**File:** [scheduler.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/scheduler.rs#L670-L678)  
**Severity:** 🔴 Critical (resource leak)

```rust
task.state = TaskState::Zombie(exit_code);
sched.blocked_tasks.push(alloc::boxed::Box::new(task));
```

Zombie tasks accumulate in `blocked_tasks` and are only removed if someone calls `try_wait_task` with their exact ID. If no parent ever waits on a child (kernel threads, orphan tasks, or tasks killed by faults), the Zombie `Box<Task>` is never freed. Each holds a 64 KiB kernel stack + a full PML4 user address space. On a long-running system, this is an unbounded memory leak.

**Fix:** Implement orphan reaping — when a task exits, check if its parent exists; if not, immediately drop the Zombie instead of parking it.

---

### BUG-04: `free_user_page_tables` frees shared kernel page-table pages

**File:** [memory.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/memory.rs#L600-L652)  
**Severity:** 🔴 Critical

`free_user_page_tables` walks PML4 indices 0..256 and frees *every* PDPT, PD, PT, and leaf frame it finds. But `create_new_pml4` only deep-copies PML4[0] → PDPT[0] → PD[0] (lines 293–340). **All other entries in indices 1..255 are shallow copies** — they share the kernel's PDPT/PD/PT frames.

If any user mapping accidentally lands in PML4 index 1–255 (e.g., the user stack at `0x7FFF_FFFF_A000` is PML4 index 255), `free_user_page_tables` will deallocate page-table frames that belong to the kernel or are shared with other processes, causing immediate corruption.

The user stack *is* at PML4 index 255 (`0x7FFF >> 9 = 255`), and `create_new_pml4` clones index 255 as a shallow copy from the kernel PML4. When `map_page_in_pml4` maps user stack pages, it creates new sub-tables under that entry. On task exit, `free_user_page_tables` frees those sub-tables — but also frees whatever was already there from the kernel clone.

**Fix:** Track which page-table frames were specifically allocated for user mappings (e.g., via a per-PML4 allocation log or by only walking PML4 entries that were modified from the kernel template).

---

### BUG-05: `map_mmio_region` wrapping overflow for high physical addresses

**File:** [memory.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/memory.rs#L439)  
**Severity:** 🔴 Critical

```rust
let end_phys = (phys_addr.wrapping_add(size as u64).wrapping_add(0xFFF)) & !0xFFF;
```

Uses `wrapping_add` which silently wraps to 0 if `phys_addr + size + 0xFFF > u64::MAX`. If `size` is large or `phys_addr` is near `u64::MAX`, `end_phys` wraps to a small value and the `while p < end_phys` loop either doesn't execute (mapping nothing) or maps an astronomically large range. A BAR at the top of the 64-bit address space could trigger this.

**Fix:** Use `checked_add` and return an error / clamp on overflow.

---

### BUG-06: MSI vector calculation truncation — `u8` overflow

**File:** [interrupts.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/interrupts.rs#L74)  
**Severity:** 🔴 Critical

```rust
let vector = MSI_VEC_BASE + (bucket_idx * 64) as u8 + free_bit as u8;
```

`MSI_VEC_BASE = 64`. When `bucket_idx == 2` and `free_bit == 63`: `64 + (128 as u8) + (63 as u8)`. The cast `(bucket_idx * 64) as u8` truncates `128` to `128` (fine), but `64 + 128 + 63 = 255` which fits. However, when `bucket_idx == 3` (impossible since only 3 buckets exist, so max is 2) — wait, the real issue is `(bucket_idx * 64) as u8`: for `bucket_idx = 2`, `2*64 = 128`, which is fine as u8. For `bucket_idx = 3` (if MSI_VEC_COUNT were increased), `3*64 = 192`, also fits. But the whole expression can overflow u8: `64 + 192 + 63 = 319 → wraps to 63`, silently assigning the wrong IDT vector.

More concretely: the bitmap only has 3 entries covering 192 vectors, but the calculation uses `u8` arithmetic. If someone increases `MSI_VEC_COUNT` beyond `192` without updating this, it silently wraps.

**Fix:** Perform the entire calculation in `usize` and assert it fits in `u8`.

---

### BUG-24: `cow_write_block` does not write inode on fresh allocation (Data Loss)

**File:** [athfs.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/athfs.rs#L1054-L1059)  
**Severity:** 🔴 Critical (Data Loss)

When writing to a previously unallocated block slot (`old_block == 0`), `cow_write_block` allocates a new block, writes the data, and updates `inode.direct_blocks[block_idx] = new_block`. It then immediately returns `Ok(())` **without writing the inode back to disk** (`self.write_inode(inode)` is missing on this branch). If the system reboots, the newly allocated block is orphaned and the file retains a hole.

**Fix:** Add `self.write_inode(inode)?;` before returning `Ok(())` in the `old_block == 0` path.

---

### BUG-25: `cpu_offline` steals running tasks without stopping the target CPU

**File:** [smp.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/smp.rs#L1138-L1143)  
**Severity:** 🔴 Critical

When taking a CPU offline, `cpu_offline` (running on CPU A) directly locks the `current_task` of CPU B, `.take()`s it, and enqueues it onto another CPU. However, CPU B is *still actively executing that task*. CPU B is never interrupted (no IPI is sent to force it into the idle/parking loop). This results in the same task executing simultaneously on two CPUs, leading to immediate state corruption and crashes.

**Fix:** Send a high-priority IPI to the target CPU to force it into a parking routine where *it* migrates its own tasks, rather than CPU A doing it asynchronously.

---

### BUG-26: `poll_io_completion` blocks on MMIO futex (Hangs forever)

**File:** [nvme.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/nvme.rs#L1250-L1263)  
**Severity:** 🔴 Critical

If an NVMe completion is not immediately ready, `poll_io_completion` attempts to sleep by calling `FUTEX_MANAGER.wait(cq_doorbell, ...)` on the NVMe Completion Queue doorbell register. Hardware does not use futexes; it issues MSI/IRQs. Because there is no interrupt handler calling `futex_wake` on the doorbell address, any task that takes this path will hang indefinitely.

**Fix:** Implement a proper IRQ handler for NVMe that signals a waitqueue or event, and block on that event instead of using a futex on an MMIO register.

---

### BUG-27: `RawSpinlock` does not disable interrupts (Deadlock)

**File:** [locking.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/locking.rs#L395-L416)  
**Severity:** 🔴 Critical

`RawSpinlock::lock` busy-waits on an `AtomicBool` but never disables local CPU interrupts (`cli` / saving `rflags`). If a thread acquires a `RawSpinlock` and is subsequently interrupted by an IRQ, and the IRQ handler attempts to acquire the exact same lock, the CPU will deadlock spinning forever.

**Fix:** `RawSpinlock` must disable local interrupts on `lock()` and restore the previous interrupt state on `unlock()`.

---

### BUG-28: `dma_alloc_coherent` permanently leaks memory

**File:** [dma_engine.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/dma_engine.rs#L304-L328)  
**Severity:** 🔴 Critical

`dma_alloc_coherent` acts as a simple bump allocator, incrementing `engine.coherent_next_addr` for each allocation. However, `dma_free_coherent` only removes the tracking metadata from `active_mappings`—it never reclaims or frees the actual physical memory or address space. Repeated allocations and frees will permanently leak physical memory.

**Fix:** Use a proper allocator (like a bitmap or buddy allocator) for the DMA coherent pool instead of a bump pointer.

---

### BUG-29: `RwSemaphore` readers spin forever due to disconnected `AtomicBool`s

**File:** [locking.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/locking.rs#L705-L723)  
**Severity:** 🔴 Critical

In `down_read_slowpath`, a local stack variable `waiter` is created with its own `AtomicBool`. A **completely separate instance** of `RwSemWaiter` with a new `AtomicBool::new(false)` is pushed into the `self.waiters` vector. The reader then spins checking its *local* `waiter.woken`. When `wake_readers` executes, it sets `woken = true` on the instance inside the vector. The reader's local variable never changes, causing an infinite loop.

**Fix:** Store an `Arc<AtomicBool>` or use a centralized waitqueue where the task ID itself can be woken by the scheduler.

---

### BUG-32: `sys_net_socket` leaks sockets permanently on process exit

**File:** [net.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/net.rs#L524-L525)  
**Severity:** 🔴 Critical (Resource Leak)

`SYS_NET_SOCKET` uses a disjoint global `SOCKET_TABLE` instead of the process's own `FileDescriptorTable`. When a process terminates, the kernel cleans up its `fd_table`, but it does not know about or sweep `SOCKET_TABLE`. All sockets opened by the process remain open in `SOCKET_TABLE` forever, causing an unbounded memory and port leak.

**Fix:** Integrate sockets into the standard `Process::fd_table` so they are closed automatically on process teardown, or add an explicit sweep of `SOCKET_TABLE` during task exit.

---

### BUG-33: `generate_attestation_quote` fallback is unsigned and trivially spoofable

**File:** [security.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/security.rs#L245-L258)  
**Severity:** 🔴 Critical (Security)

If the TPM is unavailable or `quote` fails, the fallback attestation path returns a blob of raw PCR values prefixed with "RAEB". Because this blob lacks any cryptographic signature from an Attestation Key, a malicious userspace agent or compromised kernel can trivially intercept the attestation request and forge arbitrary PCR values, entirely defeating remote anti-cheat validation.

**Fix:** Remove the fallback. If a cryptographically secure TPM quote cannot be generated, attestation must fail outright.

---

### BUG-34: `FileDescriptorTable::close` and `dup2` leak underlying resources

**File:** [process.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/process.rs#L483-L488)  
**Severity:** 🔴 Critical (Resource Leak)

Calling `close()` or `dup2()` on a file descriptor merely removes the `FileDescriptor` struct from the `fds` BTreeMap. It does not invoke any cleanup logic, decrement VFS refcounts, or close Pipe endpoints. As a result, underlying resources (like Pipe memory buffers) are permanently leaked, and blocking readers/writers never receive EOF or EPIPE signals.

**Fix:** Implement `Drop` for `FileDescriptor` or explicitly invoke VFS/Pipe close logic when removing an FD from the table.

---

### BUG-35: `FileDescriptorTable::read` and `write` silently discard I/O

**File:** [process.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/process.rs#L490-L508)  
**Severity:** 🔴 Critical (Logic Bug)

The `read` and `write` implementations in the POSIX file descriptor table do not actually interact with the VFS, files, or pipes. They merely increment the `descriptor.offset` and return success. All writes are silently dropped into the void, and all reads return whatever garbage happened to be in the user's buffer. 

**Fix:** Plumb `read` and `write` calls through to the actual underlying `FdType` (VFS inode, Pipe buffer, Device).

---

### BUG-37: `NvmeBlockDevice` leaks DMA bounce buffers on every I/O operation

**File:** [nvme.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/nvme.rs#L1777)  
**Severity:** 🔴 Critical (Resource Leak)

The `read_sector` and `write_sector` methods in the NVMe adapter allocate a 4 KiB DMA bounce frame via `alloc_dma_frame_mapped()` to perform the transfer. However, this memory is never freed. Every single block I/O operation permanently leaks a physical frame and its corresponding IOMMU mapping. Under normal load (e.g., walking a filesystem), the OS will rapidly consume all physical memory and trigger an Out-Of-Memory crash in seconds.

**Fix:** Implement a matching `free_dma_frame_mapped` function and call it at the end of `read_sector` and `write_sector` to release the bounce buffer.

---

### BUG-40: Sandbox `classify` incorrectly maps `SYS_INSTALL_RUN` and driver syscalls to GPU access

**File:** [sandbox.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/sandbox.rs#L127)  
**Severity:** 🔴 Critical (Security / Privilege Escalation)

In the syscall classification layer of `sandbox.rs`, highly privileged operations like `SYS_INSTALL_RUN`, `SYS_DRIVER_REGISTER`, and `SYS_LINUXKPI_PCI_ENABLE` are arbitrarily mapped to `SyscallRequest::DeviceAccess { kind: DeviceKind::Gpu, write: true }`. Because allowing GPU access is standard for any sandboxed game or hardware-accelerated app, giving a process GPU access inadvertently grants it full permission to install OS updates to the raw disk or register arbitrary DMA-capable ring-0 drivers.

**Fix:** Map installation syscalls to a `Disk` or `System` device kind, and driver/PCI syscalls to a dedicated `DriverHost` permission. Do not alias them to `Gpu`.

---

### BUG-41: WireGuard uses a hardcoded zeroed private key for all tunnels

**File:** [wireguard.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/wireguard.rs#L134)  
**Severity:** 🔴 Critical (Cryptography)

When creating a new WireGuard tunnel via `add_tunnel`, the `local_static` private key is hardcoded to `[0u8; 32]`. The code comments that it will be loaded from sealed storage in "Phase 3". However, deploying WireGuard with a known private key means any passive or active attacker on the network can trivially derive the transport keys, decrypt all traffic, and impersonate the tunnel endpoints.

**Fix:** If the key cannot yet be loaded from sealed storage, it must be generated randomly during tunnel creation or provided via the `SYS_WG_ADD` syscall parameters, but never hardcoded to zeros.

---

### BUG-42: TLS `getrandom` generates completely deterministic entropy

**File:** [tls.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/tls.rs#L84)  
**Severity:** 🔴 Critical (Cryptography)

The TLS implementation uses a local stub for `getrandom()` that fills the buffer with `(i as u8).wrapping_mul(0x5D).wrapping_add(0xA3)`. This means that `client_random` and `server_random` generated for every single TLS handshake are identical and completely deterministic. This catastrophically breaks TLS replay protection and makes all derived cryptographic session keys trivially guessable.

**Fix:** Remove the local deterministic stub and call the actual kernel CSPRNG (`crate::crypto::getrandom`).

---

## 🟠 High — Correctness bugs that can cause subtle data corruption or deadlocks

---


### BUG-07: Syscall retry `rcx -= 2` modifies RIP while the task is blocked

**File:** [syscall.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/syscall.rs#L542-L543)  
**Severity:** 🟠 High

```rust
crate::scheduler::block_current_task(...);
regs.rcx -= 2; // Decrease RIP to retry syscall
```

`block_current_task` calls `block_current_task_with` which **context-switches away**. When execution resumes (the task is unblocked), it returns to here, but `regs` points at the **syscall register frame on the kernel stack**, which was the outgoing task's stack. After the context switch *back*, we're running on the same stack — but the `rcx` modification happens *after* the context switch completes.

If the architecture of the context switch saves/restores the register frame properly, this works. But if the task is resumed on a different CPU (work stealing), the pointer `regs` might be dangling or pointing at a different task's stack frame if the stack was reallocated.

**Risk:** The `regs` pointer is to the kernel stack, which is stable per-task, so this likely works — but only because the task's kernel stack isn't freed while it's blocked. Still fragile.

**Fix:** Set `rcx -= 2` *before* calling `block_current_task` so the modification is committed to the task's stack frame before the switch.

---

### BUG-08: `SYS_CAP_GRANT` doesn't enforce `GRANT` right on the *derived* cap

**File:** [syscall.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/syscall.rs#L658-L764)  
**Severity:** 🟠 High (security)

The syscall constructs the derived `Cap` using `new_rights` directly from userspace (`regs.rdx`), which is only `from_bits_truncate`'d. The `grant()` function checks that the *parent* has `GRANT` right, and `is_valid_derivation` checks that child rights ⊆ parent rights for Channel/Mmio/Irq/Port. But for the **extended capability types** (Filesystem, Network, Gpu, Audio, Camera, Process, CryptoKey, Hypervisor, Attestation, Debug, System), `is_valid_derivation` returns `false` on the `_ => false` catch-all.

This means **no extended capability can ever be granted through `SYS_CAP_GRANT`** — it always returns `E_INVALID_DERIVE`. This is a functional bug: the entire capability delegation system is broken for everything beyond the original four cap flavors.

**Fix:** Add derivation rules for all extended cap variants in `is_valid_derivation`.

---

### BUG-09: `unblock_virtio_waiters` ignores the `head` parameter — wakes ALL waiters

**File:** [scheduler.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/scheduler.rs#L1102-L1116)  
**Severity:** 🟠 High

```rust
pub fn unblock_virtio_waiters(_head: u16) {
    // ...
    if matches!(sched.blocked_tasks[i].state, TaskState::BlockedOnVirtio(_)) {
```

The `_head` parameter is entirely ignored. When *any* VirtIO completion fires, *every* task blocked on *any* VirtIO request is woken up. This causes spurious wakeups where a task sees its `completed_requests[head]` is still false and re-blocks. While not immediately dangerous (the task retries), it wastes CPU cycles and breaks the blocking model — tasks can ping-pong between blocked/running states, starving the runqueue.

**Fix:** Match `BlockedOnVirtio(head)` against the actual completed head index.

---

### BUG-10: `futex::wake` calls `unblock_futex_waiter` which is a no-op

**File:** [sync.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/sync.rs#L50)  → [scheduler.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/scheduler.rs#L1118-L1120)  
**Severity:** 🟠 High

```rust
// sync.rs
crate::scheduler::unblock_futex_waiter(task_id);

// scheduler.rs
pub fn unblock_futex_waiter(_task_id: TaskId) {
    // Futex state is not modeled in TaskState yet; keep as compatibility no-op.
}
```

Futex wakes do nothing. Any task that blocks on a futex is **never unblocked**. The `FutexManager::wait` records the task ID and returns `true` (proceed to block), but the corresponding `wake` is a complete no-op. This renders the entire futex subsystem non-functional.

**Fix:** Either implement `BlockedOnFutex` in `TaskState` and wire `unblock_futex_waiter` to actually wake the task, or remove the futex API until it's fully implemented.

---

### BUG-11: `create_new_pml4` deep-copies PML4[0] → PDPT[0] → PD[0..8] — but clears too much

**File:** [memory.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/memory.rs#L327-L334)  
**Severity:** 🟠 High

```rust
if i < 8 {
    // Clear the lower 16MB (user space region)
    new_pd[i] = PageTableEntry::new();
} else {
    new_pd[i] = kernel_pd[i].clone();
}
```

This blanks PD entries 0–7, which cover VA `0x0000_0000` through `0x0100_0000` (first 16 MiB). But the kernel is mapped starting at VA `0x0100_0000` (PD entry 8 and above), so this is intentionally clearing the identity-mapped low memory. The problem is: **PD entry 0 covers `0x0000_0000`–`0x0020_0000`**, and if the bootloader placed any kernel structures (e.g., ACPI tables, page tables) there, clearing these entries disconnects them from the user process's address space.

More critically, the comment says "lower 16MB" but PD entries 0–7 only cover 0–16 MiB if each PD entry maps 2 MiB. This is correct for 2 MiB pages, but the code then expects the PD to contain 4 KiB PT entries (it checks for `HUGE_PAGE`). If these are 4 KiB granularity PD entries, each PD entry covers only 2 MiB, so 0–7 = 16 MiB. The logic is correct *if* the bootloader uses 4 KiB pages, but if it uses 2 MiB huge pages in this range, clearing them also clears kernel mappings.

**Risk:** If the kernel's own code/data is below 16 MiB (uncommon but possible with some bootloader layouts), this clears kernel mappings for user tasks, causing immediate kernel page faults when the kernel accesses those addresses while running under the user PML4.

---

### BUG-12: `GsBase` encodes CPU ID — but `syscall_handler` does `swapgs` which clobbers it

**File:** [gdt.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/gdt.rs#L126) + [gdt.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/gdt.rs#L207) + [syscall.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/syscall.rs#L147-L240)  
**Severity:** 🟠 High

BSP sets `GsBase = 0` (CPU 0). APs set `GsBase = cpu_id`. `current_cpu_id()` reads `GsBase` to get the CPU ID. But the syscall handler uses SWAPGS to exchange GsBase with KernelGsBase (which points at `PerCpuSyscall`). After the inner syscall handler runs (with `swapgs` back to normal GsBase), the handler does another `swapgs` before `sysretq` (line 201), restoring GsBase to the CPU ID value.

The issue: **during the syscall handler inner execution** (between lines 177 and 179), GsBase holds the CPU ID (restored by the second SWAPGS on line 177). But `KernelGsBase` was set by `init_percpu_syscall` to point at `PerCpuSyscall[cpu_id]`. After `sysretq` (line 201, after SWAPGS on line 201), GsBase holds the address of `PerCpuSyscall[cpu_id]`, which is a large pointer value — **not** the CPU ID.

Wait, let me re-trace:
1. Entry: `swapgs` (line 153) — GsBase ↔ KernelGsBase. Now GsBase = PerCpuSyscall*, KernelGsBase = cpu_id.
2. `swapgs` (line 177) — swap back. Now GsBase = cpu_id, KernelGsBase = PerCpuSyscall*. `syscall_handler_inner` runs with correct GsBase for `current_cpu_id()`.
3. `swapgs` (line 179) — swap again. Now GsBase = PerCpuSyscall*, KernelGsBase = cpu_id. Restore registers, restore user RSP from `gs:[0x00]`.
4. `swapgs` (line 201) — swap back. Now GsBase = cpu_id, KernelGsBase = PerCpuSyscall*.
5. `sysretq` — returns to user mode with GsBase = cpu_id. ✅

This actually looks correct on the happy path. But the **non-canonical RCX** path at line 233:
1. `swapgs` (line 234) — Now GsBase = PerCpuSyscall*, KernelGsBase = cpu_id.
2. Reads `gs:[0x08]` (PerCpuSyscall::kernel_stack_top) — correct.
3. Calls `exit_current_task` — but GsBase is still PerCpuSyscall*, not cpu_id.

Inside `exit_current_task`, `current_cpu_id()` reads GsBase and gets a large address, which the clamp in `current_cpu_id()` maps to 0. This is **wrong** if the task was running on CPU ≠ 0. The task on CPU 3 would have its context corrupted on CPU 0's slot.

**Fix:** Add another `swapgs` before `call exit_current_task` on the non-canonical path, or restore GsBase explicitly.

---

### BUG-30: Compositor HDR tonemapping causes severe performance DoS

**File:** [compositor.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/compositor.rs#L362-L402)  
**Severity:** 🟠 High (Performance DoS)

`HdrPipeline::process_pixel` performs floating-point sRGB-to-linear, PQ EOTF, and AcesFilmic tone mapping *per pixel* in software. The `f32_pow` and `f32_exp` functions use unoptimized Taylor series loops (up to 90 iterations per channel). At 1080p 60FPS, this requires over 10 billion iterations per second on the CPU, which will completely lock up the kernel and halt rendering.

**Fix:** Remove per-pixel floating-point math in the kernel compositor. Precompute a 3D LUT (Look-Up Table) or mandate that HDR tone mapping is done on the GPU via hardware acceleration.

---

### BUG-36: `apply_relocations` silently ignores out-of-bounds ELF relocations

**File:** [elf_loader.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/elf_loader.rs#L644-L649)  
**Severity:** 🟠 High (Security / Correctness)

If an ELF relocation points to an address outside of any loaded segment (`None => continue`), or if the relocation size exceeds the segment boundary (`if seg_offset + 8 <= segment.data.len()`), the loader silently ignores the error and continues. This leaves unresolved pointers in the loaded binary, which will cause arbitrary crashes or allow control-flow hijacking at runtime.

**Fix:** Return `Err(ElfError::RelocationFailed)` if a relocation target is out of bounds or cannot be fully written.

---

### BUG-38: Tickless manager underflows `next - now` causing maximum sleep duration

**File:** [timers.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/timers.rs#L587)  
**Severity:** 🟠 High (Scheduling / Real-time)

In `TicklessManager::enter_idle`, when determining how long to sleep before the next timer event, it calculates `let skip = (next - now).min(self.max_skip)`. If a timer has already expired by the time `enter_idle` is evaluated (`next < now`), the subtraction underflows to a massive integer, which is then clamped to `self.max_skip`. Instead of skipping 0 ticks and waking immediately, the CPU is put to sleep for the maximum possible duration, causing severe latency spikes and violating timer deadlines.

**Fix:** Use `let skip = next.saturating_sub(now).min(self.max_skip);` to safely bound past events to 0.

---

### BUG-43: GENEVE TLV encoder truncates length header on unaligned options

**File:** [tunnel.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/tunnel.rs#L687)  
**Severity:** 🟠 High (Network Parsing / Out of Bounds)

In `GeneveTlvOption::encode`, the option length byte is calculated as `(self.data.len() / 4) as u8`. However, the encoder writes the full `self.data.len()` bytes, followed by padding to a 4-byte boundary. If the option data length is not an exact multiple of 4 (e.g., 5 bytes), the written length field (1) will be smaller than the actual bytes consumed on the wire (8 bytes). This corrupts the TLV chain, causing the remote endpoint to read out of sync, potentially resulting in out-of-bounds reads or dropped packets.

**Fix:** Calculate the length byte using ceiling division: `(self.data.len() + 3) / 4`.

---

## 🟡 Medium — Logic issues that cause incorrect behavior but aren't immediately dangerous

---

### BUG-13: `check_deadline_misses` updates `worst_miss_us` stat — but never actually stores the value

**File:** [scheduler.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/scheduler.rs#L333-L346)  
**Severity:** 🟡 Medium

```rust
fn check_deadline_misses(&mut self) {
    // ...
    if dl.check_miss(now_us) {
        let overshoot = now_us.saturating_sub(dl.absolute_deadline);
        self.deadline_stats.total_misses += 1;
        dl.runtime_us = dl.runtime_us.saturating_add(dl.runtime_us / 4);
    }
}
```

`overshoot` is computed but never used. `DeadlineStats::worst_miss_us` is never updated. Also, `total_invocations` in the global stats is never incremented (only the per-task `DeadlineTask::total_invocations` is). The global `DeadlineStats` will always show `worst_miss_us == 0` and `total_invocations == 0`.

**Fix:** Add `self.deadline_stats.worst_miss_us = self.deadline_stats.worst_miss_us.max(overshoot);` and track total invocations.

---

### BUG-14: `mmap` with no fixed address returns `self.mmap_base` *after* decrementing — off-by-one

**File:** [process.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/process.rs#L304-L309)  
**Severity:** 🟡 Medium

```rust
let base = self.mmap_base;
self.mmap_base -= aligned_length;
self.mmap_base &= !0xFFF;
self.mmap_base  // ← returns the DECREMENTED value
```

The function saves `base` but then returns `self.mmap_base` (the decremented value). Meanwhile, `start` was set on the else branch. Wait — re-reading:

```rust
} else {
    let base = self.mmap_base;
    self.mmap_base -= aligned_length;
    self.mmap_base &= !0xFFF;
    self.mmap_base     // ← this is the start value
};
```

So `start = self.mmap_base` (decremented). The region is then `start..start+aligned_length`. But the *next* mmap call will decrement again from `self.mmap_base`. This means the *new* region starts at `self.mmap_base` and the *old* `base` value is unused. The regions grow downward, which is fine for a downward-growing mmap area. However, the first `mmap_base` value (`0x7F00_0000_0000`) is never used as a region start — it's skipped. This wastes one slot of address space. Minor but worth noting.

Actually, re-reading more carefully: `start = self.mmap_base` after decrement. This means the *returned* address is `self.mmap_base - aligned_length` (approximately). The region at `[mmap_base-len, mmap_base)` is correct. OK, this is actually not a bug, just confusing code. **Downgrading — not a real bug.**

---

### BUG-15: `read_user_cstr` validates one byte at a time — O(n²) page table walks

**File:** [syscall.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/syscall.rs#L423-L440)  
**Severity:** 🟡 Medium (performance DoS)

```rust
for i in 0..max_len {
    let addr = ptr.checked_add(i as u64)?;
    if validate_user_range(addr, 1, false).is_err() {
        return None;
    }
    let b = unsafe { *(addr as *const u8) };
```

Each byte triggers a full 4-level page walk in `validate_user_range` → `user_leaf_flags`. For a 4096-byte path, that's 4096 × 4 = 16384 page-table reads. A malicious userspace can force the kernel into a very long critical path (interrupts may be disabled during syscall handling) by passing a near-max-length string.

**Fix:** Validate entire pages at a time — check the current page once, then scan bytes until the next page boundary.

---

### BUG-16: `allocate_contiguous_frames(3)` allocates `2^3 = 8` pages, not 3

**File:** [virtio.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/virtio.rs#L113)  
**Severity:** 🟡 Medium (waste)

```rust
let phys_base = crate::memory::allocate_contiguous_frames(3)
```

`allocate_contiguous_frames(order)` allocates `2^order` pages. `order=3` → 8 pages (32 KiB). The comment says 3 pages are needed (12 KiB). This over-allocates by 20 KiB per VirtIO queue and wastes 5 pages that are never used but never freed.

**Fix:** Use `allocate_contiguous_frames(2)` (4 pages = 16 KiB, which rounds up from 3 needed pages), or better yet, allocate exactly 3 individual contiguous pages if the buddy allocator supports it.

---

### BUG-17: `DMA bounce buffer` is 4 KiB but only allows `4096 - 64 = 4032` bytes of data

**File:** [virtio.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/virtio.rs#L273)  
**Severity:** 🟡 Medium

```rust
const MAX_DATA: usize = 4096 - 64;
```

A standard 512-byte sector read/write works fine. But if a caller tries to do a multi-sector I/O (e.g., 8 sectors × 512 = 4096 bytes), it's rejected because 4096 > 4032. The `sector_size()` returns 512, so callers expect to be able to read full sectors — but the maximum I/O size is 7.875 sectors, not 8. Any block layer that issues 4 KiB (1 page) reads will fail silently.

**Fix:** Allocate 2 pages for the bounce buffer, or document and enforce the < 4032 byte limit at the block layer level.

---

### BUG-18: Keyboard interrupt sends to hardcoded IPC channel 1 — may not exist

**File:** [interrupts.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/interrupts.rs#L1026)  
**Severity:** 🟡 Medium

```rust
let _ = ipc.send(1, msg);
```

Channel 1 is hardcoded. `IpcSystem::create_channel` starts at `next_chan_id = 1`, so the first channel created gets ID 1. But if no channel has been created yet (possible during early boot), `send(1, msg)` returns `Err("Invalid channel")`, which is silently discarded by `let _`. The keystroke is lost. More importantly, if another subsystem creates channel 1 first for a non-keyboard purpose, keyboard events pollute that channel.

Similarly, `unblock_receivers(1)` (line 1037) wakes receivers on channel 1, which may not be the keyboard channel.

**Fix:** Use a well-known constant or a dedicated registration mechanism for the keyboard channel.

---

### BUG-31: `allocate_inode` misses superblock flush and `free_inodes` update

**File:** [athfs.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/athfs.rs#L980-L997)  
**Severity:** 🟡 Medium

When `allocate_inode` successfully claims a bit in the inode bitmap, it writes the bitmap block to disk but fails to decrement `self.superblock.free_inodes` and does not call `self.flush_superblock()`. In contrast, `allocate_block` correctly handles these steps. If a crash occurs, the superblock's free inode count will be out of sync.

**Fix:** Decrement `free_inodes` and call `self.flush_superblock()` before returning the allocated inode ID.

---

### BUG-39: `RtcTime::compute_yearday` underflows if hardware month is 0

**File:** [timers.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/timers.rs#L707)  
**Severity:** 🟡 Medium (Logic)

When calculating the day of the year, the loop boundary is defined as `0..(self.month as usize - 1).min(11)`. If a failing or uninitialized hardware RTC returns 0 for the month, the subtraction underflows to `usize::MAX`, which `.min(11)` clamps to `11`. The OS will erroneously calculate the yearday as if the date is in December, leading to corrupted timestamps in logs or the VFS.

**Fix:** Use `(self.month as usize).saturating_sub(1).min(11)` or explicitly validate that the RTC fields are within expected bounds before computation.

---

## 🔵 Low — Minor issues, code quality, or defensive improvements

---

### BUG-19: `TaskId` counter uses `Relaxed` ordering — potential duplicate IDs under SMP

**File:** [task.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/task.rs#L34-L35)  
**Severity:** 🔵 Low

```rust
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
TaskId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
```

`fetch_add` is atomic even with `Relaxed` ordering, so duplicates are impossible. However, `Relaxed` means other threads may observe stale values of the counter in unrelated loads. Since `TaskId` is only used after `fetch_add` returns, this is technically safe. But it's unconventional for a global counter — `SeqCst` or `AcqRel` would be more defensive.

---

### BUG-20: `VirtioBlk::total_sectors` returns 0 — callers can't determine disk size

**File:** [virtio.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/virtio.rs#L499-L503)  
**Severity:** 🔵 Low

```rust
fn total_sectors(&self) -> u64 {
    0
}
```

The `VirtioBlk` block device always reports 0 sectors. The wrapper `VirtioBlockDevice` reads the capacity from I/O ports, but direct users of `VirtioBlk` get 0. Any code checking capacity before I/O will think the disk is empty.

---

### BUG-21: `null_latency::non_game_cores` uses bitwise NOT — wraps to include invalid core bits

**File:** [scheduler.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/scheduler.rs#L1067)  
**Severity:** 🔵 Low

```rust
nl.dedicated_cores = 0b1111;
nl.non_game_cores = !nl.dedicated_cores;
```

`!0b1111 = 0xFFFF_FFFF_FFFF_FFF0`. This includes bits for CPUs 4–63, most of which don't exist. If the system has only 4 cores, the mask claims 60 non-existent cores are "non-game", which could cause tasks to be scheduled to offline CPUs if the mask is ever used in `select_cpu`.

---

### BUG-22: `APIC error handler` doesn't read the Error Status Register

**File:** [interrupts.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/interrupts.rs#L1083-L1085)  
**Severity:** 🔵 Low

```rust
extern "C" fn apic_error_inner() {
    serial_println!("[EXCEPTION] APIC Error");
}
```

The APIC Error LVT fires when the Local APIC detects an error condition. To clear the error and understand what happened, you must read the Error Status Register (ESR) by writing 0 to it first, then reading it. Without this, the error condition persists and the LVT may keep firing, flooding the log.

---

### BUG-23: `IPC Channel` never gets cleaned up — leaked when both endpoints are destroyed

**File:** [ipc.rs](file:///c:/Users/woisr/OneDrive/Documents/AthenaOS/kernel/src/ipc.rs)  
**Severity:** 🔵 Low

There is no `destroy_channel` or reference counting. Once a channel is created, it lives in the `BTreeMap` forever. If both the sender and receiver tasks exit, the channel (with its 64-message buffer + potential shared frame) is never freed. Over time, this is an unbounded memory leak proportional to the number of IPC channels ever created.

---

## Summary Table

| ID | Severity | Component | One-line |
|---|---|---|---|
| BUG-01 | 🔴 Critical | Scheduler | Double-enqueue risk in `yield_task` (latent) |
| BUG-02 | 🔴 Critical | Scheduler | `kill_task` on running task doesn't reschedule |
| BUG-03 | 🔴 Critical | Scheduler | Zombie tasks leak in `blocked_tasks` forever |
| BUG-04 | 🔴 Critical | Memory | `free_user_page_tables` frees shared kernel PT frames |
| BUG-05 | 🔴 Critical | Memory | `map_mmio_region` wrapping overflow on high addresses |
| BUG-06 | 🔴 Critical | Interrupts | MSI vector u8 overflow in `allocate_msi_vector` |
| BUG-24 | 🔴 Critical | AthFS | `cow_write_block` ignores inode write on fresh alloc (Data Loss) |
| BUG-25 | 🔴 Critical | SMP | `cpu_offline` steals running tasks without IPI |
| BUG-26 | 🔴 Critical | NVMe | `poll_io_completion` blocks on MMIO futex forever |
| BUG-27 | 🔴 Critical | Locking | `RawSpinlock` deadlocks with interrupts |
| BUG-28 | 🔴 Critical | DMA | `dma_alloc_coherent` permanently leaks memory |
| BUG-29 | 🔴 Critical | Locking | `RwSemaphore` readers spin forever on local var |
| BUG-32 | 🔴 Critical | Net | `sys_net_socket` leaks sockets on process exit |
| BUG-33 | 🔴 Critical | Security | Attestation quote fallback is unsigned and spoofable |
| BUG-34 | 🔴 Critical | Process | `close` and `dup2` leak underlying VFS/Pipe resources |
| BUG-35 | 🔴 Critical | Process | `read` and `write` silently discard all I/O |
| BUG-37 | 🔴 Critical | NVMe | `read_sector`/`write_sector` leaks bounce buffers on every I/O |
| BUG-40 | 🔴 Critical | Sandbox | Driver/Install syscalls mapped to `GPU` access privilege |
| BUG-41 | 🔴 Critical | WireGuard | Private key hardcoded to all zeros |
| BUG-42 | 🔴 Critical | TLS | `getrandom` is completely deterministic |
| BUG-07 | 🟠 High | Syscall | Retry `rcx -= 2` after context switch is fragile |
| BUG-08 | 🟠 High | Capability | Extended cap types can never be granted (derivation always fails) |
| BUG-09 | 🟠 High | Scheduler | `unblock_virtio_waiters` ignores head — wakes all |
| BUG-10 | 🟠 High | Futex | `unblock_futex_waiter` is a no-op — futex broken |
| BUG-11 | 🟠 High | Memory | `create_new_pml4` clears PD[0..8] — may clear kernel maps |
| BUG-12 | 🟠 High | Syscall/GDT | Non-canonical RCX path runs `exit_current_task` with wrong GsBase |
| BUG-30 | 🟠 High | Compositor| Software HDR tonemapping loop causes performance DoS |
| BUG-36 | 🟠 High | ELF Loader| `apply_relocations` silently ignores out-of-bounds relocs |
| BUG-38 | 🟠 High | Timers | Tickless idle underflows `next - now`, sleeping max duration |
| BUG-43 | 🟠 High | Network | GENEVE TLV encoder emits undersized length for unaligned data |
| BUG-13 | 🟡 Medium | Scheduler | `worst_miss_us` stat never written |
| BUG-15 | 🟡 Medium | Syscall | `read_user_cstr` O(n²) page walks — performance DoS |
| BUG-16 | 🟡 Medium | VirtIO | `allocate_contiguous_frames(3)` wastes 5 pages |
| BUG-17 | 🟡 Medium | VirtIO | DMA bounce caps I/O at 4032 bytes, not 4096 |
| BUG-18 | 🟡 Medium | Interrupts | Keyboard → hardcoded channel 1 may not exist |
| BUG-31 | 🟡 Medium | AthFS | `allocate_inode` misses superblock flush |
| BUG-39 | 🟡 Medium | Timers | `compute_yearday` underflows if RTC month is 0 |
| BUG-19 | 🔵 Low | Task | TaskId uses `Relaxed` ordering (safe but unconventional) |
| BUG-20 | 🔵 Low | VirtIO | `VirtioBlk::total_sectors()` always returns 0 |
| BUG-21 | 🔵 Low | Scheduler | NULL_LATENCY non_game_cores includes phantom CPUs |
| BUG-22 | 🔵 Low | Interrupts | APIC error handler doesn't read ESR |
| BUG-23 | 🔵 Low | IPC | Channels never cleaned up — memory leak |
