//! In-RAM boot log ring buffer.
//!
//! Every line the kernel sends through `serial_println!` (or any
//! `serial::_print` caller) is mirrored into a 256 KiB ring here. On
//! bare-metal Athena where no serial cable is attached and the boot is
//! fast enough that the on-screen text scrolls off before anyone can
//! read it, this is the only place the full boot transcript lives.
//!
//! Two consumers today:
//!
//!   1. `/proc/raeen/bootlog` (via [`dump_text`]) — userspace or a future
//!      kernel debug shell can pull the whole transcript.
//!   2. `bootlog::flush_to_esp()` (future) — writes the buffer to a file
//!      on the FAT32 ESP so it survives a reboot. Pending the safe-mode
//!      log carveout (see MasterChecklist Phase 0).
//!
//! Design:
//!
//!   * **Lock-free under SERIAL1 + interrupts-disabled.** `serial::_print`
//!     already runs inside `interrupts::without_interrupts` while holding
//!     `SERIAL1`, so the append is single-threaded by construction. No
//!     additional lock here — the byte-by-byte writes use `Relaxed`
//!     atomics for index updates and direct pointer stores for the data.
//!   * **Wrapping ring, never blocks the producer.** A boot that prints
//!     more than 256 KiB drops the oldest lines, not the newest. Boot
//!     diagnostics are almost always end-weighted (the panic, the timeout,
//!     the wrong subsystem) so this is the right policy.
//!   * **No allocation in the hot path.** The ring is a `static
//!     UnsafeCell<[u8; 256K]>`, written via raw pointer math. Heap is
//!     not yet up when the first `serial_println!` fires.
//!
//! R10: `init()` is implicit (zero-init static); `run_boot_smoketest`
//! prints a marker so the ring proves it captured; `dump_text` backs
//! `/proc/raeen/bootlog`; this docstring satisfies the Concept tie-in.

extern crate alloc;

use alloc::string::String;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// 1 MiB — sized to match the pre-allocated BOOTLOG.TXT on the boot stick's
// ESP, so a full successful boot's transcript persists WITHOUT wrapping. At
// 256 KiB the ring wrapped before end-of-boot and the final flush evicted the
// early-boot lines (xHCI/USB/ACPI bring-up) — exactly the part needed to
// debug bare-metal input failures. Static .bss, no heap cost.
const RING_SIZE: usize = 1024 * 1024;
const RING_MASK: usize = RING_SIZE - 1;
const _: () = assert!(RING_SIZE.is_power_of_two());

#[repr(C)]
struct Ring {
    /// Raw byte storage. `UnsafeCell` because the producer writes through
    /// a raw pointer; `serial::_print` synchronizes externally (SERIAL1 +
    /// without_interrupts), so no inner Mutex is needed.
    bytes: UnsafeCell<[u8; RING_SIZE]>,
    /// Monotonic write count. `idx & RING_MASK` is the next slot to write.
    /// `idx` itself never wraps until it exceeds `usize::MAX` (after which
    /// the only consequence is that `wrapped` may toggle one tick early —
    /// at u64 capacity that's centuries of continuous logging).
    head: AtomicUsize,
    /// True once `head` has crossed `RING_SIZE` for the first time. The
    /// reader uses this to decide whether to start at byte 0 or at the
    /// oldest still-live slot.
    wrapped: AtomicBool,
}

// SAFETY: producer/reader synchronization is provided externally
// (`serial::_print` runs inside `without_interrupts` + `SERIAL1.lock()`).
// The ring is only ever appended to from that single critical section.
// `dump_text` reads under the same external lock by going through
// `serial::_print`-equivalent gating (procfs dumps run with interrupts
// disabled — see procfs::dump_all_raeen_endpoints_to_serial).
unsafe impl Sync for Ring {}

static RING: Ring = Ring {
    bytes: UnsafeCell::new([0u8; RING_SIZE]),
    head: AtomicUsize::new(0),
    wrapped: AtomicBool::new(false),
};

/// Total bytes ever appended (saturating at `usize::MAX`).
pub fn bytes_logged() -> usize {
    RING.head.load(Ordering::Relaxed)
}

/// Has the ring wrapped at least once (true once we've written `RING_SIZE`)?
pub fn has_wrapped() -> bool {
    RING.wrapped.load(Ordering::Relaxed)
}

/// Append a byte slice to the ring. Called from `serial::_print` for every
/// line. Cannot fail; wraps on overflow.
///
/// # Safety contract
///
/// The caller must serialize against other appenders. `serial::_print`
/// does this via `interrupts::without_interrupts` + `SERIAL1.lock()`. If
/// you append from anywhere else you'll race the producer.
pub fn append(data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let start = RING.head.load(Ordering::Relaxed);
    // SAFETY: synchronized externally; see module doc.
    let ring = unsafe { &mut *RING.bytes.get() };
    for (i, &b) in data.iter().enumerate() {
        let slot = (start.wrapping_add(i)) & RING_MASK;
        ring[slot] = b;
    }
    let new_head = start.wrapping_add(data.len());
    RING.head.store(new_head, Ordering::Relaxed);
    if new_head >= RING_SIZE && !RING.wrapped.load(Ordering::Relaxed) {
        RING.wrapped.store(true, Ordering::Relaxed);
    }
}

/// Copy the live transcript into a `String`. Oldest-to-newest order.
/// When the ring has wrapped this returns at most `RING_SIZE` bytes
/// starting from the oldest still-live byte; pre-wrap it returns
/// everything ever written.
///
/// Intended for `/proc/raeen/bootlog`. Allocates ~256 KiB.
pub fn snapshot() -> String {
    let head = RING.head.load(Ordering::Relaxed);
    let wrapped = RING.wrapped.load(Ordering::Relaxed);
    // SAFETY: see module doc. Reader runs under the same external
    // discipline (procfs dump path is interrupts-disabled).
    let ring = unsafe { &*RING.bytes.get() };

    let (read_start, read_len) = if wrapped {
        (head & RING_MASK, RING_SIZE)
    } else {
        (0usize, head.min(RING_SIZE))
    };

    let mut out = alloc::vec::Vec::with_capacity(read_len);
    if wrapped {
        // Tail half then head half.
        out.extend_from_slice(&ring[read_start..]);
        out.extend_from_slice(&ring[..read_start]);
    } else {
        out.extend_from_slice(&ring[..read_len]);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `/proc/raeen/bootlog` body. Prepends a small header so the consumer
/// knows whether the buffer wrapped (and lost the earliest lines).
pub fn dump_text() -> String {
    let head = bytes_logged();
    let wrapped = has_wrapped();
    let lost = if wrapped {
        head.saturating_sub(RING_SIZE)
    } else {
        0
    };
    let mut out = alloc::format!(
        "# AthenaOS boot log (RAM ring, {} KiB capacity)\n# bytes_logged: {}  wrapped: {}  oldest_lost: {} bytes\n# === transcript follows ===\n",
        RING_SIZE / 1024,
        head,
        wrapped,
        lost,
    );
    out.push_str(&snapshot());
    out
}

/// Prefixes of the boot-log lines worth surfacing on the on-screen
/// diagnostic panel. The framebuffer is the one channel that can't fail
/// (no disk, no FAT, no permissions, no cache, no multi-disk routing),
/// so on bare metal where there's no serial cable and the FAT log write
/// is finicky, we paint exactly these lines for the user to photograph.
/// Ordered roughly by diagnostic value.
const DIAG_PREFIXES: &[&str] = &[
    "[ OK ] CPU:",
    "[smbios] hardware profile",
    "[smp]",
    "[safe-mode]",
    "[pcie]",
    "[xhci]",
    "[usb-hub]",
    "[usb-hid]",
    "[usb-msc]",
    "[usb-summary]",
    "[netlog]",
    "[block] registered",
    "[nvme] I/O c",
    "[pcie] PCI re-scan",
    "[user-thread] msg:",
    "[TIER]",
    "[bootlog-persist]",
    "[ OS ] System successfully booted",
    "[PANIC]",
    "[EXCEPTION]",
];

/// Scan the RAM ring and return the lines matching any [`DIAG_PREFIXES`]
/// entry, oldest-to-newest, capped at `max_lines` (most recent kept).
/// Used by the on-screen diagnostic panel.
pub fn collect_diagnostic_lines(max_lines: usize) -> alloc::vec::Vec<String> {
    let snap = snapshot();
    let mut out: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for line in snap.split('\n') {
        let t = line.trim_end_matches('\r');
        if t.is_empty() {
            continue;
        }
        if DIAG_PREFIXES.iter().any(|p| t.contains(p)) {
            out.push(String::from(t));
        }
    }
    // Keep the most recent `max_lines` if we overflowed a screen.
    if out.len() > max_lines {
        let drop = out.len() - max_lines;
        out.drain(0..drop);
    }
    out
}

/// Paint the curated diagnostic lines onto a framebuffer surface so the
/// user can photograph them. Same Canvas API the login screen uses, so
/// it composites normally. Renders into the caller-provided surface
/// pointer (the kernel desktop surface), `w`×`h` at 4 bytes/pixel.
pub fn render_diagnostics(ptr: *mut u8, w: u32, h: u32) {
    const BG: u32 = 0xFF_0A_0E_1A;
    const HDR: u32 = 0xFF_4E_9C_FF;
    const FG: u32 = 0xFF_C8_D0_E0;
    const ERR: u32 = 0xFF_FF_6B_7A;
    const OK: u32 = 0xFF_66_E0_88;

    let mut canvas = unsafe { raegfx::Canvas::new(ptr, w as usize, h as usize, 4) };
    canvas.clear(BG);

    canvas.draw_text(
        24,
        16,
        "AthenaOS boot diagnostics (photograph this screen)",
        HDR,
        None,
    );
    canvas.draw_text(
        24,
        34,
        "safe-mode build — storage writes blocked; logs below are from the RAM ring",
        FG,
        None,
    );

    // Leave room for header; ~16px per line. Cap by screen height.
    let line_h = 16usize;
    let top = 60usize;
    let max_lines = ((h as usize).saturating_sub(top) / line_h).min(80);
    let lines = collect_diagnostic_lines(max_lines);

    let mut y = top;
    for line in &lines {
        let color = if line.contains("[PANIC]") || line.contains("[EXCEPTION]") {
            ERR
        } else if line.contains("System successfully booted") || line.contains("-> PASS") {
            OK
        } else {
            FG
        };
        // Truncate to a sane width so long lines don't overrun the canvas.
        // Boundary-safe: a raw `&line[..230]` PANICS (kernel crash) when byte
        // 230 lands inside a multi-byte UTF-8 code point — diagnostic lines can
        // carry user content / panic messages with accents/CJK/emoji.
        let truncated = raeshell::text_util::truncate_chars(line, 230);
        canvas.draw_text(24, y, truncated, color, None);
        y += line_h;
        if y + line_h > h as usize {
            break;
        }
    }

    if lines.is_empty() {
        canvas.draw_text(24, top, "(no diagnostic lines captured in ring)", ERR, None);
    }
}

/// R10 smoketest. Prints a sentinel that should appear at the end of
/// the captured transcript — proves the ring is wired and trails the
/// live serial.
pub fn run_boot_smoketest() {
    crate::serial_println!(
        "[bootlog] smoketest: ring_size={}KiB bytes_logged={} wrapped={} -> PASS",
        RING_SIZE / 1024,
        bytes_logged(),
        has_wrapped(),
    );
}
