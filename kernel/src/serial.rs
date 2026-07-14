//! Serial port driver (COM1, 16550 UART).
//!
//! Used for debug output — QEMU redirects COM1 to the host terminal,
//! so this is the primary log channel during development.

use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::SerialPort;

/// Sink that appends every `&str` chunk straight into the bootlog RAM
/// ring. Used by `_print` to fan a single `format_args!` into both the
/// UART and the ring without an intermediate truncation-prone buffer.
struct RingSink;

impl core::fmt::Write for RingSink {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        crate::bootlog::append(s.as_bytes());
        Ok(())
    }
}

lazy_static! {
    /// Global COM1 serial port, initialized at 0x3F8.
    pub static ref SERIAL1: Mutex<SerialPort> = {
        let mut serial_port = unsafe { SerialPort::new(0x3F8) };
        serial_port.init();
        Mutex::new(serial_port)
    };
}

/// Initialize COM1. Calling this once at boot guarantees the port is up
/// before any panic might try to use it.
pub fn init() {
    let _ = &*SERIAL1;
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // Disable interrupts while holding the serial lock to prevent deadlock
    // if a timer/keyboard interrupt fires while we're mid-print.
    interrupts::without_interrupts(|| {
        // Write to UART directly — uart_16550::SerialPort impls
        // fmt::Write so format_args is consumed without any
        // intermediate buffer that could truncate mid-multibyte-UTF-8
        // (the boot banner has box-drawing chars that broke a previous
        // StackBuf attempt).
        let _ = SERIAL1.lock().write_fmt(args);
    });
    // RAM ring — bare-metal Athena's only durable log when the boot
    // scrolls off the framebuffer too fast to read. Always captured,
    // even on builds without a serial cable. Formatting `args` a second
    // time costs only the formatting itself (~µs), the args carry no
    // side effects. The ring's `append` is synchronized externally by
    // running here inside without_interrupts() — only one CPU is
    // executing this critical section at a time on the BSP, and on APs
    // the same discipline applies.
    interrupts::without_interrupts(|| {
        let _ = RingSink.write_fmt(args);
    });
    // GOP framebuffer text fallback (MasterChecklist §1.1): mirror serial output
    // to the on-screen text console so boot progress is visible on real hardware
    // (e.g. Athena) when no serial cable is attached. No-op until console::init().
    //
    // CRITICAL: this runs in its OWN critical section, AFTER releasing SERIAL1,
    // and uses a best-effort try_lock (console::try_print). Nesting it inside the
    // SERIAL1 lock previously coupled SERIAL1→CONSOLE→FB and deadlocked under SMP
    // (an FB-holder logging, or IRQ-context re-entrancy) — a real hang observed
    // during the raefs boot smoketest. Interrupts stay disabled so the brief FB
    // hold inside the glyph blit can't be re-entered by a logging interrupt.
    //
    // Suppressed once the desktop owns the screen (console_mirror_enabled() ->
    // false, set in shell_runner::activate_desktop): the compositor then owns the
    // framebuffer, so raw log glyphs blitting over it made the desktop untestable
    // on iron (T1745). The UART + RAM-ring writes above are unconditional, so
    // BOOTLOG.TXT / netlog still capture everything — only the on-screen mirror
    // stops.
    if crate::console::console_mirror_enabled() {
        interrupts::without_interrupts(|| {
            crate::console::try_print(args);
        });
    }
}

/// Write directly to COM1 ONLY — no bootlog RAM ring, no framebuffer-console
/// mirror. For high-volume machine-parse diagnostics (the end-of-boot procfs
/// snapshot) that would otherwise evict the boot transcript from the 1 MiB
/// bootlog ring right before the BOOTLOG.TXT flush, and would scroll
/// unreadably on a real screen. `uart_16550` polls THR-empty per byte, so
/// QEMU serial capture is lossless and byte-identical to serial_println's.
#[doc(hidden)]
pub fn _print_serial_only(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        let _ = SERIAL1.lock().write_fmt(args);
    });
}

/// Like `serial_println!` but COM1-only (no bootlog ring, no console mirror).
#[macro_export]
macro_rules! serial_only_println {
    () => ($crate::serial::_print_serial_only(format_args!("\n")));
    ($($arg:tt)*) => ($crate::serial::_print_serial_only(format_args!("{}\n", format_args!($($arg)*))));
}

/// Print to COM1 serial (no newline).
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

/// Print to COM1 serial with trailing newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}
