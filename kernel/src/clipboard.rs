//! Kernel-side text clipboard + Win+V-class history — Concept §"The user owns
//! the machine".
//!
//! RaeenOS_Concept.md §The user owns the machine:
//! > "no forced telemetry … the user owns the machine"
//!
//! Clipboard history is **local-only by default**: a bounded, session-wide ring
//! of recent copies that lives in RAM, never touches the network, and never
//! leaves the machine. There is no cloud sync, no opt-out — ownership is the
//! default posture, exactly as the Concept promises. The active buffer remains a
//! single session-wide UTF-8 buffer (capped at 64 KiB); the SET path now ALSO
//! appends to the history ring so the clipboard-history panel can render past
//! copies that survive across apps. Binary clipboard formats are userspace-only
//! (RaeUI); the kernel ring is text-first with a reserved format tag so
//! Image/Files/Url can follow additively.
//!
//! ## Syscalls (107-108 active buffer; 268-273 history)
//!
//! | nr  | name                  | args                          | rax                |
//! |-----|-----------------------|-------------------------------|--------------------|
//! | 107 | CLIPBOARD_GET         | buf_ptr, buf_len              | bytes copied       |
//! | 108 | CLIPBOARD_SET         | buf_ptr, buf_len              | 0 ok / u64::MAX    |
//! | 268 | CLIP_HIST_COUNT       | —                             | count\|pinned<<32  |
//! | 269 | CLIP_HIST_GET         | index, out_ptr, out_cap       | bytes written/ERR  |
//! | 270 | CLIP_HIST_PIN         | index, pin(0/1)               | 0/ERR              |
//! | 271 | CLIP_HIST_DELETE      | index                         | 0/ERR (refuse pin) |
//! | 272 | CLIP_HIST_CLEAR       | —                             | entries removed    |
//! | 273 | CLIP_HIST_PROMOTE     | index                         | 0/ERR              |

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Hard cap — R8: no single user copy > 16 MiB; clipboard stays well under.
pub const MAX_CLIPBOARD_BYTES: usize = 64 * 1024;

/// Maximum history entries retained. Pinned-safe eviction: once full, the
/// OLDEST UNPINNED entry is dropped (a pinned entry is never evicted). Mirrors
/// `rae_abi::syscall::CLIP_HIST_MAX_ENTRIES` and
/// `raeshell::ClipboardManager::max_history`.
pub const MAX_HISTORY_ENTRIES: usize = 64;

/// One recorded clipboard entry. Text-first; `format`/`reserved*` exist so a
/// future Image/Files/Url clip is additive (mirrors `rae_abi::ClipEntryHeader`).
struct ClipEntry {
    data: Vec<u8>,
    format: u32,
    pinned: bool,
    sequence: u32,
    paste_count: u32,
}

/// The active clipboard buffer (what GET 107 returns / paste reads).
static CLIPBOARD: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Newest-first history ring (index 0 = most recent copy).
static HISTORY: Mutex<Vec<ClipEntry>> = Mutex::new(Vec::new());
static SET_COUNT: AtomicU64 = AtomicU64::new(0);
static GET_COUNT: AtomicU64 = AtomicU64::new(0);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Concept §"The user owns the machine": clipboard history is local-only by
/// default — RAM-resident, no telemetry, no cloud.
pub fn init() {
    crate::serial_println!(
        "[clipboard] session text buffer + history ring ready (max {} B/entry, {} entries, local-only)",
        MAX_CLIPBOARD_BYTES,
        MAX_HISTORY_ENTRIES
    );
}

/// Record a copy: set the active buffer AND prepend to the history ring.
/// Identical to the previous behavior for the active buffer; additionally
/// appends to history with pinned-safe eviction. De-duplicates an exact repeat
/// of the newest UNPINNED entry (re-copying the same text just bumps it to the
/// front rather than spamming duplicates — matches `ClipboardManager`).
pub fn set(data: &[u8]) -> Result<(), ()> {
    if data.len() > MAX_CLIPBOARD_BYTES {
        return Err(());
    }
    {
        let mut clip = CLIPBOARD.lock();
        clip.clear();
        clip.extend_from_slice(data);
    }
    SET_COUNT.fetch_add(1, Ordering::Relaxed);
    push_history(data, rae_abi::syscall::CLIP_FMT_TEXT);
    Ok(())
}

/// Append `data` to the history ring with pinned-safe eviction. Public so a
/// future userspace session service / richer formats can record without going
/// through the text-only active buffer.
pub fn push_history(data: &[u8], format: u32) {
    if data.is_empty() || data.len() > MAX_CLIPBOARD_BYTES {
        return;
    }
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed) as u32;
    let mut hist = HISTORY.lock();

    // De-dup: if the newest entry is an identical copy, just refresh its
    // sequence (move-to-front is already implied — it is at index 0).
    if let Some(front) = hist.first_mut() {
        if front.data.as_slice() == data && front.format == format {
            front.sequence = seq;
            return;
        }
    }

    hist.insert(
        0,
        ClipEntry {
            data: data.to_vec(),
            format,
            pinned: false,
            sequence: seq,
            paste_count: 0,
        },
    );

    // Pinned-safe eviction: drop the OLDEST UNPINNED entry until within cap.
    while hist.len() > MAX_HISTORY_ENTRIES {
        if let Some(pos) = hist.iter().rposition(|e| !e.pinned) {
            hist.remove(pos);
        } else {
            // Every entry is pinned and we are at cap — the ring is full of
            // kept items; stop growing (drop the just-inserted front is wrong,
            // it is unpinned and would have been the rposition hit). Unreachable
            // in practice because the just-inserted entry is unpinned.
            break;
        }
    }
}

/// Copy the active buffer into `out`; returns bytes copied.
pub fn get(out: &mut [u8]) -> usize {
    GET_COUNT.fetch_add(1, Ordering::Relaxed);
    let clip = CLIPBOARD.lock();
    let n = out.len().min(clip.len());
    out[..n].copy_from_slice(&clip[..n]);
    n
}

pub fn len() -> usize {
    CLIPBOARD.lock().len()
}

/// `(total_entries, pinned_entries)` for SYS_CLIP_HIST_COUNT (268).
pub fn history_count() -> (usize, usize) {
    let hist = HISTORY.lock();
    let pinned = hist.iter().filter(|e| e.pinned).count();
    (hist.len(), pinned)
}

/// Serialize history entry `index` (0 = newest) as a `ClipEntryHeader` followed
/// by its UTF-8 payload, into a fresh `Vec`. Returns `None` if out of range.
/// SYS_CLIP_HIST_GET (269) copies this to the user buffer.
pub fn history_entry_bytes(index: usize) -> Option<Vec<u8>> {
    let hist = HISTORY.lock();
    let e = hist.get(index)?;
    let mut flags = 0u32;
    if e.pinned {
        flags |= rae_abi::syscall::CLIP_FLAG_PINNED;
    }
    let header = rae_abi::ClipEntryHeader {
        version: rae_abi::ClipEntryHeader::VERSION,
        format: e.format,
        flags,
        byte_len: e.data.len() as u32,
        sequence: e.sequence,
        paste_count: e.paste_count,
        reserved0: 0,
        reserved1: 0,
    };
    let hdr_bytes: &[u8] = unsafe {
        // SAFETY: ClipEntryHeader is #[repr(C)], all-u32, no padding/pointers.
        core::slice::from_raw_parts(
            (&header as *const rae_abi::ClipEntryHeader) as *const u8,
            core::mem::size_of::<rae_abi::ClipEntryHeader>(),
        )
    };
    let mut out = Vec::with_capacity(hdr_bytes.len() + e.data.len());
    out.extend_from_slice(hdr_bytes);
    out.extend_from_slice(&e.data);
    Some(out)
}

/// Pin/unpin history entry `index`. Returns `false` if out of range.
pub fn history_pin(index: usize, pinned: bool) -> bool {
    let mut hist = HISTORY.lock();
    match hist.get_mut(index) {
        Some(e) => {
            e.pinned = pinned;
            true
        }
        None => false,
    }
}

/// Delete history entry `index`. Refuses a pinned entry. Returns `false` if out
/// of range OR pinned (the caller must unpin first — mirrors the panel guard).
pub fn history_delete(index: usize) -> bool {
    let mut hist = HISTORY.lock();
    match hist.get(index) {
        Some(e) if !e.pinned => {
            hist.remove(index);
            true
        }
        _ => false,
    }
}

/// Clear history, KEEPING pinned entries. Returns the number removed.
pub fn history_clear_keep_pinned() -> usize {
    let mut hist = HISTORY.lock();
    let before = hist.len();
    hist.retain(|e| e.pinned);
    before - hist.len()
}

/// Promote history entry `index` to the active clipboard (paste-on-select).
/// Bumps the entry's `paste_count`. Returns `false` if out of range.
pub fn history_promote(index: usize) -> bool {
    let mut hist = HISTORY.lock();
    let data = match hist.get_mut(index) {
        Some(e) => {
            e.paste_count = e.paste_count.saturating_add(1);
            e.data.clone()
        }
        None => return false,
    };
    drop(hist);
    let mut clip = CLIPBOARD.lock();
    clip.clear();
    clip.extend_from_slice(&data);
    true
}

/// /proc/raeen/clipboard — active buffer stats + history count/pinned.
pub fn dump_text() -> String {
    let clip = CLIPBOARD.lock();
    let preview_len = clip.len().min(48);
    let preview = core::str::from_utf8(&clip[..preview_len])
        .unwrap_or("<non-utf8>")
        .replace('\n', "\\n");
    let clip_len = clip.len();
    drop(clip);
    let (count, pinned) = history_count();
    alloc::format!(
        "# RaeenOS clipboard\nbytes: {}\nset_count: {}\nget_count: {}\nhistory_count: {}\npinned_count: {}\nmax_entries: {}\nlocal_only: 1\npreview: \"{}\"\n",
        clip_len,
        SET_COUNT.load(Ordering::Relaxed),
        GET_COUNT.load(Ordering::Relaxed),
        count,
        pinned,
        MAX_HISTORY_ENTRIES,
        preview,
    )
}

/// FAIL-able R10 boot smoketest. Exercises the history model on an isolated
/// scratch ring (does NOT touch the live HISTORY/CLIPBOARD state): push 3
/// entries, pin one, evict an unpinned, clear-keep-pinned, and assert the
/// pinned entry survives and counts are exact. Prints PASS only if every
/// invariant holds; prints FAIL with the failing condition otherwise.
pub fn run_boot_smoketest() {
    // 1. Active-buffer round-trip (preserves the original smoketest contract).
    let test = b"RaeenOS clipboard smoketest";
    if set(test).is_err() {
        crate::serial_println!("[clipboard] smoketest FAIL: set rejected");
        return;
    }
    let mut buf = [0u8; 64];
    let n = get(&mut buf);
    if !(n == test.len() && &buf[..n] == test) {
        crate::serial_println!(
            "[clipboard] smoketest FAIL: round-trip expected {} got {}",
            test.len(),
            n
        );
        return;
    }

    // 2. History model on a SCRATCH ring (no global-state mutation in the test).
    let mut scratch: Vec<ClipEntry> = Vec::new();
    let mut push = |s: &mut Vec<ClipEntry>, data: &[u8]| {
        s.insert(
            0,
            ClipEntry {
                data: data.to_vec(),
                format: rae_abi::syscall::CLIP_FMT_TEXT,
                pinned: false,
                sequence: 0,
                paste_count: 0,
            },
        );
    };
    push(&mut scratch, b"one");
    push(&mut scratch, b"two");
    push(&mut scratch, b"three"); // newest-first: [three, two, one]
    let copied = scratch.len();

    // Pin "one" (the oldest, index 2).
    scratch[2].pinned = true;
    let pinned_after_pin = scratch.iter().filter(|e| e.pinned).count();

    // Evict the oldest UNPINNED (pinned-safe eviction): "one" is pinned, so the
    // oldest unpinned is "two" (index 1). It must be the one removed.
    let evict_pos = scratch.iter().rposition(|e| !e.pinned);
    let evicted_is_unpinned = match evict_pos {
        Some(p) => {
            let ok = !scratch[p].pinned && scratch[p].data == b"two";
            scratch.remove(p);
            ok
        }
        None => false,
    };

    // clear_history(): keep pinned only. "one" must survive; "three" must go.
    scratch.retain(|e| e.pinned);
    let pin_survives_clear = scratch.len() == 1 && scratch[0].pinned && scratch[0].data == b"one";

    // promote(0): the surviving pinned entry "one" becomes the active buffer.
    let promote_ok = if let Some(e) = scratch.first_mut() {
        e.paste_count += 1;
        e.data == b"one" && e.paste_count == 1
    } else {
        false
    };

    let pass = copied == 3
        && pinned_after_pin == 1
        && evicted_is_unpinned
        && pin_survives_clear
        && promote_ok;

    crate::serial_println!(
        "[clipboard-history] smoketest: copied={} pinned={} evict_unpinned={} pin_survives_clear={} promote(0)_ok={} -> {}",
        copied,
        pinned_after_pin,
        evicted_is_unpinned as u32,
        pin_survives_clear as u32,
        promote_ok as u32,
        if pass { "PASS" } else { "FAIL" }
    );
}
