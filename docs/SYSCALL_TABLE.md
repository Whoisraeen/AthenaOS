# AthenaOS syscall table

**Authoritative source.** Required by `kernelchecklist.md` R10: every new
syscall MUST add a row here in the same commit that adds the dispatch arm.

Calling convention: `syscall` instruction with arguments in
`rdi, rsi, rdx, r10, r8, r9` (SysV-ish; `r10` replaces `rcx` because
`syscall` clobbers `rcx`). Syscall number in `rax`. Return value in `rax`.
A return of `u64::MAX` (or specific high-bit-set codes documented per
syscall) means failure.

## Block 1: Foundational (1–14)

| nr | name | rdi | rsi | rdx | r10 | rax (return) | cap required |
|---:|---|---|---|---|---|---|---|
| 1  | `SYS_PRINT`        | value         | — | — | — | 0                  | — |
| 2  | `SYS_SEND`         | cap_handle    | msg_type | arg1 | arg2 | 0/err | `Cap::Channel WRITE` |
| 3  | `SYS_RECV`         | cap_handle    | — | — | — | (status, type, args)| `Cap::Channel READ` |
| 4  | `SYS_CAP_GRANT`    | target        | src_handle | new_rights | derive_a | new_handle | `GRANT` on src |
| 5  | `SYS_CAP_REVOKE`   | target        | target_handle | — | — | 0/err | granter == revoker |
| 6  | `SYS_CAP_QUERY`    | handle        | — | — | — | (status, flavor, rights) | — |
| 7  | `SYS_MMIO_MAP`     | cap_handle    | user_virt | length | — | mapped_va | `Cap::Mmio MAP` |
| 8  | `SYS_IRQ_WAIT`     | cap_handle    | — | — | — | irq vector | `Cap::Irq WAIT` |
| 9  | `SYS_PORT_READ`    | cap_handle    | port | width | — | value | `Cap::Port READ` |
| 10 | `SYS_PORT_WRITE`   | cap_handle    | port | value | width | 0 | `Cap::Port WRITE` |
| 11 | `SYS_SPAWN`        | path_ptr      | path_len | — | — | child pid | (root by default; cap-gated later) |
| 12 | `SYS_EXIT`         | code          | — | — | — | (noreturn) | — |
| 13 | `SYS_WAIT`         | pid           | — | — | — | exit code | `Cap::Process WAIT` |
| 14 | `SYS_KILL`         | pid           | signal | — | — | 0/err | `Cap::Process WRITE` |

## Block 2: File I/O (15–23)

| nr | name | args | rax |
|---:|---|---|---|
| 15 | `SYS_OPEN`  | path_ptr, path_len, flags | fd or u64::MAX |
| 16 | `SYS_READ`  | fd, buf_ptr, buf_len | bytes read |
| 17 | `SYS_WRITE` | fd, buf_ptr, buf_len | bytes written |
| 18 | `SYS_CLOSE` | fd | 0/err |
| 19 | `SYS_MMAP`  | addr, len | mapped_va |
| 20 | `SYS_MUNMAP`| addr, len | 0/err |
| 21 | `SYS_SETPRIORITY` | pid, priority | 0/err |
| 22 | `SYS_SEEK`  | fd, offset, whence | new offset |
| 23 | `SYS_STAT`  | fd, out_ptr | bytes written |

## Block 3: Compositor (24–27)

| nr | name | args | rax |
|---:|---|---|---|
| 24 | `SYS_SURFACE_CREATE`  | width, height, user_virt | surface_id |
| 25 | `SYS_SURFACE_PRESENT` | id, x, y | 0/err |
| 26 | `SYS_SURFACE_FOCUS`   | id | 0/err |
| 27 | `SYS_SURFACE_CLOSE`   | id | 0/err |

## Block 4: Misc (28–35)

| nr | name | args | rax |
|---:|---|---|---|
| 28 | `SYS_YIELD`       | — | (no return value) |
| 29 | `SYS_GETPID`      | — | tid |
| 30 | `SYS_TIME`        | — | monotonic ns |
| 31 | `SYS_READ_KEY`    | — | scancode or 0 |
| 32 | `SYS_POLL_MOUSE`  | — | packed packet or 0 |
| 33 | `SYS_READDIR`     | buf_ptr, buf_len | entries written |
| 34 | `SYS_SCREEN_INFO` | — (rdi/rsi out) | 0/err |
| 119 | `SYS_CHANNEL_SHMEM_MAP` | channel_cap, target_virt (4 KiB aligned) | 0 / `u64::MAX` |
| 121 | `SYS_NET_SOCKET` | proto (0=TCP, 1=UDP) | fd / `u64::MAX` |
| 122 | `SYS_NET_CONNECT` | fd, ip (packed BE u32), port | 0 / `u64::MAX` |
| 123 | `SYS_NET_SEND` | fd, buf, len | bytes sent / `u64::MAX` |
| 124 | `SYS_NET_RECV` | fd, buf, cap | bytes (0=none) / `u64::MAX` |
| 125 | `SYS_NET_CLOSE` | fd | 0 / `u64::MAX` |
| 126 | `SYS_SET_FS_BASE` | virt_addr | 0/err |
| 258 | `SYS_FUTEX` | uaddr, op (0=wait,1=wake), val | WAIT: 0 woken / 1 EAGAIN / 2 fault · WAKE: count woken |
| 264 | `SYS_NET_DNS` | name ptr, name len | IPv4 packed BE u32 / `u64::MAX` |
| 265 | `SYS_NET_STATUS` | fd | flags: CONNECTED(1) READABLE(2) SENDABLE(4) CLOSED(8) / `u64::MAX` |
| 266 | `SYS_THEME_GET` | out_ptr (`ThemeInfo`), out_cap | bytes written (32) / `u64::MAX` |
| 267 | `SYS_AUDIO_SUBMIT` | samples_ptr (`*const i16`), frame_count, format_flags (0) | frames accepted / `u64::MAX` |

## Block 5: Gaming surface (40–49) — Concept §Gaming-First

| nr | name | args | rax |
|---:|---|---|---|
| 40 | `SYS_WALL_CLOCK`         | — | unix nanoseconds |
| 41 | `SYS_GAME_MODE_ENTER`    | — | 0/err |
| 42 | `SYS_GAME_MODE_EXIT`     | — | 0 |
| 43 | `SYS_GAME_MODE_STATUS`   | — | active+ratio bits |
| 44 | `SYS_NULL_LATENCY_ENTER` | tid (0=self) | 0/err |
| 45 | `SYS_NULL_LATENCY_EXIT`  | — | 0 |
| 46 | `SYS_PIN_MEMORY`         | virt_addr, byte_len | 0/E_PIN_* |
| 47 | `SYS_UNPIN_MEMORY`       | virt_addr, byte_len | 0/E_PIN_* |
| 48 | `SYS_DEADLINE_STATS`     | buf_ptr (32 B), buf_len | bytes written |
| 49 | `SYS_PERF_TSC`           | — | raw TSC |

## Block 6: Config registry (50–53) — Concept §Windows pain points

| nr | name | args | rax |
|---:|---|---|---|
| 50 | `SYS_CONFIG_GET`      | key_ptr, key_len, out_ptr, out_cap | bytes (would-be) |
| 51 | `SYS_CONFIG_SET`      | key_ptr, key_len, val_ptr, val_len | new generation |
| 52 | `SYS_CONFIG_SNAPSHOT` | — | snapshot_id |
| 53 | `SYS_CONFIG_ROLLBACK` | snapshot_id | 0/err |

## Block 7: Local search (54–57) — Concept §Windows pain points

| nr | name | args | rax |
|---:|---|---|---|
| 54 | `SYS_SEARCH_ADD`    | display_ptr, display_len, kind | item_id |
| 55 | `SYS_SEARCH_REMOVE` | item_id | 0/err |
| 56 | `SYS_SEARCH_QUERY`  | q_ptr, q_len, out_ptr, out_cap_bytes | result count |
| 57 | `SYS_SEARCH_STATS`  | out_ptr (32 B), out_cap | bytes |

`SYS_SEARCH_QUERY` (56) returns only opaque `(id, kind)` 16-byte records. For
NAMED, clickable rows (name + path) the Files app / command palette use
`SYS_SEARCH_QUERY_RESOLVED` (281) — see **Block 32** below.

## Block 8: Per-game profile (58–61) — Concept §Gaming Features

| nr | name | args | rax |
|---:|---|---|---|
| 58 | `SYS_GAME_PROFILE_SET`   | id_ptr, id_len, profile_ptr | 0/err |
| 59 | `SYS_GAME_PROFILE_GET`   | id_ptr, id_len, out_ptr | bytes/err |
| 60 | `SYS_GAME_PROFILE_APPLY` | id_ptr, id_len | 0/err |
| 61 | `SYS_GAME_PROFILE_LIST`  | out_ptr, out_cap | count |

## Block 9: Unified RGB (62–65) — Concept §Customization Engine

| nr | name | args | rax |
|---:|---|---|---|
| 62 | `SYS_RGB_LIST`   | out_ptr, out_cap_bytes | device count |
| 63 | `SYS_RGB_QUERY`  | device_id, out_ptr, out_cap | bytes/err |
| 64 | `SYS_RGB_SET`    | device_id, zone (or u32::MAX), color, brightness | 0/err |
| 65 | `SYS_RGB_EFFECT` | device_id, effect_id, speed, color | 0/err |

## Block 10: App bundle verifier (66–67) — Concept §Windows pain points

| nr | name | args | rax |
|---:|---|---|---|
| 66 | `SYS_BUNDLE_VERIFY`   | manifest_ptr, manifest_len | packed (ok\|bad<<16\|err<<32) |
| 67 | `SYS_BUNDLE_REGISTER` | name_ptr, name_len, version, sha_ptr | 0/err |

## Block 11: Compositor capture (68–70) — Concept §Gaming Features

| nr | name | args | rax |
|---:|---|---|---|
| 68 | `SYS_CAPTURE_BEGIN` | rxry packed, rwrh packed, format, continuous | session_id |
| 69 | `SYS_CAPTURE_END`   | session_id | 0 |
| 70 | `SYS_CAPTURE_READ`  | session_id, out_ptr, out_cap_bytes | bytes |

## Block 12: Permission prompts (71–73) — Concept §Security

| nr | name | args | rax |
|---:|---|---|---|
| 71 | `SYS_PERM_LIST`    | out_ptr, out_cap | pending count |
| 72 | `SYS_PERM_RESPOND` | request_id, approved(0/1) | 0 |
| 73 | `SYS_PERM_STATS`   | out_ptr (16 B), out_cap | bytes |

## Block 13: Theme engine (74–77) — Concept §Customization Engine

| nr | name | args | rax |
|---:|---|---|---|
| 74 | `SYS_THEME_LIST`     | out_ptr, out_cap | count |
| 75 | `SYS_THEME_QUERY`    | theme_id, out_ptr, out_cap | bytes/err |
| 76 | `SYS_THEME_APPLY`    | theme_id | 0/err |
| 77 | `SYS_THEME_REGISTER` | bundle_ptr, bundle_len | new_id/err |

## Block 14: Rae scripting (78–80, 294–295) — Concept §Customization Engine

| nr | name | args | rax |
|---:|---|---|---|
| 78 | `SYS_SCRIPT_RUN`    | src_ptr, src_len, cap_mask | script_id |
| 79 | `SYS_SCRIPT_STATUS` | script_id, out_ptr, out_cap | bytes/err |
| 80 | `SYS_SCRIPT_KILL`   | script_id | 0/err |
| 294 | `SYS_SCRIPT_FETCH` | out_ptr, out_cap | bytes written (`ScriptJobAbi` header + source) / 0 none queued / `ERR_*` |
| 295 | `SYS_SCRIPT_COMPLETE` | script_id, exit_code (two's-complement i64; negative = Failed), out_ptr, r10=out_len | 0 / `ERR_*` |

`cap_mask` bits (deny-by-default; `scripting.rs` `SCRIPT_CAP_*`): 1 SYSINFO ·
2 NOTIFY · 4 THEME · 8 CONFIG · 16 WALLPAPER · 32 LAUNCH. 294/295 are the
`raelangd` daemon half: FETCH claims the next queued >64 KiB script (Queued →
Running, source handed over), COMPLETE reports exit + captured output.

## Block 15: WireGuard (81–84) — Concept §AthNet

| nr | name | args | rax |
|---:|---|---|---|
| 81 | `SYS_WG_LIST`   | out_ptr, out_cap | tunnel count |
| 82 | `SYS_WG_ADD`    | name_ptr, name_len, endpoint, key_ptr | tunnel_id |
| 83 | `SYS_WG_REMOVE` | tunnel_id | 0/err |
| 84 | `SYS_WG_STATS`  | tunnel_id, out_ptr, out_cap | bytes/err |

## Block 16: Live wallpaper (85–87) — Concept §Customization Engine

| nr | name | args | rax |
|---:|---|---|---|
| 85 | `SYS_WALLPAPER_LIST`   | out_ptr, out_cap | count |
| 86 | `SYS_WALLPAPER_SET`    | wallpaper_id | 0/err |
| 87 | `SYS_WALLPAPER_STATUS` | out_ptr, out_cap | bytes |

## Block 17: Session + diagnostics (88–95)

| nr | name | args | rax |
|---:|---|---|---|
| 88 | `SYS_SESSION_LOGIN`   | user_ptr, user_len, pass_ptr, pass_len | 0 ok / 1 fail |
| 89 | `SYS_SESSION_GUEST`   | — | 0/err |
| 90 | `SYS_SESSION_LOCK`    | — | 0 |
| 91 | `SYS_SESSION_UNLOCK`  | pass_ptr, pass_len | 0 ok / 1 fail |
| 92 | `SYS_SESSION_INFO`    | buf_ptr, buf_len | bytes |
| 93 | `SYS_SESSION_LOGOUT`  | — | 0 |
| 94 | `SYS_PROCLIST`        | buf_ptr, buf_len | count |
| 95 | `SYS_READDIR_AT`      | path_ptr, path_len, buf_ptr, buf_len | count |

## Block 18: VFS mutations (96–98) — Concept §File Explorer / M-B

| nr | name | args | rax |
|---:|---|---|---|
| 96 | `SYS_MKDIR`  | path_ptr, path_len (1..4096), mode | 0 or E_VFS_* |
| 97 | `SYS_UNLINK` | path_ptr, path_len (1..4096) | 0 or E_VFS_* |
| 98 | `SYS_RENAME` | old_ptr, old_len (1..4096), new_ptr, new_len (1..4096) | 0 or E_VFS_* |

VFS errors: `0xFFFF_FFFF_FFFF_FD01`..`FD05` in `vfs.rs`.

Capability behavior: if caller holds any `Cap::Filesystem`, at least one must include
`WRITE` or syscall fails with `E_RIGHTS`; callers with no filesystem caps retain
legacy permissive behavior for compatibility during migration.

## Block 19: Desktop integration (107–108)

| nr | name | args | rax |
|---:|---|---|---|
| 107 | `SYS_CLIPBOARD_GET` | buf_ptr, buf_len (<=64 KiB) | bytes copied |
| 108 | `SYS_CLIPBOARD_SET` | buf_ptr, buf_len (max 64 KiB) | 0 / u64::MAX |

Both clipboard syscalls reject user buffers over 64 KiB before allocation/copy.
`SYS_CLIPBOARD_SET` (108) now ALSO appends the copy to the session-wide history
ring (Block 27) — an additive behavior change, NOT an ABI break: its signature
and return convention are unchanged. The active buffer GET/SET pair is
untouched.

## Block 20: Userspace driver host ABI (109–118)

| nr | name | args | rax |
|---:|---|---|---|
| 109 | `SYS_DRIVER_REGISTER`       | name_ptr, name_len, class | driver_handle / `ERR_*` |
| 110 | `SYS_DRIVER_UNREGISTER`     | driver_handle | 0 / `ERR_*` |
| 111 | `SYS_DRIVER_CLAIM_DEVICE`   | driver_handle, packed_bdf | claim_handle / `ERR_*` |
| 112 | `SYS_DRIVER_RELEASE_DEVICE` | claim_handle | 0 / `ERR_*` |
| 113 | `SYS_DRIVER_ENABLE_DMA`     | driver_handle, flags | 0 / `ERR_*` |
| 114 | `SYS_DRIVER_LIST`           | out_ptr, out_cap | count |
| 115 | `SYS_DRIVER_QUERY`          | driver_handle, out_ptr, out_cap | bytes / err |
| 116 | `SYS_DRIVER_IRQ_SETUP`      | driver_handle, vector, channel_cap | 0 / `ERR_*` |
| 117 | `SYS_DRIVER_DMA_MAP`        | claim_handle, user_ptr, len | dma_token / `ERR_*` |
| 118 | `SYS_DRIVER_DMA_UNMAP`      | claim_handle, dma_token | 0 / `ERR_*` |

## Dedicated Linux exec surface

| nr | name | args | rax |
|---:|---|---|---|
| 5000 | `SYS_LINUX_EXEC` | path_cstr, applet_cstr_or_null | tid / `-errno` |

Capability behavior: if caller holds any `Cap::Process`, at least one must include
`EXEC` or syscall fails with `E_RIGHTS`; callers with no process caps retain
legacy behavior for compatibility.

## Block 21: Anti-cheat attestation (284–290) — Concept §Security

**RENUMBERED 2026-06-25 from 100–106 (ABI_VERSION 3 → 4).** The original 100–106
range was a hard collision: the dispatch arms for `SYS_OOM_SUBSCRIBE` (100, Block
22a) and the AthFS snapshots (101–103, Block 22b) precede the anti-cheat range arm
in `kernel/src/syscall.rs`, and Rust's match is first-arm-wins — so calling the
*documented* `SYS_AC_REGISTER_GAME` (102) actually ran `raefs::snapshot_restore`,
a destructive filesystem rollback. OOM + AthFS snapshots are live and iron-proven,
so anti-cheat (design-tier) moved to the fresh contiguous block below. The old
anti-cheat numbers never reached `anticheat.rs`, so no working consumer broke.
Canonical defs now live in `rae_abi` Block 34 (with a build-time collision guard).

| nr | name | args | rax |
|---:|---|---|---|
| 284 | `SYS_AC_REQUEST_ATTESTATION` | game_pid, vendor_id, timestamp | session_id |
| 285 | `SYS_AC_VERIFY_ATTESTATION`  | session_id, … | AC_OK or AC_ERR_* |
| 286 | `SYS_AC_REGISTER_GAME`       | pid, … | AC_OK / err |
| 287 | `SYS_AC_UNREGISTER_GAME`     | pid | AC_OK / err |
| 288 | `SYS_AC_REPORT_VIOLATION`    | session_id, code, payload | AC_OK / err |
| 289 | `SYS_AC_QUERY_STATUS`        | session_id | status word |
| 290 | `SYS_AC_HEARTBEAT`           | session_id, timestamp | AC_OK / err |

## Block 35: Surface resize protocol (291–292)

True tiling: the WM records a desired cell size; a tiling-aware client polls it and reallocates to fill the cell. Ungated, all-sandbox (a task resizes only its own window).

| nr | name | args | rax |
|---:|---|---|---|
| 291 | `SYS_SURFACE_RESIZE_REQ` | rdi=id | `w \| (h<<16)` if pending, `SURFACE_RESIZE_NONE`(0) if none, `SURFACE_RESIZE_ERR`(u64::MAX) on bad id |
| 292 | `SYS_SURFACE_RESIZE` | rdi=id, rsi=w, rdx=h, r10=new_buf (page-aligned) | 0 on success, `SURFACE_RESIZE_ERR` on bad id / non-owner / bad dims / alloc fail |

## Block 36: AthBridge per-process launcher argv (293) — RESERVED

`SYS_SPAWN_ARGS` (the launcher's argv/target-passing primitive, gate item #2 —
one `.exe`/process) is **reserved here as 293** so the number cannot collide when
the slice lands; the `rae_abi` constant + dispatch arm + the native-argv kernel
spawn impl land together as ONE `[interface]` + spawn slice **GATED on
`scheduler.rs`/`task.rs` cooling** (see `docs/components/raebridge-process-model.md`
§2 "Sequencing gate (HOT FILE)"). NOTE: the process-model doc originally
recommended 284, but 284–290 were taken by anti-cheat Block 34 and 291–292 by
Block 35 since that draft — corrected to 293 on 2026-06-29 (the §10-pitfall-#1
collision class: "19 and 27 already bit us").

| nr | name | args | rax |
|---:|---|---|---|
| 293 | `SYS_SPAWN_ARGS` *(RESERVED — impl gated)* | rdi=path_ptr, rsi=path_len, rdx=argv_ptr, r10=argv_count, r8=pty_id | child pid / err |

Packed argv blob: `argv_count` × `[u32 len][bytes]` (no NUL, length-prefixed);
bounds `SPAWN_ARGS_MAX_BYTES=65536`, `SPAWN_ARGS_MAX_COUNT=64`. Native child stack
convention (AthenaOS-native, NOT Linux auxv): `argc@[rsp]`, `argv@[rsp+8]`,
NULL-terminated `char**`. Next free after this reservation: 294.

## Block 37: netlog diagnostic (296)

| nr | name | args | rax |
|---:|---|---|---|
| 296 | `SYS_NETLOG_FLUSH` | *(none)* | chunks broadcast |

Broadcasts the kernel bootlog ring over UDP **right now** (`netlog::broadcast_ring`),
so a marker survives a subsequent hard hang that would take down the end-of-boot
flush + BOOTLOG persist (both live on CPU 0). Safe-mode safe (UDP TX, not a
sector write). Additive diagnostic slot — **no `ABI_VERSION` bump**. `amdgpud`
fences each real-`amdgpu_device_init` phase with this (sentinels 9000/9001/9002/9003)
so the netlog trail ends at the exact stage CPU 0 freezes on. Slots 297–299 are
assigned to the DRM render-service broker below.

## Block 38: DRM render-service broker (297–299)

The kernel owns `/dev/dri/renderD128`, validates Linux client pointers, and
copies bounded ioctl records. The retained upstream amdgpu object graph remains
inside the capability-confined `amdgpud`; registration succeeds only when the
caller owns the supplied live AMD LinuxKPI device handle. Nested-pointer ioctls
require explicit command marshalling and otherwise fail closed.

| nr | name | args | rax |
|---:|---|---|---|
| 297 | `SYS_DRM_SERVICE_REGISTER` | rdi=LinuxKPI device handle | 0 / `DRM_SERVICE_ERR_*` |
| 298 | `SYS_DRM_SERVICE_FETCH` | rdi=`RequestHeader*`, rsi=payload, rdx=capacity | payload bytes + 1; 0 when idle; error sentinel |
| 299 | `SYS_DRM_SERVICE_COMPLETE` | rdi=request id, rsi=signed i32 status bits, rdx=payload, r10=len | 0 / error sentinel |

Wire version 1 uses a 40-byte `rae_abi::drm_service::RequestHeader`; payloads
are capped at 64 KiB. Next free: 300.

## Block 22: AthFS game extents (99)

| nr | name | args | rax |
|---:|---|---|---|
| 99 | `SYS_RAEFS_GAME_INSTALL_HINT` | path_ptr, path_len (1..4096), expected_size (0 = default 32 KiB) | `start_block \| (block_count << 32)` or `E_RAEFS_*` |

AthFS errors: `E_RAEFS_NO_MOUNT` = `0xFFFF_FFFF_FFFF_F901`, `E_RAEFS_EXTENT_FAIL` = `F902`, `E_RAEFS_BAD_PATH` = `F903`.

## Block 22a: Memory pressure (100) — Concept §Production hardening

| nr | name | args | rax |
|---:|---|---|---|
| 100 | `SYS_OOM_SUBSCRIBE` | chan_id (IPC channel the app `recv`s on) | 0 |

The calling task is registered as a low-memory subscriber. On heap pressure
(`oom::handle_alloc_failure`), the kernel pushes an `OOM_MSG_LOW_MEMORY`
(`msg_type = 0x4F4F4D`, `arg1` = pressure level 1/2) to each subscriber's
channel and wakes its `recv`, then runs a compaction pass — apps drop caches
before any OOM-kill. Additive allocation (unreserved slot 100; no `ABI_VERSION`
bump).

Requires `Cap::Filesystem` with `WRITE` when the task holds any filesystem caps (same rule as `SYS_MKDIR`).

## Block 22b: AthFS snapshots (101–103) — Concept §AthFS CoW/snapshots

| nr | name | args | rax |
|---:|---|---|---|
| 101 | `SYS_RAEFS_SNAPSHOT_CREATE` | name_ptr, name_len (0..4096) | new snapshot id (>0) or `E_RAEFS_*` |
| 102 | `SYS_RAEFS_SNAPSHOT_RESTORE` | snap_id | 0 or `E_RAEFS_*` |
| 103 | `SYS_RAEFS_SNAPSHOT_DELETE` | snap_id | 0 or `E_RAEFS_*` |

Kernel side: `kernel/src/raefs.rs` (`snapshot_create`/`snapshot_restore`/
`snapshot_delete`). `CREATE` freezes the current FS state with a CoW refcount
bump and records the user-supplied label in `SNAPSHOT_NAMES` (id → name);
`RESTORE` atomically swaps the live bitmap + root inode back to the snapshot;
`DELETE` drops a snapshot and reclaims its block references. All three require
`Cap::Filesystem` with `WRITE` (same rule as `SYS_MKDIR`) and refuse in
safe-mode (they write FS metadata). Names + ids surface at
`/proc/raeen/raefs` (`id=.. ts=.. name=..`). Additive allocation (unreserved
slots 101-103 in the gap between `SYS_OOM_SUBSCRIBE` and the driver range; no
`ABI_VERSION` bump).

## Block 23: LinuxKPI host (127–140)

Userspace driver daemons link `components/raeen_linuxkpi` and call these from C ABI
stubs. Phase 1 (127-131): `kmalloc`, `get_jiffies_64`, `msleep`, `raeen_printk`.
Phase 2-4 (130, 132-140): the hardware bridge — `ioremap`, PCI config, zero-copy
`dma_alloc_coherent` (IOMMU-sandboxed), `request_irq` doorbells, daemon supervisor.
See `docs/LINUXKPI_PHASE1.md` and `docs/LINUXKPI_PHASE2.md`.

| nr | name | args (rdi, rsi, rdx) | rax |
|---:|---|---|---|
| 127 | `SYS_LINUXKPI_VERSION` | — | `0x524B5049_0001` ("RKPI" + v1) |
| 128 | `SYS_LINUXKPI_JIFFIES` | — | `timers::JIFFIES` |
| 129 | `SYS_LINUXKPI_MSLEEP` | ms | 0 |
| 130 | `SYS_LINUXKPI_IOREMAP` | dev_handle, bar_index | virt ptr / `E_*` |
| 131 | `SYS_LINUXKPI_PRINTK` | buf_ptr, len | 0 or `u64::MAX` |
| 132 | `SYS_LINUXKPI_PCI_ENABLE` | packed_bdf, or `LINUXKPI_PCI_MATCH` (bit 63) + class<<16 + vendor (0 = any) | dev_handle / `E_NO_DEVICE` |
| 133 | `SYS_LINUXKPI_PCI_READ_CFG` | dev_handle, offset | value / `E_*` |
| 134 | `SYS_LINUXKPI_PCI_WRITE_CFG` | dev_handle, offset, value | 0 / `E_*` |
| 135 | `SYS_LINUXKPI_DMA_ALLOC` | dev_handle, size, out_ptr | 0 / `E_*` (writes `[virt,phys,size,token]`) |
| 136 | `SYS_LINUXKPI_DMA_FREE` | dev_handle, token | 0 / `E_NO_DMA` |
| 137 | `SYS_LINUXKPI_REQUEST_IRQ` | dev_handle, vector | irq_handle / `E_*` |
| 138 | `SYS_LINUXKPI_IRQ_WAIT` | irq_handle | vector fired / `E_*` |
| 139 | `SYS_LINUXKPI_IOUNMAP` | virt, len | 0 |
| 140 | `SYS_LINUXKPI_SUPERVISOR` | op (1=reg,2=heartbeat,3=count), dev_handle | per-op |
| 142 | `SYS_LINUXKPI_REQUEST_FIRMWARE` | name_ptr, name_len, out_ptr | 0 / `E_*` (writes `[user_virt,size]`) |
| 143 | `SYS_RAEGFX_REGISTER_SCANOUT` | dev_handle, phys, (width<<32\|height), stride | 1 attached / 0 reject |
| 144 | `SYS_LINUXKPI_MAP_PHYS` | dev_handle, phys, size | user virt / `E_*` |

`SYS_LINUXKPI_MAP_PHYS` (144) maps a NON-BAR reserved/carveout physical range
(APU/UMA VRAM — where the GART table + CPU-visible kernel BOs live, beyond the
small BAR0 aperture) into the owning daemon. Two gates: the caller must own
`dev_handle`, and EVERY page in `[phys, phys+size)` must be firmware-reserved
(`memory::phys_is_usable_ram` false) — usable RAM is refused, so a driver can
never map kernel or another process's memory. Additive (no `ABI_VERSION` bump).

`SYS_RAEGFX_REGISTER_SCANOUT` (143) is the first slot claimed from the AthGFX
reserved range (143–199; additive, no `ABI_VERSION` bump). A GPU driver daemon
(amdgpud, via `raeen_drm::kms::atomic_commit`) hands its display scanout
framebuffer to the in-kernel compositor, which then presents THROUGH the
device's display engine (the amdgpu DCN scans the same physical pages the
compositor blits into). SECURITY: `phys` must be a DMA region the caller already
owns on `dev_handle` and large enough for `height * stride`, so a daemon can
expose only its own buffer, never arbitrary physical memory.

LinuxKPI error sentinels live in `0xFFFF_FFFF_FFFF_FCxx`: `E_NO_DEVICE`=FC01,
`E_NOT_OWNER`=FC02, `E_BAD_BAR`=FC03, `E_NO_DMA`=FC04, `E_NO_IRQ`=FC05,
`E_DENIED`=FC06, `E_BAD_ARG`=FC07, `E_NO_FIRMWARE`=FC08.

`SYS_LINUXKPI_REQUEST_FIRMWARE` (142) is the `request_firmware()` host call —
loads a named blob from the initramfs `firmware/<name>` tree and maps it into
the daemon's address space (`[user_virt, size]` written to `out_ptr`). Every
Linux GPU/Wi-Fi driver (amdgpu, i915, iwlwifi) needs it before hardware
bring-up. Additive allocation (no `ABI_VERSION` bump): carved from the unused
low end of the AthGFX reserved range, which now starts at 143.

## Block 23a: Debug print (141)

Standalone debug-print to the kernel serial port. relibc's printf, the
hello_relibc smoketest, and any kernel-side userspace stub use this. Was
syscall 27 in `ABI_VERSION = 1`, which collided with `SYS_SURFACE_CLOSE`
in Block 3 — the compositor close-surface arm was unreachable because
Rust match dispatches on first-arm-wins. Moved to 141 in `ABI_VERSION = 2`
(see `components/rae_abi/src/lib.rs::syscall::SYS_DEBUG_PRINT`); relibc's
`raeenOS_syscall::SYS_DEBUG_PRINT` was updated in the same commit. AthGFX's
reserved range was slid to 142–199 to accommodate, then to 143–199 when
`SYS_LINUXKPI_REQUEST_FIRMWARE` took 142 (additive, see Block 23).

| nr | name | args (rdi, rsi) | rax |
|---:|---|---|---|
| 141 | `SYS_DEBUG_PRINT` | buf_ptr, len (≤ 4096) | bytes written / `u64::MAX` |

No capability required (debug aid; same as `SYS_PRINT` policy). Output is
mirrored to the framebuffer console via the `SERIAL1 → CONSOLE` lock
ordering — do NOT call from inside an interrupt handler that already holds
either lock.

## Block 24: Installer (256–257)

The userspace `raeinstaller` calls these to install AthenaOS onto the target disk.
Both require `Cap::System{WRITE}` (seeded only when the kernel boots in installer
mode). See `kernel/src/installer.rs`, `docs` Phase 3.

| nr | name | args | rax |
|---:|---|---|---|
| 256 | `SYS_INSTALL_RUN` | — | stage bitmask (5 bits = full install) / `u64::MAX` denied |
| 257 | `SYS_INSTALL_CREATE_ACCOUNT` | user_ptr, user_len, pass_ptr, pass_len, disp_ptr, disp_len | new user id / `u64::MAX` |

Install stage bits: GPT=1, ESP_FORMAT=2, BOOT_TREE=4, RAEFS_FORMAT=8, VERIFY=16.

## Block 25: Live theme (266) — Concept §Customization Engine

| nr | name | args | rax |
|---:|---|---|---|
| 266 | `SYS_THEME_GET` | out_ptr (`ThemeInfo`), out_cap (bytes) | bytes written (32) / `u64::MAX` |

`SYS_THEME_GET` writes a `rae_abi::ThemeInfo` (`#[repr(C)]`, 32 bytes: `version,
accent_argb, bg_argb, fg_argb, is_dark, blur_radius, palette_id, reserved`) to
the user buffer via `copy_to_user` (validated with `validate_user_range`, no raw
deref). `accent_argb` is `theme_engine::active_accent()` — the SAME live seed the
in-kernel surfaces read — so the 6 bundled apps (separate ELF processes that
cannot call the kernel fn) re-skin to match Vibe Mode. Read-only, **no
capability** and allowed in safe mode (theme colours carry no secret; every app
already renders against the active accent). Apps call it once at launch and fall
back to `THEME_DEFAULT_ACCENT` (RaeBlue, `0xFF4E9CFF`) on any error. Additive
allocation (fresh slot 266 in the experimental range, next free after 264
`SYS_NET_DNS` / 265 `SYS_NET_STATUS`); no `ABI_VERSION` bump.

## Block 26: Audio submit (267) — Concept §AthAudio (sub-3ms audio)

| nr | name | args | rax |
|---:|---|---|---|
| 267 | `SYS_AUDIO_SUBMIT` | samples_ptr (`*const i16`), frame_count, format_flags | frames accepted / `u64::MAX` |

`SYS_AUDIO_SUBMIT` feeds PCM samples into the AthAudio mixer, completing the
audio pillar end-to-end (app → mixer → ring → HDA). The kernel validates the
buffer with `validate_user_range` and `copy_from_user`s it (no raw deref), then
enqueues it into the calling task's per-PID `SourceKind::Pcm` mixer voice —
`AudioMixer::mix()` drains that queue every period into `AUDIO_RING` and onward
to the HDA DMA buffer, the same production path the boot test-tone exercises.

**Fixed format (v1):** interleaved 48 kHz **i16 stereo** — each frame is 2
samples (L, R) = 4 bytes, so the read length is `frame_count * 4`.
`format_flags` is RESERVED and must be `0` (a future value selects mono / a
different rate without moving this slot); a non-zero value is rejected.

Returns the number of frames accepted (`<= frame_count`; fewer when the
per-source queue is near full — the app re-submits the remainder next period),
or `u64::MAX` if audio isn't initialised, `format_flags` is unsupported,
`frame_count` exceeds `AUDIO_SUBMIT_MAX_FRAMES` (512 frames ≈ 4 DMA periods), or
the user buffer is unmapped/too small. **No capability gate** (audio output
carries no secret and every app may make sound) and allowed in safe mode.
Additive allocation (fresh slot 267 in the experimental range, next free after
266 `SYS_THEME_GET`); no `ABI_VERSION` bump. The kernel side is
`kernel/src/audio.rs::submit_samples`; the raekit wrapper is
`raekit::sys::audio_submit`.

## Block 27: Clipboard history (268–273) — Concept §"The user owns the machine"

Win+V-class clipboard history + pin over the session clipboard. RAM-only and
**local by default** — no cloud sync, no telemetry, the Concept's ownership
posture. The active buffer + GET/SET (107/108) are UNCHANGED; SET now also
appends to a bounded history ring (`CLIP_HIST_MAX_ENTRIES` = 64) with
pinned-safe eviction (oldest UNPINNED dropped first; pinned never evicted).
Kernel side: `kernel/src/clipboard.rs`. raekit wrapper: `raekit::sys::clip_*`.

| nr | name | args (rdi, rsi, rdx) | rax |
|---:|---|---|---|
| 268 | `SYS_CLIP_HIST_COUNT`   | — | `count \| (pinned_count << 32)` |
| 269 | `SYS_CLIP_HIST_GET`     | index, out_ptr, out_cap | bytes written / `CLIP_ERR` |
| 270 | `SYS_CLIP_HIST_PIN`     | index, pin (1) / unpin (0) | 0 / `CLIP_ERR` |
| 271 | `SYS_CLIP_HIST_DELETE`  | index | 0 / `CLIP_ERR` (refuses pinned) |
| 272 | `SYS_CLIP_HIST_CLEAR`   | — | entries removed (keeps pinned) |
| 273 | `SYS_CLIP_HIST_PROMOTE` | index | 0 / `CLIP_ERR` |

`CLIP_ERR` = `u64::MAX`. `SYS_CLIP_HIST_GET` writes a `rae_abi::ClipEntryHeader`
(`#[repr(C)]`, 32 bytes: `version, format, flags, byte_len, sequence,
paste_count, reserved0, reserved1`) immediately followed by `byte_len` bytes of
UTF-8 text payload. `format` is `CLIP_FMT_TEXT` (0) today; `CLIP_FMT_{RICH_TEXT,
IMAGE,FILES,URL,COLOR}` (1–5) are RESERVED tags so richer clips can be added
without an ABI break. `flags` carries `CLIP_FLAG_PINNED` (bit 0).

**No capability gate** (clipboard contents are the user's own, same posture as
GET 107) and **allowed in safe mode** (history is RAM, writes no block device).
Index 0 is the newest copy. Additive allocation (fresh experimental block
268–273, next free after 267 `SYS_AUDIO_SUBMIT`); no `ABI_VERSION` bump — SET
(108) is extended, not changed, and 268–273 are new numbers.

## Block 28: Screen capture (274–276) — Concept §creators

Exposes the EXISTING in-kernel compositor capture engine
(`kernel/src/compositor.rs::{start_capture,read_capture,stop_capture}`, which
already read real composited pixels off the front buffer) to userspace —
unblocking the screenshot tool (parity §F) and the Game Bar overlay (parity §N),
which reuse the same path. The legacy ungated `SYS_CAPTURE_BEGIN/END/READ` block
(68–70) is **deprecated** in favour of these properly-gated numbers.

| nr | name | args (rdi, rsi, rdx, r10) | rax |
|---:|---|---|---|
| 274 | `SYS_CAPTURE_START` | region_xy (`x\|y<<32`), region_wh (`w\|h<<32`), format (`CAPTURE_FMT_*`), flags (`CAPTURE_FLAG_*`) | capture_id / `CAPTURE_ERR` |
| 275 | `SYS_CAPTURE_READ`  | capture_id, out_ptr, out_cap_bytes | bytes written (`CaptureHeader` + pixels) / `CAPTURE_ERR` |
| 276 | `SYS_CAPTURE_STOP`  | capture_id | 0 / `CAPTURE_ERR` |

`CAPTURE_ERR` = `u64::MAX`. **All three require `Cap::ScreenCapture`** (flavor 16,
a fresh TAIL variant on the `Cap` enum — screen pixels can carry passwords/PII,
so this is privacy-gated and fails CLOSED, unlike the fail-open FS/Process
migration bridges). `SYS_CAPTURE_START` is **additionally refused in safe mode**
(no screen reads off a safe-image boot).

`SYS_CAPTURE_READ` writes a `rae_abi::CaptureHeader` (`#[repr(C)]`, 16 bytes:
`width, height, format, bytes`) immediately followed by `bytes` of pixel data
(`width*height*4`, ARGB or BGRA per the session format) via **validated
`copy_to_user`** (`validate_user_range(write)` + `copy_to_user`, no raw
user-pointer deref — matches the net-syscall hardening). `format` is
`CAPTURE_FMT_ARGB32` (0) or `CAPTURE_FMT_BGRA32` (1). `flags` carries
`CAPTURE_FLAG_CONTINUOUS` (bit 0 — keep the session live for Game Bar/recording;
clear = single-shot screenshot); the upper bits are RESERVED for future
window/region-follow modes.

Each session is **tagged with the calling task** and auto-reclaimed in
`scheduler::reclaim_task_resources` (next to the socket/audio-voice sweep) so a
crashed capturer can't leak a session. Active sessions surface at
`/proc/raeen/capture` (`active_sessions: N` + one line per session). Additive
allocation (fresh block 274–276, next free after clipboard history 273) and a
fresh tail `Cap` variant — **no `ABI_VERSION` bump** (the `Cap` wire contract is
flavor-tag-serialized via `flavor_id`, never index/bit-packed, so appending a
variant breaks nothing). Kernel side: `kernel/src/syscall.rs` + `compositor.rs`;
raekit wrappers: `raekit::sys::capture_{start,read,stop}`.

## Block 29: Accessibility tree (277-278) — Concept §Security + Phase 19 a11y

The assistive-technology (AT) read/dispatch surface — the seam the screen
reader, magnifier, and keyboard-nav all consume. Exposes the kernel-owned,
window-tier accessibility tree (built from the compositor surface list,
AccessKit-compatible) to a privileged AT client. Kernel side:
`kernel/src/a11y.rs` + `kernel/src/syscall.rs`. raekit wrappers:
`raekit::sys::a11y_{snapshot,action}`.

| nr | name | args (rdi, rsi, rdx) | rax |
|---:|---|---|---|
| 277 | `SYS_A11Y_SNAPSHOT` | out_ptr, out_cap_bytes | bytes written (`A11ySnapshotHeader` + `A11yNode[]`) / `A11Y_ERR` |
| 278 | `SYS_A11Y_ACTION`   | node_id, action (`A11Y_ACTION_*`), arg | 0 / `A11Y_ERR` |

`A11Y_ERR` = `u64::MAX`. **Both require `Cap::Accessibility`** (flavor 17, a
fresh TAIL variant on the `Cap` enum) — SNAPSHOT needs `READ`, ACTION needs
`WRITE`. Like `Cap::ScreenCapture`, this fails **CLOSED**: an AT client reads
other apps' UI structure + labels and can drive their widgets (the analogue of
macOS TCC Accessibility / Windows UIA), so a task must explicitly hold the cap.
Unlike screen capture, SNAPSHOT is **NOT** refused in safe mode (UI structure
carries no pixel data / PII; the cap gate alone is the control).

`SYS_A11Y_SNAPSHOT` writes a `rae_abi::A11ySnapshotHeader` (`#[repr(C)]`, 16
bytes: `version, node_count, focused_id`) immediately followed by `node_count`
`rae_abi::A11yNode` records (`#[repr(C)]`, 96 bytes each: `id, parent, role,
state, x, y, w, h, actions, name_len, name[48]`) via **validated
`copy_to_user`** (`validate_user_range(write)` + `copy_to_user`, no raw deref —
matches the net/capture hardening). The tree is a flat arena: `parent`
references another node's `id` (`0` = the root desktop node). `role` is an
`A11Y_ROLE_*` tag, `state` an `A11Y_STATE_*` bitfield, `actions` an
`A11Y_ACTIONBIT_*` bitfield — all chosen to map 1:1 onto
`raeui::accessibility`'s `AccessibilityRole` / `AccessibilityTraits` /
`AccessibilityAction` so the kernel serializes raeui's live tree with no lossy
remapping.

`SYS_A11Y_ACTION`'s `action` (`rsi`) is a single `A11Y_ACTION_*` selector
(`FOCUS`=0, `ACTIVATE`=1, `SCROLL`=2, `SET_VALUE`=3, `INCREMENT`=4,
`DECREMENT`=5, `DISMISS`=6); `arg` (`rdx`) is action-specific (`0` when unused).
At the window tier FOCUS/ACTIVATE raise+focus the owning surface and DISMISS
closes it; widget-tier actions route through the AthUI provider (implementer's
next slice) and refuse (`A11Y_ERR`) until that lands rather than faking success.

Active state surfaces at `/proc/raeen/a11y` (`nodes: N`, `focused_id`, one line
per node). Additive allocation (fresh block 277-278, next free after screen
capture 276) and a fresh tail `Cap` variant — **no `ABI_VERSION` bump** (the
`Cap` wire contract is flavor-tag-serialized via `flavor_id`, never
index/bit-packed, so appending a variant breaks nothing — identical reasoning
to `Cap::ScreenCapture`).

## Block 30: Absolute cursor position (279) — Concept §"a mouse-first desktop"

The cursor-poll seam for app mouse HIT-TESTING. Every bundled app
(Notes/Clock/Calculator/Files/...) gets relative mouse deltas + per-event button
state from `SYS_POLL_MOUSE` (32), but that is a destructive per-task event queue
with NO way to read the cursor's live ABSOLUTE screen position, so an app cannot
tell *where* a click landed (which button/tab/list-item). This fills that gap.

| nr | name | args | rax |
|---:|---|---|---|
| 279 | `SYS_INPUT_CURSOR` | — | `x \| (y << 16)` (absolute cursor position) |

Returns the compositor's current absolute cursor position packed `x | (y << 16)`
— both coordinates fit `u16` (clamped to the compositor extent on every move).
Bits `[63:32]` are RESERVED (currently `0`) for a future live button bitmask;
apps that need button state today combine this with the existing `SYS_POLL_MOUSE`
event stream. Read from a **lock-free atomic** (`compositor::CURSOR_POS_PACKED`,
mirrored from the authoritative `cursor.x/y` by `move_cursor` and read by
`compositor::cursor_position_fast()`), so the syscall NEVER takes the compositor
lock and never blocks — cheap to poll each frame.

**No capability gate** (cursor position carries no secret, same posture as
reading input via `SYS_READ_KEY`/`SYS_POLL_MOUSE`) and **allowed in every sandbox
level / safe mode**. `CURSOR_ERR` = `u64::MAX` is the documented forward-compat
sentinel; the current accessor always yields a valid position (`0,0` before the
first move), so the live path does not fail. Additive allocation (fresh slot 279,
next free after accessibility 277-278); **no `ABI_VERSION` bump** (a new number
breaks no existing signature). Kernel side: `kernel/src/syscall.rs` (arm 279) +
`kernel/src/compositor.rs` (`cursor_position_fast`). Surfaces at
`/proc/raeen/compositor` (`cursor: x,y`). raekit wrapper:
`raekit::sys::cursor_pos() -> (u32 x, u32 y, u32 buttons)`.

## Block 31: Live surface origin (280)

A mouse-first app converts the absolute cursor from `SYS_INPUT_CURSOR` (279) into
surface-local coordinates for hit-testing by subtracting its window origin. But
the origin it passed to `SYS_SURFACE_PRESENT` (25) goes STALE the moment the
window manager moves the window — Overview / Spaces / tiling all reposition
windows via the compositor's `set_surface_origin`. A hardcoded origin then makes
clicks miss. This poll returns the LIVE origin so hit-testing stays correct.

| nr | name | args | rax |
|---:|---|---|---|
| 280 | `SYS_SURFACE_ORIGIN` | rdi = surface id | `x \| (y << 16)` / `SURFACE_ORIGIN_ERR` |

Returns the surface's current absolute origin packed `x | (y << 16)` (both `u16`,
same packing as `SYS_INPUT_CURSOR`; negative origins from a window dragged partly
off the left/top edge clamp to `0`), or `SURFACE_ORIGIN_ERR` = `u64::MAX` if the
id is unknown or the compositor is not up — an app MUST check the sentinel before
unpacking (`u64::MAX` would otherwise unpack to the off-screen `0xFFFF,0xFFFF`).
Reads the compositor's authoritative `Surface.x/y` under the short compositor
lock (read-only over the surface list — never alters move/render/present); a
cheap once-per-frame poll, never blocks on I/O.

**No capability gate** (a surface's screen position carries no secret, same
posture as the cursor poll) and **allowed in every sandbox level / safe mode**.
Additive allocation (fresh slot 280, next free after cursor 279); **no
`ABI_VERSION` bump** (a new number breaks no existing signature). Kernel side:
`kernel/src/syscall.rs` (arm 280) + `kernel/src/compositor.rs`
(`surface_origin(id) -> Option<(u32,u32)>`). raekit wrapper:
`raekit::sys::surface_origin(sid: u64) -> Option<(u32 x, u32 y)>`. Boot proof:
`compositor::run_surface_origin_smoketest` (`[compositor] surface_origin
reads_a=… tracks_move=… unknown_none=… -> PASS`).

## Block 32: Resolved search query (281) — Concept §"Search is broken"

`SYS_SEARCH_QUERY` (56) returns only opaque `(id, kind)` pairs — a result you
can't name or click. `SYS_SEARCH_QUERY_RESOLVED` serializes the RESOLVED hits
(name + path + kind + folder flag) so the Files app, start menu, and command
palette render NAMED, clickable rows with a real `Open` target. The kernel
already resolves a hit to its display info in one lock-critical section
(`search_index::query_resolved`); this exposes that to a separate-process client.

| nr | name | args | rax |
|---:|---|---|---|
| 281 | `SYS_SEARCH_QUERY_RESOLVED` | rdi=q_ptr, rsi=q_len, rdx=out_ptr, r10=out_cap_bytes | record count |

**Return:** the number of WHOLE records written, in `rax`. `0` on an empty query,
an un-initialised index, or no matches — never an error sentinel (same graceful
empty-result posture as `SYS_SEARCH_QUERY`).

**Wire format — variable-length records.** The kernel writes back-to-back records
into `[out_ptr, out_ptr + out_cap_bytes)`. Each record is a FIXED 24-byte header
(`rae_abi::SearchResolvedHeader`, `#[repr(C)]`, little-endian) immediately
followed by `name_len` bytes of UTF-8 name then `path_len` bytes of UTF-8 path
(no NUL terminators, no inter-record padding). The next record's header begins at
`this_header + 24 + name_len + path_len`.

```text
offset size field      meaning
  0     8   id         stable index item id (LE u64; 0 if the resolver carries none)
  8     4   kind       SEARCH_KIND_* tag (LE u32): 1 App 2 File 3 Setting 4 Contact 5 Document 99 Other
 12     1   is_folder  1 = directory, 0 = file/other
 13     1   reserved0  0 (future flags)
 14     2   reserved1  0 (keeps name_len 2-aligned)
 16     2   name_len   UTF-8 name byte count following the header (LE u16)
 18     2   path_len   UTF-8 path byte count following name (LE u16)
 20     4   reserved2  0 (rounds the header to 24 bytes)
 24    name_len  name  UTF-8 leaf name (the row title)
 24+name_len path_len path  UTF-8 absolute path (the Open target; empty for apps/settings)
```

**Bounds / truncation rules (both encoder and decoder MUST honor):**
- The kernel emits only WHOLE records that fit in `out_cap_bytes`; it stops before
  a record whose `24 + name_len + path_len` would overflow the remaining capacity.
  A partially-written trailing record is **never** produced.
- The count is capped at `SEARCH_RESOLVED_MAX_RESULTS` (64).
- Each `name`/`path` is clamped to `SEARCH_RESOLVED_MAX_STR` (1024) bytes,
  truncated on a UTF-8 char boundary by the kernel encoder.
- A decoder MUST clamp its walk to the returned count AND bounds-check every
  `name_len`/`path_len` against the remaining buffer, stopping (never panicking,
  never over-reading) on a short/garbage record.

**No capability gate** (search names carry no secret beyond what the indexer
already holds; same posture as `SYS_SEARCH_QUERY` 56) and **allowed in every
sandbox level / safe mode**. Additive allocation (fresh slot 281, next free after
surface origin 280); `ABI_VERSION` bumped to **v3** as a courtesy marker for the
new wire record (no existing field or signature moved). Kernel side:
`kernel/src/syscall.rs` (arm 281) + `kernel/src/search_index.rs`
(`serialize_resolved(query, out_cap) -> (Vec<u8>, count)`). raekit:
`raekit::syscalls::search::{query_resolved, decode_resolved, ResolvedHit}`. Boot
proof: `search_index::run_boot_smoketest` (`[search] crawl-query+resolve
smoketest: … wire_ok=… tiny_ok=… -> PASS`). Host KAT: `cargo test -p raekit`
(`syscalls::tests::*` — round-trip + truncated/garbage no-panic).

## Block 33: AthBridge real-MSVC-CRT ABI (282–283) — Concept §Compatibility

The gate from "a hand-built PE runs" to "a real MSVC-compiled `.exe` runs". Every
MSVC-CRT binary reads its Thread Environment Block via `gs:[0x30]` on entry (and
`gs`-relative TEB fields throughout), and the loader must flip relocated `.text`
RW→RX. See `docs/components/raebridge-real-crt-abi.md` for the full design.

| nr | name | args | rax |
|---:|---|---|---|
| 282 | `SYS_SET_GS_BASE` | rdi=base (TEB virt addr) | 0 on success / `u64::MAX` non-canonical |
| 283 | `SYS_MPROTECT` | rdi=addr (page-aligned), rsi=len, rdx=prot | 0 on success / `u64::MAX` on bad range/unmapped/W^X |

**`SYS_SET_GS_BASE` (282)** sets the user-visible GS base to the Win32 TEB pointer
for a AthBridge guest. Mirrors `SYS_SET_FS_BASE` (126): persists in the per-task
`Task::gs_base`, restored across context switches (the scheduler restores it
field-vs-field, write-only-if-changed, like `fs_base`). Rejects
`base >= 0x0000_8000_0000_0000`. **Subtlety:** the kernel writes the value via the
ACTIVE `IA32_GS_BASE` MSR — AthenaOS's `syscall_handler` does an EVEN number of
`swapgs` between the dispatch arm and `sysretq`, so the active GS base set in the
arm is the one that survives to user mode, and `IA32_KERNEL_GS_BASE` is left
holding the per-CPU pointer for the next syscall entry. To keep SMP correct under
guest-controlled GS bases, `gdt::current_cpu_id()` reads the CPU id from the
per-CPU `PerCpuSyscall.cpu_id` (kernel-GS block) when the active GS base is a large
(TEB) value, only using the active GS base when it is a small integer.

**`SYS_MPROTECT` (283)** changes the protection flags of already-mapped 4 KiB user
pages in `[addr, addr+len)` per the `PROT_*` bits (R=1, W=2, X=4 — the same
convention `SYS_MMAP` uses, `R|W == 3`). Flips `WRITABLE`/`NO_EXECUTE` and flushes
the TLB per page. Page-aligned `addr`; `len` rounded up to a page; range must stay
below `0x0000_8000_0000_0000`; every page must already be mapped (a hole →
`u64::MAX`, no demand-mapping). Validates the whole range mapped BEFORE flipping
any page (atomic in practice). The AthGuard W^X gate (refuse simultaneous
`PROT_WRITE | PROT_EXEC`) lives in this arm, off until Phase 9 enforcement
(`memory::set_wx_policy_enforced`). The RW→RX flip the loader needs is
`PROT_READ | PROT_EXEC`.

Both **ungated** (a guest setting its own TEB / narrowing its own page protections
grants no new authority and reaches no other address space) and **allowed in every
sandbox level / safe mode** (no block-device write, no secret read). Additive
numbers — **no `ABI_VERSION` bump**. Kernel side: `kernel/src/syscall.rs` (arms
282/283), `kernel/src/task.rs` (`Task::gs_base`), `kernel/src/scheduler.rs` (GS
save/restore at the 3 switch sites), `kernel/src/gdt.rs` (cpu_id moved off the
active GS base), `kernel/src/memory.rs` (`sys_mprotect` + `prot_to_pte_flags`).
AthBridge wiring (`syscalls.rs` wrappers + `exec.rs` steps 4b/5/6) is the
raeen-compat follow-up.

## Reserved ranges (do NOT use without updating this table)

- **96–99**: VFS mutations (96–98) + AthFS game hint (99).
- **107–108**: desktop integration (clipboard); **109–126**: AthGuard + net block (121–125) + TLS (126).
- **127–140**: LinuxKPI host (Phase 1-4).
- **141–199**: reserved for AthGFX runtime.
- **200–255**: reserved for Linux compat shim (linux_syscall.rs).
- **256–257**: Installer (Phase 3 + 16.1).
- **258–263**: native synchronization (258 = `SYS_FUTEX`; rest unallocated).
- **264–267**: experimental, allocated: 264 `SYS_NET_DNS`, 265 `SYS_NET_STATUS`, 266 `SYS_THEME_GET`, 267 `SYS_AUDIO_SUBMIT`.
- **268–273**: clipboard history (268 count, 269 get, 270 pin, 271 delete, 272 clear, 273 promote).
- **274–276**: screen capture (274 start, 275 read, 276 stop). Cap::ScreenCapture-gated.
- **277–278**: accessibility tree (277 snapshot, 278 action). Cap::Accessibility-gated.
- **279**: `SYS_INPUT_CURSOR` — absolute cursor position for app hit-testing. Ungated.
- **280**: `SYS_SURFACE_ORIGIN` — live surface origin for hit-testing. Ungated.
- **281**: `SYS_SEARCH_QUERY_RESOLVED` — named (name + path) search results. Ungated.
- **282–283**: AthBridge real-MSVC-CRT ABI (282 `SYS_SET_GS_BASE`, 283 `SYS_MPROTECT`). Ungated.
- **284–290**: anti-cheat attestation (Block 34, `SYS_AC_*`).
- **291–292**: surface resize protocol (Block 35).
- **293**: `SYS_SPAWN_ARGS` — AthBridge launcher argv (Block 36, **RESERVED**, impl gated on `scheduler.rs` cooling).
- **Next free: 294.** 294+ experimental — must be documented and approved before promotion.

## How error codes are returned

Three conventions in use, documented per syscall:

1. **`u64::MAX` on failure** — plain syscalls (file I/O, read_key, etc.).
2. **`capability::E_*` constants** — capability-related (E_NO_HANDLE = MAX-1,
   E_RIGHTS = MAX-2, E_INVALID_DERIVE = MAX-3, E_NO_TASK = MAX-4,
   E_WRONG_FLAVOR = MAX-5, E_INVAL = MAX-6).
3. **Subsystem-specific error codes** in the `0xFFFF_FFFF_FFFF_FXxx` range —
   pinning (FE0x), game_profile (FF0x), config (etc.). Each module defines
   its own constants — see the module docstring.

Always test for failure as `result > 0xFFFF_FFFF_F000_0000` if you want
to catch *any* documented error code without enumerating them.
