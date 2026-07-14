//! AthenaOS ABI — the single, version-locked serialization point.
//!
//! Every development slice (Opus/kernel, Gemini/subsystems, Composer/drivers)
//! imports its cross-slice constants from HERE, not from magic numbers scattered
//! across crates. This is the contract that lets three agents work in parallel
//! without colliding on interface drift.
//!
//! # OWNERSHIP — read before editing
//!
//! **Opus is the sole editor of this crate.** Gemini and Composer import it but
//! NEVER change it. A changed value here ripples through every slice, so changes
//! are deliberate, batched events — never a side effect of subsystem work. The
//! `architecture-gate.sh` pre-commit hook rejects any diff to this crate unless
//! `ATHENA_AGENT=opus` AND the commit message carries the `[interface]` tag.
//!
//! # Versioning
//!
//! [`ABI_VERSION`] is bumped by Opus on any breaking change. Slices may assert
//! against it at build time to catch a stale checkout.
//!
//! Authoritative human-readable companion: `docs/SYSCALL_TABLE.md`. When this
//! file and that doc disagree, fix whichever is wrong in the SAME commit.

#![no_std]
#![allow(dead_code)]

/// Bumped by Opus on any breaking ABI change. Slices can `const _: () = assert!(...)`.
///
/// History:
/// - v1: initial frozen contract (driver fw 109-118, LinuxKPI 127-140, etc.)
/// - v2: SYS_DEBUG_PRINT moved from 27 (collided with SYS_SURFACE_CLOSE — the
///       compositor close-surface path was dead code because Rust match picks
///       the first arm) to 141. AthGFX reserved range slid to 142-199. relibc
///       printf updated in lockstep.
/// - v3: SYS_SEARCH_QUERY_RESOLVED (281) — a NAMED, variable-length search-result
///       surface so the Files app / command palette can render clickable rows
///       (name + path), not just opaque `(id, kind)` pairs from SYS_SEARCH_QUERY
///       (56). Strictly ADDITIVE (a fresh number + a new variable-length record
///       layout; no existing field or signature moved), so an older binary keeps
///       working unchanged. The bump is a courtesy marker for the new wire record
///       so a client can assert the layout it decodes is the one the kernel
///       encodes — the record format is documented inline at the const and in
///       docs/SYSCALL_TABLE.md, the single source of truth both sides quote.
pub const ABI_VERSION: u32 = 4;

// ════════════════════════════════════════════════════════════════════════════
// Syscall numbers (frozen). The big match in kernel/src/syscall.rs dispatches on
// these. Cross-slice-relevant numbers only — per-subsystem internal syscalls
// stay in their own crates. Full list: docs/SYSCALL_TABLE.md.
// ════════════════════════════════════════════════════════════════════════════
pub mod syscall {
    // ── Core process / IO ──
    pub const SYS_PRINT: u64 = 1;
    pub const SYS_EXIT: u64 = 12;
    pub const SYS_SEND: u64 = 5; // IPC send (see ipc module)
    pub const SYS_RECV: u64 = 6; // IPC recv

    // ── Debug-print to kernel serial (ABI v2) ──
    // 27 is SYS_SURFACE_CLOSE in this contract; relibc printf and the
    // hello_relibc smoketest use this. Was 27 in v1 which collided with the
    // compositor's close-surface arm in kernel/src/syscall.rs (first match
    // wins, so the surface_close arm was dead code). Moved to 141 in v2.
    pub const SYS_DEBUG_PRINT: u64 = 141;

    // ── AthFS game-install hint ──
    pub const SYS_ATHFS_GAME_INSTALL_HINT: u64 = 99;

    /// Subscribe the calling task to low-memory notifications (Phase 4.1).
    /// rdi = IPC channel id the app `recv`s on; the kernel pushes an
    /// `OOM_MSG_LOW_MEMORY` message + wakes the receiver before OOM-killing.
    /// Additive (unreserved slot 100); no ABI_VERSION bump.
    pub const SYS_OOM_SUBSCRIBE: u64 = 100;

    // ── AthFS snapshot management (Phase 5.1) ──
    // Additive (unreserved slots 101-103); no ABI_VERSION bump. Core CoW
    // snapshot paths live in kernel/src/athfs.rs; the kernel side is Opus.
    // All three require Cap::Filesystem{WRITE} (they mutate the live FS).
    /// `CREATE(name_ptr, name_len)` — freeze the current FS state as a named
    /// snapshot. `rax` = new snapshot id (>0) on success, error sentinel
    /// (`E_ATHFS_*`, high bits set) on failure.
    pub const SYS_ATHFS_SNAPSHOT_CREATE: u64 = 101;
    /// `RESTORE(snap_id)` — atomically roll the live FS back to `snap_id`.
    /// `rax` = 0 on success, error sentinel on failure.
    pub const SYS_ATHFS_SNAPSHOT_RESTORE: u64 = 102;
    /// `DELETE(snap_id)` — drop a snapshot and reclaim its CoW block refs.
    /// `rax` = 0 on success, error sentinel on failure.
    pub const SYS_ATHFS_SNAPSHOT_DELETE: u64 = 103;

    // ── Rae scripting (78-80, 294-295) — Concept §Customization Engine ──
    // The dispatch arms for 78-80 have been live in kernel/src/scripting.rs
    // since the scripting bring-up but were never recorded here; named now
    // (with the daemon half) so they can't be double-allocated. Additive —
    // no ABI_VERSION bump. Wire records (`ScriptAbi`, `ScriptJobAbi`) are
    // documented in kernel/src/scripting.rs + docs/SYSCALL_TABLE.md Block 14.
    /// `RUN(src_ptr, src_len, cap_mask)` → script id. Sources ≤64 KiB run
    /// inline (fuel-limited) under the cap_mask-gated kernel host; larger
    /// sources queue for `athlangd`.
    pub const SYS_SCRIPT_RUN: u64 = 78;
    /// `STATUS(script_id, out_ptr, out_cap)` → bytes written (`ScriptAbi`).
    pub const SYS_SCRIPT_STATUS: u64 = 79;
    /// `KILL(script_id)` → 0/err.
    pub const SYS_SCRIPT_KILL: u64 = 80;
    /// `FETCH(out_ptr, out_cap)` → bytes written (`ScriptJobAbi` header +
    /// source), 0 when nothing is queued, `ERR_*` sentinel otherwise.
    /// Claims the job (Queued → Running). The `athlangd` daemon half.
    pub const SYS_SCRIPT_FETCH: u64 = 294;
    /// `COMPLETE(script_id, exit_code, out_ptr, out_len)` → 0/err.
    /// exit_code is a two's-complement i64; negative marks the script Failed.
    pub const SYS_SCRIPT_COMPLETE: u64 = 295;
    /// `NETLOG_FLUSH()` → chunks broadcast. Diagnostic: broadcast the kernel
    /// bootlog ring over UDP *right now* (crate::netlog::broadcast_ring), so a
    /// marker survives a subsequent hard hang that would take down the
    /// end-of-boot flush + BOOTLOG persist (both on CPU 0). No args. Safe-mode
    /// safe (UDP TX, not a sector write). Additive diagnostic slot — NO
    /// `ABI_VERSION` bump. Used by amdgpud to fence each real-init phase onto
    /// the wire before the (possibly wedging) next stage.
    pub const SYS_NETLOG_FLUSH: u64 = 296;

    // ── DRM render-service broker (297-299) ──
    // The kernel owns /dev/dri/renderD128 and user-pointer validation; the
    // LinuxKPI amdgpud daemon owns the retained upstream amdgpu object graph.
    // These calls are the bounded, copy-based seam between those two trust
    // domains.  Registration additionally requires ownership of the supplied
    // AMD LinuxKPI device handle. Additive; no ABI_VERSION bump.
    /// `REGISTER(lkpi_device_handle)` -> 0 or `DRM_SERVICE_ERR_*`.
    pub const SYS_DRM_SERVICE_REGISTER: u64 = 297;
    /// `FETCH(header_ptr, payload_ptr, payload_cap)` -> payload bytes + 1, 0 if idle.
    pub const SYS_DRM_SERVICE_FETCH: u64 = 298;
    /// `COMPLETE(request_id, status_i32, payload_ptr, payload_len)` -> 0/error.
    pub const SYS_DRM_SERVICE_COMPLETE: u64 = 299;

    // Script capability bits for SYS_SCRIPT_RUN's cap_mask (deny by
    // default: 0 = pure computation). The kernel host and `athlangd` gate
    // every system binding on these.
    pub const SCRIPT_CAP_SYSINFO: u64 = 1 << 0;
    pub const SCRIPT_CAP_NOTIFY: u64 = 1 << 1;
    pub const SCRIPT_CAP_THEME: u64 = 1 << 2;
    pub const SCRIPT_CAP_CONFIG: u64 = 1 << 3;
    pub const SCRIPT_CAP_WALLPAPER: u64 = 1 << 4;
    pub const SCRIPT_CAP_LAUNCH: u64 = 1 << 5;
    pub const SCRIPT_CAP_ALL: u64 = (1 << 6) - 1;

    // ── System control ──
    pub const SYS_ATHENA_SHUTDOWN: u64 = 120; // requires Cap::System{WRITE}

    // ── AthNet sockets (121-125) ──
    // The dispatch arms have been live in kernel/src/syscall.rs since the
    // socket bring-up but the slots were never recorded here; named now so they
    // can't be double-allocated. 121-125 = the TCP/UDP socket surface
    // (kernel/src/net.rs). (126 is SYS_SET_FS_BASE — see SYSCALL_TABLE.md.)
    // SYS_NET_DNS (264) is the hostname resolver added 2026-06-13 — the
    // gateway to a browser.
    pub const SYS_NET_SOCKET: u64 = 121; // rdi=proto(0=TCP,1=UDP) -> fd
    pub const SYS_NET_CONNECT: u64 = 122; // rdi=fd, rsi=ip(packed BE u32), rdx=port
    pub const SYS_NET_SEND: u64 = 123; // rdi=fd, rsi=buf, rdx=len -> bytes sent
    pub const SYS_NET_RECV: u64 = 124; // rdi=fd, rsi=buf, rdx=cap -> bytes (0=none)
    pub const SYS_NET_CLOSE: u64 = 125; // rdi=fd
    /// Resolve a hostname to an IPv4 address — `rdi=name ptr`, `rsi=name len`.
    /// Returns the address as a packed big-endian `u32` (`a<<24|b<<16|c<<8|d`),
    /// or `u64::MAX` on failure. Static/cached hits return without network I/O;
    /// a miss does one UDP query to the configured server. Additive (no
    /// ABI_VERSION bump): a fresh slot at the head of the experimental range.
    pub const SYS_NET_DNS: u64 = 264;
    /// Socket readiness — `rdi=fd`. Returns the `NET_STATUS_*` flags below, or
    /// `u64::MAX` for an unknown fd. A client polls this between `connect` and
    /// `send`/`recv` so it never sends before the handshake nor mistakes
    /// "no data yet" for "closed". Additive (no ABI_VERSION bump).
    pub const SYS_NET_STATUS: u64 = 265;
    /// `SYS_NET_STATUS` result bits.
    pub const NET_STATUS_CONNECTED: u64 = 1 << 0;
    pub const NET_STATUS_READABLE: u64 = 1 << 1;
    pub const NET_STATUS_SENDABLE: u64 = 1 << 2;
    pub const NET_STATUS_CLOSED: u64 = 1 << 3;

    /// Read the LIVE desktop theme — `rdi = out ptr` to a [`crate::ThemeInfo`],
    /// `rsi = out capacity (bytes)`. The kernel `copy_to_user`s the current
    /// accent + palette so a *separate-process* app (the 6 bundled apps are
    /// distinct ELFs that cannot call `theme_engine::active_accent()` directly)
    /// can re-skin to match Vibe Mode — completing "one tap re-skins the WHOLE
    /// desktop, including running apps" (Concept §Customization Engine).
    /// Returns the number of bytes written (`size_of::<ThemeInfo>()`), or
    /// `u64::MAX` if the out buffer is too small / unmapped. Read-only and
    /// non-privileged (no `Cap`): theme colours carry no secret, every app
    /// already renders against the active accent. Additive (fresh experimental
    /// slot 266; no `ABI_VERSION` bump). Apps read once at launch and fall back
    /// to [`THEME_DEFAULT_ACCENT`] (RaeBlue) on any error.
    pub const SYS_THEME_GET: u64 = 266;

    /// Feed PCM samples into the AthAudio mixer — `rdi = samples ptr`
    /// (`*const i16`), `rsi = frame_count`, `rdx = format_flags`. Completes the
    /// audio pillar end-to-end (app → mixer → ring → HDA): the kernel
    /// `copy_from_user`s the samples and enqueues them into the calling task's
    /// per-PID `SourceKind::Pcm` mixer voice, which `AudioMixer::mix()` drains
    /// (zero-filling on underrun) into `AUDIO_RING` and onward to the HDA DMA
    /// buffer — the same production path the boot test-tone exercises.
    ///
    /// **Fixed format (v1):** interleaved 48 kHz **i16 stereo** — each frame is
    /// 2 samples (L, R) = 4 bytes, so the read length is `frame_count * 4`.
    /// `format_flags` is RESERVED (must be `0` today); a future value will
    /// select mono / a different rate without moving this slot. A non-zero
    /// `format_flags` is rejected.
    ///
    /// Returns the number of frames accepted (`<= frame_count`; fewer when the
    /// per-source queue is near full — the app re-submits the remainder next
    /// period), or [`AUDIO_SUBMIT_ERR`] (`u64::MAX`) if audio isn't initialised,
    /// `format_flags` is unsupported, `frame_count` exceeds
    /// [`AUDIO_SUBMIT_MAX_FRAMES`], or the user buffer is unmapped/too small.
    /// No capability gate (audio output carries no secret and every app may
    /// make sound); allowed in safe mode. Additive (fresh experimental slot
    /// 267, next free after `SYS_THEME_GET`); no `ABI_VERSION` bump.
    pub const SYS_AUDIO_SUBMIT: u64 = 267;

    /// `SYS_AUDIO_SUBMIT` failure sentinel.
    pub const AUDIO_SUBMIT_ERR: u64 = u64::MAX;

    // ── Clipboard history (268-273) — Concept §"The user owns the machine" ──
    // Win+V-class history + pin over the existing session clipboard. The active
    // buffer + SET/GET (107/108) are UNCHANGED; SET now ALSO appends to a
    // bounded, session-wide history ring so the clipboard-history panel can
    // render past copies. History is RAM-only / local by default — no cloud,
    // no telemetry (the Concept's ownership posture). Text-first: the entry
    // header carries a `format` tag + reserved fields so Image/Files/Url can be
    // added later WITHOUT moving a field or bumping ABI_VERSION again.
    // Additive (fresh experimental block 268-273; no ABI_VERSION bump).
    //
    // Kernel side: kernel/src/clipboard.rs. athkit wrapper: athkit::sys::clip_*.

    /// History entry count — returns `count | (pinned_count << 32)`. `count` is
    /// the total entries (pinned + recent); `pinned_count` the pinned subset.
    /// No args. Never fails (returns `0` when empty). No capability.
    pub const SYS_CLIP_HIST_COUNT: u64 = 268;

    /// Read history entry `rdi = index` (0 = newest) into `rsi = out ptr`,
    /// `rdx = out capacity (bytes)`. The kernel writes a [`crate::ClipEntryHeader`]
    /// followed by the entry's UTF-8 text payload (`header.byte_len` bytes).
    /// Returns the total bytes written (`size_of::<ClipEntryHeader>() +
    /// byte_len`), or [`CLIP_ERR`] if the index is out of range or the buffer is
    /// too small / unmapped. No capability (clipboard contents are the user's
    /// own; same posture as GET 107).
    pub const SYS_CLIP_HIST_GET: u64 = 269;

    /// Pin/unpin history entry `rdi = index`: `rsi = 1` pins, `rsi = 0` unpins.
    /// Pinned entries are exempt from eviction and from `CLEAR`. Returns `0` on
    /// success, [`CLIP_ERR`] if the index is out of range. No capability.
    pub const SYS_CLIP_HIST_PIN: u64 = 270;

    /// Delete history entry `rdi = index`. Refuses a PINNED entry (returns
    /// [`CLIP_ERR`] — unpin first, mirroring the panel's pinned-delete guard).
    /// Returns `0` on success. No capability.
    pub const SYS_CLIP_HIST_DELETE: u64 = 271;

    /// Clear history, KEEPING pinned entries (Win+V "Clear all" semantics).
    /// No args. Returns the number of entries removed. No capability.
    pub const SYS_CLIP_HIST_CLEAR: u64 = 272;

    /// Promote history entry `rdi = index` to the ACTIVE clipboard (the
    /// paste-on-select / "queue without re-copy" path): a subsequent
    /// `SYS_CLIPBOARD_GET` (107) returns this entry's content. Bumps the
    /// entry's `paste_count`. Returns `0` on success, [`CLIP_ERR`] if the index
    /// is out of range. No capability.
    pub const SYS_CLIP_HIST_PROMOTE: u64 = 273;

    /// Clipboard-history failure sentinel (shared by 269-273).
    pub const CLIP_ERR: u64 = u64::MAX;

    // ── Screen capture (274-276) — Concept §creators ──
    // "Capture & stream at the compositor — zero-cost recording, no OBS
    // overhead." Exposes the EXISTING in-kernel compositor capture engine
    // (kernel/src/compositor.rs::{start_capture,read_capture,stop_capture},
    // which already read real composited pixels off the front buffer) to
    // userspace — the screenshot tool and Game Bar overlay reuse this path.
    //
    // PRIVACY-GATED: all three require `Cap::ScreenCapture` (flavor 16), and
    // START is REFUSED in safe mode (screen pixels are sensitive). This is the
    // properly-capability-gated, validated-`copy_to_user` surface; the legacy
    // 68-70 block (`SYS_CAPTURE_BEGIN/END/READ`) is ungated + raw-pointer and is
    // deprecated in favour of 274-276. Additive (fresh block 274-276, next free
    // after clipboard history 273); no `ABI_VERSION` bump — new numbers + a new
    // TAIL Cap variant (flavor-tag-serialized, not index/bit-packed).
    //
    // Kernel side: kernel/src/syscall.rs + compositor.rs. athkit wrappers:
    // athkit::sys::capture_{start,read,stop}.

    /// Start a capture session — `rdi = region_xy` (`x | y<<32`),
    /// `rsi = region_wh` (`w | h<<32`), `rdx = format` (`CAPTURE_FMT_*`),
    /// `r10 = flags` (`CAPTURE_FLAG_*`). Returns a `capture_id` (u64) on success
    /// or [`CAPTURE_ERR`] on failure (no `Cap::ScreenCapture`, safe mode,
    /// unsupported format, or zero-area region). The session is tagged with the
    /// calling task and auto-reclaimed on task exit. Requires `Cap::ScreenCapture`.
    pub const SYS_CAPTURE_START: u64 = 274;

    /// Read the latest captured frame for `rdi = capture_id` into `rsi = out ptr`,
    /// `rdx = out capacity (bytes)`. The kernel writes a [`crate::CaptureHeader`]
    /// (16 bytes: `width, height, format, bytes`) followed by `bytes` of pixel
    /// data (ARGB or BGRA per the session format, row-major, `width*height*4`
    /// bytes) via validated `copy_to_user`. Returns the total bytes written
    /// (`size_of::<CaptureHeader>() + bytes`), or [`CAPTURE_ERR`] if the id is
    /// unknown, the buffer is too small / unmapped, or the caller lacks the cap.
    /// Requires `Cap::ScreenCapture`.
    pub const SYS_CAPTURE_READ: u64 = 275;

    /// Stop + free capture session `rdi = capture_id`. Returns `0` on success,
    /// [`CAPTURE_ERR`] if the caller lacks the cap. Idempotent for an unknown id
    /// (the session may already have been reclaimed on a prior exit). Requires
    /// `Cap::ScreenCapture`.
    pub const SYS_CAPTURE_STOP: u64 = 276;

    /// Screen-capture failure sentinel (shared by 274-276).
    pub const CAPTURE_ERR: u64 = u64::MAX;

    // ── Accessibility tree (277-278) — Concept §Security + Phase 19 a11y ──
    // The assistive-tech (AT) read/dispatch surface — the seam the screen
    // reader, magnifier, and keyboard-nav all consume. Exposes the (kernel-
    // owned, window-tier) accessibility tree to a privileged AT client:
    // SNAPSHOT serializes the live tree to userspace; ACTION dispatches a focus/
    // activate/scroll/set-value op to a node. Both are gated on
    // `Cap::Accessibility` (flavor 17, a fresh TAIL variant) — assistive tech
    // reading another app's UI tree + driving its widgets is privileged, like
    // macOS TCC Accessibility / Windows UIA. SNAPSHOT writes via VALIDATED
    // copy_to_user (no raw deref — the net/capture hardening pattern).
    //
    // Additive (fresh block 277-278, next free after screen capture 276) and a
    // fresh tail `Cap` variant — **no `ABI_VERSION` bump** (the `Cap` wire
    // contract is flavor-tag-serialized via `flavor_id`, never index/bit-packed,
    // so appending a variant breaks nothing — identical reasoning to
    // `Cap::ScreenCapture`). Kernel side: kernel/src/a11y.rs +
    // kernel/src/syscall.rs. athkit wrappers: athkit::sys::a11y_{snapshot,action}.

    /// Snapshot the accessibility tree — `rdi = out ptr`, `rsi = out capacity
    /// (bytes)`. The kernel writes an [`crate::A11ySnapshotHeader`] (16 bytes:
    /// `version, node_count, focused_id`) immediately followed by `node_count`
    /// [`crate::A11yNode`] records (96 bytes each) via validated `copy_to_user`.
    /// Returns the total bytes written
    /// (`size_of::<A11ySnapshotHeader>() + node_count * size_of::<A11yNode>()`),
    /// or [`A11Y_ERR`] if the caller lacks `Cap::Accessibility{READ}` or the
    /// buffer is too small / unmapped. Requires `Cap::Accessibility` (READ).
    pub const SYS_A11Y_SNAPSHOT: u64 = 277;

    /// Dispatch an action to a node — `rdi = node_id`, `rsi = action`
    /// (`A11Y_ACTION_*`), `rdx = arg` (action-specific; e.g. set-value payload /
    /// scroll delta, `0` when unused). The kernel routes to the owning surface
    /// (focus → raise+focus the window; activate → default action; scroll /
    /// set-value → the node's value channel). Returns `0` on success, or
    /// [`A11Y_ERR`] if the caller lacks `Cap::Accessibility{WRITE}`, the node id
    /// is unknown, or the action is unsupported for that node. Requires
    /// `Cap::Accessibility` (WRITE).
    pub const SYS_A11Y_ACTION: u64 = 278;

    /// Accessibility-syscall failure sentinel (shared by 277-278).
    pub const A11Y_ERR: u64 = u64::MAX;

    /// `A11yNode::role` tags. Mirror BOTH AccessKit's vocabulary AND
    /// `components/athui::accessibility::AccessibilityRole` so the kernel can
    /// serialize athui's live widget tree into this wire repr with NO lossy
    /// remapping (each athui role maps to exactly one tag here). `Desktop` (the
    /// synthetic root) and `Unknown` have no athui counterpart (root/fallback).
    pub const A11Y_ROLE_UNKNOWN: u32 = 0;
    pub const A11Y_ROLE_DESKTOP: u32 = 1; // synthetic root (parent of all windows)
    pub const A11Y_ROLE_WINDOW: u32 = 2;
    pub const A11Y_ROLE_BUTTON: u32 = 3;
    pub const A11Y_ROLE_LABEL: u32 = 4;
    pub const A11Y_ROLE_TEXT_FIELD: u32 = 5;
    pub const A11Y_ROLE_SLIDER: u32 = 6;
    pub const A11Y_ROLE_CHECKBOX: u32 = 7;
    pub const A11Y_ROLE_TOGGLE: u32 = 8;
    pub const A11Y_ROLE_IMAGE: u32 = 9;
    pub const A11Y_ROLE_LINK: u32 = 10;
    pub const A11Y_ROLE_HEADING: u32 = 11;
    pub const A11Y_ROLE_LIST: u32 = 12;
    pub const A11Y_ROLE_LIST_ITEM: u32 = 13;
    pub const A11Y_ROLE_TAB: u32 = 14;
    pub const A11Y_ROLE_TAB_BAR: u32 = 15;
    pub const A11Y_ROLE_SCROLL_VIEW: u32 = 16;
    pub const A11Y_ROLE_DIALOG: u32 = 17;
    pub const A11Y_ROLE_ALERT: u32 = 18;
    pub const A11Y_ROLE_MENU: u32 = 19;
    pub const A11Y_ROLE_MENU_ITEM: u32 = 20;
    pub const A11Y_ROLE_PROGRESS_BAR: u32 = 21;
    pub const A11Y_ROLE_SWITCH: u32 = 22;
    pub const A11Y_ROLE_TOOLBAR: u32 = 23;
    pub const A11Y_ROLE_GROUP: u32 = 24;

    /// `A11yNode::state` bitfield. Mirrors the boolean traits a screen reader
    /// announces (`AccessibilityTraits` in athui) plus the window-tier states the
    /// kernel knows from the compositor surface list. A future flag appends here
    /// (the upper bits are reserved) without moving any field.
    pub const A11Y_STATE_FOCUSED: u32 = 1 << 0;
    pub const A11Y_STATE_VISIBLE: u32 = 1 << 1;
    pub const A11Y_STATE_DISABLED: u32 = 1 << 2;
    pub const A11Y_STATE_CHECKED: u32 = 1 << 3;
    pub const A11Y_STATE_EXPANDED: u32 = 1 << 4;
    pub const A11Y_STATE_SELECTED: u32 = 1 << 5;
    pub const A11Y_STATE_MINIMIZED: u32 = 1 << 6;
    pub const A11Y_STATE_OFFSCREEN: u32 = 1 << 7;
    pub const A11Y_STATE_MODAL: u32 = 1 << 8;
    pub const A11Y_STATE_FOCUSABLE: u32 = 1 << 9;

    /// `A11yNode::actions` bitfield — which `A11Y_ACTION_*` ops the node accepts.
    /// An AT client reads this to know whether `SYS_A11Y_ACTION` will succeed
    /// before calling. Mirrors athui `AccessibilityAction`.
    pub const A11Y_ACTIONBIT_FOCUS: u32 = 1 << 0;
    pub const A11Y_ACTIONBIT_ACTIVATE: u32 = 1 << 1;
    pub const A11Y_ACTIONBIT_SCROLL: u32 = 1 << 2;
    pub const A11Y_ACTIONBIT_SET_VALUE: u32 = 1 << 3;
    pub const A11Y_ACTIONBIT_INCREMENT: u32 = 1 << 4;
    pub const A11Y_ACTIONBIT_DECREMENT: u32 = 1 << 5;
    pub const A11Y_ACTIONBIT_DISMISS: u32 = 1 << 6;

    /// `SYS_A11Y_ACTION` `rsi` action selector — ONE op per value (not a
    /// bitfield; the node's `actions` field is the bitfield of *accepted* ops).
    /// `FOCUS` raises+focuses the owning window; `ACTIVATE` is the default action
    /// (press a button / toggle); `SCROLL` / `SET_VALUE` carry `rdx = arg`.
    pub const A11Y_ACTION_FOCUS: u64 = 0;
    pub const A11Y_ACTION_ACTIVATE: u64 = 1;
    pub const A11Y_ACTION_SCROLL: u64 = 2;
    pub const A11Y_ACTION_SET_VALUE: u64 = 3;
    pub const A11Y_ACTION_INCREMENT: u64 = 4;
    pub const A11Y_ACTION_DECREMENT: u64 = 5;
    pub const A11Y_ACTION_DISMISS: u64 = 6;

    // ── Block 30: Absolute cursor position (279) — Concept §"a mouse-first desktop" ──
    /// Poll the compositor's current ABSOLUTE cursor position so an app can
    /// hit-test where a click landed (buttons/tabs/list items). The existing
    /// `SYS_POLL_MOUSE` (32) gives apps relative deltas + per-event button state
    /// via a destructive per-task event queue, but NO way to read the cursor's
    /// live absolute screen position — this fills that gap.
    ///
    /// Takes no arguments. Returns the position packed `x | (y << 16)` — both
    /// coordinates fit `u16` on any panel (clamped to the compositor extent).
    /// Bits `[63:32]` are RESERVED (currently `0`) for a future live button
    /// bitmask; apps that need button state today combine this poll with the
    /// existing `SYS_POLL_MOUSE`/`athkit::sys::poll_mouse` event stream. Reads a
    /// lock-free atomic the compositor updates on every cursor move, so it never
    /// blocks — cheap to call each frame. Returns [`CURSOR_ERR`] only if the
    /// compositor is not yet up.
    ///
    /// **No capability gate** (cursor position carries no secret, same posture as
    /// reading input) and **allowed in every sandbox level / safe mode**.
    pub const SYS_INPUT_CURSOR: u64 = 279;

    /// `SYS_INPUT_CURSOR` failure sentinel (compositor not initialised).
    pub const CURSOR_ERR: u64 = u64::MAX;

    /// Unpack the `SYS_INPUT_CURSOR` return value into `(x, y, buttons)`.
    /// `buttons` is the RESERVED upper word (currently always `0`; see
    /// `SYS_INPUT_CURSOR`). Apps SHOULD use this helper rather than re-deriving
    /// the bit layout, so a future button-bitmask rollout is transparent.
    #[inline]
    pub fn unpack_cursor(packed: u64) -> (u32, u32, u32) {
        (
            (packed & 0xFFFF) as u32,
            ((packed >> 16) & 0xFFFF) as u32,
            ((packed >> 32) & 0xFFFF_FFFF) as u32,
        )
    }

    // ── Block 31: Live surface origin (280) — Concept §"a mouse-first desktop" ──
    /// Query a surface's CURRENT absolute origin `(x, y)` on screen. Apps convert
    /// the absolute cursor from [`SYS_INPUT_CURSOR`] (279) into surface-local
    /// coordinates for hit-testing by subtracting their window origin — but the
    /// origin they passed to `SYS_SURFACE_PRESENT` (25) goes STALE the moment the
    /// window manager moves the window (Overview / Spaces / tiling all call the
    /// compositor's `set_surface_origin`). This poll returns the live origin so
    /// hit-testing stays correct after a move, keeping the mouse-first desktop
    /// robust under window management.
    ///
    /// `rdi` = surface id. Returns the origin packed `x | (y << 16)` (both `u16`,
    /// clamped to the compositor extent — same packing as [`SYS_INPUT_CURSOR`]),
    /// or [`SURFACE_ORIGIN_ERR`] (`u64::MAX`) if the id is unknown or the
    /// compositor is not yet up. Reads the compositor's authoritative origin via a
    /// short lock; never blocks on I/O — a cheap once-per-frame poll.
    ///
    /// **No capability gate** (a surface's screen position carries no secret, same
    /// posture as reading the cursor) and **allowed in every sandbox level / safe
    /// mode**. Position is not privileged.
    pub const SYS_SURFACE_ORIGIN: u64 = 280;

    /// `SYS_SURFACE_ORIGIN` failure sentinel (unknown surface id / compositor
    /// down). Distinct from a real origin: an app MUST check for this before
    /// unpacking, because `u64::MAX` unpacks to the off-screen `(0xFFFF, 0xFFFF)`.
    pub const SURFACE_ORIGIN_ERR: u64 = u64::MAX;

    /// Unpack the `SYS_SURFACE_ORIGIN` return value into `(x, y)`. Call only after
    /// confirming the raw value is not [`SURFACE_ORIGIN_ERR`].
    #[inline]
    pub fn unpack_surface_origin(packed: u64) -> (u32, u32) {
        ((packed & 0xFFFF) as u32, ((packed >> 16) & 0xFFFF) as u32)
    }

    // ── Block 32: Resolved search query (281) — Concept §"Search is broken" ──
    /// Query the local-first search index and serialize the RESOLVED hits —
    /// name + path + kind + folder-flag, not just the opaque `(id, kind)` pairs
    /// that `SYS_SEARCH_QUERY` (56) returns — so the Files app, start menu, and
    /// command palette can render NAMED, clickable rows with a real `Open`
    /// target. The kernel already resolves a hit to its display info in one
    /// lock-critical section (`search_index::query_resolved`); this exposes that
    /// path to a separate-process client that cannot call into the kernel index
    /// directly. Concept §Windows pain points: "Search is broken → Local-first,
    /// indexed, sub-100ms results" — a result you can't name and click is not a
    /// search result.
    ///
    /// **Args:** `rdi = q_ptr`, `rsi = q_len`, `rdx = out_ptr`,
    /// `r10 = out_cap_bytes`. **Returns** the number of records written in `rax`
    /// (`0` on an empty query, an un-initialised index, or no matches — never an
    /// error sentinel; same graceful empty-result posture as `SYS_SEARCH_QUERY`).
    ///
    /// **Wire format (variable-length records).** The kernel writes back-to-back
    /// records into `[out_ptr, out_ptr + out_cap_bytes)`. Each record is a
    /// FIXED 24-byte header ([`SearchResolvedHeader`], `#[repr(C)]`) immediately
    /// followed by `name_len` bytes of UTF-8 name then `path_len` bytes of UTF-8
    /// path (no NUL terminators, no per-record padding — the next header begins
    /// at `header + 24 + name_len + path_len`). The header is:
    ///
    /// ```text
    /// offset size field      meaning
    ///   0     8   id         stable index item id (little-endian u64)
    ///   8     4   kind       SEARCH_KIND_* tag (little-endian u32)
    ///  12     1   is_folder  1 = directory, 0 = file/other
    ///  13     1   reserved0  0 (future flags)
    ///  14     2   reserved1  0 (future use; keeps name_len 2-aligned)
    ///  16     2   name_len   UTF-8 name byte count that follows the header
    ///  18     2   path_len   UTF-8 path byte count that follows name
    ///  20     4   reserved2  0 (rounds the header to a clean 24 bytes)
    /// ```
    ///
    /// The kernel writes only WHOLE records that fit in `out_cap_bytes`: it stops
    /// before a record whose `24 + name_len + path_len` would overflow the
    /// remaining capacity (a partially-written trailing record is never emitted),
    /// and caps the count at [`SEARCH_RESOLVED_MAX_RESULTS`]. `name_len` /
    /// `path_len` are each clamped to [`SEARCH_RESOLVED_MAX_STR`] so a single
    /// pathological entry can't blow the buffer; an over-long string is truncated
    /// on a UTF-8 char boundary. A defensive decoder MUST clamp its walk to the
    /// returned count AND bounds-check every `name_len`/`path_len` against the
    /// remaining buffer (the documented athkit decoder does both, never panicking
    /// on a short/garbage buffer).
    ///
    /// **No capability gate** (search results carry no secret beyond names the
    /// indexer already holds; same posture as `SYS_SEARCH_QUERY` 56) and
    /// **allowed in every sandbox level / safe mode**. Additive (fresh slot 281,
    /// next free after `SYS_SURFACE_ORIGIN` 280).
    pub const SYS_SEARCH_QUERY_RESOLVED: u64 = 281;

    // ── Block 33: AthBridge real-MSVC-CRT ABI (282–283) ─────────────────────
    // Two syscalls that let AthBridge run REAL MSVC-compiled `.exe`s: every
    // MSVC-CRT binary reads `gs:[0x30]` for its TEB on entry, and the loader
    // needs to flip relocated `.text` RW→RX. Both ungated, allowed in every
    // sandbox level (a guest setting its own TEB pointer / narrowing its own
    // page protections grants no new authority and reaches no other address
    // space). Additive numbers — NO `ABI_VERSION` bump. See
    // docs/components/athbridge-real-crt-abi.md.

    /// `SYS_SET_GS_BASE(base)` — set the user-visible GS base to the Win32 TEB
    /// pointer for a AthBridge guest. `rdi = base` (TEB virtual address);
    /// returns `0` on success, `u64::MAX` on a non-canonical / kernel-half
    /// address. Mirrors `SYS_SET_FS_BASE` (126); the kernel persists it in the
    /// per-task GS base and restores it across context switches. Ungated,
    /// all-sandbox. Additive (fresh slot 282, next free after
    /// `SYS_SEARCH_QUERY_RESOLVED` 281).
    pub const SYS_SET_GS_BASE: u64 = 282;

    /// `SYS_MPROTECT(addr, len, prot)` — change the protection flags of
    /// already-mapped 4 KiB user pages. `rdi = addr` (page-aligned),
    /// `rsi = len`, `rdx = prot` (the `PROT_*` bits below); returns `0` on
    /// success, `u64::MAX` on a bad range / unmapped page / disallowed (W^X)
    /// transition. Maps nothing new and reaches no other address space.
    /// Ungated, all-sandbox. Additive (fresh slot 283).
    pub const SYS_MPROTECT: u64 = 283;

    // ── Block 34: Anti-cheat attestation (284–290) ──────────────────────────
    // Concept §Security: "anti-cheat vendors (EAC/BattlEye/Vanguard) use a
    // AthGuard attestation API WITHOUT owning ring 0." Handlers live in
    // kernel/src/anticheat.rs.
    //
    // RENUMBERED 2026-06-25 from the original 100–106. Those numbers were a hard
    // ABI COLLISION: dispatch arms 100 (SYS_OOM_SUBSCRIBE) and 101–103 (AthFS
    // snapshot create/restore/delete) precede the range arm in the match, so the
    // first-match-wins rule meant calling the *documented* SYS_AC_REGISTER_GAME
    // (102) actually executed `athfs::snapshot_restore` — a destructive FS
    // rollback. OOM(100) + AthFS snapshots(101–103) are live and iron-proven, so
    // anti-cheat (design-tier) moved to this fresh contiguous block. ABI_VERSION
    // bumped to 4 (a number CHANGED — not merely additive). The 100–106 anti-cheat
    // numbers never worked, so no real consumer breaks.
    pub const SYS_AC_REQUEST_ATTESTATION: u64 = 284;
    pub const SYS_AC_VERIFY_ATTESTATION: u64 = 285;
    pub const SYS_AC_REGISTER_GAME: u64 = 286;
    pub const SYS_AC_UNREGISTER_GAME: u64 = 287;
    pub const SYS_AC_REPORT_VIOLATION: u64 = 288;
    pub const SYS_AC_QUERY_STATUS: u64 = 289;
    pub const SYS_AC_HEARTBEAT: u64 = 290;

    // ── Block 35: Surface resize protocol (291–292) ─────────────────────────
    // Concept §AthUI: "tiling, stacking, floating are POLICIES over the
    // compositor." A real tiling WM does not merely move a window into a cell —
    // it RESIZES the client to FILL the cell (i3/sway tiling; Win11 Snap
    // Layouts). The compositor owns the surface's backing frames, so the client
    // cannot resize itself unilaterally; the WM RECORDS a desired size and the
    // client honors it on its own terms. These two syscalls are that handshake.
    // Handlers live in kernel/src/syscall.rs; mechanism in compositor.rs
    // (`surface_resize_request` / `resize_user_surface`). Additive numbers — NO
    // `ABI_VERSION` bump (fresh slots 291–292, next free after 290).

    /// `SYS_SURFACE_RESIZE_REQ(id)` — poll whether the window manager wants this
    /// surface at a NEW size (it tiled/snapped the window into a cell). The
    /// compositor records the desired cell size when the WM applies a tiling
    /// layout; a tiling-aware client polls this each frame and, when a request
    /// is pending, reallocates its framebuffer to the requested dimensions and
    /// acks via [`SYS_SURFACE_RESIZE`] (292). A client that ignores it keeps its
    /// old size (graceful — the window is merely positioned, not reflowed; the
    /// old behavior).
    ///
    /// `rdi` = surface id. Returns the requested size packed `w | (h << 16)`
    /// (both `u16`; surfaces are capped at 8192 per side so they fit) when a
    /// resize is pending, [`SURFACE_RESIZE_NONE`] (`0`) when none is pending, or
    /// [`SURFACE_RESIZE_ERR`] (`u64::MAX`) if the id is unknown / the compositor
    /// is down. No capability gate, allowed in every sandbox level / safe mode —
    /// a window's requested size carries no secret (same posture as the
    /// `SYS_SURFACE_ORIGIN` 280 poll).
    pub const SYS_SURFACE_RESIZE_REQ: u64 = 291;

    /// `SYS_SURFACE_RESIZE(id, w, h, new_buf)` — the client ACKS a pending resize
    /// (see [`SYS_SURFACE_RESIZE_REQ`] 291): it has allocated a FRESH user buffer
    /// of `w × h × 4` bytes at page-aligned `new_buf`, and asks the compositor to
    /// rebind the surface to it. The kernel allocates new contiguous backing
    /// frames, maps them into the caller's address space at `new_buf`, unmaps and
    /// frees the OLD frames, updates the surface's dimensions, and clears the
    /// pending request. The client then renders into `new_buf` at the new size.
    /// Passing a fresh vaddr (rather than growing in place) mirrors
    /// `SYS_SURFACE_CREATE` exactly and is collision-free.
    ///
    /// `rdi` = surface id, `rsi` = new width, `rdx` = new height,
    /// `r10` = `new_buf` (page-aligned user vaddr). Returns `0` on success or
    /// [`SURFACE_RESIZE_ERR`] (`u64::MAX`) on a bad id / non-owner caller /
    /// invalid dimensions / unaligned-or-kernel-half `new_buf` / allocation
    /// failure. The caller MUST own the surface (the kernel checks
    /// `owner_task`); a task can only resize its own window, so this is
    /// all-sandbox and ungated.
    pub const SYS_SURFACE_RESIZE: u64 = 292;

    /// [`SYS_SURFACE_RESIZE_REQ`] (291) result when no resize is pending. `0` is
    /// unambiguous: a real request always has `w > 0`.
    pub const SURFACE_RESIZE_NONE: u64 = 0;

    /// [`SYS_SURFACE_RESIZE_REQ`] / [`SYS_SURFACE_RESIZE`] failure sentinel
    /// (unknown surface id, non-owner caller, or compositor down).
    pub const SURFACE_RESIZE_ERR: u64 = u64::MAX;

    /// Unpack the [`SYS_SURFACE_RESIZE_REQ`] (291) return value into `(w, h)`.
    /// Call only after confirming the raw value is neither
    /// [`SURFACE_RESIZE_NONE`] nor [`SURFACE_RESIZE_ERR`].
    pub fn unpack_surface_resize(packed: u64) -> (u32, u32) {
        ((packed & 0xFFFF) as u32, ((packed >> 16) & 0xFFFF) as u32)
    }

    /// `prot` bit values shared by `SYS_MMAP` and `SYS_MPROTECT`. POSIX-
    /// compatible numerics (a local ABI constant, not an imported architecture)
    /// so both sides agree: `PROT_READ | PROT_WRITE == 3` matches the existing
    /// mmap call sites. `PROT_READ` is implicit on x86_64 (a present user page
    /// is always readable); the meaningful flips are WRITE and EXEC.
    pub const PROT_NONE: u64 = 0;
    pub const PROT_READ: u64 = 1; // bit 0
    pub const PROT_WRITE: u64 = 2; // bit 1
    pub const PROT_EXEC: u64 = 4; // bit 2

    /// Fixed per-record header size for `SYS_SEARCH_QUERY_RESOLVED` (281), in
    /// bytes. The variable name+path payload follows immediately. Equals
    /// `core::mem::size_of::<crate::SearchResolvedHeader>()`.
    pub const SEARCH_RESOLVED_HEADER_SIZE: usize = 24;

    /// Maximum records `SYS_SEARCH_QUERY_RESOLVED` (281) will write in one call,
    /// independent of buffer capacity (bounds the kernel-side work + the client's
    /// decode loop). The command palette shows a short ranked list; 64 is ample.
    pub const SEARCH_RESOLVED_MAX_RESULTS: usize = 64;

    /// Maximum UTF-8 byte length of a single `name`/`path` field a resolved
    /// record carries (each clamped independently). A longer string is truncated
    /// on a char boundary by the kernel encoder so one entry can't dominate the
    /// buffer. A full AthFS path fits comfortably; this is a safety ceiling.
    pub const SEARCH_RESOLVED_MAX_STR: usize = 1024;

    /// Inline name buffer length for [`crate::A11yNode`] (matches the compositor
    /// `Surface.title` width + `ThemeInfo`/`CaptureHeader` inline-buffer style).
    pub const A11Y_NAME_LEN: usize = 48;

    /// `SYS_CAPTURE_START` / `CaptureHeader::format` values.
    /// `CAPTURE_FMT_ARGB32` = `0xAARRGGBB` little-endian (matches the
    /// compositor's native front-buffer + `ath_tokens` ARGB). `CAPTURE_FMT_BGRA32`
    /// = byte order B,G,R,A (what some image encoders / GPU upload paths want).
    pub const CAPTURE_FMT_ARGB32: u32 = 0;
    pub const CAPTURE_FMT_BGRA32: u32 = 1;

    /// `SYS_CAPTURE_START` `flags` bits. `CONTINUOUS` keeps the session live
    /// across frames (Game Bar / recording — read repeatedly); without it the
    /// session captures one frame then deactivates (a single screenshot). The
    /// upper bits are RESERVED for future window/region-follow modes so the
    /// screenshot tool's window-pick mode can be added without moving this slot.
    pub const CAPTURE_FLAG_CONTINUOUS: u64 = 1 << 0;
    pub const CAPTURE_FLAG_RESERVED_MASK: u64 = !1;

    /// Maximum history entries the kernel ring retains. Pinned-safe eviction:
    /// once full, the OLDEST UNPINNED entry is dropped to make room (a pinned
    /// entry is never evicted). Mirrors `athshell::ClipboardManager::max_history`.
    pub const CLIP_HIST_MAX_ENTRIES: u64 = 64;

    /// `ClipEntryHeader::format` values. Text-first today; the rest are RESERVED
    /// tags so a future kernel can carry richer clips without an ABI break — the
    /// panel reads `format` to pick a renderer and treats unknown tags as
    /// opaque. Mirrors `athshell::clipboard::ClipboardFormat`.
    pub const CLIP_FMT_TEXT: u32 = 0;
    pub const CLIP_FMT_RICH_TEXT: u32 = 1; // reserved
    pub const CLIP_FMT_IMAGE: u32 = 2; // reserved
    pub const CLIP_FMT_FILES: u32 = 3; // reserved
    pub const CLIP_FMT_URL: u32 = 4; // reserved
    pub const CLIP_FMT_COLOR: u32 = 5; // reserved

    /// `ClipEntryHeader::flags` bits.
    pub const CLIP_FLAG_PINNED: u32 = 1 << 0;

    /// Maximum frames a single `SYS_AUDIO_SUBMIT` may carry. Bounds the
    /// kernel-side `copy_from_user` and the per-source queue depth: 4 DMA
    /// periods (4 × 128 = 512 frames ≈ 10.7 ms @ 48 kHz) — enough head-room for
    /// an app's buffer-ahead without letting one call pin unbounded kernel
    /// memory. A larger `frame_count` is rejected with [`AUDIO_SUBMIT_ERR`].
    pub const AUDIO_SUBMIT_MAX_FRAMES: u64 = 512;

    /// Bytes per frame for the fixed `SYS_AUDIO_SUBMIT` format (i16 stereo).
    pub const AUDIO_SUBMIT_BYTES_PER_FRAME: u64 = 4;

    /// RaeBlue — the default accent every app falls back to when
    /// `SYS_THEME_GET` is unavailable (host tests, an unthemed early boot).
    /// Mirrors `kernel::theme_engine::RAEBLUE` / `ath_tokens::RAEBLUE`; kept on
    /// the ABI so the fallback is a single shared constant, not a per-app magic
    /// number.
    pub const THEME_DEFAULT_ACCENT: u32 = 0xFF_4E_9C_FF;

    // ── Userspace driver framework (FROZEN — Composer's hardware isolation) ──
    // syscalls 109-118. Drivers are mechanically isolated behind these.
    pub const SYS_DRIVER_REGISTER: u64 = 109;
    pub const SYS_DRIVER_UNREGISTER: u64 = 110;
    pub const SYS_DRIVER_CLAIM_DEVICE: u64 = 111; // sys_claim_device — see device module
    pub const SYS_DRIVER_RELEASE_DEVICE: u64 = 112;
    pub const SYS_DRIVER_ENABLE_DMA: u64 = 113;
    pub const SYS_DRIVER_LIST: u64 = 114;
    pub const SYS_DRIVER_QUERY: u64 = 115;
    pub const SYS_DRIVER_IRQ_SETUP: u64 = 116;
    pub const SYS_DRIVER_DMA_MAP: u64 = 117;
    pub const SYS_DRIVER_DMA_UNMAP: u64 = 118;

    /// IPC channel shared-memory map — `rdi=channel cap handle`, `rsi=target
    /// virt addr` (4 KiB aligned). The dispatch arm has been live since the
    /// secure-IPC shmem work but the slot was never recorded here; recorded
    /// now so it can never be double-allocated (a draft SYS_FUTEX briefly
    /// collided with it — caught in steward review, futex moved to 258).
    pub const SYS_CHANNEL_SHMEM_MAP: u64 = 119;

    // ── LinuxKPI host (Path C — Composer's userspace Linux-driver shim) ──
    // syscalls 127-140. Kernel side in kernel/src/linuxkpi_host.rs (Opus); the
    // userspace shim components/ath_linuxkpi (Composer) calls these.
    pub const SYS_LINUXKPI_VERSION: u64 = 127;
    pub const SYS_LINUXKPI_JIFFIES: u64 = 128;
    pub const SYS_LINUXKPI_MSLEEP: u64 = 129;
    pub const SYS_LINUXKPI_IOREMAP: u64 = 130;
    pub const SYS_LINUXKPI_PRINTK: u64 = 131;
    pub const SYS_LINUXKPI_PCI_ENABLE: u64 = 132;
    /// Match-mode flag for `SYS_LINUXKPI_PCI_ENABLE`'s `rdi`. With bit 63
    /// CLEAR, `rdi` is the legacy packed BDF `(bus<<16)|(dev<<8)|func`. With
    /// bit 63 SET, the host scans its PCI table and claims the FIRST device
    /// matching `rdi` bits 16-23 = PCI class code (required) and bits 0-15 =
    /// vendor id (`0x0000` = any vendor). This is how Linux drivers actually
    /// bind (`pci_device_id` match tables, not fixed BDFs) — a GPU daemon asks
    /// for "class 0x03, vendor 0x1002" and works whether the silicon sits at
    /// 00:01.0 (QEMU) or c4:00.0 (Athena). Additive (v2 → v2): bit 63 was
    /// previously an out-of-range bus and always failed with E_NO_DEVICE.
    pub const LINUXKPI_PCI_MATCH: u64 = 1 << 63;
    pub const SYS_LINUXKPI_PCI_READ_CFG: u64 = 133;
    pub const SYS_LINUXKPI_PCI_WRITE_CFG: u64 = 134;
    pub const SYS_LINUXKPI_DMA_ALLOC: u64 = 135;
    pub const SYS_LINUXKPI_DMA_FREE: u64 = 136;
    pub const SYS_LINUXKPI_REQUEST_IRQ: u64 = 137;
    pub const SYS_LINUXKPI_IRQ_WAIT: u64 = 138;
    pub const SYS_LINUXKPI_IOUNMAP: u64 = 139;
    pub const SYS_LINUXKPI_SUPERVISOR: u64 = 140;
    /// `request_firmware()` host call — loads a named microcode/firmware blob
    /// from the initramfs `firmware/` tree and maps it into the driver's
    /// address space. Required by every Linux GPU/Wi-Fi driver (amdgpu, i915,
    /// iwlwifi) before it can bring the hardware up. Additive (v2 → v2, no
    /// breaking change): carved from the previously-unused low end of the
    /// AthGFX reserved range, which now starts at 143.
    pub const SYS_LINUXKPI_REQUEST_FIRMWARE: u64 = 142;

    /// `SYS_RAEGFX_REGISTER_SCANOUT` — a GPU driver daemon (amdgpud, via
    /// `ath_drm::kms::atomic_commit`) registers its display scanout framebuffer
    /// so the in-kernel compositor presents THROUGH the device's display engine
    /// (the amdgpu DCN). `rdi`=dev_handle (from pci_enable), `rsi`=phys (which
    /// MUST be a DMA region the caller already owns on that device — so a daemon
    /// can expose only its own buffer, never arbitrary physical memory),
    /// `rdx`=(width << 32 | height), `r10`=stride bytes. Returns 1 if the
    /// compositor attached it, 0 on reject. First slot claimed from the AthGFX
    /// reserved range (143-199); additive (no `ABI_VERSION` bump).
    pub const SYS_RAEGFX_REGISTER_SCANOUT: u64 = 143;

    /// `SYS_LINUXKPI_MAP_PHYS` — a GPU driver daemon maps a physical range that is
    /// NOT a PCI BAR into its own user space. Needed for APU (UMA) VRAM: on
    /// Phoenix the "VRAM" is a carved-out region of system RAM at a high physical
    /// address (e.g. 0x4_5fd00000), and CPU-visible kernel BOs (the GART page
    /// table, ring/fence buffers) live there — beyond the small CPU-visible BAR0
    /// aperture — so `ioremap` of a BAR cannot reach them. `rdi`=dev_handle (the
    /// caller MUST own it, same gate as ioremap), `rsi`=phys, `rdx`=size. Returns
    /// the mapped user virtual address, or an error sentinel (high bit set).
    /// SECURITY: the kernel REFUSES any range that overlaps usable RAM
    /// (`phys_is_usable_ram`) — only firmware-reserved / carveout physical memory
    /// can be mapped, so a driver can never map kernel or another process's
    /// memory. Additive (fresh slot 144, LinuxKPI facility range); no
    /// `ABI_VERSION` bump.
    pub const SYS_LINUXKPI_MAP_PHYS: u64 = 144;

    // ── Installer (Composer) ──
    pub const SYS_INSTALL_RUN: u64 = 256;
    pub const SYS_INSTALL_CREATE_ACCOUNT: u64 = 257;

    // ── Native synchronization (258-263) ──
    /// Native futex — `rdi=uaddr`, `rsi=op` (0=WAIT, 1=WAKE), `rdx=val`
    /// (WAIT: expected *uaddr; WAKE: max waiters to wake). WAIT returns 0 on
    /// wake, 1 on value-mismatch (EAGAIN), 2 on fault; WAKE returns the count
    /// woken. Backs relibc's `sync` (mutex/once/rwlock) for NATIVE-ABI apps —
    /// the Linux ABI already had futex via syscall 202, but native relibc apps
    /// (hello_relibc, ports) hit ENOSYS without this. Kernel reuses the
    /// existing futex table. Additive (no ABI_VERSION bump). NOTE: a draft
    /// assigned this to 119, which collided with the live
    /// SYS_CHANNEL_SHMEM_MAP arm — renumbered here to a fresh block.
    pub const SYS_FUTEX: u64 = 258;

    /// Reserved ranges (do not allocate without an `[interface]` Opus commit):
    /// 141 SYS_DEBUG_PRINT · 142 SYS_LINUXKPI_REQUEST_FIRMWARE · 143-199 AthGFX
    /// runtime · 200-255 Linux compat shim · 258-263 native sync · 264-267
    /// experimental (264 net_dns, 265 net_status, 266 theme_get, 267
    /// audio_submit) · 268-273 clipboard history · 274-276 screen capture ·
    /// 277-278 accessibility · 279 input_cursor · 280 surface_origin ·
    /// 281 search_query_resolved · 282-283 athbridge MSVC-CRT · 284-290
    /// anti-cheat attestation · 291-292 surface resize · 293+ free.
    pub const RESERVED_RAEGFX_LO: u64 = 143;
    pub const RESERVED_RAEGFX_HI: u64 = 199;
    pub const RESERVED_LINUXCOMPAT_LO: u64 = 200;
    pub const RESERVED_LINUXCOMPAT_HI: u64 = 255;
    pub const RESERVED_SYNC_LO: u64 = 258;
    pub const RESERVED_SYNC_HI: u64 = 263;
    pub const EXPERIMENTAL_LO: u64 = 264;
}

/// Bounded wire contract for the kernel-owned DRM render node and the
/// daemon-owned upstream amdgpu instance. No raw client pointer crosses this
/// boundary: ioctl arguments are copied into `payload`, and command-specific
/// auxiliary buffers are appended after the flat argument.
pub mod drm_service {
    pub const VERSION: u32 = 1;
    pub const MAX_PAYLOAD: usize = 64 * 1024;

    pub const OP_OPEN: u32 = 1;
    pub const OP_CLOSE: u32 = 2;
    pub const OP_IOCTL: u32 = 3;
    pub const OP_MMAP: u32 = 4;

    pub const FLAG_INFO_AUX: u32 = 1 << 0;
    pub const FLAG_VERSION_AUX: u32 = 1 << 1;
    pub const FLAG_BO_LIST_AUX: u32 = 1 << 2;
    pub const FLAG_CS_AUX: u32 = 1 << 3;

    pub const ERR_UNAVAILABLE: u64 = u64::MAX - 1;
    pub const ERR_DENIED: u64 = u64::MAX - 2;
    pub const ERR_FAULT: u64 = u64::MAX - 3;
    pub const ERR_INVALID: u64 = u64::MAX - 4;
    pub const ERR_BUSY: u64 = u64::MAX - 5;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct RequestHeader {
        pub version: u32,
        pub op: u32,
        pub request_id: u64,
        pub client_id: u64,
        pub ioctl_cmd: u32,
        pub flags: u32,
        pub arg_len: u32,
        pub payload_len: u32,
    }

    const _: () = {
        assert!(core::mem::size_of::<RequestHeader>() == 40);
    };
}

// ════════════════════════════════════════════════════════════════════════════
// sys_claim_device contract (FROZEN). The choke point that mechanically isolates
// every driver. Composer's whole slice lives behind this.
// ════════════════════════════════════════════════════════════════════════════
pub mod device {
    /// Pack a PCI Bus:Device.Function into the single u64 argument that
    /// `SYS_DRIVER_CLAIM_DEVICE` (111) takes. This packing is part of the ABI.
    #[inline]
    pub const fn pack_bdf(bus: u8, dev: u8, func: u8) -> u64 {
        ((bus as u64) << 16) | ((dev as u64) << 8) | (func as u64)
    }

    #[inline]
    pub const fn unpack_bdf(packed: u64) -> (u8, u8, u8) {
        (
            ((packed >> 16) & 0xFF) as u8,
            ((packed >> 8) & 0xFF) as u8,
            (packed & 0xFF) as u8,
        )
    }

    /// A claim handle is `(driver_handle << 32) | claim_id`. Drivers pass it back
    /// to DMA-map / IRQ-setup / release. Part of the frozen surface.
    #[inline]
    pub const fn driver_handle_of(claim_handle: u64) -> u64 {
        claim_handle >> 32
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Capability surface. AthGuard (Opus) is the authority; every privileged op
// goes through a Cap. These flag values are the wire contract — never bypass.
// ════════════════════════════════════════════════════════════════════════════
pub mod cap {
    /// Rights bitflags carried by every capability.
    pub const RIGHTS_READ: u32 = 1 << 0;
    pub const RIGHTS_WRITE: u32 = 1 << 1;
    pub const RIGHTS_MAP: u32 = 1 << 2;
    pub const RIGHTS_GRANT: u32 = 1 << 3;
    pub const RIGHTS_REVOKE: u32 = 1 << 4;
    pub const RIGHTS_WAIT: u32 = 1 << 5;
    pub const RIGHTS_ALL: u32 = 0xFFFF_FFFF;

    /// Capability error sentinels returned in `rax` (u64::MAX - n).
    pub const E_NO_HANDLE: u64 = u64::MAX - 1;
    pub const E_RIGHTS: u64 = u64::MAX - 2;
    pub const E_INVALID_DERIVE: u64 = u64::MAX - 3;
    pub const E_NO_TASK: u64 = u64::MAX - 4;
    pub const E_WRONG_FLAVOR: u64 = u64::MAX - 5;
    pub const E_INVAL: u64 = u64::MAX - 6;

    /// True if `result` is any documented error sentinel (>= 0xFFFF_FFFF_F000_0000).
    #[inline]
    pub const fn is_err(result: u64) -> bool {
        result >= 0xFFFF_FFFF_F000_0000
    }
}

// ════════════════════════════════════════════════════════════════════════════
// IPC surface. Bounded channels with flow control; the cross-slice primitive
// for driver doorbells, shell input, and service messaging.
// ════════════════════════════════════════════════════════════════════════════
pub mod ipc {
    /// Well-known channel ids seeded at boot.
    pub const CHAN_KEYBOARD: u32 = 1;
    pub const CHAN_MOUSE: u32 = 2;

    /// Max bytes per IPC message (bounded channel contract).
    pub const MSG_MAX_BYTES: usize = 4096;
}

// ════════════════════════════════════════════════════════════════════════════
// Live theme snapshot. The small, stable struct `SYS_THEME_GET` (266) writes to
// a user buffer so a separate-process app can re-skin to the active Vibe-Mode
// theme. A subset of `kernel::theme_engine::ThemeAbi` — accent + the handful of
// fields an app needs to match the desktop; deliberately NOT the whole ThemeAbi
// (font/cursor/particle belong to the compositor, not app chrome).
// ════════════════════════════════════════════════════════════════════════════

/// Live theme info, written by `SYS_THEME_GET` (syscall 266). `#[repr(C)]`,
/// fixed-size, all `u32` — stable across builds and trivially `copy_to_user`-able.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeInfo {
    /// Struct version (= [`ThemeInfo::VERSION`]). Lets a future field append
    /// stay additive: an older app reads the prefix it understands.
    pub version: u32,
    /// The LIVE accent (ARGB, opaque) — `theme_engine::active_accent()`. This is
    /// the seed every surface feeds into `ath_tokens::derive_accent`. Equals
    /// [`THEME_DEFAULT_ACCENT`] when no theme override is active.
    pub accent_argb: u32,
    /// Background ARGB of the active palette (the desktop's deepest chrome).
    pub bg_argb: u32,
    /// Foreground / primary-text ARGB of the active palette.
    pub fg_argb: u32,
    /// `1` if the active palette is dark, `0` if light — picks `ath_tokens::DARK`
    /// vs the light palette without re-deriving from `bg_argb` luminance.
    pub is_dark: u32,
    /// Glassmorphism blur radius in pixels (`ThemeAbi::blur_radius`).
    pub blur_radius: u32,
    /// Active theme id (`ThemeAbi::id`); `0` = the builtin default / accent-only
    /// override. An app can cache this to detect a theme change cheaply.
    pub palette_id: u32,
    /// Reserved for a future field append; `0` today. Keeps the struct a round
    /// 32 bytes and lets v2 grow without moving any existing field.
    pub reserved: u32,
}

impl ThemeInfo {
    /// Current struct version. Bump only when a field is APPENDED (older readers
    /// keep working by reading the shorter prefix), never when one moves.
    pub const VERSION: u32 = 1;
}

// ════════════════════════════════════════════════════════════════════════════
// Clipboard-history entry header. The fixed-size prefix `SYS_CLIP_HIST_GET`
// (269) writes to a user buffer, immediately followed by `byte_len` bytes of
// UTF-8 text payload. Text-first: `format` is `CLIP_FMT_TEXT` today, and the
// `reserved*` fields are zero — they exist so a future kernel can describe an
// Image (width/height/mime) or Files (count) clip WITHOUT moving any field or
// bumping ABI_VERSION. The panel reads `format` to pick a row renderer.
// ════════════════════════════════════════════════════════════════════════════

/// Per-entry header written by `SYS_CLIP_HIST_GET` (269). `#[repr(C)]`,
/// fixed-size, trivially `copy_to_user`-able. The text payload (`byte_len`
/// bytes of UTF-8) follows immediately after this header in the same buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipEntryHeader {
    /// Struct version (= [`ClipEntryHeader::VERSION`]) — lets a future field
    /// append stay additive (an older panel reads the prefix it understands).
    pub version: u32,
    /// One of the `syscall::CLIP_FMT_*` tags. `CLIP_FMT_TEXT` today; reserved
    /// tags let Image/Files/Url/Color be added without an ABI break.
    pub format: u32,
    /// `syscall::CLIP_FLAG_*` bits (`CLIP_FLAG_PINNED` today).
    pub flags: u32,
    /// Length in bytes of the UTF-8 text payload that follows this header.
    /// Capped at [`crate::MAX_CLIPBOARD_BYTES`].
    pub byte_len: u32,
    /// Monotonic copy sequence (newer entries have a larger value). Lets the
    /// panel sort/label without a separate timestamp syscall; `0` if unknown.
    pub sequence: u32,
    /// How many times this entry has been promoted to the active clipboard
    /// (`SYS_CLIP_HIST_PROMOTE`) — feeds future most-used ordering. `0` today.
    pub paste_count: u32,
    /// RESERVED (image width / file count / etc.); `0` today. Append future
    /// rich-format dimensions here without moving a field.
    pub reserved0: u32,
    /// RESERVED (image height / mime tag / etc.); `0` today.
    pub reserved1: u32,
}

impl ClipEntryHeader {
    /// Current struct version. Bump only on a field APPEND (never a move).
    pub const VERSION: u32 = 1;
}

// ════════════════════════════════════════════════════════════════════════════
// Screen-capture frame header. The fixed-size prefix `SYS_CAPTURE_READ` (275)
// writes to a user buffer, immediately followed by `bytes` of raw pixel data
// (ARGB or BGRA per `format`, row-major, `width*height*4` bytes). Mirrors the
// ClipEntryHeader pattern: a small `#[repr(C)]` prefix so the reader knows the
// dimensions + payload length before parsing the pixels.
// ════════════════════════════════════════════════════════════════════════════

/// Per-frame header written by `SYS_CAPTURE_READ` (275). `#[repr(C)]`,
/// fixed-size (16 bytes), trivially `copy_to_user`-able. The pixel payload
/// (`bytes` bytes = `width * height * 4`) follows immediately after this header
/// in the same buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureHeader {
    /// Captured region width in pixels.
    pub width: u32,
    /// Captured region height in pixels.
    pub height: u32,
    /// Pixel format of the payload — one of `syscall::CAPTURE_FMT_*`.
    pub format: u32,
    /// Length in bytes of the pixel payload that follows this header
    /// (`width * height * 4`). The reader copies exactly this many bytes.
    pub bytes: u32,
}

impl CaptureHeader {
    /// Header size in bytes (the payload offset). 16 bytes, 4 × u32.
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

// ════════════════════════════════════════════════════════════════════════════
// Accessibility snapshot wire format. `SYS_A11Y_SNAPSHOT` (277) writes an
// `A11ySnapshotHeader` followed by `node_count` `A11yNode` records, both
// `#[repr(C)]`, fixed-size, trivially `copy_to_user`-able. An AT client (screen
// reader / magnifier / keyboard-nav) reads the header to learn the node count +
// focused id, then walks the flat node array — the same arena shape AccessKit
// and `athui::accessibility::AccessibilityTree` use (parent ids, not nested),
// so a future userspace AccessKit adapter maps 1:1.
// ════════════════════════════════════════════════════════════════════════════

/// Snapshot header written by `SYS_A11Y_SNAPSHOT` (277). `#[repr(C)]`, 16 bytes.
/// The `node_count` [`A11yNode`] records follow immediately after this header in
/// the same buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct A11ySnapshotHeader {
    /// Struct version (= [`A11ySnapshotHeader::VERSION`]) — lets a future field
    /// append stay additive (an older AT client reads the prefix it understands).
    pub version: u32,
    /// Number of [`A11yNode`] records that follow this header.
    pub node_count: u32,
    /// Node id of the currently focused node (`0` = root/none). Saves the AT
    /// client a linear scan for the `A11Y_STATE_FOCUSED` node on every snapshot.
    pub focused_id: u64,
}

impl A11ySnapshotHeader {
    /// Current struct version. Bump only on a field APPEND (never a move).
    pub const VERSION: u32 = 1;
    /// Header size in bytes (the node-array offset). 16 bytes.
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

/// One accessibility node in the flat snapshot array (`SYS_A11Y_SNAPSHOT`, 277).
/// `#[repr(C)]`, fixed-size (96 bytes), trivially `copy_to_user`-able. The tree
/// is an arena: `parent` references another node's `id` (`0` = the root desktop
/// node, which itself has `id = 0` semantics at the window tier). `name` is an
/// inline UTF-8 buffer (`name_len` valid bytes; the rest are `0`) — bounded and
/// copy-friendly, no trailing string blob to offset into. The role/state/action
/// tags are the `A11Y_ROLE_*` / `A11Y_STATE_*` / `A11Y_ACTIONBIT_*` constants,
/// chosen to map 1:1 onto `athui::accessibility`'s `AccessibilityRole`,
/// `AccessibilityTraits`, and `AccessibilityAction` so the kernel serializes
/// athui's live tree with no lossy remapping.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct A11yNode {
    /// Stable node id. At the window tier this equals the compositor
    /// `Surface.id`; widget-tier nodes get ids from the AthUI provider.
    pub id: u64,
    /// Parent node id (`0` = the root desktop node).
    pub parent: u64,
    /// One of the `A11Y_ROLE_*` tags.
    pub role: u32,
    /// `A11Y_STATE_*` bitfield (focused / visible / disabled / checked / …).
    pub state: u32,
    /// Bounds x (screen-space, pixels). Signed: a node may sit off the left/top.
    pub x: i32,
    /// Bounds y (screen-space, pixels).
    pub y: i32,
    /// Bounds width (pixels).
    pub w: u32,
    /// Bounds height (pixels).
    pub h: u32,
    /// `A11Y_ACTIONBIT_*` bitfield — which actions this node accepts.
    pub actions: u32,
    /// Valid UTF-8 byte count in `name` (`<= A11Y_NAME_LEN`).
    pub name_len: u32,
    /// Inline UTF-8 label, `name_len` valid bytes, zero-padded. Bounded at
    /// [`syscall::A11Y_NAME_LEN`] (48) — matches the compositor `Surface.title`.
    /// Trailing inline buffer rounds the struct to exactly 96 bytes (two u64 +
    /// eight u32 + 48-byte name), 8-aligned with a round per-node stride. A
    /// future field appends after `name` (the version field in the header gates
    /// it) without moving any existing field.
    pub name: [u8; 48],
}

impl A11yNode {
    /// Record size in bytes (the per-node stride in the snapshot array). 96 bytes.
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

// ════════════════════════════════════════════════════════════════════════════
// Resolved-search wire record. `SYS_SEARCH_QUERY_RESOLVED` (281) writes a
// sequence of these 24-byte `#[repr(C)]` headers, EACH immediately followed by
// `name_len` UTF-8 name bytes then `path_len` UTF-8 path bytes (variable-length
// — unlike the fixed `(id, kind)` 16-byte records of `SYS_SEARCH_QUERY` 56).
// The next record's header begins at `this_header + 24 + name_len + path_len`.
// A reader walks the buffer record-by-record, bounds-checking each length
// against the remaining capacity (see the athkit decoder). The const layout is
// the single contract both the kernel encoder and the athkit decoder quote;
// docs/SYSCALL_TABLE.md mirrors it byte-for-byte.
// ════════════════════════════════════════════════════════════════════════════

/// Per-record header written by `SYS_SEARCH_QUERY_RESOLVED` (281). `#[repr(C)]`,
/// fixed-size (24 bytes), trivially `copy_to_user`-able. The variable payload —
/// `name_len` UTF-8 name bytes then `path_len` UTF-8 path bytes — follows
/// immediately after this header in the same buffer. See
/// [`syscall::SYS_SEARCH_QUERY_RESOLVED`] for the exact byte layout and the
/// truncation/bounds rules.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchResolvedHeader {
    /// Stable index item id (mirrors `kernel::search_index` item id).
    pub id: u64,
    /// One of the `syscall::SEARCH_KIND_*` tags (App/File/Setting/…).
    pub kind: u32,
    /// `1` if the item is a directory, `0` for a file/other. The palette renders
    /// a folder vs document row from this.
    pub is_folder: u8,
    /// Reserved for future flags; `0` today.
    pub reserved0: u8,
    /// Reserved; `0` today. Keeps `name_len` naturally 2-aligned.
    pub reserved1: u16,
    /// UTF-8 byte length of the `name` payload that follows this header (clamped
    /// to [`syscall::SEARCH_RESOLVED_MAX_STR`]).
    pub name_len: u16,
    /// UTF-8 byte length of the `path` payload that follows `name` (clamped to
    /// [`syscall::SEARCH_RESOLVED_MAX_STR`]).
    pub path_len: u16,
    /// Reserved; `0` today. Rounds the header to a clean 24 bytes so a future
    /// field can be appended without moving any existing field.
    pub reserved2: u32,
}

impl SearchResolvedHeader {
    /// Header size in bytes (the per-record payload offset). 24 bytes.
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

// ── `SYS_SEARCH_QUERY_RESOLVED` / `SYS_SEARCH_QUERY` kind tags ──
// Mirror `kernel::search_index::Kind` (the `#[repr(u32)]` discriminants). Named
// on the ABI so the resolved-record `kind` field and the legacy 16-byte
// `(id, kind)` record share ONE source of truth. athkit re-exports the same
// values in its `sys` module (it deliberately doesn't depend on ath_abi).
/// `SearchResolvedHeader::kind` — an installed application.
pub const SEARCH_KIND_APP: u32 = 1;
/// `SearchResolvedHeader::kind` — a filesystem entry (file or folder).
pub const SEARCH_KIND_FILE: u32 = 2;
/// `SearchResolvedHeader::kind` — a Settings key.
pub const SEARCH_KIND_SETTING: u32 = 3;
/// `SearchResolvedHeader::kind` — a contact.
pub const SEARCH_KIND_CONTACT: u32 = 4;
/// `SearchResolvedHeader::kind` — a document.
pub const SEARCH_KIND_DOCUMENT: u32 = 5;
/// `SearchResolvedHeader::kind` — anything else.
pub const SEARCH_KIND_OTHER: u32 = 99;

/// The clipboard byte cap mirrored on the ABI so a userspace caller can size
/// its read buffer without a kernel round-trip. Matches
/// `kernel::clipboard::MAX_CLIPBOARD_BYTES`.
pub const MAX_CLIPBOARD_BYTES: u32 = 64 * 1024;

/// Compile-time self-check: ABI is internally consistent.
const _: () = {
    assert!(syscall::SYS_DRIVER_CLAIM_DEVICE == 111);
    assert!(syscall::SYS_INSTALL_RUN == 256);
    assert!(syscall::SYS_DEBUG_PRINT == 141);
    // SYS_DEBUG_PRINT must NOT collide with anything in the lower allocated
    // range; the v1→v2 reason for the move was 27 conflict with the
    // compositor's SYS_SURFACE_CLOSE. 142 is SYS_LINUXKPI_REQUEST_FIRMWARE
    // (additive); the reserved AthGFX range now starts at 143.
    assert!(syscall::SYS_LINUXKPI_REQUEST_FIRMWARE == 142);
    assert!(syscall::RESERVED_RAEGFX_LO == 143);
    // AthFS snapshot trio is additive in the unreserved 101-103 gap between
    // SYS_OOM_SUBSCRIBE (100) and the frozen driver range (109+).
    assert!(syscall::SYS_ATHFS_SNAPSHOT_CREATE == 101);
    assert!(syscall::SYS_ATHFS_SNAPSHOT_RESTORE == 102);
    assert!(syscall::SYS_ATHFS_SNAPSHOT_DELETE == 103);
    assert!(syscall::SYS_ATHFS_SNAPSHOT_DELETE < syscall::SYS_DRIVER_REGISTER);
    // SYS_THEME_GET is a fresh additive slot in the experimental range (264+);
    // 264 = SYS_NET_DNS, 265 = SYS_NET_STATUS, so 266 is the next free number.
    assert!(syscall::SYS_THEME_GET == 266);
    assert!(syscall::SYS_THEME_GET > syscall::SYS_NET_STATUS);
    // SYS_AUDIO_SUBMIT is the next fresh additive slot after SYS_THEME_GET.
    assert!(syscall::SYS_AUDIO_SUBMIT == 267);
    assert!(syscall::SYS_AUDIO_SUBMIT > syscall::SYS_THEME_GET);
    // Fixed format invariant: i16 stereo = 4 bytes/frame, so a frame_count's
    // byte length is frame_count * AUDIO_SUBMIT_BYTES_PER_FRAME.
    assert!(syscall::AUDIO_SUBMIT_BYTES_PER_FRAME == 4);
    assert!(syscall::AUDIO_SUBMIT_MAX_FRAMES == 512);
    // ThemeInfo is exactly 8 u32 = 32 bytes (round, stable, copy_to_user-friendly).
    assert!(core::mem::size_of::<ThemeInfo>() == 32);
    assert!(syscall::THEME_DEFAULT_ACCENT == 0xFF_4E_9C_FF);
    // Clipboard-history block (268-273) is a fresh additive run in the
    // experimental range, immediately after SYS_AUDIO_SUBMIT (267). Six
    // contiguous numbers, one op each — no magic op-mux.
    assert!(syscall::SYS_CLIP_HIST_COUNT == 268);
    assert!(syscall::SYS_CLIP_HIST_GET == 269);
    assert!(syscall::SYS_CLIP_HIST_PIN == 270);
    assert!(syscall::SYS_CLIP_HIST_DELETE == 271);
    assert!(syscall::SYS_CLIP_HIST_CLEAR == 272);
    assert!(syscall::SYS_CLIP_HIST_PROMOTE == 273);
    assert!(syscall::SYS_CLIP_HIST_COUNT > syscall::SYS_AUDIO_SUBMIT);
    // ClipEntryHeader is exactly 8 u32 = 32 bytes (round, stable header prefix).
    assert!(core::mem::size_of::<ClipEntryHeader>() == 32);
    // Screen capture (274-276) is a fresh additive block immediately after the
    // clipboard-history run (273), one op each — no magic op-mux. The new
    // Cap::ScreenCapture is a TAIL enum variant (flavor-tag-serialized), so it
    // is additive too — NO ABI_VERSION bump.
    assert!(syscall::SYS_CAPTURE_START == 274);
    assert!(syscall::SYS_CAPTURE_READ == 275);
    assert!(syscall::SYS_CAPTURE_STOP == 276);
    assert!(syscall::SYS_CAPTURE_START > syscall::SYS_CLIP_HIST_PROMOTE);
    // CaptureHeader is exactly 4 u32 = 16 bytes (small, stable frame prefix).
    assert!(core::mem::size_of::<CaptureHeader>() == 16);
    assert!(CaptureHeader::SIZE == 16);
    // Accessibility (277-278) is a fresh additive block immediately after the
    // screen-capture block (276), one op each — no magic op-mux. The new
    // Cap::Accessibility is a TAIL enum variant (flavor-tag-serialized, fresh
    // flavor 17), so it is additive too — NO ABI_VERSION bump.
    assert!(syscall::SYS_A11Y_SNAPSHOT == 277);
    assert!(syscall::SYS_A11Y_ACTION == 278);
    assert!(syscall::SYS_A11Y_SNAPSHOT > syscall::SYS_CAPTURE_STOP);
    // A11ySnapshotHeader is exactly 16 bytes (2 u32 + 1 u64), 8-aligned.
    assert!(core::mem::size_of::<A11ySnapshotHeader>() == 16);
    assert!(A11ySnapshotHeader::SIZE == 16);
    // A11yNode is exactly 96 bytes (2 u64 + 6 u32 + 48-byte name + 1 reserved
    // u32), 8-aligned and a round per-node stride. Inline name like ThemeInfo /
    // Surface.title — no trailing string blob to offset into.
    assert!(core::mem::size_of::<A11yNode>() == 96);
    assert!(A11yNode::SIZE == 96);
    assert!(syscall::A11Y_NAME_LEN == 48);
    // Live surface origin (280) is the next free number after the absolute cursor
    // poll (279), a fresh additive slot — NO ABI_VERSION bump. Same x|y<<16 packing
    // as SYS_INPUT_CURSOR; the unpack helper round-trips disjoint 16-bit lanes.
    assert!(syscall::SYS_SURFACE_ORIGIN == 280);
    assert!(syscall::SYS_SURFACE_ORIGIN == syscall::SYS_INPUT_CURSOR + 1);
    assert!(syscall::SURFACE_ORIGIN_ERR == u64::MAX);
    // x|y<<16 packing keeps the two coordinates in disjoint 16-bit lanes.
    let packed_origin: u64 = (1280u64 & 0xFFFF) | (720u64 << 16);
    assert!((packed_origin & 0xFFFF) == 1280);
    assert!(((packed_origin >> 16) & 0xFFFF) == 720);
    // Resolved search query (281) — the next free number after SYS_SURFACE_ORIGIN
    // (280). Additive variable-length surface (named hits for the Files app /
    // command palette); ABI_VERSION bumped to v3 as a courtesy marker for the new
    // wire record (no existing field or signature moved).
    assert!(syscall::SYS_SEARCH_QUERY_RESOLVED == 281);
    assert!(syscall::SYS_SEARCH_QUERY_RESOLVED == syscall::SYS_SURFACE_ORIGIN + 1);
    // The header is exactly 24 bytes (1 u64 + 1 u32 + 2 u8 + 3 u16 + 1 u32),
    // 8-aligned, and SEARCH_RESOLVED_HEADER_SIZE matches it — the kernel encoder
    // and athkit decoder both quote this single number.
    assert!(core::mem::size_of::<SearchResolvedHeader>() == 24);
    assert!(SearchResolvedHeader::SIZE == 24);
    assert!(syscall::SEARCH_RESOLVED_HEADER_SIZE == 24);
    // Kind tags mirror kernel::search_index::Kind discriminants.
    assert!(SEARCH_KIND_APP == 1);
    assert!(SEARCH_KIND_FILE == 2);
    assert!(SEARCH_KIND_DOCUMENT == 5);
    assert!(SEARCH_KIND_OTHER == 99);
    // Block 33: AthBridge real-MSVC-CRT ABI (282–283). Contiguous, additive,
    // no ABI_VERSION bump.
    assert!(syscall::SYS_SET_GS_BASE == 282);
    assert!(syscall::SYS_SET_GS_BASE == syscall::SYS_SEARCH_QUERY_RESOLVED + 1);
    assert!(syscall::SYS_MPROTECT == 283);
    assert!(syscall::SYS_MPROTECT == syscall::SYS_SET_GS_BASE + 1);
    // PROT bits: R|W == 3 (the existing mmap call-site convention), and the
    // three permission bits are disjoint.
    assert!(syscall::PROT_READ == 1);
    assert!(syscall::PROT_WRITE == 2);
    assert!(syscall::PROT_EXEC == 4);
    assert!(syscall::PROT_READ | syscall::PROT_WRITE == 3);
    // Block 34: Anti-cheat attestation (284–290). Contiguous, and — critically —
    // ABOVE the OOM/AthFS-snapshot numbers (100–103) that previously shadowed
    // them in the dispatch. This guard fails the build if anti-cheat ever drifts
    // back onto a colliding number.
    assert!(syscall::SYS_AC_REQUEST_ATTESTATION == 284);
    assert!(syscall::SYS_AC_REQUEST_ATTESTATION == syscall::SYS_MPROTECT + 1);
    assert!(syscall::SYS_AC_HEARTBEAT == 290);
    assert!(syscall::SYS_AC_HEARTBEAT == syscall::SYS_AC_REQUEST_ATTESTATION + 6);
    // The whole anti-cheat block must clear the live OOM(100)/AthFS-snapshot
    // (101–103) numbers it was rescued from (the original collision).
    assert!(syscall::SYS_AC_REQUEST_ATTESTATION > 106);
    // Block 35: Surface resize protocol (291–292). Contiguous, additive, no
    // ABI_VERSION bump — sits just above the anti-cheat block.
    assert!(syscall::SYS_SURFACE_RESIZE_REQ == 291);
    assert!(syscall::SYS_SURFACE_RESIZE_REQ == syscall::SYS_AC_HEARTBEAT + 1);
    assert!(syscall::SYS_SURFACE_RESIZE == 292);
    assert!(syscall::SYS_SURFACE_RESIZE == syscall::SYS_SURFACE_RESIZE_REQ + 1);
    // The "no resize pending" sentinel is 0 and must differ from the error
    // sentinel; a real request always packs a non-zero width.
    assert!(syscall::SURFACE_RESIZE_NONE == 0);
    assert!(syscall::SURFACE_RESIZE_ERR == u64::MAX);
    assert!(ABI_VERSION == 4);
};
