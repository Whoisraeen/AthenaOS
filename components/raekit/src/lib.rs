//! AthKit — application development SDK for AthenaOS.
//!
//! Declarative, Rust-first. SwiftUI-style ergonomics without the Apple lock-in.
//!
//! # Modules
//!
//! - [`view`] — `ViewNode` enum, `View` trait, `Color`, `TextStyle`, supporting types
//! - [`builders`] — SwiftUI-style method-chaining builders (`Text`, `VStack`, etc.)
//! - [`state`] — Reactive state management (`State`, `Binding`, `ObservableObject`)
//! - [`app`] — Application lifecycle (`RaeApp` trait, `AppRunner`, `AppEvent`)
//! - [`nav`] — Navigation (`NavigationStack`, `TabViewNav`, `Router`)
//! - [`syscalls`] — High-level wrappers over raw syscalls (fs, surface, process, cap)
//! - [`sys`] — Raw syscall primitives (low-level, prefer `syscalls::*` in app code)
//! - [`ipc`] — IPC channel helpers
//!
//! # Quick Start
//!
//! ```ignore
//! use raekit::prelude::*;
//!
//! struct MyApp {
//!     counter: State<i64>,
//! }
//!
//! impl RaeApp for MyApp {
//!     fn name(&self) -> &str { "My App" }
//!
//!     fn on_launch(&mut self) -> ViewNode {
//!         VStack::new()
//!             .spacing(12.0)
//!             .child(Text::new("Hello, AthenaOS!").font_size(24.0).bold())
//!             .child(ButtonBuilder::new("Click me").action(1))
//!             .padding(16.0)
//!             .build()
//!     }
//!
//!     fn on_event(&mut self, event: &AppEvent) -> Option<ViewNode> {
//!         match event {
//!             AppEvent::Action { id: 1 } => {
//!                 self.counter.modify(|v| *v += 1);
//!                 Some(Text::new("Clicked!").build())
//!             }
//!             _ => None,
//!         }
//!     }
//! }
//! ```

// `no_std` for the real userspace target; under `cargo test` OR the `host`
// feature we build against the host std so the consumer (the test harness, or
// the `tools/ui_screenshot` renderer linking `apps/files` -> `raekit`) gets
// std's panic handler / allocator. The PURE wire decoders
// (`syscalls::search::decode_*`) plus the syscall-free draw seams are the only
// things exercised on the host — no syscall is issued there.
#![cfg_attr(not(any(test, feature = "host")), no_std)]

extern crate alloc;

#[cfg(not(any(test, feature = "host")))]
use core::alloc::{GlobalAlloc, Layout};
#[cfg(not(any(test, feature = "host")))]
use core::ptr::null_mut;

// ── Submodules ───────────────────────────────────────────────────────────

pub mod app;
pub mod builders;
pub mod layout;
pub mod nav;
pub mod state;
pub mod syscalls;
pub mod view;

// ── Macros ───────────────────────────────────────────────────────────────

#[macro_export]
macro_rules! view {
    // Recursive case for Stack with children
    (VStack { $($child:tt)* }) => {
        $crate::builders::VStack::new()
            $(.child($crate::view!($child)))*
            .build()
    };
    (HStack { $($child:tt)* }) => {
        $crate::builders::HStack::new()
            $(.child($crate::view!($child)))*
            .build()
    };
    (ZStack { $($child:tt)* }) => {
        $crate::builders::ZStackBuilder::new()
            $(.child($crate::view!($child)))*
            .build()
    };

    // Terminals
    (Text($content:expr)) => {
        $crate::builders::Text::new($content).build()
    };
    (Button($label:expr) { $action:expr }) => {
        $crate::builders::ButtonBuilder::new($label).action($action).build()
    };
    (Spacer()) => {
        $crate::builders::Spacer::new()
    };
    (Divider()) => {
        $crate::builders::Divider::new()
    };

    // Catch-all for expressions that are already ViewNodes
    ($e:expr) => {
        $e
    };
}

// ── R10 Artifacts ────────────────────────────────────────────────────────

pub fn run_boot_smoketest() -> bool {
    // Test basic tree construction via builders (avoiding macro issues for now)
    let _tree = crate::builders::VStack::new()
        .child(crate::builders::Text::new("AthKit Test").build())
        .build();

    // Test layout engine
    let constraints = crate::layout::Constraints::loose(800.0, 600.0);
    let layout = crate::layout::compute_layout(&_tree, &constraints);

    let layout_ok = layout.frame.width > 0.0;

    // Test state and binding
    let mut state = crate::state::State::new(42);
    let binding = state.binding();
    binding.set(100);
    let state_ok = state.get() == &100 && state.generation() == 1;

    layout_ok && state_ok
}

// ── Prelude ──────────────────────────────────────────────────────────────

/// Convenience re-exports for app development. `use raekit::prelude::*;`
/// brings in everything needed to build a typical AthKit app.
pub mod prelude {
    pub use crate::app::{AppDescriptor, AppEvent, AppRunner, LifecycleState, MouseButton, RaeApp};
    pub use crate::builders::{
        if_view, ButtonBuilder, Divider, HStack, ImageBuilder, ListBuilder, NavigationViewBuilder,
        PaddingBuilder, RectBuilder, ScrollViewBuilder, SheetBuilder, SliderBuilder, Spacer, Text,
        TextFieldBuilder, ToggleBuilder, VStack, ZStackBuilder,
    };
    pub use crate::nav::{NavigationStack, Router, TabViewNav};
    pub use crate::state::{Binding, Environment, Float64, ObservableObject, State, StateValue};
    pub use crate::view::{
        Alignment, ButtonVariant, Color, Edges, FontWeight, ImageFit, ImageSource, ScrollDirection,
        StackDirection, TextStyle, View, ViewNode,
    };
}

// ── Syscall numbers (must match kernel/src/syscall.rs dispatch table) ────

pub mod sys {
    pub const SYS_PRINT: u64 = 1;
    pub const SYS_SEND: u64 = 2;
    pub const SYS_RECV: u64 = 3;
    pub const SYS_CAP_GRANT: u64 = 4;
    pub const SYS_CAP_REVOKE: u64 = 5;
    pub const SYS_CAP_QUERY: u64 = 6;
    pub const SYS_MMIO_MAP: u64 = 7;
    pub const SYS_IRQ_WAIT: u64 = 8;
    pub const SYS_PORT_READ: u64 = 9;
    pub const SYS_PORT_WRITE: u64 = 10;
    pub const SYS_SPAWN: u64 = 11;
    pub const SYS_EXIT: u64 = 12;
    pub const SYS_WAIT: u64 = 13;
    pub const SYS_KILL: u64 = 14;
    pub const SYS_OPEN: u64 = 15;
    pub const SYS_READ: u64 = 16;
    pub const SYS_WRITE: u64 = 17;
    pub const SYS_CLOSE: u64 = 18;
    pub const SYS_MMAP: u64 = 19;
    pub const SYS_MUNMAP: u64 = 20;
    pub const SYS_SETPRIORITY: u64 = 21;
    pub const SYS_SEEK: u64 = 22;
    pub const SYS_STAT: u64 = 23;
    pub const SYS_SURFACE_CREATE: u64 = 24;
    pub const SYS_SURFACE_PRESENT: u64 = 25;
    pub const SYS_SURFACE_FOCUS: u64 = 26;
    pub const SYS_SURFACE_CLOSE: u64 = 27;
    pub const SYS_YIELD: u64 = 28;
    pub const SYS_GETPID: u64 = 29;
    pub const SYS_TIME: u64 = 30;
    pub const SYS_READ_KEY: u64 = 31;
    pub const SYS_POLL_MOUSE: u64 = 32;
    pub const SYS_READDIR: u64 = 33;
    pub const SYS_SCREEN_INFO: u64 = 34;
    pub const SYS_PTY_OPEN: u64 = 35;
    pub const SYS_PTY_READ: u64 = 36;
    pub const SYS_PTY_WRITE: u64 = 37;
    pub const SYS_PTY_POLL: u64 = 38;
    pub const SYS_PTY_SLAVE_IO: u64 = 39;
    pub const SYS_SESSION_LOGIN: u64 = 88;
    pub const SYS_SESSION_GUEST: u64 = 89;
    pub const SYS_SESSION_LOCK: u64 = 90;
    pub const SYS_SESSION_UNLOCK: u64 = 91;
    pub const SYS_SESSION_INFO: u64 = 92;
    pub const SYS_SESSION_LOGOUT: u64 = 93;
    pub const SYS_PROCLIST: u64 = 94;
    pub const SYS_READDIR_AT: u64 = 95;
    /// Create a directory (rae_abi SYS_MKDIR). rdi=path_ptr, rsi=path_len, rdx=mode.
    pub const SYS_MKDIR: u64 = 96;
    /// Remove a file/empty dir (rae_abi SYS_UNLINK). rdi=path_ptr, rsi=path_len.
    pub const SYS_UNLINK: u64 = 97;
    /// Move/rename (rae_abi SYS_RENAME). rdi=old_ptr, rsi=old_len, rdx=new_ptr, r10=new_len.
    pub const SYS_RENAME: u64 = 98;
    pub const SYS_CONFIG_GET: u64 = 50;
    pub const SYS_CONFIG_SET: u64 = 51;
    // ── Rae scripting (rae_abi scripting block, SYSCALL_TABLE.md Block 14) ──
    /// Run a Rae script. rdi=src_ptr, rsi=src_len, rdx=cap_mask -> script id.
    pub const SYS_SCRIPT_RUN: u64 = 78;
    /// Script status. rdi=id, rsi=out_ptr, rdx=out_cap -> bytes written
    /// (ScriptAbi, 56 B; with a larger buffer the captured print output
    /// follows the struct).
    pub const SYS_SCRIPT_STATUS: u64 = 79;
    // ── Local search index (rae_abi search block 54-57, docs/SYSCALL_TABLE.md) ──
    /// Add an item to the kernel search index. rdi=display_ptr, rsi=display_len,
    /// rdx=kind (SEARCH_KIND_*) -> item id / u64::MAX.
    pub const SYS_SEARCH_ADD: u64 = 54;
    /// Remove an item by id. rdi=item_id -> 0 / u64::MAX.
    pub const SYS_SEARCH_REMOVE: u64 = 55;
    /// Query the index. rdi=q_ptr, rsi=q_len, rdx=out_ptr, r10=out_cap_bytes ->
    /// number of results written. Each result is 16 bytes: `[u64 id][u32 kind][u32 pad]`.
    pub const SYS_SEARCH_QUERY: u64 = 56;
    /// Index stats. rdi=out_ptr (>=32 B: items,tokens,queries,last_cycles as u64×4),
    /// rsi=out_cap -> bytes written (32) / 0.
    pub const SYS_SEARCH_STATS: u64 = 57;
    /// `Kind` tags for `SYS_SEARCH_ADD` / decoded from `SYS_SEARCH_QUERY` results
    /// (mirror `kernel::search_index::Kind`).
    pub const SEARCH_KIND_APP: u32 = 1;
    pub const SEARCH_KIND_FILE: u32 = 2;
    pub const SEARCH_KIND_SETTING: u32 = 3;
    pub const SEARCH_KIND_CONTACT: u32 = 4;
    pub const SEARCH_KIND_DOCUMENT: u32 = 5;
    pub const SEARCH_KIND_OTHER: u32 = 99;
    /// Bytes per `SYS_SEARCH_QUERY` result record (`[u64 id][u32 kind][u32 pad]`).
    pub const SEARCH_RESULT_STRIDE: usize = 16;
    /// Resolved search query (rae_abi::syscall::SYS_SEARCH_QUERY_RESOLVED, 281) —
    /// returns NAMED hits (name + path), not just `(id, kind)`. rdi=q_ptr,
    /// rsi=q_len, rdx=out_ptr, r10=out_cap_bytes -> record count. Each record is
    /// a `SEARCH_RESOLVED_HEADER_SIZE` (24)-byte header followed by `name_len`
    /// UTF-8 name bytes then `path_len` UTF-8 path bytes (see
    /// `raekit::sys_calls::search::query_resolved` / docs/SYSCALL_TABLE.md).
    pub const SYS_SEARCH_QUERY_RESOLVED: u64 = 281;
    /// Fixed per-record header size for `SYS_SEARCH_QUERY_RESOLVED` (mirrors
    /// `rae_abi::syscall::SEARCH_RESOLVED_HEADER_SIZE`). Header layout (LE):
    /// `[u64 id][u32 kind][u8 is_folder][u8 r0][u16 r1][u16 name_len][u16 path_len][u32 r2]`.
    pub const SEARCH_RESOLVED_HEADER_SIZE: usize = 24;
    // ── Userspace net block (rae_abi net 121-125 + DNS 264) ──
    /// Create a socket. rdi=proto (0=TCP, 1=UDP) -> fd / u64::MAX.
    pub const SYS_NET_SOCKET: u64 = 121;
    /// Connect TCP. rdi=fd, rsi=ip (packed BE u32), rdx=port -> 0 / u64::MAX.
    pub const SYS_NET_CONNECT: u64 = 122;
    /// Send. rdi=fd, rsi=buf_ptr, rdx=len -> bytes sent / u64::MAX.
    pub const SYS_NET_SEND: u64 = 123;
    /// Receive. rdi=fd, rsi=buf_ptr, rdx=cap -> bytes (0=none) / u64::MAX.
    pub const SYS_NET_RECV: u64 = 124;
    /// Close. rdi=fd -> 0 / u64::MAX.
    pub const SYS_NET_CLOSE: u64 = 125;
    /// Resolve a hostname. rdi=name ptr, rsi=name len (<256) -> IPv4 packed
    /// BE u32 (octet[0] in high byte) / u64::MAX.
    pub const SYS_NET_DNS: u64 = 264;
    /// Live theme read (rae_abi::syscall::SYS_THEME_GET). rdi=out ptr, rsi=cap.
    pub const SYS_THEME_GET: u64 = 266;
    /// Feed PCM into the AthAudio mixer (rae_abi::syscall::SYS_AUDIO_SUBMIT).
    /// rdi=samples ptr (*const i16), rsi=frame_count, rdx=format_flags.
    pub const SYS_AUDIO_SUBMIT: u64 = 267;

    // ── Clipboard history (rae_abi::syscall::SYS_CLIP_HIST_*) ──
    /// History entry count -> `count | (pinned_count << 32)`.
    pub const SYS_CLIP_HIST_COUNT: u64 = 268;
    /// Read entry: rdi=index (0=newest), rsi=out ptr, rdx=out cap.
    pub const SYS_CLIP_HIST_GET: u64 = 269;
    /// Pin/unpin: rdi=index, rsi=1 pin / 0 unpin.
    pub const SYS_CLIP_HIST_PIN: u64 = 270;
    /// Delete entry: rdi=index (refuses pinned).
    pub const SYS_CLIP_HIST_DELETE: u64 = 271;
    /// Clear unpinned, keep pinned -> # removed.
    pub const SYS_CLIP_HIST_CLEAR: u64 = 272;
    /// Promote entry to active clipboard: rdi=index.
    pub const SYS_CLIP_HIST_PROMOTE: u64 = 273;

    // ── Screen capture (rae_abi::syscall::SYS_CAPTURE_*, 274-276) ──
    /// Start a capture session: rdi=region_xy (x|y<<32), rsi=region_wh (w|h<<32),
    /// rdx=format (CAPTURE_FMT_*), r10=flags (CAPTURE_FLAG_*) -> capture_id.
    pub const SYS_CAPTURE_START: u64 = 274;
    /// Read a frame: rdi=capture_id, rsi=out ptr, rdx=out cap -> bytes written.
    pub const SYS_CAPTURE_READ: u64 = 275;
    /// Stop + free a session: rdi=capture_id -> 0.
    pub const SYS_CAPTURE_STOP: u64 = 276;
    /// Screen-capture failure sentinel (mirrors `rae_abi::syscall::CAPTURE_ERR`).
    pub const CAPTURE_ERR: u64 = u64::MAX;

    // ── Accessibility tree (rae_abi::syscall::SYS_A11Y_*, 277-278) ──
    /// Snapshot the a11y tree: rdi=out ptr, rsi=out cap -> bytes written
    /// (A11ySnapshotHeader + A11yNode array). Requires `Cap::Accessibility{READ}`.
    pub const SYS_A11Y_SNAPSHOT: u64 = 277;
    /// Dispatch a node action: rdi=node_id, rsi=action (A11Y_ACTION_*), rdx=arg
    /// -> 0 / A11Y_ERR. Requires `Cap::Accessibility{WRITE}`.
    pub const SYS_A11Y_ACTION: u64 = 278;
    /// Accessibility-syscall failure sentinel (mirrors `rae_abi::syscall::A11Y_ERR`).
    pub const A11Y_ERR: u64 = u64::MAX;
    /// `A11yNodeInfo::role` tags (mirror `rae_abi::syscall::A11Y_ROLE_*`).
    pub const A11Y_ROLE_DESKTOP: u32 = 1;
    pub const A11Y_ROLE_WINDOW: u32 = 2;
    pub const A11Y_ROLE_BUTTON: u32 = 3;
    /// `A11yNodeInfo::state` bits (mirror `rae_abi::syscall::A11Y_STATE_*`).
    pub const A11Y_STATE_FOCUSED: u32 = 1 << 0;
    pub const A11Y_STATE_VISIBLE: u32 = 1 << 1;
    pub const A11Y_STATE_MINIMIZED: u32 = 1 << 6;
    /// `A11yNodeInfo::actions` bits (mirror `rae_abi::syscall::A11Y_ACTIONBIT_*`).
    pub const A11Y_ACTIONBIT_FOCUS: u32 = 1 << 0;
    pub const A11Y_ACTIONBIT_ACTIVATE: u32 = 1 << 1;
    /// `SYS_A11Y_ACTION` action selectors (mirror `rae_abi::syscall::A11Y_ACTION_*`).
    pub const A11Y_ACTION_FOCUS: u64 = 0;
    pub const A11Y_ACTION_ACTIVATE: u64 = 1;
    pub const A11Y_ACTION_SCROLL: u64 = 2;
    pub const A11Y_ACTION_SET_VALUE: u64 = 3;
    pub const A11Y_ACTION_DISMISS: u64 = 6;
    /// Pixel-format tags (mirror `rae_abi::syscall::CAPTURE_FMT_*`).
    pub const CAPTURE_FMT_ARGB32: u32 = 0;
    pub const CAPTURE_FMT_BGRA32: u32 = 1;
    /// `SYS_CAPTURE_START` flags (mirror `rae_abi::syscall::CAPTURE_FLAG_*`).
    pub const CAPTURE_FLAG_CONTINUOUS: u64 = 1 << 0;

    // ── Absolute cursor position (rae_abi::syscall::SYS_INPUT_CURSOR, 279) ──
    /// Poll the ABSOLUTE cursor position for hit-testing -> `x | (y << 16)`.
    /// No args, no capability, allowed everywhere. Bits [63:32] reserved (0).
    pub const SYS_INPUT_CURSOR: u64 = 279;

    // ── Live surface origin (rae_abi::syscall::SYS_SURFACE_ORIGIN, 280) ──
    /// Query a surface's CURRENT absolute origin -> `x | (y << 16)`, or
    /// `SURFACE_ORIGIN_ERR` for an unknown id. `rdi` = surface id. No capability,
    /// allowed everywhere. Lets a mouse-first app subtract its LIVE window origin
    /// (not the stale one it presented at) when hit-testing the absolute cursor.
    pub const SYS_SURFACE_ORIGIN: u64 = 280;
    /// `SYS_SURFACE_ORIGIN` failure sentinel (mirrors
    /// `rae_abi::syscall::SURFACE_ORIGIN_ERR`).
    pub const SURFACE_ORIGIN_ERR: u64 = u64::MAX;

    /// Clipboard-history failure sentinel (mirrors `rae_abi::syscall::CLIP_ERR`).
    pub const CLIP_ERR: u64 = u64::MAX;
    /// `ClipEntry::format` tags (mirror `rae_abi::syscall::CLIP_FMT_*`).
    pub const CLIP_FMT_TEXT: u32 = 0;
    pub const CLIP_FMT_RICH_TEXT: u32 = 1;
    pub const CLIP_FMT_IMAGE: u32 = 2;
    pub const CLIP_FMT_FILES: u32 = 3;
    pub const CLIP_FMT_URL: u32 = 4;
    pub const CLIP_FMT_COLOR: u32 = 5;
    /// `ClipEntry::flags` bit: the entry is pinned.
    pub const CLIP_FLAG_PINNED: u32 = 1 << 0;

    /// RaeBlue — the fallback accent when `SYS_THEME_GET` is unavailable.
    /// Mirrors `rae_abi::THEME_DEFAULT_ACCENT` / `rae_tokens::RAEBLUE` (raekit
    /// deliberately does not depend on rae_abi to keep the app crate graph thin).
    pub const THEME_DEFAULT_ACCENT: u32 = 0xFF_4E_9C_FF;

    /// Live desktop theme, mirroring `rae_abi::ThemeInfo` (32 bytes, `#[repr(C)]`,
    /// all `u32`). Read at app launch so a separate-process app re-skins to the
    /// active Vibe-Mode theme.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ThemeInfo {
        pub version: u32,
        pub accent_argb: u32,
        pub bg_argb: u32,
        pub fg_argb: u32,
        pub is_dark: u32,
        pub blur_radius: u32,
        pub palette_id: u32,
        pub reserved: u32,
    }

    /// Read the LIVE desktop theme via `SYS_THEME_GET` (266). Returns `None` on
    /// any error (syscall unsupported, buffer rejected) so the caller falls back
    /// to [`THEME_DEFAULT_ACCENT`]. Apps call this ONCE at launch.
    pub fn theme_get() -> Option<ThemeInfo> {
        let mut info = ThemeInfo {
            version: 0,
            accent_argb: 0,
            bg_argb: 0,
            fg_argb: 0,
            is_dark: 0,
            blur_radius: 0,
            palette_id: 0,
            reserved: 0,
        };
        let n = unsafe {
            syscall2(
                SYS_THEME_GET,
                &mut info as *mut ThemeInfo as u64,
                core::mem::size_of::<ThemeInfo>() as u64,
            )
        };
        if n == core::mem::size_of::<ThemeInfo>() as u64 {
            Some(info)
        } else {
            None
        }
    }

    /// The live accent (ARGB) the desktop is using right now, or
    /// [`THEME_DEFAULT_ACCENT`] (RaeBlue) when the theme syscall is unavailable.
    /// This is the seed an app feeds into `rae_tokens::derive_accent`.
    pub fn theme_accent() -> u32 {
        match theme_get() {
            Some(info) if info.accent_argb != 0 => info.accent_argb,
            _ => THEME_DEFAULT_ACCENT,
        }
    }

    /// Feed interleaved 48 kHz i16-stereo PCM into the AthAudio mixer via
    /// `SYS_AUDIO_SUBMIT` (267) — the app → mixer → ring → HDA path. `samples`
    /// is interleaved L,R pairs, so `samples.len()` must be even; a trailing
    /// odd sample is dropped from the frame count. Returns the number of frames
    /// (L+R pairs) the mixer accepted — call again with the remainder when it
    /// is less than `samples.len() / 2`. Returns `0` on any error (audio
    /// unavailable, buffer rejected, oversized chunk: submit at most
    /// `512` frames per call). `format_flags` is reserved (passed as 0).
    pub fn audio_submit(samples: &[i16]) -> u64 {
        let frames = (samples.len() / 2) as u64;
        if frames == 0 {
            return 0;
        }
        let r = unsafe { syscall3(SYS_AUDIO_SUBMIT, samples.as_ptr() as u64, frames, 0) };
        if r == u64::MAX {
            0
        } else {
            r
        }
    }

    // ── Clipboard history (Win+V-class panel surface) ─────────────────────────
    // The clipboard-history panel uses these to render past copies, pin keepers,
    // delete, clear-keeping-pinned, and paste-on-select. History is session-wide
    // and RAM-only (local by default — the Concept's ownership posture).

    /// One history entry returned by [`clip_hist_get`]: the `rae_abi::ClipEntryHeader`
    /// fields plus its owned UTF-8 text payload.
    #[derive(Debug, Clone)]
    pub struct ClipEntry {
        /// One of `CLIP_FMT_*` (`CLIP_FMT_TEXT` today; the rest reserved).
        pub format: u32,
        /// `CLIP_FLAG_*` bits (`CLIP_FLAG_PINNED`).
        pub flags: u32,
        /// Monotonic copy sequence (larger = newer); `0` if unknown.
        pub sequence: u32,
        /// Times this entry has been promoted to the active clipboard.
        pub paste_count: u32,
        /// The entry's UTF-8 text (already capped at 64 KiB kernel-side).
        pub text: alloc::string::String,
    }

    impl ClipEntry {
        /// True if the entry is pinned (exempt from eviction + clear).
        pub fn is_pinned(&self) -> bool {
            self.flags & CLIP_FLAG_PINNED != 0
        }
    }

    /// Clipboard-history counts: `(total_entries, pinned_entries)`.
    /// Wraps `SYS_CLIP_HIST_COUNT` (268).
    pub fn clip_hist_count() -> (u32, u32) {
        let r = unsafe { syscall0(SYS_CLIP_HIST_COUNT) };
        ((r & 0xFFFF_FFFF) as u32, (r >> 32) as u32)
    }

    /// Read history entry `index` (0 = newest) via `SYS_CLIP_HIST_GET` (269).
    /// Returns `None` if the index is out of range. The header (32 bytes) is
    /// parsed off the front of the kernel-written buffer; the remaining
    /// `byte_len` bytes are the UTF-8 text. Caller-agnostic buffer sizing: a
    /// 64 KiB + 32 byte scratch covers any single entry (`MAX_CLIPBOARD_BYTES`).
    pub fn clip_hist_get(index: u32) -> Option<ClipEntry> {
        const HDR: usize = 32; // size_of::<rae_abi::ClipEntryHeader>()
        let mut buf = alloc::vec![0u8; HDR + 64 * 1024];
        let n = unsafe {
            syscall3(
                SYS_CLIP_HIST_GET,
                index as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == CLIP_ERR || (n as usize) < HDR {
            return None;
        }
        let n = n as usize;
        // Header layout (all little-endian u32): version, format, flags,
        // byte_len, sequence, paste_count, reserved0, reserved1.
        let u32_at = |b: &[u8], off: usize| -> u32 {
            u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
        };
        let format = u32_at(&buf, 4);
        let flags = u32_at(&buf, 8);
        let byte_len = u32_at(&buf, 12) as usize;
        let sequence = u32_at(&buf, 16);
        let paste_count = u32_at(&buf, 20);
        let end = (HDR + byte_len).min(n);
        let text = alloc::string::String::from_utf8_lossy(&buf[HDR..end]).into_owned();
        Some(ClipEntry {
            format,
            flags,
            sequence,
            paste_count,
            text,
        })
    }

    /// Pin (`pinned = true`) or unpin a history entry. `SYS_CLIP_HIST_PIN` (270).
    /// Returns `true` on success, `false` if the index is out of range.
    pub fn clip_hist_pin(index: u32, pinned: bool) -> bool {
        let r = unsafe { syscall2(SYS_CLIP_HIST_PIN, index as u64, pinned as u64) };
        r != CLIP_ERR
    }

    /// Delete history entry `index`. `SYS_CLIP_HIST_DELETE` (271). Returns
    /// `false` if the index is out of range OR the entry is PINNED (unpin
    /// first — mirrors the panel's pinned-delete guard).
    pub fn clip_hist_delete(index: u32) -> bool {
        let r = unsafe { syscall1(SYS_CLIP_HIST_DELETE, index as u64) };
        r != CLIP_ERR
    }

    /// Clear history, KEEPING pinned entries (Win+V "Clear all").
    /// `SYS_CLIP_HIST_CLEAR` (272). Returns the number of entries removed.
    pub fn clip_hist_clear() -> u64 {
        unsafe { syscall0(SYS_CLIP_HIST_CLEAR) }
    }

    /// Promote history entry `index` to the ACTIVE clipboard (paste-on-select):
    /// a following [`clipboard_get`]-equivalent returns this entry's content.
    /// `SYS_CLIP_HIST_PROMOTE` (273). Returns `false` if out of range.
    pub fn clip_hist_promote(index: u32) -> bool {
        let r = unsafe { syscall1(SYS_CLIP_HIST_PROMOTE, index as u64) };
        r != CLIP_ERR
    }

    // ── Screen capture (screenshot tool + Game Bar overlay) ───────────────────
    // Wraps the kernel compositor's capture engine via SYS_CAPTURE_* (274-276).
    // PRIVACY-GATED: the calling task must hold `Cap::ScreenCapture` (the
    // screenshot tool / Game Bar are seeded it) and START is refused in safe
    // mode; every fn returns None/false when the cap is missing.

    /// A captured frame: dimensions + owned ARGB/BGRA pixel buffer (one `u32`
    /// per pixel, row-major). `format` is one of `CAPTURE_FMT_*`.
    #[derive(Debug, Clone)]
    pub struct CapturedImage {
        pub w: u32,
        pub h: u32,
        pub format: u32,
        pub pixels: alloc::vec::Vec<u32>,
    }

    /// Start a screen-capture session over the region `(x, y, w, h)` in
    /// `format` (`CAPTURE_FMT_ARGB32`/`BGRA32`). `continuous = true` keeps the
    /// session live across frames (Game Bar / recording — read repeatedly);
    /// `false` captures a single frame (a screenshot). Returns the `capture_id`
    /// to pass to [`capture_read`] / [`capture_stop`], or `None` if the caller
    /// lacks `Cap::ScreenCapture`, the system is in safe mode, or the region is
    /// degenerate. Wraps `SYS_CAPTURE_START` (274).
    pub fn capture_start(
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        format: u32,
        continuous: bool,
    ) -> Option<u64> {
        let region_xy = (x as u64) | ((y as u64) << 32);
        let region_wh = (w as u64) | ((h as u64) << 32);
        let flags = if continuous {
            CAPTURE_FLAG_CONTINUOUS
        } else {
            0
        };
        let r = unsafe {
            syscall4(
                SYS_CAPTURE_START,
                region_xy,
                region_wh,
                format as u64,
                flags,
            )
        };
        if r == CAPTURE_ERR {
            None
        } else {
            Some(r)
        }
    }

    /// Read the latest frame of capture session `id` via `SYS_CAPTURE_READ`
    /// (275). The kernel writes a 16-byte header (`width, height, format,
    /// bytes`) followed by the pixels; this parses both into a [`CapturedImage`].
    /// Returns `None` if the id is unknown, the caller lacks the cap, or the
    /// scratch buffer was too small.
    pub fn capture_read(id: u64) -> Option<CapturedImage> {
        const HDR: usize = 16; // size_of::<rae_abi::CaptureHeader>()
                               // First read the header to learn the payload size, sizing the buffer
                               // generously for a common screenshot region. A degenerate caller can
                               // call again with a larger buffer; we size for up to 4K here.
        let cap_bytes = HDR + 3840 * 2160 * 4;
        let mut buf = alloc::vec![0u8; cap_bytes];
        let n = unsafe {
            syscall3(
                SYS_CAPTURE_READ,
                id,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == CAPTURE_ERR || (n as usize) < HDR {
            return None;
        }
        let n = n as usize;
        let u32_at = |b: &[u8], off: usize| -> u32 {
            u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
        };
        let w = u32_at(&buf, 0);
        let h = u32_at(&buf, 4);
        let format = u32_at(&buf, 8);
        let bytes = u32_at(&buf, 12) as usize;
        let end = (HDR + bytes).min(n);
        let px_count = (end - HDR) / 4;
        let mut pixels = alloc::vec::Vec::with_capacity(px_count);
        let mut off = HDR;
        for _ in 0..px_count {
            pixels.push(u32::from_ne_bytes([
                buf[off],
                buf[off + 1],
                buf[off + 2],
                buf[off + 3],
            ]));
            off += 4;
        }
        Some(CapturedImage {
            w,
            h,
            format,
            pixels,
        })
    }

    /// Stop + free capture session `id` via `SYS_CAPTURE_STOP` (276). Idempotent
    /// for an already-reclaimed id. (Sessions are also auto-reclaimed when the
    /// owning task exits, so this is a clean-shutdown courtesy, not mandatory.)
    pub fn capture_stop(id: u64) {
        unsafe {
            syscall1(SYS_CAPTURE_STOP, id);
        }
    }

    // ── Accessibility tree (screen reader / magnifier / keyboard-nav client) ──
    // Wraps the kernel a11y tree via SYS_A11Y_* (277-278). Cap-gated: the
    // calling task must hold `Cap::Accessibility{READ}` to snapshot and
    // `{WRITE}` to dispatch actions; every fn returns None/false otherwise.

    /// One accessibility node, mirroring `rae_abi::A11yNode` (96 bytes). The
    /// snapshot is a flat arena: `parent` references another node's `id`
    /// (`0` = the root desktop node). `role`/`state`/`actions` are the
    /// `A11Y_ROLE_*` / `A11Y_STATE_*` / `A11Y_ACTIONBIT_*` tags.
    #[derive(Debug, Clone)]
    pub struct A11yNodeInfo {
        pub id: u64,
        pub parent: u64,
        pub role: u32,
        pub state: u32,
        pub x: i32,
        pub y: i32,
        pub w: u32,
        pub h: u32,
        pub actions: u32,
        pub name: alloc::string::String,
    }

    /// Snapshot the accessibility tree via `SYS_A11Y_SNAPSHOT` (277). Returns the
    /// flat node list, or `None` if the caller lacks `Cap::Accessibility{READ}`
    /// or the scratch buffer was too small. The screen reader / magnifier /
    /// keyboard-nav all build on this.
    pub fn a11y_snapshot() -> Option<alloc::vec::Vec<A11yNodeInfo>> {
        const HDR: usize = 16; // size_of::<rae_abi::A11ySnapshotHeader>()
        const STRIDE: usize = 96; // size_of::<rae_abi::A11yNode>()
                                  // Size generously: a desktop rarely has > ~256 nodes at the window
                                  // tier; the widget tier can re-call with a larger buffer if needed.
        let cap_bytes = HDR + 1024 * STRIDE;
        let mut buf = alloc::vec![0u8; cap_bytes];
        let n = unsafe { syscall2(SYS_A11Y_SNAPSHOT, buf.as_mut_ptr() as u64, buf.len() as u64) };
        if n == A11Y_ERR || (n as usize) < HDR {
            return None;
        }
        let n = n as usize;
        let u32_at = |b: &[u8], o: usize| -> u32 {
            u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
        };
        let u64_at = |b: &[u8], o: usize| -> u64 {
            u64::from_le_bytes([
                b[o],
                b[o + 1],
                b[o + 2],
                b[o + 3],
                b[o + 4],
                b[o + 5],
                b[o + 6],
                b[o + 7],
            ])
        };
        let node_count = u32_at(&buf, 4) as usize;
        let mut out = alloc::vec::Vec::with_capacity(node_count);
        for i in 0..node_count {
            let off = HDR + i * STRIDE;
            if off + STRIDE > n {
                break;
            }
            // A11yNode layout: id@0 parent@8 role@16 state@20 x@24 y@28 w@32
            // h@36 actions@40 name_len@44 name@48.
            let name_len = (u32_at(&buf, off + 44) as usize).min(48);
            let name_off = off + 48;
            let name = alloc::string::String::from_utf8_lossy(&buf[name_off..name_off + name_len])
                .into_owned();
            out.push(A11yNodeInfo {
                id: u64_at(&buf, off),
                parent: u64_at(&buf, off + 8),
                role: u32_at(&buf, off + 16),
                state: u32_at(&buf, off + 20),
                x: u32_at(&buf, off + 24) as i32,
                y: u32_at(&buf, off + 28) as i32,
                w: u32_at(&buf, off + 32),
                h: u32_at(&buf, off + 36),
                actions: u32_at(&buf, off + 40),
                name,
            });
        }
        Some(out)
    }

    /// Dispatch an action to node `node_id` via `SYS_A11Y_ACTION` (278).
    /// `action` is an `A11Y_ACTION_*` selector; `arg` is action-specific (`0`
    /// when unused). Returns `true` on success, `false` if the caller lacks
    /// `Cap::Accessibility{WRITE}`, the node id is unknown, or the action is
    /// unsupported for that node.
    pub fn a11y_action(node_id: u64, action: u64, arg: u64) -> bool {
        let r = unsafe { syscall3(SYS_A11Y_ACTION, node_id, action, arg) };
        r == 0
    }

    // ── Userspace TCP/UDP sockets + DNS (rae_abi net block 121-125 + 264) ─────
    // Thin, never-panicking wrappers over the EXISTING socket syscalls. These
    // are the live-network seam the HTTP client (`raenet::http1`) rides on, the
    // AthNet Concept pillar's userspace path above L3. No new syscall numbers —
    // every wrapper matches docs/SYSCALL_TABLE.md exactly.

    /// `SYS_NET_SOCKET` (121). Socket protocol selector (`rdi`).
    pub const NET_PROTO_TCP: u64 = 0;
    pub const NET_PROTO_UDP: u64 = 1;
    /// Net-syscall failure sentinel (every net syscall returns this on error).
    pub const NET_ERR: u64 = u64::MAX;

    /// Pack four IPv4 octets into the **big-endian** u32 the net syscalls use:
    /// `octets[0]` lands in the high byte. This is the SAME packing
    /// `SYS_NET_DNS` (264) returns and `SYS_NET_CONNECT` (122) expects, so a
    /// resolved address feeds straight into `tcp_connect` with no re-ordering.
    #[inline]
    pub fn net_pack_ipv4(octets: [u8; 4]) -> u64 {
        ((octets[0] as u64) << 24)
            | ((octets[1] as u64) << 16)
            | ((octets[2] as u64) << 8)
            | (octets[3] as u64)
    }

    /// Inverse of [`net_pack_ipv4`]: unpack a big-endian packed IPv4 u32 back to
    /// four octets (`octets[0]` from the high byte).
    #[inline]
    pub fn net_unpack_ipv4(packed: u64) -> [u8; 4] {
        [
            ((packed >> 24) & 0xFF) as u8,
            ((packed >> 16) & 0xFF) as u8,
            ((packed >> 8) & 0xFF) as u8,
            (packed & 0xFF) as u8,
        ]
    }

    /// Create a socket via `SYS_NET_SOCKET` (121). `proto` is [`NET_PROTO_TCP`]
    /// or [`NET_PROTO_UDP`]. Returns the raw fd, or `None` if the kernel could
    /// not allocate one (table full / net not up).
    pub fn net_socket(proto: u64) -> Option<u64> {
        let fd = unsafe { syscall1(SYS_NET_SOCKET, proto) };
        if fd == NET_ERR {
            None
        } else {
            Some(fd)
        }
    }

    /// Connect socket `fd` to `ip:port` via `SYS_NET_CONNECT` (122). `ip` is the
    /// four octets in dotted order (`net_pack_ipv4` applies the BE packing).
    /// Returns `true` on success.
    pub fn net_connect(fd: u64, ip: [u8; 4], port: u16) -> bool {
        let r = unsafe { syscall3(SYS_NET_CONNECT, fd, net_pack_ipv4(ip), port as u64) };
        r != NET_ERR
    }

    /// Resolve `host` to an IPv4 address via `SYS_NET_DNS` (264). Returns the
    /// four octets in dotted order, or `None` on failure (resolution failed, no
    /// DHCP lease, host too long). The hostname is bounded to 255 bytes (the
    /// kernel rejects `>= 256`); longer names resolve to `None` without a
    /// syscall.
    pub fn dns_resolve(host: &str) -> Option<[u8; 4]> {
        let bytes = host.as_bytes();
        if bytes.is_empty() || bytes.len() >= 256 {
            return None;
        }
        let packed = unsafe { syscall2(SYS_NET_DNS, bytes.as_ptr() as u64, bytes.len() as u64) };
        if packed == NET_ERR {
            None
        } else {
            Some(net_unpack_ipv4(packed))
        }
    }

    /// Open a TCP socket and connect it to `ip:port` (socket + connect in one
    /// step). Returns the connected fd, or `None` if either step fails (the
    /// socket is closed on a failed connect so no fd leaks). Use [`dns_resolve`]
    /// first to turn a hostname into `ip`.
    pub fn tcp_connect(ip: [u8; 4], port: u16) -> Option<u64> {
        let fd = net_socket(NET_PROTO_TCP)?;
        if net_connect(fd, ip, port) {
            Some(fd)
        } else {
            sock_close(fd);
            None
        }
    }

    /// Send `buf` on socket `fd` via `SYS_NET_SEND` (123). Returns the number of
    /// bytes the kernel accepted (may be a short write — the caller loops on the
    /// remainder), or `-1` (as `isize`) on a hard error / bad fd. An empty
    /// `buf` is a no-op returning `0`. The buffer is bounded to 65535 bytes per
    /// call (the kernel's `copy_from_user` limit); a longer slice is truncated
    /// to that bound so the syscall never rejects the whole write.
    pub fn sock_send(fd: u64, buf: &[u8]) -> isize {
        if buf.is_empty() {
            return 0;
        }
        let len = buf.len().min(65535);
        let r = unsafe { syscall3(SYS_NET_SEND, fd, buf.as_ptr() as u64, len as u64) };
        if r == NET_ERR {
            -1
        } else {
            r as isize
        }
    }

    /// Receive up to `buf.len()` bytes on socket `fd` via `SYS_NET_RECV` (124).
    /// Returns the number of bytes read (`0` = no data available yet, non-
    /// blocking), or `-1` (as `isize`) on a hard error / bad fd. The read length
    /// is bounded to 65535 bytes per call (the kernel limit); a larger buffer is
    /// filled at most that many bytes per call.
    pub fn sock_recv(fd: u64, buf: &mut [u8]) -> isize {
        if buf.is_empty() {
            return 0;
        }
        let cap = buf.len().min(65535);
        let r = unsafe { syscall3(SYS_NET_RECV, fd, buf.as_mut_ptr() as u64, cap as u64) };
        if r == NET_ERR {
            -1
        } else {
            r as isize
        }
    }

    /// Close socket `fd` via `SYS_NET_CLOSE` (125). Idempotent for an already-
    /// closed fd (the kernel returns an error which is swallowed); call on drop
    /// so a crashed/exiting task does not leak a socket.
    pub fn sock_close(fd: u64) {
        unsafe {
            syscall1(SYS_NET_CLOSE, fd);
        }
    }

    #[inline]
    pub unsafe fn syscall0(nr: u64) -> u64 {
        let r: u64;
        core::arch::asm!("syscall", inout("rax") nr => r, out("rcx") _, out("r11") _);
        r
    }

    #[inline]
    pub unsafe fn syscall1(nr: u64, a1: u64) -> u64 {
        let r: u64;
        core::arch::asm!("syscall", inout("rax") nr => r, in("rdi") a1, out("rcx") _, out("r11") _);
        r
    }

    #[inline]
    pub unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> u64 {
        let r: u64;
        core::arch::asm!("syscall", inout("rax") nr => r, in("rdi") a1, in("rsi") a2, out("rcx") _, out("r11") _);
        r
    }

    #[inline]
    pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> u64 {
        let r: u64;
        core::arch::asm!("syscall", inout("rax") nr => r, in("rdi") a1, in("rsi") a2, in("rdx") a3, out("rcx") _, out("r11") _);
        r
    }

    #[inline]
    pub unsafe fn syscall4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
        let r: u64;
        core::arch::asm!(
            "syscall",
            inout("rax") nr => r,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            out("rcx") _,
            out("r11") _,
        );
        r
    }

    pub fn exit(code: u64) -> ! {
        unsafe {
            syscall1(SYS_EXIT, code);
        }
        loop {}
    }

    pub fn print_debug(msg: u64) {
        unsafe {
            syscall1(SYS_PRINT, msg);
        }
    }

    pub fn write(fd: u64, buf: &[u8]) -> u64 {
        unsafe { syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) }
    }

    pub fn yield_now() {
        unsafe {
            syscall0(SYS_YIELD);
        }
    }

    pub fn spawn(path: &str) -> u64 {
        unsafe { syscall3(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, 0) }
    }

    /// Spawn a child and attach it to a PTY slave endpoint (`pty_id` from `pty_open`).
    pub fn spawn_pty(path: &str, pty_id: u64) -> u64 {
        unsafe { syscall3(SYS_SPAWN, path.as_ptr() as u64, path.len() as u64, pty_id) }
    }

    /// Submit a Rae script (Concept §Customization Engine) under the given
    /// `SCRIPT_CAP_*` mask. Inline (≤64 KiB) sources have already finished
    /// when this returns; larger ones run in `raelangd` — poll
    /// [`script_status`]. Returns the script id.
    pub fn script_run(src: &[u8], cap_mask: u64) -> u64 {
        unsafe {
            syscall3(
                SYS_SCRIPT_RUN,
                src.as_ptr() as u64,
                src.len() as u64,
                cap_mask,
            )
        }
    }

    /// Read a script's `ScriptAbi` (56 bytes: state u32 @16, exit_code i32
    /// @20) — and, when `buf` is larger, the captured `print` output after
    /// the struct. Returns total bytes written, or `u64::MAX` on a bad id.
    pub fn script_status(id: u64, buf: &mut [u8]) -> u64 {
        unsafe {
            syscall3(
                SYS_SCRIPT_STATUS,
                id,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        }
    }

    pub fn pty_open() -> u64 {
        unsafe { syscall0(SYS_PTY_OPEN) }
    }

    pub fn pty_read(pty_id: u64, buf: &mut [u8]) -> u64 {
        unsafe {
            syscall3(
                SYS_PTY_READ,
                pty_id,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        }
    }

    pub fn pty_write(pty_id: u64, data: &[u8]) -> u64 {
        unsafe {
            syscall3(
                SYS_PTY_WRITE,
                pty_id,
                data.as_ptr() as u64,
                data.len() as u64,
            )
        }
    }

    pub fn pty_poll(pty_id: u64) -> u64 {
        unsafe { syscall1(SYS_PTY_POLL, pty_id) }
    }

    pub fn pty_slave_read(buf: &mut [u8]) -> u64 {
        unsafe {
            syscall3(
                SYS_PTY_SLAVE_IO,
                0,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        }
    }

    pub fn pty_slave_write(data: &[u8]) -> u64 {
        unsafe { syscall3(SYS_PTY_SLAVE_IO, 1, data.as_ptr() as u64, data.len() as u64) }
    }

    pub fn read_key() -> u64 {
        unsafe { syscall0(SYS_READ_KEY) }
    }

    pub fn poll_mouse() -> u64 {
        unsafe { syscall0(SYS_POLL_MOUSE) }
    }

    /// Poll the compositor's ABSOLUTE cursor position for hit-testing — where the
    /// click landed (buttons/tabs/list items). Returns `(x, y, buttons)`; `buttons`
    /// is currently always `0` (RESERVED — combine with [`poll_mouse`] for live
    /// button state). Wraps `SYS_INPUT_CURSOR` (279); never blocks, cheap per frame.
    pub fn cursor_pos() -> (u32, u32, u32) {
        let packed = unsafe { syscall0(SYS_INPUT_CURSOR) };
        (
            (packed & 0xFFFF) as u32,
            ((packed >> 16) & 0xFFFF) as u32,
            ((packed >> 32) & 0xFFFF_FFFF) as u32,
        )
    }

    /// Query a surface's CURRENT absolute origin `(x, y)` on screen, or `None` if
    /// the id is unknown. Apps subtract this LIVE origin from [`cursor_pos`] to
    /// convert the absolute cursor into surface-local coords for hit-testing —
    /// instead of the origin they passed to [`surface_present`], which goes stale
    /// the moment the window manager moves the window (Overview / Spaces / tiling).
    /// Wraps `SYS_SURFACE_ORIGIN` (280); never blocks, cheap to call each frame.
    pub fn surface_origin(sid: u64) -> Option<(u32, u32)> {
        let packed = unsafe { syscall1(SYS_SURFACE_ORIGIN, sid) };
        if packed == SURFACE_ORIGIN_ERR {
            None
        } else {
            Some(((packed & 0xFFFF) as u32, ((packed >> 16) & 0xFFFF) as u32))
        }
    }

    pub fn mmap(addr: u64, len: u64) -> u64 {
        unsafe { syscall2(SYS_MMAP, addr, len) }
    }

    pub fn munmap(addr: u64, len: u64) -> u64 {
        unsafe { syscall2(SYS_MUNMAP, addr, len) }
    }

    pub fn surface_create(width: u64, height: u64, user_virt: u64) -> u64 {
        unsafe { syscall3(SYS_SURFACE_CREATE, width, height, user_virt) }
    }

    pub fn surface_present(id: u64, x: u64, y: u64) -> u64 {
        unsafe { syscall3(SYS_SURFACE_PRESENT, id, x, y) }
    }

    pub fn surface_focus(id: u64) -> u64 {
        unsafe { syscall1(SYS_SURFACE_FOCUS, id) }
    }

    pub fn surface_close(id: u64) -> u64 {
        unsafe { syscall1(SYS_SURFACE_CLOSE, id) }
    }

    pub fn ipc_send(channel: u64, data: u64) -> u64 {
        unsafe { syscall2(SYS_SEND, channel, data) }
    }

    pub fn ipc_recv(channel: u64) -> u64 {
        unsafe { syscall1(SYS_RECV, channel) }
    }

    pub fn time_ns() -> u64 {
        unsafe { syscall0(SYS_TIME) }
    }

    pub fn getpid() -> u64 {
        unsafe { syscall0(SYS_GETPID) }
    }

    pub fn open(path: &str, flags: u64) -> u64 {
        unsafe { syscall3(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, flags) }
    }

    pub fn read(fd: u64, buf: &mut [u8]) -> u64 {
        unsafe { syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) }
    }

    pub fn close(fd: u64) -> u64 {
        unsafe { syscall1(SYS_CLOSE, fd) }
    }

    pub fn seek(fd: u64, offset: u64) -> u64 {
        unsafe { syscall2(SYS_SEEK, fd, offset) }
    }

    pub fn stat(fd: u64) -> u64 {
        unsafe { syscall1(SYS_STAT, fd) }
    }

    pub fn kill(pid: u64) -> u64 {
        unsafe { syscall1(SYS_KILL, pid) }
    }

    pub fn wait(pid: u64) -> u64 {
        unsafe { syscall1(SYS_WAIT, pid) }
    }

    /// List files in the VFS root. Returns the number of entries written into
    /// `buf`. Each entry is encoded as: `[name_len: u16][size: u32][name: u8 * name_len]`.
    pub fn readdir(buf: &mut [u8]) -> u64 {
        unsafe { syscall2(SYS_READDIR, buf.as_mut_ptr() as u64, buf.len() as u64) }
    }

    /// List files in `path`. Same entry encoding as `readdir`.
    pub fn readdir_at(path: &str, buf: &mut [u8]) -> u64 {
        unsafe {
            syscall4(
                SYS_READDIR_AT,
                path.as_ptr() as u64,
                path.len() as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        }
    }

    /// VFS error codes returned by `mkdir`/`unlink`/`rename` (kernel `vfs.rs`).
    /// `Ok` is `0`; anything else is one of these (or `u64::MAX` for a bad arg).
    pub const E_VFS_READONLY: u64 = 0xFFFF_FFFF_FFFF_FD01;
    pub const E_VFS_NOT_FOUND: u64 = 0xFFFF_FFFF_FFFF_FD02;
    pub const E_VFS_EXISTS: u64 = 0xFFFF_FFFF_FFFF_FD03;
    pub const E_VFS_NOT_EMPTY: u64 = 0xFFFF_FFFF_FFFF_FD04;
    pub const E_VFS_INVAL: u64 = 0xFFFF_FFFF_FFFF_FD05;

    /// Create a directory at `path` (idempotent callers should treat
    /// `E_VFS_EXISTS` as success). Returns `Ok(())` or the raw VFS error.
    pub fn mkdir(path: &str) -> Result<(), u64> {
        let r = unsafe { syscall3(SYS_MKDIR, path.as_ptr() as u64, path.len() as u64, 0o755) };
        if r == 0 {
            Ok(())
        } else {
            Err(r)
        }
    }

    /// Remove a file or empty directory. Returns `Ok(())` or the raw VFS error
    /// (e.g. [`E_VFS_NOT_EMPTY`], [`E_VFS_NOT_FOUND`]).
    pub fn unlink(path: &str) -> Result<(), u64> {
        let r = unsafe { syscall2(SYS_UNLINK, path.as_ptr() as u64, path.len() as u64) };
        if r == 0 {
            Ok(())
        } else {
            Err(r)
        }
    }

    /// Move/rename `old` → `new` (a CoW move within the session home on AthFS).
    /// Returns `Ok(())`, or [`E_VFS_EXISTS`] when `new` is taken (the source is
    /// preserved, so the caller can disambiguate and retry).
    pub fn rename(old: &str, new: &str) -> Result<(), u64> {
        let r = unsafe {
            syscall4(
                SYS_RENAME,
                old.as_ptr() as u64,
                old.len() as u64,
                new.as_ptr() as u64,
                new.len() as u64,
            )
        };
        if r == 0 {
            Ok(())
        } else {
            Err(r)
        }
    }

    /// Query screen (compositor) dimensions. Returns `(width, height)` or
    /// `None` if the compositor is not initialised.
    pub fn screen_info() -> Option<(u32, u32)> {
        let w: u64;
        let h: u64;
        let ret: u64;
        unsafe {
            core::arch::asm!(
                "syscall",
                inout("rax") SYS_SCREEN_INFO => ret,
                out("rdi") w,
                out("rsi") h,
                out("rcx") _,
                out("r11") _,
            );
        }
        if ret == 0 {
            Some((w as u32, h as u32))
        } else {
            None
        }
    }

    pub fn println(s: &str) {
        write(1, s.as_bytes());
        write(1, b"\n");
    }

    /// Password login. Returns `true` on success.
    pub fn session_login(username: &str, password: &[u8]) -> bool {
        unsafe {
            syscall4(
                SYS_SESSION_LOGIN,
                username.as_ptr() as u64,
                username.len() as u64,
                password.as_ptr() as u64,
                password.len() as u64,
            ) == 0
        }
    }

    pub fn session_guest() -> bool {
        unsafe { syscall0(SYS_SESSION_GUEST) == 0 }
    }

    pub fn session_lock() {
        unsafe {
            syscall0(SYS_SESSION_LOCK);
        }
    }

    pub fn session_unlock(password: &[u8]) -> bool {
        unsafe {
            syscall4(
                SYS_SESSION_UNLOCK,
                0,
                0,
                password.as_ptr() as u64,
                password.len() as u64,
            ) == 0
        }
    }

    pub fn session_logout() {
        unsafe {
            syscall0(SYS_SESSION_LOGOUT);
        }
    }

    /// Read session info: uid, username, phase, home path.
    /// Returns bytes written or `None` on error.
    pub fn session_info(buf: &mut [u8]) -> Option<usize> {
        let n = unsafe { syscall2(SYS_SESSION_INFO, buf.as_mut_ptr() as u64, buf.len() as u64) };
        if n == u64::MAX {
            None
        } else {
            Some(core::cmp::min(buf.len(), n as usize))
        }
    }

    /// Parse home directory from a `session_info` buffer.
    pub fn session_home_from(buf: &[u8]) -> Option<&str> {
        if buf.len() < 13 {
            return None;
        }
        let name_len = u16::from_le_bytes([buf[8], buf[9]]) as usize;
        let phase_off = 10 + name_len;
        if buf.len() < phase_off + 3 {
            return None;
        }
        let home_len = u16::from_le_bytes([buf[phase_off + 1], buf[phase_off + 2]]) as usize;
        let home_off = phase_off + 3;
        if buf.len() < home_off + home_len {
            return None;
        }
        core::str::from_utf8(&buf[home_off..home_off + home_len]).ok()
    }

    /// Read a config key into `buf`. Returns bytes written, or `None` on error.
    pub fn config_get(key: &str, buf: &mut [u8]) -> Option<usize> {
        let n = unsafe {
            syscall4(
                SYS_CONFIG_GET,
                key.as_ptr() as u64,
                key.len() as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == u64::MAX {
            None
        } else {
            Some(core::cmp::min(buf.len(), n as usize))
        }
    }

    pub fn config_set_bool(key: &str, value: bool) -> bool {
        let b = [value as u8];
        unsafe {
            syscall4(
                SYS_CONFIG_SET,
                key.as_ptr() as u64,
                key.len() as u64,
                b.as_ptr() as u64,
                1,
            ) != u64::MAX
        }
    }

    pub fn config_set_text(key: &str, value: &str) -> bool {
        unsafe {
            syscall4(
                SYS_CONFIG_SET,
                key.as_ptr() as u64,
                key.len() as u64,
                value.as_ptr() as u64,
                value.len() as u64,
            ) != u64::MAX
        }
    }

    pub fn config_set_int(key: &str, value: i64) -> bool {
        let bytes = value.to_le_bytes();
        unsafe {
            syscall4(
                SYS_CONFIG_SET,
                key.as_ptr() as u64,
                key.len() as u64,
                bytes.as_ptr() as u64,
                8,
            ) != u64::MAX
        }
    }

    /// List running tasks. Each entry is 24 bytes — see kernel `SYS_PROCLIST`.
    pub fn proclist(buf: &mut [u8]) -> u64 {
        unsafe { syscall2(SYS_PROCLIST, buf.as_mut_ptr() as u64, buf.len() as u64) }
    }
}

// ── Userspace allocator ──────────────────────────────────────────────────

#[cfg(not(any(test, feature = "host")))]
struct RaeAllocator;

#[cfg(not(any(test, feature = "host")))]
#[global_allocator]
static ALLOCATOR: RaeAllocator = RaeAllocator;

#[cfg(not(any(test, feature = "host")))]
static MMAP_NEXT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0x0000_4000_0000_0000);

#[cfg(not(any(test, feature = "host")))]
unsafe impl GlobalAlloc for RaeAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = (layout.size() + 4095) & !4095;
        let size = size.max(4096);
        let addr = MMAP_NEXT.fetch_add(size as u64, core::sync::atomic::Ordering::SeqCst);
        let result = sys::mmap(addr, size as u64);
        if result == u64::MAX {
            null_mut()
        } else {
            result as *mut u8
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

// ── IPC helpers ──────────────────────────────────────────────────────────

pub mod ipc {
    use super::sys;

    pub const CHANNEL_KEYBOARD: u64 = 0;
    pub const CHANNEL_DISPLAY: u64 = 1;
    pub const CHANNEL_MOUSE: u64 = 2;

    pub fn send(channel: u64, data: u64) -> bool {
        sys::ipc_send(channel, data) == 0
    }

    pub fn recv(channel: u64) -> Option<u64> {
        let v = sys::ipc_recv(channel);
        if v == 0 {
            None
        } else {
            Some(v)
        }
    }

    pub fn poll_keyboard() -> Option<u8> {
        recv(CHANNEL_KEYBOARD).map(|v| v as u8)
    }

    #[derive(Debug, Clone, Copy)]
    pub struct MouseEvent {
        pub dx: i16,
        pub dy: i16,
        pub buttons: u8,
    }

    pub fn poll_mouse() -> Option<MouseEvent> {
        recv(CHANNEL_MOUSE).map(|raw| MouseEvent {
            buttons: (raw & 0xFF) as u8,
            dx: ((raw >> 8) & 0xFFFF) as i16,
            dy: ((raw >> 24) & 0xFFFF) as i16,
        })
    }
}

// ── Panic handler ────────────────────────────────────────────────────────

#[cfg(not(any(test, feature = "host")))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys::exit(1);
}

#[cfg(not(any(test, feature = "host")))]
#[no_mangle]
pub extern "C" fn __rust_alloc_error_handler(_: Layout) -> ! {
    sys::exit(2);
}

#[cfg(test)]
mod hello_raekit_app_tests {
    //! The "Hello-AthKit" sample app, host-KAT'd: a declarative button with a click
    //! handler. Proves the AthKit app pattern (on_launch builds a declarative view
    //! tree with a Button; a click `AppEvent::Action` invokes the handler). In the
    //! live shell the handler fires a syscall (`raekit::syscalls`); host-side those
    //! are not issued (see the crate docs), so the KAT records the click instead.
    use crate::app::{AppEvent, RaeApp};
    use crate::builders::{ButtonBuilder, Text, VStack};
    use crate::view::ViewNode;

    const SAVE_ACTION: u32 = 1;

    struct HelloAthKit {
        clicks: u32,
    }

    impl RaeApp for HelloAthKit {
        fn name(&self) -> &str {
            "Hello AthKit"
        }
        fn on_launch(&mut self) -> ViewNode {
            // Declarative UI — a heading + a button bound to SAVE_ACTION.
            VStack::new()
                .spacing(12.0)
                .child(Text::new("Hello, AthenaOS!").build())
                .child(ButtonBuilder::new("Save").action(SAVE_ACTION).build())
                .build()
        }
        fn on_event(&mut self, event: &AppEvent) -> Option<ViewNode> {
            match event {
                AppEvent::Action { id } if *id == SAVE_ACTION => {
                    // The click handler. Live: fires a syscall via raekit::syscalls.
                    self.clicks += 1;
                    Some(self.on_launch())
                }
                _ => None,
            }
        }
    }

    /// First Button's `action_id` anywhere in a declarative tree.
    fn find_button_action(node: &ViewNode) -> Option<u32> {
        match node {
            ViewNode::Button { action_id, .. } => Some(*action_id),
            ViewNode::Stack { children, .. } | ViewNode::ZStack { children, .. } => {
                children.iter().find_map(find_button_action)
            }
            _ => None,
        }
    }

    #[test]
    fn declares_a_button_and_handles_its_click() {
        let mut app = HelloAthKit { clicks: 0 };
        assert_eq!(app.name(), "Hello AthKit");

        // on_launch builds a declarative tree that contains the Save button.
        let tree = app.on_launch();
        assert_eq!(find_button_action(&tree), Some(SAVE_ACTION));

        // A click on the button (AppEvent::Action) invokes the handler.
        assert_eq!(app.clicks, 0);
        let updated = app.on_event(&AppEvent::Action { id: SAVE_ACTION });
        assert_eq!(app.clicks, 1); // handler fired
        assert!(updated.is_some()); // returned a refreshed view

        // An unrelated action does nothing (the handler is action-scoped).
        assert!(app.on_event(&AppEvent::Action { id: 999 }).is_none());
        assert_eq!(app.clicks, 1);
    }
}
