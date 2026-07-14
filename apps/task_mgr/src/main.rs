//! Task Manager — lists kernel tasks via SYS_PROCLIST and can end a process.

#![no_std]
#![no_main]

#[allow(unused_imports)]
use raekit;

use rae_tokens::{DARK, RAEBLUE};
use raegfx::text::FontFamily;
use raegfx::Canvas;

const WIN_W: usize = 720;
const WIN_H: usize = 440;
const SURFACE_VIRT: u64 = 0x0000_7C00_0000;

// Generic chrome on `rae_tokens::DARK` + the RaeBlue accent ramp (whole-OS
// cohesion). No app-specific colors. Live Vibe accent = NEEDS-INTERFACE (report).
const BG: u32 = DARK.bg_raised;
const TITLE_BG: u32 = DARK.bg_base;
const ROW_ALT: u32 = DARK.bg_elevated;
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_DIM: u32 = DARK.text_secondary;
const STATUS_BG: u32 = DARK.bg_base;

/// The live desktop accent seed (Vibe Mode) via `SYS_THEME_GET`, or RaeBlue when
/// the theme syscall is unavailable. Read at launch so Task Manager re-skins to
/// the active theme (Concept §Customization Engine).
fn theme_seed() -> u32 {
    raekit::sys::theme_accent()
}
/// Accent base (live ramp) — the SCHED_BODY class label.
fn accent() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).base
}
/// Selected-row wash: the accent's active (pressed) shade.
fn row_sel() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).active
}

const ENTRY_BYTES: usize = 24;
const MAX_TASKS: usize = 32;

#[derive(Clone, Copy)]
struct ProcRow {
    pid: u64,
    state: u8,
    priority: u8,
    vruntime: u64,
}

struct App {
    rows: [ProcRow; MAX_TASKS],
    count: usize,
    selected: usize,
    scroll: usize,
}

impl App {
    fn new() -> Self {
        Self {
            rows: [ProcRow {
                pid: 0,
                state: 0,
                priority: 0,
                vruntime: 0,
            }; MAX_TASKS],
            count: 0,
            selected: 0,
            scroll: 0,
        }
    }

    fn refresh(&mut self) {
        let mut buf = [0u8; ENTRY_BYTES * MAX_TASKS];
        let n = raekit::sys::proclist(&mut buf) as usize;
        self.count = 0;
        for i in 0..n.min(MAX_TASKS) {
            let off = i * ENTRY_BYTES;
            let chunk = &buf[off..off + ENTRY_BYTES];
            let pid = u64::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]);
            let state = chunk[8];
            let priority = chunk[9];
            let vruntime = u64::from_le_bytes([
                chunk[16], chunk[17], chunk[18], chunk[19], chunk[20], chunk[21], chunk[22],
                chunk[23],
            ]);
            self.rows[i] = ProcRow {
                pid,
                state,
                priority,
                vruntime,
            };
            self.count += 1;
        }
        if self.selected >= self.count && self.count > 0 {
            self.selected = self.count - 1;
        }
    }

    fn move_sel(&mut self, delta: i32) {
        if self.count == 0 {
            return;
        }
        let n = self.count as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(n)) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        let visible = (WIN_H - 80) / 28;
        if self.selected >= self.scroll + visible {
            self.scroll = self.selected.saturating_sub(visible - 1);
        }
    }

    fn kill_selected(&mut self) {
        if let Some(row) = self.rows.get(self.selected) {
            if row.pid != 0 {
                let _ = raekit::sys::kill(row.pid);
            }
        }
    }
}

fn state_label(state: u8) -> &'static str {
    match state {
        1 => "Ready",
        2 => "Blocked",
        3 => "Zombie",
        _ => "Running",
    }
}

fn render(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);
    canvas.fill_rect(0, 0, WIN_W, 28, TITLE_BG);
    canvas.draw_text_aa(
        12,
        ((28 - rae_tokens::TYPE_SUBTITLE.line_height as usize) / 2) as i32,
        "Task Manager",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );

    canvas.fill_rect(0, 28, WIN_W, 24, DARK.bg_overlay);
    let hdr_ty = (28 + (24 - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32;
    canvas.draw_text_aa(
        12,
        hdr_ty,
        "PID",
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        100,
        hdr_ty,
        "State",
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        200,
        hdr_ty,
        "Class",
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        320,
        hdr_ty,
        "vruntime",
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );

    let row_h = 28;
    let list_y = 52;
    let visible = (WIN_H - 52 - 24) / row_h;

    for (i, row) in app.rows[..app.count]
        .iter()
        .enumerate()
        .skip(app.scroll)
        .take(visible)
    {
        let y = list_y + (i - app.scroll) * row_h;
        if i == app.selected {
            canvas.fill_rect(0, y, WIN_W, row_h, row_sel());
        } else if i % 2 == 1 {
            canvas.fill_rect(0, y, WIN_W, row_h, ROW_ALT);
        }

        let row_ty = (y + (row_h - rae_tokens::TYPE_BODY.line_height as usize) / 2) as i32;
        let mut buf = [0u8; 16];
        let n = fmt_u64(row.pid, &mut buf);
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            canvas.draw_text_aa(
                12,
                row_ty,
                s,
                rae_tokens::TYPE_BODY,
                TEXT_FG,
                FontFamily::Sans,
            );
        }

        canvas.draw_text_aa(
            100,
            row_ty,
            state_label(row.state),
            rae_tokens::TYPE_BODY,
            TEXT_FG,
            FontFamily::Sans,
        );
        let class = if row.priority != 0 { "Game" } else { "Normal" };
        canvas.draw_text_aa(
            200,
            row_ty,
            class,
            rae_tokens::TYPE_BODY,
            accent(),
            FontFamily::Sans,
        );

        let mut vbuf = [0u8; 20];
        let vn = fmt_u64(row.vruntime, &mut vbuf);
        if let Ok(vs) = core::str::from_utf8(&vbuf[..vn]) {
            canvas.draw_text_aa(
                320,
                row_ty,
                vs,
                rae_tokens::TYPE_BODY,
                TEXT_DIM,
                FontFamily::Sans,
            );
        }
    }

    let st_y = WIN_H - 24;
    canvas.fill_rect(0, st_y, WIN_W, 24, STATUS_BG);
    canvas.draw_text_aa(
        12,
        (st_y + (24 - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32,
        "Up/Down: select   Del: end task   R: refresh   Esc: close",
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
}

fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    let mut n = 0;
    while i > 0 {
        i -= 1;
        out[n] = tmp[i];
        n += 1;
    }
    n
}

// ── Design proof (R10: a fail-able check the token wiring is correct) ─────
//
// `cargo test` can't host a libtest harness in this `#![no_main]` bin (raekit's
// `#[panic_handler]` + std's = duplicate lang item). This pure `rae_tokens`
// proof is the fail-able authority; the ramp is host-KAT'd by
// `cargo test -p rae_tokens`.

/// True iff Task Manager's chrome is wired to the shared design tokens.
#[must_use]
pub fn design_proof() -> bool {
    let ramp = rae_tokens::derive_accent(theme_seed(), &DARK);
    accent() == ramp.base
        && row_sel() == ramp.active
        && BG == DARK.bg_raised
        && TITLE_BG == DARK.bg_base
        && ROW_ALT == DARK.bg_elevated
        && TEXT_FG == DARK.text_primary
        && TEXT_DIM == DARK.text_secondary
        && raekit::sys::THEME_DEFAULT_ACCENT == RAEBLUE
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    if !design_proof() {
        raekit::sys::exit(3);
    }
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }

    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };
    let mut app = App::new();
    app.refresh();
    render(&app, &mut canvas);
    raekit::sys::surface_present(sid, 200, 70);

    let mut extended = false;
    let mut tick: u64 = 0;

    loop {
        tick = tick.wrapping_add(1);
        if tick % 500_000 == 0 {
            app.refresh();
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, 200, 70);
        }

        let key = raekit::sys::read_key();
        if key == 0 {
            raekit::sys::yield_now();
            continue;
        }

        let sc = key as u8;
        if sc == 0xE0 {
            extended = true;
            continue;
        }
        let ext = core::mem::replace(&mut extended, false);
        if sc & 0x80 != 0 {
            continue;
        }
        let code = sc & 0x7F;

        let mut dirty = false;
        match (ext, code) {
            (true, 0x48) => {
                app.move_sel(-1);
                dirty = true;
            }
            (true, 0x50) => {
                app.move_sel(1);
                dirty = true;
            }
            (false, 0x53) => {
                app.kill_selected();
                app.refresh();
                dirty = true;
            }
            (false, 0x13) => {
                app.refresh();
                dirty = true;
            }
            (false, 0x01) => raekit::sys::exit(0),
            _ => {}
        }

        if dirty {
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, 200, 70);
        }
    }
}
