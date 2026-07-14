//! VT100/xterm-compatible terminal emulator for AthenaOS.
//!
//! Processes raw byte streams, interprets ANSI/VT100/xterm escape sequences,
//! and renders the result to a Canvas via the 8×8 bitmap font.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ── Colour palette ───────────────────────────────────────────────────────

const TERM_BG: u32 = 0xFF_0A_0E_1A;
const TERM_FG: u32 = 0xFF_D0_D0_E0;
const TERM_CURSOR: u32 = 0xFF_4E_9C_FF;
const TERM_SELECT: u32 = 0xFF_33_55_88;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

fn default_palette() -> [u32; 256] {
    let mut p = [0u32; 256];
    // Standard 16 colors (dark then bright)
    p[0] = 0xFF_00_00_00; // black
    p[1] = 0xFF_CC_33_33; // red
    p[2] = 0xFF_33_CC_33; // green
    p[3] = 0xFF_CC_CC_33; // yellow
    p[4] = 0xFF_33_66_CC; // blue
    p[5] = 0xFF_CC_33_CC; // magenta
    p[6] = 0xFF_33_CC_CC; // cyan
    p[7] = 0xFF_CC_CC_CC; // white
    p[8] = 0xFF_66_66_66; // bright black
    p[9] = 0xFF_FF_66_66; // bright red
    p[10] = 0xFF_66_FF_66; // bright green
    p[11] = 0xFF_FF_FF_66; // bright yellow
    p[12] = 0xFF_66_99_FF; // bright blue
    p[13] = 0xFF_FF_66_FF; // bright magenta
    p[14] = 0xFF_66_FF_FF; // bright cyan
    p[15] = 0xFF_FF_FF_FF; // bright white

    // 216-color cube (indices 16..231)
    for r in 0..6u32 {
        for g in 0..6u32 {
            for b in 0..6u32 {
                let idx = 16 + r * 36 + g * 6 + b;
                let rv = if r == 0 { 0 } else { 55 + r * 40 };
                let gv = if g == 0 { 0 } else { 55 + g * 40 };
                let bv = if b == 0 { 0 } else { 55 + b * 40 };
                p[idx as usize] = 0xFF_00_00_00 | (rv << 16) | (gv << 8) | bv;
            }
        }
    }
    // 24-step greyscale ramp (indices 232..255)
    for i in 0..24u32 {
        let v = 8 + i * 10;
        p[(232 + i) as usize] = 0xFF_00_00_00 | (v << 16) | (v << 8) | v;
    }
    p
}

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Underline,
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseTrackingMode {
    None,
    X10,
    Normal,
    Highlight,
    ButtonEvent,
    AnyEvent,
}

#[derive(Debug, Clone, Copy)]
pub struct CellAttributes {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

impl CellAttributes {
    pub const fn default() -> Self {
        Self {
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            blink: false,
            inverse: false,
            hidden: false,
            strikethrough: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub character: char,
    pub fg: u32,
    pub bg: u32,
    pub attrs: CellAttributes,
}

impl Cell {
    pub fn blank() -> Self {
        Self {
            character: ' ',
            fg: TERM_FG,
            bg: TERM_BG,
            attrs: CellAttributes::default(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub x: usize,
    pub y: usize,
    pub visible: bool,
    pub style: CursorStyle,
}

pub struct TerminalMode {
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub auto_wrap: bool,
    pub insert_mode: bool,
    pub origin_mode: bool,
    pub bracketed_paste: bool,
    pub mouse_tracking: MouseTrackingMode,
    pub alternate_screen: bool,
}

impl TerminalMode {
    pub fn new() -> Self {
        Self {
            application_cursor: false,
            application_keypad: false,
            auto_wrap: true,
            insert_mode: false,
            origin_mode: false,
            bracketed_paste: false,
            mouse_tracking: MouseTrackingMode::None,
            alternate_screen: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

pub struct TerminalColors {
    pub palette: [u32; 256],
}

impl TerminalColors {
    pub fn new() -> Self {
        Self {
            palette: default_palette(),
        }
    }
}

pub struct TerminalBuffer {
    pub cells: Vec<Vec<Cell>>,
    pub scrollback: Vec<Vec<Cell>>,
    pub max_scrollback: usize,
}

impl TerminalBuffer {
    pub fn new(width: usize, height: usize, max_scrollback: usize) -> Self {
        let cells = (0..height)
            .map(|_| (0..width).map(|_| Cell::blank()).collect())
            .collect();
        Self {
            cells,
            scrollback: Vec::new(),
            max_scrollback,
        }
    }

    fn ensure_row(&mut self, y: usize, width: usize) {
        while self.cells.len() <= y {
            self.cells.push((0..width).map(|_| Cell::blank()).collect());
        }
    }
}

// ── Escape-sequence parser states ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    Ground,
    Escape,
    Csi,
    Osc,
    Utf8 { remaining: u8, codepoint: u32 },
}

// ── Terminal ─────────────────────────────────────────────────────────────

pub struct Terminal {
    pub buffer: TerminalBuffer,
    pub cursor: CursorState,
    pub scroll_region: (usize, usize),
    pub attrs: CellAttributes,
    pub saved_cursor: Option<CursorState>,
    pub mode: TerminalMode,
    pub tab_stops: Vec<usize>,
    pub title: String,
    pub input_buffer: Vec<u8>,
    pub output_buffer: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub scroll_offset: i32,
    pub history_lines: usize,
    pub selection: Option<Selection>,
    pub colors: TerminalColors,

    parse_state: ParseState,
    csi_params: Vec<u16>,
    csi_intermediate: Vec<u8>,
    osc_string: String,
    saved_attrs: CellAttributes,
    /// Active SGR foreground/background colours (ARGB). Set by `sgr()` and
    /// stamped onto every printed cell. Before 2026-06-11 these did not exist —
    /// `put_char` hardcoded the defaults, so SGR colour codes (`\x1b[31m`, …)
    /// were parsed but never rendered, and all program output was monochrome.
    current_fg: u32,
    current_bg: u32,
}

impl Terminal {
    pub fn new(width: usize, height: usize) -> Self {
        let mut tab_stops = Vec::new();
        let mut col = 0;
        while col < width {
            tab_stops.push(col);
            col += 8;
        }
        Self {
            buffer: TerminalBuffer::new(width, height, 10_000),
            cursor: CursorState {
                x: 0,
                y: 0,
                visible: true,
                style: CursorStyle::Block,
            },
            scroll_region: (0, height.saturating_sub(1)),
            attrs: CellAttributes::default(),
            saved_cursor: None,
            mode: TerminalMode::new(),
            tab_stops,
            title: String::from("Terminal"),
            input_buffer: Vec::new(),
            output_buffer: Vec::new(),
            width,
            height,
            scroll_offset: 0,
            history_lines: 0,
            selection: None,
            colors: TerminalColors::new(),
            parse_state: ParseState::Ground,
            csi_params: Vec::new(),
            csi_intermediate: Vec::new(),
            osc_string: String::new(),
            saved_attrs: CellAttributes::default(),
            current_fg: TERM_FG,
            current_bg: TERM_BG,
        }
    }

    // ── Main entry point for incoming data ───────────────────────────

    pub fn process_byte(&mut self, byte: u8) {
        match self.parse_state {
            ParseState::Ground => self.ground_byte(byte),
            ParseState::Escape => self.process_escape(byte),
            ParseState::Csi => self.process_csi(byte),
            ParseState::Osc => self.process_osc(byte),
            ParseState::Utf8 {
                remaining,
                codepoint,
            } => {
                if byte & 0xC0 != 0x80 {
                    self.parse_state = ParseState::Ground;
                    self.put_char('?');
                    return;
                }
                let cp = (codepoint << 6) | (byte as u32 & 0x3F);
                if remaining == 1 {
                    self.parse_state = ParseState::Ground;
                    let ch = char::from_u32(cp).unwrap_or('?');
                    self.put_char(ch);
                } else {
                    self.parse_state = ParseState::Utf8 {
                        remaining: remaining - 1,
                        codepoint: cp,
                    };
                }
            }
        }
    }

    pub fn process_bytes(&mut self, data: &[u8]) {
        for &b in data {
            self.process_byte(b);
        }
    }

    fn ground_byte(&mut self, byte: u8) {
        match byte {
            0x00 => {} // NUL — ignore
            0x07 => {} // BEL
            0x08 => self.backspace(),
            0x09 => self.tab(),
            0x0A | 0x0B | 0x0C => self.new_line(),
            0x0D => self.carriage_return(),
            0x1B => self.parse_state = ParseState::Escape,
            0x20..=0x7E => self.put_char(byte as char),
            0xC0..=0xDF => {
                self.parse_state = ParseState::Utf8 {
                    remaining: 1,
                    codepoint: (byte as u32) & 0x1F,
                };
            }
            0xE0..=0xEF => {
                self.parse_state = ParseState::Utf8 {
                    remaining: 2,
                    codepoint: (byte as u32) & 0x0F,
                };
            }
            0xF0..=0xF7 => {
                self.parse_state = ParseState::Utf8 {
                    remaining: 3,
                    codepoint: (byte as u32) & 0x07,
                };
            }
            _ => {}
        }
    }

    // ── ESC sequences ────────────────────────────────────────────────

    fn process_escape(&mut self, byte: u8) {
        self.parse_state = ParseState::Ground;
        match byte {
            b'[' => {
                self.parse_state = ParseState::Csi;
                self.csi_params.clear();
                self.csi_intermediate.clear();
            }
            b']' => {
                self.parse_state = ParseState::Osc;
                self.osc_string.clear();
            }
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.index_down(),
            b'M' => self.reverse_index(),
            b'E' => {
                self.carriage_return();
                self.new_line();
            }
            b'c' => self.reset(),
            b'H' => self.set_tab_stop(),
            _ => {}
        }
    }

    // ── CSI sequences ────────────────────────────────────────────────

    fn process_csi(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                let digit = (byte - b'0') as u16;
                if let Some(last) = self.csi_params.last_mut() {
                    *last = last.saturating_mul(10).saturating_add(digit);
                } else {
                    self.csi_params.push(digit);
                }
            }
            b';' => {
                if self.csi_params.is_empty() {
                    self.csi_params.push(0);
                }
                self.csi_params.push(0);
            }
            b' ' | b'?' | b'!' | b'>' => {
                self.csi_intermediate.push(byte);
            }
            _ => {
                self.parse_state = ParseState::Ground;
                let is_private = self.csi_intermediate.contains(&b'?');
                self.dispatch_csi(byte, is_private);
            }
        }
    }

    fn dispatch_csi(&mut self, final_byte: u8, private: bool) {
        let p =
            |i: usize, def: u16| -> u16 { self.csi_params.get(i).copied().unwrap_or(def).max(def) };

        if private {
            match final_byte {
                b'h' => {
                    let params: Vec<u16> = self.csi_params.clone();
                    for &param in &params {
                        self.set_dec_mode(param, true);
                    }
                }
                b'l' => {
                    let params: Vec<u16> = self.csi_params.clone();
                    for &param in &params {
                        self.set_dec_mode(param, false);
                    }
                }
                _ => {}
            }
            return;
        }

        match final_byte {
            b'A' => {
                let n = p(0, 1) as usize;
                self.cursor.y = self.cursor.y.saturating_sub(n);
            }
            b'B' => {
                let n = p(0, 1) as usize;
                self.cursor.y = (self.cursor.y + n).min(self.height - 1);
            }
            b'C' => {
                let n = p(0, 1) as usize;
                self.cursor.x = (self.cursor.x + n).min(self.width - 1);
            }
            b'D' => {
                let n = p(0, 1) as usize;
                self.cursor.x = self.cursor.x.saturating_sub(n);
            }
            b'E' => {
                let n = p(0, 1) as usize;
                self.cursor.x = 0;
                self.cursor.y = (self.cursor.y + n).min(self.height - 1);
            }
            b'F' => {
                let n = p(0, 1) as usize;
                self.cursor.x = 0;
                self.cursor.y = self.cursor.y.saturating_sub(n);
            }
            b'G' => {
                let col = p(0, 1) as usize;
                self.cursor.x = col.saturating_sub(1).min(self.width - 1);
            }
            b'H' | b'f' => {
                let row = p(0, 1) as usize;
                let col = if self.csi_params.len() > 1 {
                    p(1, 1) as usize
                } else {
                    1
                };
                self.set_cursor_pos(row.saturating_sub(1), col.saturating_sub(1));
            }
            b'J' => {
                let mode = p(0, 0);
                self.erase_display(mode);
            }
            b'K' => {
                let mode = p(0, 0);
                self.erase_line(mode);
            }
            b'L' => {
                let n = p(0, 1) as usize;
                for _ in 0..n {
                    self.insert_line();
                }
            }
            b'M' => {
                let n = p(0, 1) as usize;
                for _ in 0..n {
                    self.delete_line();
                }
            }
            b'@' => {
                let n = p(0, 1) as usize;
                self.insert_characters(n);
            }
            b'P' => {
                let n = p(0, 1) as usize;
                self.delete_characters(n);
            }
            b'X' => {
                let n = p(0, 1) as usize;
                self.erase_characters(n);
            }
            b'S' => {
                let n = p(0, 1) as usize;
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            b'T' => {
                let n = p(0, 1) as usize;
                for _ in 0..n {
                    self.scroll_down();
                }
            }
            b'd' => {
                let row = p(0, 1) as usize;
                self.cursor.y = row.saturating_sub(1).min(self.height - 1);
            }
            b'm' => self.sgr(),
            b'r' => {
                let top = p(0, 1) as usize;
                let bot = if self.csi_params.len() > 1 {
                    p(1, self.height as u16) as usize
                } else {
                    self.height
                };
                self.scroll_region = (
                    top.saturating_sub(1),
                    bot.saturating_sub(1).min(self.height - 1),
                );
                self.cursor.x = 0;
                self.cursor.y = 0;
            }
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b'n' => {
                if p(0, 0) == 6 {
                    let response =
                        alloc::format!("\x1B[{};{}R", self.cursor.y + 1, self.cursor.x + 1);
                    self.input_buffer.extend_from_slice(response.as_bytes());
                }
            }
            _ => {}
        }
    }

    fn set_dec_mode(&mut self, param: u16, enabled: bool) {
        match param {
            1 => self.mode.application_cursor = enabled,
            7 => self.mode.auto_wrap = enabled,
            12 => {} // blinking cursor — cosmetic only
            25 => self.cursor.visible = enabled,
            1000 => {
                self.mode.mouse_tracking = if enabled {
                    MouseTrackingMode::Normal
                } else {
                    MouseTrackingMode::None
                }
            }
            1002 => {
                self.mode.mouse_tracking = if enabled {
                    MouseTrackingMode::ButtonEvent
                } else {
                    MouseTrackingMode::None
                }
            }
            1003 => {
                self.mode.mouse_tracking = if enabled {
                    MouseTrackingMode::AnyEvent
                } else {
                    MouseTrackingMode::None
                }
            }
            1049 => self.mode.alternate_screen = enabled,
            2004 => self.mode.bracketed_paste = enabled,
            _ => {}
        }
    }

    // ── OSC sequences ────────────────────────────────────────────────

    fn process_osc(&mut self, byte: u8) {
        match byte {
            0x07 => {
                self.parse_state = ParseState::Ground;
                self.dispatch_osc();
            }
            0x1B => {
                self.parse_state = ParseState::Ground;
                self.dispatch_osc();
            }
            _ => {
                if self.osc_string.len() < 512 {
                    self.osc_string.push(byte as char);
                }
            }
        }
    }

    fn dispatch_osc(&mut self) {
        if let Some(sep) = self.osc_string.find(';') {
            let cmd = &self.osc_string[..sep];
            let arg = &self.osc_string[sep + 1..];
            match cmd {
                "0" | "2" => {
                    self.title.clear();
                    self.title.push_str(arg);
                }
                _ => {}
            }
        }
    }

    // ── SGR (Select Graphic Rendition) ───────────────────────────────

    fn sgr(&mut self) {
        if self.csi_params.is_empty() {
            self.attrs.reset();
            self.current_fg = TERM_FG;
            self.current_bg = TERM_BG;
            return;
        }

        let mut i = 0;
        while i < self.csi_params.len() {
            let code = self.csi_params[i];
            match code {
                0 => {
                    self.attrs.reset();
                    self.current_fg = TERM_FG;
                    self.current_bg = TERM_BG;
                }
                1 => self.attrs.bold = true,
                2 => self.attrs.dim = true,
                3 => self.attrs.italic = true,
                4 => self.attrs.underline = true,
                5 => self.attrs.blink = true,
                7 => self.attrs.inverse = true,
                8 => self.attrs.hidden = true,
                9 => self.attrs.strikethrough = true,
                22 => {
                    self.attrs.bold = false;
                    self.attrs.dim = false;
                }
                23 => self.attrs.italic = false,
                24 => self.attrs.underline = false,
                25 => self.attrs.blink = false,
                27 => self.attrs.inverse = false,
                28 => self.attrs.hidden = false,
                29 => self.attrs.strikethrough = false,
                // Foreground: normal (30-37) + bright (90-97) + default (39).
                30..=37 => self.current_fg = self.colors.palette[(code - 30) as usize],
                90..=97 => self.current_fg = self.colors.palette[(code - 90 + 8) as usize],
                39 => self.current_fg = TERM_FG,
                // Background: normal (40-47) + bright (100-107) + default (49).
                40..=47 => self.current_bg = self.colors.palette[(code - 40) as usize],
                100..=107 => self.current_bg = self.colors.palette[(code - 100 + 8) as usize],
                49 => self.current_bg = TERM_BG,
                // Extended colour: 38/48 ; 5 ; N (256) or ; 2 ; r ; g ; b (truecolor).
                38 | 48 => {
                    let is_fg = code == 38;
                    let mode = self.csi_params.get(i + 1).copied().unwrap_or(0);
                    let color = if mode == 5 {
                        let n = self.csi_params.get(i + 2).copied().unwrap_or(0) as usize;
                        i += 2;
                        self.colors.palette[n & 0xFF]
                    } else if mode == 2 {
                        let r = self.csi_params.get(i + 2).copied().unwrap_or(0) as u32 & 0xFF;
                        let g = self.csi_params.get(i + 3).copied().unwrap_or(0) as u32 & 0xFF;
                        let b = self.csi_params.get(i + 4).copied().unwrap_or(0) as u32 & 0xFF;
                        i += 4;
                        0xFF00_0000 | (r << 16) | (g << 8) | b
                    } else {
                        if is_fg {
                            self.current_fg
                        } else {
                            self.current_bg
                        }
                    };
                    if is_fg {
                        self.current_fg = color;
                    } else {
                        self.current_bg = color;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    // ── Character output ─────────────────────────────────────────────

    fn put_char(&mut self, ch: char) {
        if self.cursor.x >= self.width {
            if self.mode.auto_wrap {
                self.cursor.x = 0;
                self.new_line();
            } else {
                self.cursor.x = self.width - 1;
            }
        }

        self.buffer.ensure_row(self.cursor.y, self.width);
        if self.cursor.x < self.buffer.cells[self.cursor.y].len() {
            // `inverse` (SGR 7) swaps fg/bg at render time, matching xterm.
            let (fg, bg) = if self.attrs.inverse {
                (self.current_bg, self.current_fg)
            } else {
                (self.current_fg, self.current_bg)
            };
            self.buffer.cells[self.cursor.y][self.cursor.x] = Cell {
                character: ch,
                fg,
                bg,
                attrs: self.attrs,
            };
        }
        self.cursor.x += 1;
    }

    // ── Cursor movement helpers ──────────────────────────────────────

    pub fn set_cursor_pos(&mut self, row: usize, col: usize) {
        self.cursor.y = row.min(self.height.saturating_sub(1));
        self.cursor.x = col.min(self.width.saturating_sub(1));
    }

    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor);
        self.saved_attrs = self.attrs;
    }

    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor {
            self.cursor = saved;
            self.attrs = self.saved_attrs;
        }
    }

    // ── Line operations ──────────────────────────────────────────────

    pub fn new_line(&mut self) {
        if self.cursor.y >= self.scroll_region.1 {
            self.scroll_up();
        } else {
            self.cursor.y += 1;
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor.x = 0;
    }

    pub fn backspace(&mut self) {
        if self.cursor.x > 0 {
            self.cursor.x -= 1;
        }
    }

    pub fn tab(&mut self) {
        let next = self.tab_stops.iter().find(|&&t| t > self.cursor.x);
        self.cursor.x = next
            .copied()
            .unwrap_or(self.width.saturating_sub(1))
            .min(self.width - 1);
    }

    fn set_tab_stop(&mut self) {
        if !self.tab_stops.contains(&self.cursor.x) {
            self.tab_stops.push(self.cursor.x);
            self.tab_stops.sort();
        }
    }

    fn index_down(&mut self) {
        if self.cursor.y >= self.scroll_region.1 {
            self.scroll_up();
        } else {
            self.cursor.y += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor.y <= self.scroll_region.0 {
            self.scroll_down();
        } else {
            self.cursor.y -= 1;
        }
    }

    // ── Scroll ───────────────────────────────────────────────────────

    pub fn scroll_up(&mut self) {
        let (top, bot) = self.scroll_region;
        if top < self.buffer.cells.len() && bot < self.buffer.cells.len() && top <= bot {
            let removed = self.buffer.cells.remove(top);
            if top == 0 {
                self.buffer.scrollback.push(removed);
                if self.buffer.scrollback.len() > self.buffer.max_scrollback {
                    self.buffer.scrollback.remove(0);
                }
                self.history_lines += 1;
            }
            let blank_row: Vec<Cell> = (0..self.width).map(|_| Cell::blank()).collect();
            if bot < self.buffer.cells.len() {
                self.buffer.cells.insert(bot, blank_row);
            } else {
                self.buffer.cells.push(blank_row);
            }
        }
    }

    pub fn scroll_down(&mut self) {
        let (top, bot) = self.scroll_region;
        if top < self.buffer.cells.len() && bot < self.buffer.cells.len() && top <= bot {
            if bot < self.buffer.cells.len() {
                self.buffer.cells.remove(bot);
            }
            let blank_row: Vec<Cell> = (0..self.width).map(|_| Cell::blank()).collect();
            self.buffer.cells.insert(top, blank_row);
        }
    }

    // ── Erase ────────────────────────────────────────────────────────

    pub fn erase_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // Cursor to end
                self.erase_line(0);
                for y in self.cursor.y + 1..self.height {
                    if y < self.buffer.cells.len() {
                        for cell in &mut self.buffer.cells[y] {
                            *cell = Cell::blank();
                        }
                    }
                }
            }
            1 => {
                // Start to cursor
                self.erase_line(1);
                for y in 0..self.cursor.y {
                    if y < self.buffer.cells.len() {
                        for cell in &mut self.buffer.cells[y] {
                            *cell = Cell::blank();
                        }
                    }
                }
            }
            2 | 3 => {
                for row in &mut self.buffer.cells {
                    for cell in row {
                        *cell = Cell::blank();
                    }
                }
            }
            _ => {}
        }
    }

    pub fn erase_line(&mut self, mode: u16) {
        if self.cursor.y >= self.buffer.cells.len() {
            return;
        }
        let row = &mut self.buffer.cells[self.cursor.y];
        match mode {
            0 => {
                for x in self.cursor.x..row.len() {
                    row[x] = Cell::blank();
                }
            }
            1 => {
                for x in 0..=self.cursor.x.min(row.len().saturating_sub(1)) {
                    row[x] = Cell::blank();
                }
            }
            2 => {
                for cell in row.iter_mut() {
                    *cell = Cell::blank();
                }
            }
            _ => {}
        }
    }

    pub fn erase_characters(&mut self, n: usize) {
        if self.cursor.y >= self.buffer.cells.len() {
            return;
        }
        let row = &mut self.buffer.cells[self.cursor.y];
        for x in self.cursor.x..self.cursor.x.saturating_add(n).min(row.len()) {
            row[x] = Cell::blank();
        }
    }

    pub fn insert_line(&mut self) {
        let y = self.cursor.y;
        let bot = self.scroll_region.1;
        if y <= bot && bot < self.buffer.cells.len() {
            self.buffer.cells.remove(bot);
            let blank_row = (0..self.width).map(|_| Cell::blank()).collect();
            self.buffer.cells.insert(y, blank_row);
        }
    }

    pub fn delete_line(&mut self) {
        let y = self.cursor.y;
        let bot = self.scroll_region.1;
        if y < self.buffer.cells.len() && y <= bot {
            self.buffer.cells.remove(y);
            let blank_row = (0..self.width).map(|_| Cell::blank()).collect();
            if bot < self.buffer.cells.len() {
                self.buffer.cells.insert(bot, blank_row);
            } else {
                self.buffer.cells.push(blank_row);
            }
        }
    }

    pub fn insert_characters(&mut self, n: usize) {
        if self.cursor.y >= self.buffer.cells.len() {
            return;
        }
        let row = &mut self.buffer.cells[self.cursor.y];
        for _ in 0..n {
            if self.cursor.x < row.len() {
                row.insert(self.cursor.x, Cell::blank());
                row.truncate(self.width);
            }
        }
    }

    pub fn delete_characters(&mut self, n: usize) {
        if self.cursor.y >= self.buffer.cells.len() {
            return;
        }
        let row = &mut self.buffer.cells[self.cursor.y];
        for _ in 0..n {
            if self.cursor.x < row.len() {
                row.remove(self.cursor.x);
                row.push(Cell::blank());
            }
        }
    }

    fn reset(&mut self) {
        self.attrs.reset();
        self.cursor = CursorState {
            x: 0,
            y: 0,
            visible: true,
            style: CursorStyle::Block,
        };
        self.scroll_region = (0, self.height.saturating_sub(1));
        self.mode = TerminalMode::new();
        self.erase_display(2);
    }

    // ── Render ───────────────────────────────────────────────────────

    pub fn render(&self, canvas: &mut raegfx::Canvas, ox: usize, oy: usize) {
        canvas.fill_rect(ox, oy, self.width * GLYPH_W, self.height * GLYPH_H, TERM_BG);

        for row in 0..self.height {
            if row >= self.buffer.cells.len() {
                break;
            }
            let cells = &self.buffer.cells[row];
            for col in 0..self.width {
                if col >= cells.len() {
                    break;
                }
                let cell = &cells[col];
                let px = ox + col * GLYPH_W;
                let py = oy + row * GLYPH_H;

                let (fg, bg) = if cell.attrs.inverse {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };

                if bg != TERM_BG {
                    canvas.fill_rect(px, py, GLYPH_W, GLYPH_H, bg);
                }

                if cell.character != ' ' && !cell.attrs.hidden {
                    canvas.draw_glyph(px, py, cell.character, fg, None);
                }

                if cell.attrs.underline {
                    for ux in px..px + GLYPH_W {
                        canvas.draw_pixel(ux, py + GLYPH_H - 1, fg);
                    }
                }

                if cell.attrs.strikethrough {
                    for sx in px..px + GLYPH_W {
                        canvas.draw_pixel(sx, py + GLYPH_H / 2, fg);
                    }
                }
            }
        }

        // Selection highlight
        if let Some(sel) = &self.selection {
            let (sy, sx) = sel.start;
            let (ey, ex) = sel.end;
            for row in sy..=ey {
                let start_col = if row == sy { sx } else { 0 };
                let end_col = if row == ey { ex } else { self.width };
                for col in start_col..end_col {
                    let px = ox + col * GLYPH_W;
                    let py = oy + row * GLYPH_H;
                    canvas.fill_rect(px, py, GLYPH_W, GLYPH_H, TERM_SELECT);
                }
            }
        }

        // Cursor
        if self.cursor.visible && self.cursor.y < self.height && self.cursor.x < self.width {
            let cx = ox + self.cursor.x * GLYPH_W;
            let cy = oy + self.cursor.y * GLYPH_H;
            match self.cursor.style {
                CursorStyle::Block => {
                    canvas.fill_rect(cx, cy, GLYPH_W, GLYPH_H, TERM_CURSOR);
                    if self.cursor.y < self.buffer.cells.len() {
                        let row = &self.buffer.cells[self.cursor.y];
                        if self.cursor.x < row.len() {
                            let ch = row[self.cursor.x].character;
                            if ch != ' ' {
                                canvas.draw_glyph(cx, cy, ch, TERM_BG, None);
                            }
                        }
                    }
                }
                CursorStyle::Underline => {
                    for ux in cx..cx + GLYPH_W {
                        canvas.draw_pixel(ux, cy + GLYPH_H - 1, TERM_CURSOR);
                        canvas.draw_pixel(ux, cy + GLYPH_H - 2, TERM_CURSOR);
                    }
                }
                CursorStyle::Bar => {
                    for uy in cy..cy + GLYPH_H {
                        canvas.draw_pixel(cx, uy, TERM_CURSOR);
                        canvas.draw_pixel(cx + 1, uy, TERM_CURSOR);
                    }
                }
            }
        }
    }
}

/// Exercise the VT100/ANSI escape parser against known sequences and report
/// the sub-results (text, CUP absolute positioning, SGR colour, ED erase).
/// This is the first test coverage for the 1000-line terminal parser, which
/// was previously only validated by eyeballing the shell — the kernel calls it
/// at boot and logs `[raeshell] terminal smoketest: ...`. Returns
/// `(text_ok, cup_ok, sgr_ok, ed_ok)`.
pub fn run_smoketest() -> (bool, bool, bool, bool) {
    let mut term = Terminal::new(20, 5);

    // 1. Plain text lands left-to-right and advances the cursor.
    term.process_bytes(b"hello");
    let text_ok = term.buffer.cells[0][0].character == 'h'
        && term.buffer.cells[0][4].character == 'o'
        && term.cursor.x == 5;

    // 2. CUP (ESC[row;colH, 1-based) positions the cursor; next glyph lands there.
    term.process_bytes(b"\x1b[2;3HX"); // row 2, col 3 -> cells[1][2]
    let cup_ok = term.buffer.cells[1][2].character == 'X';

    // 3. SGR: home, set red foreground, print — the cell's fg must change from
    //    the default and the glyph must NOT be a raw escape byte.
    term.process_bytes(b"\x1b[H");
    let default_fg = Cell::blank().fg;
    term.process_bytes(b"\x1b[31mR");
    let sgr_ok =
        term.buffer.cells[0][0].character == 'R' && term.buffer.cells[0][0].fg != default_fg;

    // 4. ED (ESC[2J) clears the screen; a previously-written cell goes blank.
    term.process_bytes(b"\x1b[2J");
    let ed_ok = term.buffer.cells[1][2].character == ' ';

    (text_ok, cup_ok, sgr_ok, ed_ok)
}
