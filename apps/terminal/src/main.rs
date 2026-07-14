#![no_std]
#![no_main]

#[allow(unused_imports)]
use raekit;

use rae_tokens::{DARK, RAEBLUE};
use raegfx::text::FontFamily;
use raegfx::Canvas;

const WIN_W: usize = 640;
const WIN_H: usize = 400;
const SURFACE_VIRT: u64 = 0x0000_7800_0000;

const COLS: usize = 80;
const ROWS: usize = 50;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

// Generic chrome on `rae_tokens::DARK` + the RaeBlue accent ramp (whole-OS
// cohesion). `BG`/`FG` stay `const` because `Cell::blank()` is a `const fn`
// (the palette fields are const-accessible). The prompt/cursor accent is
// derived (non-const) so it lives in `accent()`. Per-cell SGR text colors are
// left untouched (the VT engine owns those). Live Vibe accent = NEEDS-INTERFACE.
const BG: u32 = DARK.bg_base; // terminal void (deepest layer)
const FG: u32 = DARK.text_primary; // default glyph color
const TITLE_BG: u32 = DARK.bg_overlay; // title strip

/// The live desktop accent seed (Vibe Mode) via `SYS_THEME_GET`, or RaeBlue when
/// the theme syscall is unavailable. Read at launch so the terminal re-skins to
/// the active theme (Concept §Customization Engine).
fn theme_seed() -> u32 {
    raekit::sys::theme_accent()
}
/// Prompt / cursor accent (live ramp base). Matches the desktop accent.
fn accent() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).base
}

struct Term {
    cells: [[Cell; COLS]; ROWS],
    cursor_row: usize,
    cursor_col: usize,
    shift_held: bool,
}

#[derive(Clone, Copy)]
struct Cell {
    ch: u8,
    fg: u32,
    bg: u32,
}

impl Cell {
    const fn blank() -> Self {
        Self {
            ch: b' ',
            fg: FG,
            bg: BG,
        }
    }
}

impl Term {
    fn new() -> Self {
        Self {
            cells: [[Cell::blank(); COLS]; ROWS],
            cursor_row: 0,
            cursor_col: 0,
            shift_held: false,
        }
    }

    fn put_char(&mut self, ch: u8, fg: u32) {
        if ch == b'\n' {
            self.cursor_col = 0;
            self.cursor_row += 1;
            if self.cursor_row >= ROWS {
                self.scroll_up();
                self.cursor_row = ROWS - 1;
            }
            return;
        }
        if ch == b'\r' {
            self.cursor_col = 0;
            return;
        }
        if ch == 0x08 {
            if self.cursor_col > 0 {
                self.cursor_col -= 1;
                self.cells[self.cursor_row][self.cursor_col] = Cell::blank();
            }
            return;
        }
        if self.cursor_col >= COLS {
            self.cursor_col = 0;
            self.cursor_row += 1;
            if self.cursor_row >= ROWS {
                self.scroll_up();
                self.cursor_row = ROWS - 1;
            }
        }
        if ch >= 0x20 {
            self.cells[self.cursor_row][self.cursor_col] = Cell { ch, fg, bg: BG };
            self.cursor_col += 1;
        }
    }

    fn put_bytes(&mut self, bytes: &[u8], fg: u32) {
        for &ch in bytes {
            self.put_char(ch, fg);
        }
    }

    fn scroll_up(&mut self) {
        for r in 1..ROWS {
            self.cells[r - 1] = self.cells[r];
        }
        self.cells[ROWS - 1] = [Cell::blank(); COLS];
    }

    fn clear(&mut self) {
        self.cells = [[Cell::blank(); COLS]; ROWS];
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    fn render(&self, canvas: &mut Canvas) {
        // Title strip: crisp AA, RaeMono (the terminal's monospace face).
        canvas.fill_rect(0, 0, WIN_W, 12, TITLE_BG);
        canvas.draw_text_aa(
            8,
            -1,
            "AthenaOS Terminal",
            rae_tokens::TYPE_CAPTION,
            DARK.text_secondary,
            FontFamily::Mono,
        );

        // Grid: the 80x50 cell matrix is a fixed-pitch 8px monospace grid — the
        // caret lands at `cursor_col * GLYPH_W` and each cell carries its own SGR
        // fg, so per-cell glyphs stay on the 8x8 bitmap font (same approach as the
        // live raeshell terminal grid). A proportional AA grid would desync the
        // caret + cell backgrounds from the glyphs.
        for row in 0..ROWS {
            let y = 12 + row * GLYPH_H;
            for col in 0..COLS {
                let x = col * GLYPH_W;
                let cell = &self.cells[row][col];
                if cell.ch != b' ' {
                    let s = [cell.ch];
                    if let Ok(txt) = core::str::from_utf8(&s) {
                        canvas.draw_text(x, y, txt, cell.fg, None);
                    }
                }
            }
        }

        let cx = self.cursor_col * GLYPH_W;
        let cy = 12 + self.cursor_row * GLYPH_H;
        canvas.fill_rect(cx, cy, 2, GLYPH_H, accent());
    }
}

fn scancode_to_ascii(code: u8, shift: bool) -> Option<u8> {
    #[rustfmt::skip]
    const UNSHIFTED: [u8; 58] = [
        0, 0x1B, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8',
        b'9', b'0', b'-', b'=', 0x08, b'\t', b'q', b'w', b'e', b'r',
        b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', 0,
        b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
        b'\'', b'`', 0, b'\\', b'z', b'x', b'c', b'v', b'b', b'n',
        b'm', b',', b'.', b'/', 0, b'*', 0, b' ',
    ];
    #[rustfmt::skip]
    const SHIFTED: [u8; 58] = [
        0, 0x1B, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*',
        b'(', b')', b'_', b'+', 0x08, b'\t', b'Q', b'W', b'E', b'R',
        b'T', b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', 0,
        b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
        b'"', b'~', 0, b'|', b'Z', b'X', b'C', b'V', b'B', b'N',
        b'M', b'<', b'>', b'?', 0, b'*', 0, b' ',
    ];
    if code >= 58 {
        return None;
    }
    let ch = if shift {
        SHIFTED[code as usize]
    } else {
        UNSHIFTED[code as usize]
    };
    if ch == 0 {
        None
    } else {
        Some(ch)
    }
}

fn drain_pty(term: &mut Term, pty_id: u64) {
    let mut buf = [0u8; 256];
    while raekit::sys::pty_poll(pty_id) > 0 {
        let n = raekit::sys::pty_read(pty_id, &mut buf);
        if n == 0 || n == u64::MAX {
            break;
        }
        term.put_bytes(&buf[..n as usize], FG);
    }
}

// ── Design proof (R10: a fail-able check the token wiring is correct) ─────
//
// `cargo test` can't host a libtest harness in this `#![no_main]` bin (raekit's
// `#[panic_handler]` + std's = duplicate lang item). This pure `rae_tokens`
// proof is the fail-able authority; the ramp is host-KAT'd by
// `cargo test -p rae_tokens`.

/// True iff the Terminal's generic chrome is wired to the shared tokens. (The
/// VT engine's per-cell SGR colors are deliberately out of scope.)
#[must_use]
pub fn design_proof() -> bool {
    let ramp = rae_tokens::derive_accent(theme_seed(), &DARK);
    accent() == ramp.base
        && BG == DARK.bg_base
        && FG == DARK.text_primary
        && TITLE_BG == DARK.bg_overlay
        && raekit::sys::THEME_DEFAULT_ACCENT == RAEBLUE
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // exit(4) reserved for a token-wiring regression (1..=3 are already used
    // below for surface/pty/spawn failures).
    if !design_proof() {
        raekit::sys::exit(4);
    }
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }
    let _ = raekit::sys::surface_focus(sid);

    let pty_id = raekit::sys::pty_open();
    if pty_id == u64::MAX {
        raekit::sys::exit(2);
    }

    let shell_pid = raekit::sys::spawn_pty("rae-sh", pty_id);
    if shell_pid == u64::MAX {
        raekit::sys::exit(3);
    }

    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };
    let mut term = Term::new();

    term.put_bytes(b"AthenaOS Terminal (PTY)\n", accent());
    term.put_bytes(b"Attached to rae-sh.\n\n", FG);

    canvas.clear(BG);
    term.render(&mut canvas);
    raekit::sys::surface_present(sid, 0, 0);

    loop {
        drain_pty(&mut term, pty_id);

        let key = raekit::sys::read_key();
        if key != 0 {
            let scancode = key as u8;
            let is_release = scancode & 0x80 != 0;
            let code = scancode & 0x7F;

            if code == 0x2A || code == 0x36 {
                term.shift_held = !is_release;
            } else if !is_release {
                if code == 0x0E {
                    let _ = raekit::sys::pty_write(pty_id, &[0x08]);
                } else if code == 0x1C {
                    let _ = raekit::sys::pty_write(pty_id, b"\n");
                } else if let Some(ascii) = scancode_to_ascii(code, term.shift_held) {
                    if ascii == b'\t' {
                        let _ = raekit::sys::pty_write(pty_id, b"\t");
                    } else if ascii >= 0x20 {
                        let _ = raekit::sys::pty_write(pty_id, &[ascii]);
                    }
                }
            }
        }

        drain_pty(&mut term, pty_id);

        canvas.clear(BG);
        term.render(&mut canvas);
        raekit::sys::surface_present(sid, 0, 0);
        raekit::sys::yield_now();
    }
}
