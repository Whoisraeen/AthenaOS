//! In-kernel DWARF symbolization — Concept §"prefer the change that makes iron
//! more diagnosable".
//!
//! Turns a raw panic RIP into a source `(file, line)` on iron — the in-kernel
//! equivalent of Redox's `backtrace.sh`. `gimli` parses the kernel's own
//! `.debug_line` line-number program. A panic handler must never itself panic,
//! so every parse here is fallible and returns `None` on absent/garbage data.
//!
//! Wiring status: the resolver is complete; feeding it the live `.debug_line`
//! needs that section embedded at build time (a `build.rs` step, deferred) or
//! read from the loaded kernel image. Until then it is exercised on synthetic
//! input by the smoketest.

use gimli::{DebugLine, DebugLineOffset, LittleEndian};

/// Resolve `probe` against a raw `.debug_line` section: returns the
/// `(file_index, line)` of the last row whose address is <= `probe`. `None` on
/// empty/garbage input or any parse error — never panics.
pub fn resolve_line(debug_line_bytes: &[u8], probe: u64) -> Option<(u64, u64)> {
    if debug_line_bytes.is_empty() {
        return None;
    }
    let debug_line = DebugLine::new(debug_line_bytes, LittleEndian);
    // Parse the line-number program at offset 0 (64-bit target => address_size 8).
    let program = debug_line.program(DebugLineOffset(0), 8, None, None).ok()?;
    let mut rows = program.rows();
    let mut best: Option<(u64, u64)> = None;
    while let Ok(Some((_, row))) = rows.next_row() {
        if !row.end_sequence() && row.address() <= probe {
            let line = row.line().map(|l| l.get()).unwrap_or(0);
            best = Some((row.file_index(), line));
        }
    }
    best
}

pub fn init() {
    crate::serial_println!("[ OK ] dwarf_sym: gimli DWARF line resolver ready (embed pending)");
}

/// R10 smoketest — must be able to print FAIL. A panic-time symbolizer must
/// stay panic-free on absent/garbage debug data; assert graceful `None`.
pub fn run_boot_smoketest() {
    let empty = resolve_line(&[], 0x1000).is_none();
    let garbage = resolve_line(&[0xFF, 0x00, 0xAB, 0xCD, 0x01, 0x02], 0x1000).is_none();
    let pass = empty && garbage;
    crate::selftest::record_smoketest("dwarf_sym", pass);
    crate::serial_println!(
        "[dwarf_sym] graceful: empty={} garbage={} -> {}",
        empty,
        garbage,
        if pass { "PASS" } else { "FAIL" }
    );
}
