//! Full VT100/xterm-compatible terminal emulator for RaeenOS.
//!
//! Implements: CSI/SGR/OSC/DCS parsing, 256-colour + 24-bit true-colour,
//! primary & alternate screen buffers, scrollback, character sets, DEC
//! private modes, mouse tracking, PTY abstraction, tabs, splits, shell
//! integration, search, selection, and colour schemes.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ── Colour palette ───────────────────────────────────────────────────────

const TERM_BG: u32 = 0xFF_0A_0E_1A;
const TERM_FG: u32 = 0xFF_D0_D0_E0;
const TERM_CURSOR: u32 = 0xFF_4E_9C_FF;
const TERM_SELECT: u32 = 0xFF_33_55_88;
const TAB_BG: u32 = 0xFF_14_16_22;
const TAB_ACTIVE: u32 = 0xFF_22_24_38;
const TAB_FG: u32 = 0xFF_C0_C0_D0;
const TAB_ACCENT: u32 = 0xFF_4E_9C_FF;
const SPLIT_BORDER: u32 = 0xFF_33_33_55;
const SEARCH_MATCH: u32 = 0xFF_FF_CC_00;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

fn default_256_palette() -> [u32; 256] {
    let mut p = [0u32; 256];
    p[0] = 0xFF_00_00_00;
    p[1] = 0xFF_CC_33_33;
    p[2] = 0xFF_33_CC_33;
    p[3] = 0xFF_CC_CC_33;
    p[4] = 0xFF_33_66_CC;
    p[5] = 0xFF_CC_33_CC;
    p[6] = 0xFF_33_CC_CC;
    p[7] = 0xFF_CC_CC_CC;
    p[8] = 0xFF_66_66_66;
    p[9] = 0xFF_FF_66_66;
    p[10] = 0xFF_66_FF_66;
    p[11] = 0xFF_FF_FF_66;
    p[12] = 0xFF_66_99_FF;
    p[13] = 0xFF_FF_66_FF;
    p[14] = 0xFF_66_FF_FF;
    p[15] = 0xFF_FF_FF_FF;
    for r in 0..6u32 {
        for g in 0..6u32 {
            for b in 0..6u32 {
                let idx = (16 + r * 36 + g * 6 + b) as usize;
                let rv = if r == 0 { 0 } else { 55 + r * 40 };
                let gv = if g == 0 { 0 } else { 55 + g * 40 };
                let bv = if b == 0 { 0 } else { 55 + b * 40 };
                p[idx] = 0xFF_00_00_00 | (rv << 16) | (gv << 8) | bv;
            }
        }
    }
    for i in 0..24u32 {
        let v = 8 + i * 10;
        p[(232 + i) as usize] = 0xFF_00_00_00 | (v << 16) | (v << 8) | v;
    }
    p
}

// ── Colour type ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    pub fn to_argb(self, palette: &[u32; 256], default: u32) -> u32 {
        match self {
            Color::Default => default,
            Color::Indexed(i) => palette[i as usize],
            Color::Rgb(r, g, b) => 0xFF_00_00_00 | (r as u32) << 16 | (g as u32) << 8 | b as u32,
        }
    }
}

// ── Underline style ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineStyle {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

// ── Cell attributes ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellAttrs {
    pub fg: Color,
    pub bg: Color,
    pub underline_color: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: UnderlineStyle,
    pub blink_slow: bool,
    pub blink_rapid: bool,
    pub reverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
    pub overline: bool,
}

impl CellAttrs {
    pub const fn default_attrs() -> Self {
        Self {
            fg: Color::Default,
            bg: Color::Default,
            underline_color: Color::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: UnderlineStyle::None,
            blink_slow: false,
            blink_rapid: false,
            reverse: false,
            hidden: false,
            strikethrough: false,
            overline: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default_attrs();
    }
}

// ── Hyperlink ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    pub id: String,
    pub uri: String,
}

// ── Cell ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub width: u8,
    pub attrs: CellAttrs,
    pub hyperlink: Option<Hyperlink>,
}

impl Cell {
    pub fn blank() -> Self {
        Self {
            ch: ' ',
            width: 1,
            attrs: CellAttrs::default_attrs(),
            hyperlink: None,
        }
    }

    pub fn with_char(ch: char, attrs: CellAttrs) -> Self {
        let width = if is_wide_char(ch) { 2 } else { 1 };
        Self {
            ch,
            width,
            attrs,
            hyperlink: None,
        }
    }
}

fn is_wide_char(ch: char) -> bool {
    let c = ch as u32;
    (0x1100..=0x115F).contains(&c)
        || (0x2E80..=0x303E).contains(&c)
        || (0x3041..=0x33BF).contains(&c)
        || (0x3400..=0x4DBF).contains(&c)
        || (0x4E00..=0x9FFF).contains(&c)
        || (0xA000..=0xA4CF).contains(&c)
        || (0xAC00..=0xD7AF).contains(&c)
        || (0xF900..=0xFAFF).contains(&c)
        || (0xFE30..=0xFE6F).contains(&c)
        || (0xFF01..=0xFF60).contains(&c)
        || (0xFFE0..=0xFFE6).contains(&c)
        || (0x20000..=0x2FFFF).contains(&c)
        || (0x30000..=0x3FFFF).contains(&c)
}

// ── Row ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

impl Row {
    pub fn new(cols: usize) -> Self {
        Self {
            cells: vec![Cell::blank(); cols],
            wrapped: false,
        }
    }

    pub fn resize(&mut self, cols: usize) {
        self.cells.resize_with(cols, Cell::blank);
    }

    pub fn clear(&mut self, attrs: &CellAttrs) {
        for c in &mut self.cells {
            c.ch = ' ';
            c.width = 1;
            c.attrs = *attrs;
            c.hyperlink = None;
        }
        self.wrapped = false;
    }
}

// ── Character sets ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterSet {
    UsAscii,
    Uk,
    DecSpecialGraphics,
    IsoLatin1,
}

impl CharacterSet {
    pub fn translate(self, ch: char) -> char {
        match self {
            CharacterSet::DecSpecialGraphics => translate_dec_special(ch),
            CharacterSet::Uk => {
                if ch == '#' {
                    '£'
                } else {
                    ch
                }
            }
            _ => ch,
        }
    }
}

fn translate_dec_special(ch: char) -> char {
    match ch {
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'q' => '─',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'h' => '░',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        '`' => '◆',
        'o' => '⎺',
        'p' => '⎻',
        'r' => '⎼',
        's' => '⎽',
        _ => ch,
    }
}

// ── Charset state ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CharsetState {
    pub g: [CharacterSet; 4],
    pub gl: usize,
    pub gr: usize,
    pub single_shift: Option<usize>,
}

impl CharsetState {
    pub fn new() -> Self {
        Self {
            g: [CharacterSet::UsAscii; 4],
            gl: 0,
            gr: 1,
            single_shift: None,
        }
    }

    pub fn translate(&mut self, ch: char) -> char {
        let set_idx = if let Some(ss) = self.single_shift.take() {
            ss
        } else if (ch as u32) < 0x80 {
            self.gl
        } else {
            self.gr
        };
        self.g[set_idx].translate(ch)
    }
}

// ── Parser state machine ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseState {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsPassthrough,
    DcsIgnore,
    SosPmApcString,
}

#[derive(Debug, Clone)]
pub struct Parser {
    pub state: ParseState,
    pub params: Vec<u16>,
    /// Parallel to `params`: entry `i` is `true` when param `i` was joined to
    /// param `i-1` by a COLON (ECMA-48 sub-parameter form, e.g. `4:3`), and
    /// `false` when it followed a semicolon or is the first param. This lets the
    /// SGR dispatcher tell a legitimate sub-parameter (`4:3`) apart from two
    /// distinct `;`-separated attributes (`4;7`). See `dispatch_sgr`.
    pub param_is_colon: Vec<bool>,
    /// Whether the param currently being accumulated was colon-prefixed.
    pub current_param_colon: bool,
    pub intermediates: Vec<u8>,
    pub osc_data: Vec<u8>,
    pub dcs_data: Vec<u8>,
    pub current_param: Option<u32>,
    pub utf8_buf: [u8; 4],
    pub utf8_len: u8,
    pub utf8_needed: u8,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: ParseState::Ground,
            params: Vec::new(),
            param_is_colon: Vec::new(),
            current_param_colon: false,
            intermediates: Vec::new(),
            osc_data: Vec::new(),
            dcs_data: Vec::new(),
            current_param: None,
            utf8_buf: [0; 4],
            utf8_len: 0,
            utf8_needed: 0,
        }
    }

    pub fn reset(&mut self) {
        self.state = ParseState::Ground;
        self.params.clear();
        self.param_is_colon.clear();
        self.current_param_colon = false;
        self.intermediates.clear();
        self.osc_data.clear();
        self.dcs_data.clear();
        self.current_param = None;
        self.utf8_len = 0;
        self.utf8_needed = 0;
    }

    pub fn finish_param(&mut self) {
        let val = self.current_param.unwrap_or(0).min(u16::MAX as u32) as u16;
        self.params.push(val);
        // Keep the colon-flag vector length-locked to `params`. The flag for the
        // param just pushed reflects the separator that preceded it.
        self.param_is_colon.push(self.current_param_colon);
        self.current_param = None;
        self.current_param_colon = false;
    }

    pub fn param(&self, idx: usize, default: u16) -> u16 {
        self.params
            .get(idx)
            .copied()
            .filter(|&v| v != 0)
            .unwrap_or(default)
    }
}

// ── Cursor ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

#[derive(Debug, Clone)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub visible: bool,
    pub blinking: bool,
    pub attrs: CellAttrs,
    pub origin_mode: bool,
    pub wrap_pending: bool,
}

impl Cursor {
    pub fn new() -> Self {
        Self {
            row: 0,
            col: 0,
            shape: CursorShape::Block,
            visible: true,
            blinking: true,
            attrs: CellAttrs::default_attrs(),
            origin_mode: false,
            wrap_pending: false,
        }
    }
}

// ── Saved cursor (DECSC/DECRC) ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SavedCursor {
    pub row: usize,
    pub col: usize,
    pub attrs: CellAttrs,
    pub origin_mode: bool,
    pub charset_state: CharsetState,
}

// ── Scroll region ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ScrollRegion {
    pub top: usize,
    pub bottom: usize,
}

// ── DEC private modes ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DecModes {
    pub cursor_keys_application: bool, // DECCKM (1)
    pub ansi_mode: bool,               // DECANM (2)
    pub column_132: bool,              // DECCOLM (3)
    pub smooth_scroll: bool,           // DECSCLM (4)
    pub screen_reverse: bool,          // DECSCNM (5)
    pub origin_mode: bool,             // DECOM (6)
    pub auto_wrap: bool,               // DECAWM (7)
    pub auto_repeat: bool,             // DECARM (8)
    pub cursor_visible: bool,          // DECTCEM (25)
    pub alt_screen: bool,              // 1049
    pub bracketed_paste: bool,         // 2004
    pub focus_events: bool,            // 1004
    pub mouse_x10: bool,               // 9
    pub mouse_normal: bool,            // 1000
    pub mouse_button_event: bool,      // 1002
    pub mouse_any_event: bool,         // 1003
    pub mouse_sgr_ext: bool,           // 1006
    pub mouse_utf8: bool,              // 1005
    pub mouse_urxvt: bool,             // 1015
    pub alt_sends_escape: bool,        // 1036
    pub meta_sends_escape: bool,       // 1039
    pub save_cursor_on_alt: bool,      // 1048
    pub modify_other_keys: u8,         // level 0/1/2
    pub kitty_keyboard: u8,            // kitty keyboard protocol flags
}

impl DecModes {
    pub fn new() -> Self {
        Self {
            cursor_keys_application: false,
            ansi_mode: true,
            column_132: false,
            smooth_scroll: false,
            screen_reverse: false,
            origin_mode: false,
            auto_wrap: true,
            auto_repeat: true,
            cursor_visible: true,
            alt_screen: false,
            bracketed_paste: false,
            focus_events: false,
            mouse_x10: false,
            mouse_normal: false,
            mouse_button_event: false,
            mouse_any_event: false,
            mouse_sgr_ext: false,
            mouse_utf8: false,
            mouse_urxvt: false,
            alt_sends_escape: true,
            meta_sends_escape: false,
            save_cursor_on_alt: true,
            modify_other_keys: 0,
            kitty_keyboard: 0,
        }
    }
}

// ── Mouse state ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    Released,
    Button4,
    Button5,
}

#[derive(Debug, Clone)]
pub struct MouseState {
    pub last_button: MouseButton,
    pub col: usize,
    pub row: usize,
    pub pressed: bool,
    pub selecting: bool,
    pub sel_start: Option<(usize, usize)>,
    pub sel_end: Option<(usize, usize)>,
}

impl MouseState {
    pub fn new() -> Self {
        Self {
            last_button: MouseButton::Released,
            col: 0,
            row: 0,
            pressed: false,
            selecting: false,
            sel_start: None,
            sel_end: None,
        }
    }
}

// ── Selection ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    Character,
    Word,
    Line,
    Block,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub mode: SelectionMode,
    pub anchor: (usize, usize),
    pub end: (usize, usize),
    pub active: bool,
}

impl Selection {
    pub fn new() -> Self {
        Self {
            mode: SelectionMode::Character,
            anchor: (0, 0),
            end: (0, 0),
            active: false,
        }
    }

    pub fn start(&mut self, row: usize, col: usize, mode: SelectionMode) {
        self.anchor = (row, col);
        self.end = (row, col);
        self.mode = mode;
        self.active = true;
    }

    pub fn update(&mut self, row: usize, col: usize) {
        if self.active {
            self.end = (row, col);
        }
    }

    pub fn clear(&mut self) {
        self.active = false;
    }

    pub fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor.0 < self.end.0
            || (self.anchor.0 == self.end.0 && self.anchor.1 <= self.end.1)
        {
            (self.anchor, self.end)
        } else {
            (self.end, self.anchor)
        }
    }

    pub fn contains(&self, row: usize, col: usize) -> bool {
        if !self.active {
            return false;
        }
        let (start, end) = self.normalized();
        match self.mode {
            SelectionMode::Block => {
                let min_col = start.1.min(end.1);
                let max_col = start.1.max(end.1);
                row >= start.0 && row <= end.0 && col >= min_col && col <= max_col
            }
            _ => {
                if row < start.0 || row > end.0 {
                    return false;
                }
                if row == start.0 && row == end.0 {
                    col >= start.1 && col <= end.1
                } else if row == start.0 {
                    col >= start.1
                } else if row == end.0 {
                    col <= end.1
                } else {
                    true
                }
            }
        }
    }
}

// ── Search ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SearchState {
    pub query: String,
    pub matches: Vec<(usize, usize, usize)>,
    pub current_match: usize,
    pub active: bool,
    pub case_sensitive: bool,
    pub regex_mode: bool,
    pub wrap_around: bool,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current_match: 0,
            active: false,
            case_sensitive: false,
            regex_mode: false,
            wrap_around: true,
        }
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.current_match = 0;
        self.active = false;
    }

    pub fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.current_match = (self.current_match + 1) % self.matches.len();
        }
    }

    pub fn prev_match(&mut self) {
        if !self.matches.is_empty() {
            self.current_match = self
                .current_match
                .checked_sub(1)
                .unwrap_or(self.matches.len() - 1);
        }
    }
}

// ── PTY (pseudo-terminal) ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyInputMode {
    Raw,
    Cooked,
}

#[derive(Debug, Clone)]
pub struct TermiosSettings {
    pub echo: bool,
    pub icanon: bool,
    pub isig: bool,
    pub opost: bool,
    pub onlcr: bool,
    pub icrnl: bool,
    pub ixon: bool,
    pub ixoff: bool,
    pub intr_char: u8,
    pub quit_char: u8,
    pub erase_char: u8,
    pub kill_char: u8,
    pub eof_char: u8,
    pub susp_char: u8,
    pub start_char: u8,
    pub stop_char: u8,
    pub lnext_char: u8,
    pub werase_char: u8,
    pub vmin: u8,
    pub vtime: u8,
}

impl TermiosSettings {
    pub fn default_cooked() -> Self {
        Self {
            echo: true,
            icanon: true,
            isig: true,
            opost: true,
            onlcr: true,
            icrnl: true,
            ixon: true,
            ixoff: false,
            intr_char: 0x03,
            quit_char: 0x1C,
            erase_char: 0x7F,
            kill_char: 0x15,
            eof_char: 0x04,
            susp_char: 0x1A,
            start_char: 0x11,
            stop_char: 0x13,
            lnext_char: 0x16,
            werase_char: 0x17,
            vmin: 1,
            vtime: 0,
        }
    }

    pub fn default_raw() -> Self {
        Self {
            echo: false,
            icanon: false,
            isig: false,
            opost: false,
            onlcr: false,
            icrnl: false,
            ixon: false,
            ixoff: false,
            ..Self::default_cooked()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pty {
    pub master_fd: i32,
    pub slave_fd: i32,
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub termios: TermiosSettings,
    pub input_mode: PtyInputMode,
    pub line_buffer: Vec<u8>,
    pub output_buffer: Vec<u8>,
    pub baud_rate: u32,
}

impl Pty {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            master_fd: -1,
            slave_fd: -1,
            cols,
            rows,
            pixel_width: cols * GLYPH_W as u16,
            pixel_height: rows * GLYPH_H as u16,
            termios: TermiosSettings::default_cooked(),
            input_mode: PtyInputMode::Cooked,
            line_buffer: Vec::new(),
            output_buffer: Vec::new(),
            baud_rate: 115200,
        }
    }

    pub fn set_window_size(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.pixel_width = cols * GLYPH_W as u16;
        self.pixel_height = rows * GLYPH_H as u16;
    }

    pub fn set_raw_mode(&mut self) {
        self.termios = TermiosSettings::default_raw();
        self.input_mode = PtyInputMode::Raw;
    }

    pub fn set_cooked_mode(&mut self) {
        self.termios = TermiosSettings::default_cooked();
        self.input_mode = PtyInputMode::Cooked;
    }

    pub fn write_master(&mut self, data: &[u8]) {
        self.output_buffer.extend_from_slice(data);
    }

    pub fn process_input(&mut self, byte: u8) -> Option<u8> {
        match self.input_mode {
            PtyInputMode::Raw => Some(byte),
            PtyInputMode::Cooked => {
                if self.termios.isig {
                    if byte == self.termios.intr_char {
                        return Some(byte);
                    }
                    if byte == self.termios.susp_char {
                        return Some(byte);
                    }
                }
                if self.termios.icanon {
                    if byte == self.termios.erase_char {
                        self.line_buffer.pop();
                        return None;
                    }
                    if byte == self.termios.kill_char {
                        self.line_buffer.clear();
                        return None;
                    }
                    if byte == self.termios.werase_char {
                        while self.line_buffer.last() == Some(&b' ') {
                            self.line_buffer.pop();
                        }
                        while let Some(&last) = self.line_buffer.last() {
                            if last == b' ' {
                                break;
                            }
                            self.line_buffer.pop();
                        }
                        return None;
                    }
                    self.line_buffer.push(byte);
                    if byte == b'\n' || byte == self.termios.eof_char {
                        return Some(byte);
                    }
                    None
                } else {
                    Some(byte)
                }
            }
        }
    }

    pub fn drain_output(&mut self) -> Vec<u8> {
        let out = self.output_buffer.clone();
        self.output_buffer.clear();
        out
    }
}

// ── Screen buffer ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ScreenBuffer {
    pub rows: Vec<Row>,
    pub cols: usize,
    pub num_rows: usize,
    pub cursor: Cursor,
    pub scroll_region: ScrollRegion,
    pub saved_cursor: Option<SavedCursor>,
    pub charset_state: CharsetState,
}

impl ScreenBuffer {
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut lines = Vec::with_capacity(rows);
        for _ in 0..rows {
            lines.push(Row::new(cols));
        }
        Self {
            rows: lines,
            cols,
            num_rows: rows,
            cursor: Cursor::new(),
            scroll_region: ScrollRegion {
                top: 0,
                bottom: rows.saturating_sub(1),
            },
            saved_cursor: None,
            charset_state: CharsetState::new(),
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.num_rows = rows;
        self.rows.resize_with(rows, || Row::new(cols));
        for row in &mut self.rows {
            row.resize(cols);
        }
        self.scroll_region = ScrollRegion {
            top: 0,
            bottom: rows.saturating_sub(1),
        };
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(cols.saturating_sub(1));
    }

    pub fn scroll_up(&mut self, count: usize, scrollback: &mut Vec<Row>) {
        let top = self.scroll_region.top;
        let bottom = self.scroll_region.bottom;
        for _ in 0..count {
            if top == 0 {
                scrollback.push(self.rows[top].clone());
            }
            self.rows.remove(top);
            self.rows.insert(bottom, Row::new(self.cols));
        }
    }

    pub fn scroll_down(&mut self, count: usize) {
        let top = self.scroll_region.top;
        let bottom = self.scroll_region.bottom;
        for _ in 0..count {
            self.rows.remove(bottom);
            self.rows.insert(top, Row::new(self.cols));
        }
    }

    pub fn erase_display(&mut self, mode: u16) {
        let attrs = self.cursor.attrs;
        match mode {
            0 => {
                for col in self.cursor.col..self.cols {
                    self.rows[self.cursor.row].cells[col] = Cell::with_char(' ', attrs);
                }
                for row in (self.cursor.row + 1)..self.num_rows {
                    self.rows[row].clear(&attrs);
                }
            }
            1 => {
                for row in 0..self.cursor.row {
                    self.rows[row].clear(&attrs);
                }
                for col in 0..=self.cursor.col.min(self.cols.saturating_sub(1)) {
                    self.rows[self.cursor.row].cells[col] = Cell::with_char(' ', attrs);
                }
            }
            2 => {
                for row in 0..self.num_rows {
                    self.rows[row].clear(&attrs);
                }
            }
            // ED 3 ("Erase Saved Lines") clears only the scrollback, which lives
            // on the emulator, not this ScreenBuffer — it is handled at the CSI
            // `J` dispatch site and must NOT touch the visible grid here.
            _ => {}
        }
    }

    pub fn erase_line(&mut self, mode: u16) {
        let attrs = self.cursor.attrs;
        let row = self.cursor.row;
        match mode {
            0 => {
                for col in self.cursor.col..self.cols {
                    self.rows[row].cells[col] = Cell::with_char(' ', attrs);
                }
            }
            1 => {
                for col in 0..=self.cursor.col.min(self.cols.saturating_sub(1)) {
                    self.rows[row].cells[col] = Cell::with_char(' ', attrs);
                }
            }
            2 => {
                self.rows[row].clear(&attrs);
            }
            _ => {}
        }
    }

    pub fn insert_lines(&mut self, count: usize) {
        let row = self.cursor.row;
        let bottom = self.scroll_region.bottom;
        for _ in 0..count.min(bottom - row + 1) {
            if bottom < self.rows.len() {
                self.rows.remove(bottom);
            }
            self.rows.insert(row, Row::new(self.cols));
        }
    }

    pub fn delete_lines(&mut self, count: usize) {
        let row = self.cursor.row;
        let bottom = self.scroll_region.bottom;
        for _ in 0..count.min(bottom - row + 1) {
            self.rows.remove(row);
            self.rows.insert(bottom, Row::new(self.cols));
        }
    }

    pub fn insert_chars(&mut self, count: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        for _ in 0..count {
            if self.rows[row].cells.len() >= self.cols {
                self.rows[row].cells.pop();
            }
            self.rows[row].cells.insert(col, Cell::blank());
        }
    }

    pub fn delete_chars(&mut self, count: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        for _ in 0..count {
            if col < self.rows[row].cells.len() {
                self.rows[row].cells.remove(col);
                self.rows[row].cells.push(Cell::blank());
            }
        }
    }

    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(SavedCursor {
            row: self.cursor.row,
            col: self.cursor.col,
            attrs: self.cursor.attrs,
            origin_mode: self.cursor.origin_mode,
            charset_state: self.charset_state.clone(),
        });
    }

    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor.take() {
            self.cursor.row = saved.row.min(self.num_rows.saturating_sub(1));
            self.cursor.col = saved.col.min(self.cols.saturating_sub(1));
            self.cursor.attrs = saved.attrs;
            self.cursor.origin_mode = saved.origin_mode;
            self.charset_state = saved.charset_state;
        }
    }
}

// ── Colour schemes ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Default,
    Solarized,
    Dracula,
    Nord,
    Monokai,
    OneDark,
    Gruvbox,
    TokyoNight,
    Catppuccin,
}

#[derive(Debug, Clone)]
pub struct SchemeColors {
    pub bg: u32,
    pub fg: u32,
    pub cursor: u32,
    pub selection: u32,
    pub palette_overrides: [(usize, u32); 16],
}

impl ColorScheme {
    pub fn colors(self) -> SchemeColors {
        match self {
            ColorScheme::Solarized => SchemeColors {
                bg: 0xFF_00_2B_36,
                fg: 0xFF_83_94_96,
                cursor: 0xFF_93_A1_A1,
                selection: 0xFF_07_36_42,
                palette_overrides: [
                    (0, 0xFF_07_36_42),
                    (1, 0xFF_DC_32_2F),
                    (2, 0xFF_85_99_00),
                    (3, 0xFF_B5_89_00),
                    (4, 0xFF_26_8B_D2),
                    (5, 0xFF_D3_36_82),
                    (6, 0xFF_2A_A1_98),
                    (7, 0xFF_EE_E8_D5),
                    (8, 0xFF_00_2B_36),
                    (9, 0xFF_CB_4B_16),
                    (10, 0xFF_58_6E_75),
                    (11, 0xFF_65_7B_83),
                    (12, 0xFF_83_94_96),
                    (13, 0xFF_6C_71_C4),
                    (14, 0xFF_93_A1_A1),
                    (15, 0xFF_FD_F6_E3),
                ],
            },
            ColorScheme::Dracula => SchemeColors {
                bg: 0xFF_28_2A_36,
                fg: 0xFF_F8_F8_F2,
                cursor: 0xFF_F8_F8_F2,
                selection: 0xFF_44_47_5A,
                palette_overrides: [
                    (0, 0xFF_21_22_2C),
                    (1, 0xFF_FF_55_55),
                    (2, 0xFF_50_FA_7B),
                    (3, 0xFF_F1_FA_8C),
                    (4, 0xFF_BD_93_F9),
                    (5, 0xFF_FF_79_C6),
                    (6, 0xFF_8B_E9_FD),
                    (7, 0xFF_F8_F8_F2),
                    (8, 0xFF_62_72_A4),
                    (9, 0xFF_FF_6E_6E),
                    (10, 0xFF_69_FF_94),
                    (11, 0xFF_FF_FF_A5),
                    (12, 0xFF_D6_AC_FF),
                    (13, 0xFF_FF_92_DF),
                    (14, 0xFF_A4_FF_FF),
                    (15, 0xFF_FF_FF_FF),
                ],
            },
            ColorScheme::Nord => SchemeColors {
                bg: 0xFF_2E_34_40,
                fg: 0xFF_D8_DE_E9,
                cursor: 0xFF_D8_DE_E9,
                selection: 0xFF_43_4C_5E,
                palette_overrides: [
                    (0, 0xFF_3B_42_52),
                    (1, 0xFF_BF_61_6A),
                    (2, 0xFF_A3_BE_8C),
                    (3, 0xFF_EB_CB_8B),
                    (4, 0xFF_81_A1_C1),
                    (5, 0xFF_B4_8E_AD),
                    (6, 0xFF_88_C0_D0),
                    (7, 0xFF_E5_E9_F0),
                    (8, 0xFF_4C_56_6A),
                    (9, 0xFF_BF_61_6A),
                    (10, 0xFF_A3_BE_8C),
                    (11, 0xFF_EB_CB_8B),
                    (12, 0xFF_81_A1_C1),
                    (13, 0xFF_B4_8E_AD),
                    (14, 0xFF_8F_BC_BB),
                    (15, 0xFF_EC_EF_F4),
                ],
            },
            ColorScheme::Monokai => SchemeColors {
                bg: 0xFF_27_28_22,
                fg: 0xFF_F8_F8_F2,
                cursor: 0xFF_F8_F8_F0,
                selection: 0xFF_49_48_3E,
                palette_overrides: [
                    (0, 0xFF_27_28_22),
                    (1, 0xFF_F9_26_72),
                    (2, 0xFF_A6_E2_2E),
                    (3, 0xFF_F4_BF_75),
                    (4, 0xFF_66_D9_EF),
                    (5, 0xFF_AE_81_FF),
                    (6, 0xFF_A1_EF_E4),
                    (7, 0xFF_F8_F8_F2),
                    (8, 0xFF_75_71_5E),
                    (9, 0xFF_F9_26_72),
                    (10, 0xFF_A6_E2_2E),
                    (11, 0xFF_F4_BF_75),
                    (12, 0xFF_66_D9_EF),
                    (13, 0xFF_AE_81_FF),
                    (14, 0xFF_A1_EF_E4),
                    (15, 0xFF_F9_F8_F5),
                ],
            },
            ColorScheme::OneDark => SchemeColors {
                bg: 0xFF_28_2C_34,
                fg: 0xFF_AB_B2_BF,
                cursor: 0xFF_52_8B_FF,
                selection: 0xFF_3E_44_52,
                palette_overrides: [
                    (0, 0xFF_28_2C_34),
                    (1, 0xFF_E0_6C_75),
                    (2, 0xFF_98_C3_79),
                    (3, 0xFF_E5_C0_7B),
                    (4, 0xFF_61_AF_EF),
                    (5, 0xFF_C6_78_DD),
                    (6, 0xFF_56_B6_C2),
                    (7, 0xFF_AB_B2_BF),
                    (8, 0xFF_54_58_62),
                    (9, 0xFF_E0_6C_75),
                    (10, 0xFF_98_C3_79),
                    (11, 0xFF_E5_C0_7B),
                    (12, 0xFF_61_AF_EF),
                    (13, 0xFF_C6_78_DD),
                    (14, 0xFF_56_B6_C2),
                    (15, 0xFF_FF_FF_FF),
                ],
            },
            ColorScheme::Gruvbox => SchemeColors {
                bg: 0xFF_28_28_28,
                fg: 0xFF_EB_DB_B2,
                cursor: 0xFF_EB_DB_B2,
                selection: 0xFF_50_49_45,
                palette_overrides: [
                    (0, 0xFF_28_28_28),
                    (1, 0xFF_CC_24_1D),
                    (2, 0xFF_98_97_1A),
                    (3, 0xFF_D7_99_21),
                    (4, 0xFF_45_85_88),
                    (5, 0xFF_B1_62_86),
                    (6, 0xFF_68_9D_6A),
                    (7, 0xFF_A8_99_84),
                    (8, 0xFF_92_83_74),
                    (9, 0xFF_FB_49_34),
                    (10, 0xFF_B8_BB_26),
                    (11, 0xFF_FA_BD_2F),
                    (12, 0xFF_83_A5_98),
                    (13, 0xFF_D3_86_9B),
                    (14, 0xFF_8E_C0_7C),
                    (15, 0xFF_EB_DB_B2),
                ],
            },
            ColorScheme::TokyoNight => SchemeColors {
                bg: 0xFF_1A_1B_26,
                fg: 0xFF_C0_CA_F5,
                cursor: 0xFF_C0_CA_F5,
                selection: 0xFF_28_3B_4D,
                palette_overrides: [
                    (0, 0xFF_15_16_1E),
                    (1, 0xFF_F7_76_8E),
                    (2, 0xFF_9E_CE_6A),
                    (3, 0xFF_E0_AF_68),
                    (4, 0xFF_7A_A2_F7),
                    (5, 0xFF_BB_9A_F7),
                    (6, 0xFF_7D_CF_FF),
                    (7, 0xFF_A9_B1_D6),
                    (8, 0xFF_41_48_68),
                    (9, 0xFF_F7_76_8E),
                    (10, 0xFF_9E_CE_6A),
                    (11, 0xFF_E0_AF_68),
                    (12, 0xFF_7A_A2_F7),
                    (13, 0xFF_BB_9A_F7),
                    (14, 0xFF_7D_CF_FF),
                    (15, 0xFF_C0_CA_F5),
                ],
            },
            ColorScheme::Catppuccin => SchemeColors {
                bg: 0xFF_1E_1E_2E,
                fg: 0xFF_CD_D6_F4,
                cursor: 0xFF_F5_E0_DC,
                selection: 0xFF_45_47_5A,
                palette_overrides: [
                    (0, 0xFF_45_47_5A),
                    (1, 0xFF_F3_8B_A8),
                    (2, 0xFF_A6_E3_A1),
                    (3, 0xFF_F9_E2_AF),
                    (4, 0xFF_89_B4_FA),
                    (5, 0xFF_CB_A6_F7),
                    (6, 0xFF_94_E2_D5),
                    (7, 0xFF_BA_C2_DE),
                    (8, 0xFF_58_5B_70),
                    (9, 0xFF_F3_8B_A8),
                    (10, 0xFF_A6_E3_A1),
                    (11, 0xFF_F9_E2_AF),
                    (12, 0xFF_89_B4_FA),
                    (13, 0xFF_CB_A6_F7),
                    (14, 0xFF_94_E2_D5),
                    (15, 0xFF_A6_AD_C8),
                ],
            },
            ColorScheme::Default => SchemeColors {
                bg: TERM_BG,
                fg: TERM_FG,
                cursor: TERM_CURSOR,
                selection: TERM_SELECT,
                palette_overrides: [
                    (0, 0),
                    (1, 0),
                    (2, 0),
                    (3, 0),
                    (4, 0),
                    (5, 0),
                    (6, 0),
                    (7, 0),
                    (8, 0),
                    (9, 0),
                    (10, 0),
                    (11, 0),
                    (12, 0),
                    (13, 0),
                    (14, 0),
                    (15, 0),
                ],
            },
        }
    }
}

// ── Profile ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub scheme: ColorScheme,
    pub font_family: String,
    pub font_size: u16,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    pub padding: u16,
    pub opacity: u8,
    pub blur: bool,
    pub scrollback_lines: usize,
    pub ligatures: bool,
    pub nerd_font: bool,
    pub bold_is_bright: bool,
}

impl Profile {
    pub fn default_profile() -> Self {
        Self {
            name: String::from("Default"),
            scheme: ColorScheme::Default,
            font_family: String::from("Mono"),
            font_size: 14,
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            padding: 4,
            opacity: 100,
            blur: false,
            scrollback_lines: 10000,
            ligatures: false,
            nerd_font: true,
            bold_is_bright: false,
        }
    }
}

// ── Shell integration ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticZone {
    Prompt,
    Input,
    Output,
}

#[derive(Debug, Clone)]
pub struct ShellIntegration {
    pub enabled: bool,
    pub cwd: String,
    pub command_start_row: Option<usize>,
    pub command_end_row: Option<usize>,
    pub output_start_row: Option<usize>,
    pub last_exit_code: Option<i32>,
    pub prompt_marks: Vec<usize>,
    pub current_zone: SemanticZone,
}

impl ShellIntegration {
    pub fn new() -> Self {
        Self {
            enabled: false,
            cwd: String::new(),
            command_start_row: None,
            command_end_row: None,
            output_start_row: None,
            last_exit_code: None,
            prompt_marks: Vec::new(),
            current_zone: SemanticZone::Prompt,
        }
    }

    pub fn mark_prompt(&mut self, row: usize) {
        self.prompt_marks.push(row);
        self.current_zone = SemanticZone::Prompt;
    }

    pub fn mark_command_start(&mut self, row: usize) {
        self.command_start_row = Some(row);
        self.current_zone = SemanticZone::Input;
    }

    pub fn mark_command_end(&mut self, row: usize, exit_code: i32) {
        self.command_end_row = Some(row);
        self.last_exit_code = Some(exit_code);
    }

    pub fn mark_output_start(&mut self, row: usize) {
        self.output_start_row = Some(row);
        self.current_zone = SemanticZone::Output;
    }

    pub fn set_cwd(&mut self, cwd: &str) {
        self.cwd.clear();
        self.cwd.push_str(cwd);
    }
}

// ── Tab ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug)]
pub struct Pane {
    pub id: u64,
    pub primary: ScreenBuffer,
    pub alternate: ScreenBuffer,
    pub using_alt: bool,
    pub scrollback: Vec<Row>,
    pub scroll_offset: usize,
    pub pty: Pty,
    pub modes: DecModes,
    pub parser: Parser,
    pub mouse: MouseState,
    pub selection: Selection,
    pub search: SearchState,
    pub shell_integration: ShellIntegration,
    pub palette: [u32; 256],
    pub profile: Profile,
    pub title: String,
    pub icon_title: String,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub zoomed: bool,
}

impl Pane {
    pub fn new(id: u64, cols: usize, rows: usize) -> Self {
        Self {
            id,
            primary: ScreenBuffer::new(cols, rows),
            alternate: ScreenBuffer::new(cols, rows),
            using_alt: false,
            scrollback: Vec::new(),
            scroll_offset: 0,
            pty: Pty::new(cols as u16, rows as u16),
            modes: DecModes::new(),
            parser: Parser::new(),
            mouse: MouseState::new(),
            selection: Selection::new(),
            search: SearchState::new(),
            shell_integration: ShellIntegration::new(),
            palette: default_256_palette(),
            profile: Profile::default_profile(),
            title: String::from("Terminal"),
            icon_title: String::new(),
            x: 0,
            y: 0,
            width: cols,
            height: rows,
            zoomed: false,
        }
    }

    pub fn active_buffer(&self) -> &ScreenBuffer {
        if self.using_alt {
            &self.alternate
        } else {
            &self.primary
        }
    }

    pub fn active_buffer_mut(&mut self) -> &mut ScreenBuffer {
        if self.using_alt {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    pub fn enter_alt_screen(&mut self) {
        if !self.using_alt {
            self.primary.save_cursor();
            self.using_alt = true;
            self.alternate = ScreenBuffer::new(self.width, self.height);
            self.modes.alt_screen = true;
        }
    }

    pub fn exit_alt_screen(&mut self) {
        if self.using_alt {
            self.using_alt = false;
            self.primary.restore_cursor();
            self.modes.alt_screen = false;
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.width = cols;
        self.height = rows;
        self.primary.resize(cols, rows);
        self.alternate.resize(cols, rows);
        self.pty.set_window_size(cols as u16, rows as u16);
    }

    pub fn scroll_viewport_up(&mut self, lines: usize) {
        let max = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + lines).min(max);
    }

    pub fn scroll_viewport_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn put_char(&mut self, ch: char) {
        let auto_wrap = self.modes.auto_wrap;
        let max_sb = self.profile.scrollback_lines;

        let buf = self.active_buffer_mut();
        let attrs = buf.cursor.attrs;
        let translated = buf.charset_state.translate(ch);
        let cell_width = if is_wide_char(translated) { 2 } else { 1 };

        // A double-width glyph that will not fit in the columns remaining on this
        // line must wrap to the NEXT line and be printed there — xterm wraps
        // first, it does not drop the glyph. Fold that case into the same wrap
        // path as a pending autowrap latch, then fall through and print. (BUG #2)
        let wide_needs_wrap =
            auto_wrap && !buf.cursor.wrap_pending && buf.cursor.col + cell_width > buf.cols;

        if buf.cursor.wrap_pending || wide_needs_wrap {
            buf.rows[buf.cursor.row].wrapped = true;
            buf.cursor.col = 0;
            if buf.cursor.row == buf.scroll_region.bottom {
                let top = buf.scroll_region.top;
                let scrollback_row = if top == 0 {
                    Some(buf.rows[top].clone())
                } else {
                    None
                };
                buf.rows.remove(top);
                buf.rows
                    .insert(buf.scroll_region.bottom, Row::new(buf.cols));
                if let Some(row_data) = scrollback_row {
                    self.scrollback.push(row_data);
                    if self.scrollback.len() > max_sb {
                        self.scrollback.remove(0);
                    }
                }
                let buf = self.active_buffer_mut();
                buf.cursor.wrap_pending = false;
            } else {
                buf.cursor.row += 1;
                buf.cursor.wrap_pending = false;
            }
        }

        let buf = self.active_buffer_mut();
        if buf.cursor.col + cell_width > buf.cols {
            // Still doesn't fit even after any wrap above (e.g. a width-2 glyph
            // in a 1-column terminal): clamp into the line rather than dropping.
            buf.cursor.col = buf.cols.saturating_sub(cell_width);
        }

        let row = buf.cursor.row;
        let col = buf.cursor.col;
        buf.rows[row].cells[col] = Cell::with_char(translated, attrs);
        if cell_width == 2 && col + 1 < buf.cols {
            buf.rows[row].cells[col + 1] = Cell {
                ch: ' ',
                width: 0,
                attrs,
                hyperlink: None,
            };
        }
        buf.cursor.col += cell_width;
        if buf.cursor.col >= buf.cols {
            if auto_wrap {
                buf.cursor.col = buf.cols - 1;
                buf.cursor.wrap_pending = true;
            } else {
                buf.cursor.col = buf.cols - 1;
            }
        }
    }

    pub fn process_byte(&mut self, byte: u8) {
        match self.parser.state {
            ParseState::Ground => self.ground_byte(byte),
            ParseState::Escape => self.escape_byte(byte),
            ParseState::EscapeIntermediate => self.escape_intermediate_byte(byte),
            ParseState::CsiEntry => self.csi_entry_byte(byte),
            ParseState::CsiParam => self.csi_param_byte(byte),
            ParseState::CsiIntermediate => self.csi_intermediate_byte(byte),
            ParseState::CsiIgnore => self.csi_ignore_byte(byte),
            ParseState::OscString => self.osc_byte(byte),
            ParseState::DcsEntry => self.dcs_entry_byte(byte),
            ParseState::DcsParam => self.dcs_param_byte(byte),
            ParseState::DcsIntermediate | ParseState::DcsPassthrough => {
                self.dcs_passthrough_byte(byte)
            }
            ParseState::DcsIgnore => self.dcs_ignore_byte(byte),
            ParseState::SosPmApcString => self.sos_byte(byte),
        }
    }

    fn ground_byte(&mut self, byte: u8) {
        match byte {
            0x00 => {}
            0x07 => { /* BEL */ }
            0x08 => {
                let buf = self.active_buffer_mut();
                buf.cursor.col = buf.cursor.col.saturating_sub(1);
                buf.cursor.wrap_pending = false;
            }
            0x09 => {
                let buf = self.active_buffer_mut();
                let next_tab = ((buf.cursor.col / 8) + 1) * 8;
                buf.cursor.col = next_tab.min(buf.cols.saturating_sub(1));
                buf.cursor.wrap_pending = false;
            }
            0x0A | 0x0B | 0x0C => {
                let max_sb = self.profile.scrollback_lines;
                let buf = if self.using_alt {
                    &mut self.alternate
                } else {
                    &mut self.primary
                };
                if buf.cursor.row == buf.scroll_region.bottom {
                    buf.scroll_up(1, &mut self.scrollback);
                    if self.scrollback.len() > max_sb {
                        self.scrollback.remove(0);
                    }
                } else if buf.cursor.row < buf.num_rows - 1 {
                    buf.cursor.row += 1;
                }
                let buf = if self.using_alt {
                    &mut self.alternate
                } else {
                    &mut self.primary
                };
                buf.cursor.wrap_pending = false;
            }
            0x0D => {
                self.active_buffer_mut().cursor.col = 0;
                self.active_buffer_mut().cursor.wrap_pending = false;
            }
            0x0E => {
                self.active_buffer_mut().charset_state.gl = 1;
            }
            0x0F => {
                self.active_buffer_mut().charset_state.gl = 0;
            }
            0x1B => {
                self.parser.state = ParseState::Escape;
            }
            0x20..=0x7E => {
                if self.parser.utf8_needed > 0 {
                    self.parser.utf8_needed = 0;
                    self.parser.utf8_len = 0;
                }
                self.put_char(byte as char);
            }
            0xC0..=0xDF => {
                self.parser.utf8_buf[0] = byte;
                self.parser.utf8_len = 1;
                self.parser.utf8_needed = 2;
            }
            0xE0..=0xEF => {
                self.parser.utf8_buf[0] = byte;
                self.parser.utf8_len = 1;
                self.parser.utf8_needed = 3;
            }
            0xF0..=0xF7 => {
                self.parser.utf8_buf[0] = byte;
                self.parser.utf8_len = 1;
                self.parser.utf8_needed = 4;
            }
            0x80..=0xBF if self.parser.utf8_needed > 0 => {
                let idx = self.parser.utf8_len as usize;
                if idx < 4 {
                    self.parser.utf8_buf[idx] = byte;
                    self.parser.utf8_len += 1;
                    if self.parser.utf8_len == self.parser.utf8_needed {
                        let n = self.parser.utf8_needed as usize;
                        if let Ok(s) = core::str::from_utf8(&self.parser.utf8_buf[..n]) {
                            if let Some(ch) = s.chars().next() {
                                self.put_char(ch);
                            }
                        }
                        self.parser.utf8_len = 0;
                        self.parser.utf8_needed = 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn escape_byte(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.parser.params.clear();
                self.parser.param_is_colon.clear();
                self.parser.current_param_colon = false;
                self.parser.intermediates.clear();
                self.parser.current_param = None;
                self.parser.state = ParseState::CsiEntry;
            }
            b']' => {
                self.parser.osc_data.clear();
                self.parser.state = ParseState::OscString;
            }
            b'P' => {
                self.parser.dcs_data.clear();
                self.parser.params.clear();
                self.parser.param_is_colon.clear();
                self.parser.current_param_colon = false;
                self.parser.current_param = None;
                self.parser.state = ParseState::DcsEntry;
            }
            b'^' | b'_' | b'X' => {
                self.parser.state = ParseState::SosPmApcString;
            }
            b'7' => {
                self.active_buffer_mut().save_cursor();
                self.parser.state = ParseState::Ground;
            }
            b'8' => {
                self.active_buffer_mut().restore_cursor();
                self.parser.state = ParseState::Ground;
            }
            b'D' => {
                let buf = if self.using_alt {
                    &mut self.alternate
                } else {
                    &mut self.primary
                };
                if buf.cursor.row == buf.scroll_region.bottom {
                    buf.scroll_up(1, &mut self.scrollback);
                } else {
                    buf.cursor.row = (buf.cursor.row + 1).min(buf.num_rows - 1);
                }
                self.parser.state = ParseState::Ground;
            }
            b'M' => {
                let buf = self.active_buffer_mut();
                if buf.cursor.row == buf.scroll_region.top {
                    buf.scroll_down(1);
                } else {
                    buf.cursor.row = buf.cursor.row.saturating_sub(1);
                }
                self.parser.state = ParseState::Ground;
            }
            b'E' => {
                let buf = if self.using_alt {
                    &mut self.alternate
                } else {
                    &mut self.primary
                };
                buf.cursor.col = 0;
                if buf.cursor.row == buf.scroll_region.bottom {
                    buf.scroll_up(1, &mut self.scrollback);
                } else {
                    buf.cursor.row = (buf.cursor.row + 1).min(buf.num_rows - 1);
                }
                self.parser.state = ParseState::Ground;
            }
            b'H' => {
                self.parser.state = ParseState::Ground;
            }
            b'c' => {
                self.hard_reset();
                self.parser.state = ParseState::Ground;
            }
            b'(' => {
                self.parser.intermediates.clear();
                self.parser.intermediates.push(b'(');
                self.parser.state = ParseState::EscapeIntermediate;
            }
            b')' => {
                self.parser.intermediates.clear();
                self.parser.intermediates.push(b')');
                self.parser.state = ParseState::EscapeIntermediate;
            }
            b'*' => {
                self.parser.intermediates.clear();
                self.parser.intermediates.push(b'*');
                self.parser.state = ParseState::EscapeIntermediate;
            }
            b'+' => {
                self.parser.intermediates.clear();
                self.parser.intermediates.push(b'+');
                self.parser.state = ParseState::EscapeIntermediate;
            }
            b'N' => {
                self.active_buffer_mut().charset_state.single_shift = Some(2);
                self.parser.state = ParseState::Ground;
            }
            b'O' => {
                self.active_buffer_mut().charset_state.single_shift = Some(3);
                self.parser.state = ParseState::Ground;
            }
            b'=' => {
                self.parser.state = ParseState::Ground;
            }
            b'>' => {
                self.parser.state = ParseState::Ground;
            }
            _ => {
                self.parser.state = ParseState::Ground;
            }
        }
    }

    fn escape_intermediate_byte(&mut self, byte: u8) {
        let charset = match byte {
            b'B' => CharacterSet::UsAscii,
            b'A' => CharacterSet::Uk,
            b'0' => CharacterSet::DecSpecialGraphics,
            _ => CharacterSet::UsAscii,
        };
        let intermediate = self.parser.intermediates.first().copied();
        let buf = self.active_buffer_mut();
        match intermediate {
            Some(b'(') => buf.charset_state.g[0] = charset,
            Some(b')') => buf.charset_state.g[1] = charset,
            Some(b'*') => buf.charset_state.g[2] = charset,
            Some(b'+') => buf.charset_state.g[3] = charset,
            _ => {}
        }
        self.parser.state = ParseState::Ground;
    }

    fn csi_entry_byte(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                self.parser.current_param = Some((byte - b'0') as u32);
                self.parser.state = ParseState::CsiParam;
            }
            b';' => {
                self.parser.params.push(0);
                self.parser.param_is_colon.push(false);
                self.parser.state = ParseState::CsiParam;
            }
            b'?' | b'>' | b'!' | b'=' => {
                self.parser.intermediates.push(byte);
                self.parser.state = ParseState::CsiParam;
            }
            0x40..=0x7E => {
                self.dispatch_csi(byte);
                self.parser.state = ParseState::Ground;
            }
            _ => {
                self.parser.state = ParseState::CsiParam;
            }
        }
    }

    fn csi_param_byte(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                let v = self.parser.current_param.unwrap_or(0);
                // Saturate: a hostile stream (e.g. `ESC[9999999999H`) must clamp,
                // not overflow-panic. `finish_param` caps at u16::MAX afterwards.
                self.parser.current_param =
                    Some(v.saturating_mul(10).saturating_add((byte - b'0') as u32));
            }
            b';' => {
                self.parser.finish_param();
            }
            b':' => {
                // Colon = ECMA-48 sub-parameter separator. The NEXT param is a
                // sub-parameter of the one we just finished.
                self.parser.finish_param();
                self.parser.current_param_colon = true;
            }
            0x40..=0x7E => {
                self.parser.finish_param();
                self.dispatch_csi(byte);
                self.parser.state = ParseState::Ground;
            }
            b'?' | b'>' | b'!' | b'=' => {
                self.parser.intermediates.push(byte);
            }
            0x20..=0x2F => {
                self.parser.finish_param();
                self.parser.intermediates.push(byte);
                self.parser.state = ParseState::CsiIntermediate;
            }
            _ => {}
        }
    }

    fn csi_intermediate_byte(&mut self, byte: u8) {
        match byte {
            0x20..=0x2F => {
                self.parser.intermediates.push(byte);
            }
            0x40..=0x7E => {
                self.dispatch_csi(byte);
                self.parser.state = ParseState::Ground;
            }
            _ => {
                self.parser.state = ParseState::CsiIgnore;
            }
        }
    }

    fn csi_ignore_byte(&mut self, byte: u8) {
        if (0x40..=0x7E).contains(&byte) {
            self.parser.state = ParseState::Ground;
        }
    }

    fn dispatch_csi(&mut self, final_byte: u8) {
        let is_private = self.parser.intermediates.contains(&b'?');
        let has_gt = self.parser.intermediates.contains(&b'>');

        if is_private {
            self.dispatch_dec_private(final_byte);
            return;
        }

        match final_byte {
            b'A' => {
                // CUU — cursor up
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                buf.cursor.row = buf.cursor.row.saturating_sub(n);
                buf.cursor.wrap_pending = false;
            }
            b'B' => {
                // CUD — cursor down
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                buf.cursor.row = (buf.cursor.row + n).min(buf.num_rows - 1);
                buf.cursor.wrap_pending = false;
            }
            b'C' => {
                // CUF — cursor forward
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                buf.cursor.col = (buf.cursor.col + n).min(buf.cols - 1);
                buf.cursor.wrap_pending = false;
            }
            b'D' => {
                // CUB — cursor back
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                buf.cursor.col = buf.cursor.col.saturating_sub(n);
                buf.cursor.wrap_pending = false;
            }
            b'E' => {
                // CNL — cursor next line
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                buf.cursor.row = (buf.cursor.row + n).min(buf.num_rows - 1);
                buf.cursor.col = 0;
                buf.cursor.wrap_pending = false;
            }
            b'F' => {
                // CPL — cursor preceding line
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                buf.cursor.row = buf.cursor.row.saturating_sub(n);
                buf.cursor.col = 0;
                buf.cursor.wrap_pending = false;
            }
            b'G' => {
                // CHA — cursor character absolute
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                buf.cursor.col = (n.saturating_sub(1)).min(buf.cols - 1);
                buf.cursor.wrap_pending = false;
            }
            b'H' | b'f' => {
                // CUP / HVP — cursor position
                let row = self.parser.param(0, 1) as usize;
                let col = self.parser.param(1, 1) as usize;
                let buf = self.active_buffer_mut();
                let offset = if buf.cursor.origin_mode {
                    buf.scroll_region.top
                } else {
                    0
                };
                buf.cursor.row = (offset + row.saturating_sub(1)).min(buf.num_rows - 1);
                buf.cursor.col = (col.saturating_sub(1)).min(buf.cols - 1);
                buf.cursor.wrap_pending = false;
            }
            b'J' => {
                // ED — erase in display
                let mode = self.parser.param(0, 0);
                if mode == 3 {
                    // ED 3 = xterm "Erase Saved Lines": clear ONLY the scrollback
                    // and leave the visible grid intact. (BUG #3)
                    self.scrollback.clear();
                    self.scroll_offset = 0;
                } else {
                    self.active_buffer_mut().erase_display(mode);
                }
            }
            b'K' => {
                // EL — erase in line
                let mode = self.parser.param(0, 0);
                self.active_buffer_mut().erase_line(mode);
            }
            b'L' => {
                // IL — insert lines
                let n = self.parser.param(0, 1) as usize;
                self.active_buffer_mut().insert_lines(n);
            }
            b'M' => {
                // DL — delete lines
                let n = self.parser.param(0, 1) as usize;
                self.active_buffer_mut().delete_lines(n);
            }
            b'P' => {
                // DCH — delete characters
                let n = self.parser.param(0, 1) as usize;
                self.active_buffer_mut().delete_chars(n);
            }
            b'@' => {
                // ICH — insert characters
                let n = self.parser.param(0, 1) as usize;
                self.active_buffer_mut().insert_chars(n);
            }
            b'S' => {
                // SU — scroll up
                let n = self.parser.param(0, 1) as usize;
                let buf = if self.using_alt {
                    &mut self.alternate
                } else {
                    &mut self.primary
                };
                buf.scroll_up(n, &mut self.scrollback);
            }
            b'T' => {
                // SD — scroll down
                let n = self.parser.param(0, 1) as usize;
                self.active_buffer_mut().scroll_down(n);
            }
            b'X' => {
                // ECH — erase characters
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                let row = buf.cursor.row;
                let col = buf.cursor.col;
                let attrs = buf.cursor.attrs;
                for i in 0..n {
                    let c = col + i;
                    if c < buf.cols {
                        buf.rows[row].cells[c] = Cell::with_char(' ', attrs);
                    }
                }
            }
            b'd' => {
                // VPA — line position absolute
                let n = self.parser.param(0, 1) as usize;
                let buf = self.active_buffer_mut();
                buf.cursor.row = (n.saturating_sub(1)).min(buf.num_rows - 1);
                buf.cursor.wrap_pending = false;
            }
            b'm' => {
                // SGR — select graphic rendition
                self.dispatch_sgr();
            }
            b'r' => {
                // DECSTBM — set scrolling region
                let top = self.parser.param(0, 1) as usize;
                let buf_rows = self.active_buffer().num_rows;
                let bottom = self.parser.param(1, buf_rows as u16) as usize;
                let buf = self.active_buffer_mut();
                buf.scroll_region.top = top.saturating_sub(1);
                buf.scroll_region.bottom = (bottom.saturating_sub(1)).min(buf.num_rows - 1);
                buf.cursor.row = if buf.cursor.origin_mode {
                    buf.scroll_region.top
                } else {
                    0
                };
                buf.cursor.col = 0;
                buf.cursor.wrap_pending = false;
            }
            b's' => {
                // SCOSC — save cursor
                self.active_buffer_mut().save_cursor();
            }
            b'u' => {
                // SCORC — restore cursor
                self.active_buffer_mut().restore_cursor();
            }
            b'n' => {
                // DSR — device status report
                let mode = self.parser.param(0, 0);
                match mode {
                    5 => {
                        self.pty.write_master(b"\x1B[0n");
                    }
                    6 => {
                        let buf = self.active_buffer();
                        let r = buf.cursor.row + 1;
                        let c = buf.cursor.col + 1;
                        let mut response = Vec::new();
                        response.extend_from_slice(b"\x1B[");
                        push_decimal(&mut response, r as u32);
                        response.push(b';');
                        push_decimal(&mut response, c as u32);
                        response.push(b'R');
                        self.pty.write_master(&response);
                    }
                    _ => {}
                }
            }
            b'c' => {
                // DA — device attributes
                if has_gt {
                    self.pty.write_master(b"\x1B[>0;0;0c");
                } else {
                    self.pty.write_master(b"\x1B[?62;22c");
                }
            }
            b't' => {
                // window manipulation
                let op = self.parser.param(0, 0);
                match op {
                    22 => {} // save title
                    23 => {} // restore title
                    _ => {}
                }
            }
            b'q' => {
                // DECSCUSR — set cursor style
                if self.parser.intermediates.contains(&b' ') {
                    let style = self.parser.param(0, 1);
                    let buf = self.active_buffer_mut();
                    match style {
                        0 | 1 => {
                            buf.cursor.shape = CursorShape::Block;
                            buf.cursor.blinking = true;
                        }
                        2 => {
                            buf.cursor.shape = CursorShape::Block;
                            buf.cursor.blinking = false;
                        }
                        3 => {
                            buf.cursor.shape = CursorShape::Underline;
                            buf.cursor.blinking = true;
                        }
                        4 => {
                            buf.cursor.shape = CursorShape::Underline;
                            buf.cursor.blinking = false;
                        }
                        5 => {
                            buf.cursor.shape = CursorShape::Bar;
                            buf.cursor.blinking = true;
                        }
                        6 => {
                            buf.cursor.shape = CursorShape::Bar;
                            buf.cursor.blinking = false;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn dispatch_dec_private(&mut self, final_byte: u8) {
        match final_byte {
            b'h' => {
                // DECSET
                let params: Vec<u16> = self.parser.params.clone();
                for &p in &params {
                    self.set_dec_mode(p, true);
                }
            }
            b'l' => {
                // DECRST
                let params: Vec<u16> = self.parser.params.clone();
                for &p in &params {
                    self.set_dec_mode(p, false);
                }
            }
            _ => {}
        }
    }

    fn set_dec_mode(&mut self, mode: u16, enable: bool) {
        match mode {
            1 => self.modes.cursor_keys_application = enable,
            2 => self.modes.ansi_mode = enable,
            3 => {
                self.modes.column_132 = enable;
            }
            4 => self.modes.smooth_scroll = enable,
            5 => self.modes.screen_reverse = enable,
            6 => {
                self.modes.origin_mode = enable;
                self.active_buffer_mut().cursor.origin_mode = enable;
                let buf = self.active_buffer_mut();
                buf.cursor.row = if enable { buf.scroll_region.top } else { 0 };
                buf.cursor.col = 0;
            }
            7 => self.modes.auto_wrap = enable,
            8 => self.modes.auto_repeat = enable,
            9 => self.modes.mouse_x10 = enable,
            25 => {
                self.modes.cursor_visible = enable;
                self.active_buffer_mut().cursor.visible = enable;
            }
            1000 => self.modes.mouse_normal = enable,
            1002 => self.modes.mouse_button_event = enable,
            1003 => self.modes.mouse_any_event = enable,
            1004 => self.modes.focus_events = enable,
            1005 => self.modes.mouse_utf8 = enable,
            1006 => self.modes.mouse_sgr_ext = enable,
            1015 => self.modes.mouse_urxvt = enable,
            1036 => self.modes.alt_sends_escape = enable,
            1039 => self.modes.meta_sends_escape = enable,
            1048 => {
                if enable {
                    self.active_buffer_mut().save_cursor();
                } else {
                    self.active_buffer_mut().restore_cursor();
                }
            }
            1049 => {
                if enable {
                    self.enter_alt_screen();
                } else {
                    self.exit_alt_screen();
                }
            }
            2004 => self.modes.bracketed_paste = enable,
            _ => {}
        }
    }

    fn dispatch_sgr(&mut self) {
        let params = self.parser.params.clone();
        // Parallel colon-join flags (see `Parser::param_is_colon`). Cloned so the
        // `self.active_buffer_mut()` borrows below don't conflict with reading it.
        let is_colon = self.parser.param_is_colon.clone();
        if params.is_empty() {
            self.active_buffer_mut().cursor.attrs.reset();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            let p = params[i];
            match p {
                0 => self.active_buffer_mut().cursor.attrs.reset(),
                1 => self.active_buffer_mut().cursor.attrs.bold = true,
                2 => self.active_buffer_mut().cursor.attrs.dim = true,
                3 => self.active_buffer_mut().cursor.attrs.italic = true,
                4 => {
                    // A following param is an underline SUB-STYLE only in the
                    // ECMA-48 colon form (`4:3`). A `;`-separated param (`4;7`)
                    // is a DISTINCT attribute and must be left for the next loop
                    // iteration to process — otherwise `ESC[4;7m` loses reverse
                    // and `ESC[4;34m` loses its colour. (BUG #4)
                    let sub_joined = is_colon.get(i + 1).copied().unwrap_or(false);
                    if i + 1 < params.len() && sub_joined {
                        let sub = params[i + 1];
                        match sub {
                            0 => {
                                self.active_buffer_mut().cursor.attrs.underline =
                                    UnderlineStyle::None
                            }
                            1 => {
                                self.active_buffer_mut().cursor.attrs.underline =
                                    UnderlineStyle::Single
                            }
                            2 => {
                                self.active_buffer_mut().cursor.attrs.underline =
                                    UnderlineStyle::Double
                            }
                            3 => {
                                self.active_buffer_mut().cursor.attrs.underline =
                                    UnderlineStyle::Curly
                            }
                            4 => {
                                self.active_buffer_mut().cursor.attrs.underline =
                                    UnderlineStyle::Dotted
                            }
                            5 => {
                                self.active_buffer_mut().cursor.attrs.underline =
                                    UnderlineStyle::Dashed
                            }
                            _ => {
                                self.active_buffer_mut().cursor.attrs.underline =
                                    UnderlineStyle::Single
                            }
                        }
                        i += 1;
                    } else {
                        self.active_buffer_mut().cursor.attrs.underline = UnderlineStyle::Single;
                    }
                }
                5 => self.active_buffer_mut().cursor.attrs.blink_slow = true,
                6 => self.active_buffer_mut().cursor.attrs.blink_rapid = true,
                7 => self.active_buffer_mut().cursor.attrs.reverse = true,
                8 => self.active_buffer_mut().cursor.attrs.hidden = true,
                9 => self.active_buffer_mut().cursor.attrs.strikethrough = true,
                21 => self.active_buffer_mut().cursor.attrs.underline = UnderlineStyle::Double,
                22 => {
                    self.active_buffer_mut().cursor.attrs.bold = false;
                    self.active_buffer_mut().cursor.attrs.dim = false;
                }
                23 => self.active_buffer_mut().cursor.attrs.italic = false,
                24 => self.active_buffer_mut().cursor.attrs.underline = UnderlineStyle::None,
                25 => {
                    self.active_buffer_mut().cursor.attrs.blink_slow = false;
                    self.active_buffer_mut().cursor.attrs.blink_rapid = false;
                }
                27 => self.active_buffer_mut().cursor.attrs.reverse = false,
                28 => self.active_buffer_mut().cursor.attrs.hidden = false,
                29 => self.active_buffer_mut().cursor.attrs.strikethrough = false,
                53 => self.active_buffer_mut().cursor.attrs.overline = true,
                55 => self.active_buffer_mut().cursor.attrs.overline = false,
                30..=37 => {
                    self.active_buffer_mut().cursor.attrs.fg = Color::Indexed((p - 30) as u8);
                }
                38 => {
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            5 => {
                                if i + 2 < params.len() {
                                    self.active_buffer_mut().cursor.attrs.fg =
                                        Color::Indexed(params[i + 2] as u8);
                                    i += 2;
                                }
                            }
                            2 => {
                                if i + 4 < params.len() {
                                    self.active_buffer_mut().cursor.attrs.fg = Color::Rgb(
                                        params[i + 2] as u8,
                                        params[i + 3] as u8,
                                        params[i + 4] as u8,
                                    );
                                    i += 4;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                39 => self.active_buffer_mut().cursor.attrs.fg = Color::Default,
                40..=47 => {
                    self.active_buffer_mut().cursor.attrs.bg = Color::Indexed((p - 40) as u8);
                }
                48 => {
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            5 => {
                                if i + 2 < params.len() {
                                    self.active_buffer_mut().cursor.attrs.bg =
                                        Color::Indexed(params[i + 2] as u8);
                                    i += 2;
                                }
                            }
                            2 => {
                                if i + 4 < params.len() {
                                    self.active_buffer_mut().cursor.attrs.bg = Color::Rgb(
                                        params[i + 2] as u8,
                                        params[i + 3] as u8,
                                        params[i + 4] as u8,
                                    );
                                    i += 4;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                49 => self.active_buffer_mut().cursor.attrs.bg = Color::Default,
                58 => {
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            5 => {
                                if i + 2 < params.len() {
                                    self.active_buffer_mut().cursor.attrs.underline_color =
                                        Color::Indexed(params[i + 2] as u8);
                                    i += 2;
                                }
                            }
                            2 => {
                                if i + 4 < params.len() {
                                    self.active_buffer_mut().cursor.attrs.underline_color =
                                        Color::Rgb(
                                            params[i + 2] as u8,
                                            params[i + 3] as u8,
                                            params[i + 4] as u8,
                                        );
                                    i += 4;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                59 => self.active_buffer_mut().cursor.attrs.underline_color = Color::Default,
                90..=97 => {
                    self.active_buffer_mut().cursor.attrs.fg = Color::Indexed((p - 90 + 8) as u8);
                }
                100..=107 => {
                    self.active_buffer_mut().cursor.attrs.bg = Color::Indexed((p - 100 + 8) as u8);
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn osc_byte(&mut self, byte: u8) {
        match byte {
            0x07 => {
                self.dispatch_osc();
                self.parser.state = ParseState::Ground;
            }
            0x1B => {
                self.parser.state = ParseState::Ground;
                self.dispatch_osc();
            }
            0x9C => {
                self.dispatch_osc();
                self.parser.state = ParseState::Ground;
            }
            _ => {
                self.parser.osc_data.push(byte);
            }
        }
    }

    fn dispatch_osc(&mut self) {
        let data = core::mem::take(&mut self.parser.osc_data);
        if data.is_empty() {
            return;
        }
        let sep = data.iter().position(|&b| b == b';');
        let (cmd_bytes, payload) = if let Some(pos) = sep {
            (&data[..pos], &data[pos + 1..])
        } else {
            (data.as_slice(), &[][..])
        };

        let mut cmd = 0u32;
        for &b in cmd_bytes {
            if b.is_ascii_digit() {
                cmd = cmd * 10 + (b - b'0') as u32;
            }
        }

        match cmd {
            0 | 2 => {
                if let Ok(s) = core::str::from_utf8(payload) {
                    self.title.clear();
                    self.title.push_str(s);
                }
            }
            1 => {
                if let Ok(s) = core::str::from_utf8(payload) {
                    self.icon_title.clear();
                    self.icon_title.push_str(s);
                }
            }
            4 => { /* set colour index — we'd parse idx;spec here */ }
            7 => {
                if let Ok(s) = core::str::from_utf8(payload) {
                    self.shell_integration.set_cwd(s);
                }
            }
            8 => { /* hyperlink — OSC 8;params;uri ST */ }
            9 => { /* notification */ }
            10 => { /* foreground colour query/set */ }
            11 => { /* background colour query/set */ }
            12 => { /* cursor colour query/set */ }
            52 => { /* clipboard set/query */ }
            133 => {
                if !payload.is_empty() {
                    let buf_row = self.active_buffer().cursor.row;
                    match payload[0] {
                        b'A' => self.shell_integration.mark_prompt(buf_row),
                        b'B' => self.shell_integration.mark_command_start(buf_row),
                        b'C' => self.shell_integration.mark_output_start(buf_row),
                        b'D' => {
                            let code = if payload.len() > 2 {
                                let mut v = 0i32;
                                for &b in &payload[2..] {
                                    if b.is_ascii_digit() {
                                        v = v * 10 + (b - b'0') as i32;
                                    }
                                }
                                v
                            } else {
                                0
                            };
                            self.shell_integration.mark_command_end(buf_row, code);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn dcs_entry_byte(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                self.parser.current_param = Some((byte - b'0') as u32);
                self.parser.state = ParseState::DcsParam;
            }
            b';' => {
                self.parser.params.push(0);
                self.parser.param_is_colon.push(false);
                self.parser.state = ParseState::DcsParam;
            }
            0x40..=0x7E => {
                self.parser.state = ParseState::DcsPassthrough;
            }
            _ => {
                self.parser.state = ParseState::DcsPassthrough;
            }
        }
    }

    fn dcs_param_byte(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                let v = self.parser.current_param.unwrap_or(0);
                // Saturate on hostile input (mirror of `csi_param_byte`).
                self.parser.current_param =
                    Some(v.saturating_mul(10).saturating_add((byte - b'0') as u32));
            }
            b';' => self.parser.finish_param(),
            0x40..=0x7E => {
                self.parser.finish_param();
                self.parser.state = ParseState::DcsPassthrough;
            }
            _ => {
                self.parser.state = ParseState::DcsPassthrough;
            }
        }
    }

    fn dcs_passthrough_byte(&mut self, byte: u8) {
        match byte {
            0x1B => {
                self.parser.state = ParseState::Ground;
            }
            0x9C => {
                self.parser.state = ParseState::Ground;
            }
            _ => {
                self.parser.dcs_data.push(byte);
            }
        }
    }

    fn dcs_ignore_byte(&mut self, byte: u8) {
        if byte == 0x9C || byte == 0x1B {
            self.parser.state = ParseState::Ground;
        }
    }

    fn sos_byte(&mut self, byte: u8) {
        if byte == 0x9C || byte == 0x1B {
            self.parser.state = ParseState::Ground;
        }
    }

    pub fn hard_reset(&mut self) {
        let cols = self.width;
        let rows = self.height;
        self.primary = ScreenBuffer::new(cols, rows);
        self.alternate = ScreenBuffer::new(cols, rows);
        self.using_alt = false;
        self.scrollback.clear();
        self.scroll_offset = 0;
        self.modes = DecModes::new();
        self.parser.reset();
        self.mouse = MouseState::new();
        self.selection.clear();
        self.search.clear();
        self.palette = default_256_palette();
        self.title = String::from("Terminal");
        self.icon_title.clear();
    }

    pub fn soft_reset(&mut self) {
        self.modes.cursor_keys_application = false;
        self.modes.origin_mode = false;
        self.modes.auto_wrap = true;
        self.active_buffer_mut().cursor.attrs.reset();
        self.active_buffer_mut().cursor.origin_mode = false;
        let buf = self.active_buffer_mut();
        buf.scroll_region = ScrollRegion {
            top: 0,
            bottom: buf.num_rows - 1,
        };
        buf.charset_state = CharsetState::new();
    }

    pub fn write_input(&mut self, data: &[u8]) {
        for &byte in data {
            self.process_byte(byte);
        }
    }

    pub fn search_scrollback(&mut self, query: &str) {
        self.search.matches.clear();
        self.search.query.clear();
        self.search.query.push_str(query);
        self.search.active = true;

        let query_lower: Vec<char> = if self.search.case_sensitive {
            query.chars().collect()
        } else {
            query.chars().flat_map(|c| c.to_lowercase()).collect()
        };
        if query_lower.is_empty() {
            return;
        }

        for (row_idx, row) in self.scrollback.iter().enumerate() {
            let row_chars: Vec<char> = row
                .cells
                .iter()
                .map(|c| {
                    if self.search.case_sensitive {
                        c.ch
                    } else {
                        c.ch.to_lowercase().next().unwrap_or(c.ch)
                    }
                })
                .collect();
            for col in 0..row_chars.len().saturating_sub(query_lower.len() - 1) {
                if row_chars[col..col + query_lower.len()] == query_lower[..] {
                    self.search.matches.push((row_idx, col, query_lower.len()));
                }
            }
        }

        let sb_len = self.scrollback.len();
        let case_sensitive = self.search.case_sensitive;
        let buf = if self.using_alt {
            &self.alternate
        } else {
            &self.primary
        };
        let mut new_matches: Vec<(usize, usize, usize)> = Vec::new();
        for (row_idx, row) in buf.rows.iter().enumerate() {
            let row_chars: Vec<char> = row
                .cells
                .iter()
                .map(|c| {
                    if case_sensitive {
                        c.ch
                    } else {
                        c.ch.to_lowercase().next().unwrap_or(c.ch)
                    }
                })
                .collect();
            for col in 0..row_chars.len().saturating_sub(query_lower.len() - 1) {
                if row_chars[col..col + query_lower.len()] == query_lower[..] {
                    new_matches.push((sb_len + row_idx, col, query_lower.len()));
                }
            }
        }
        self.search.matches.extend(new_matches);
        self.search.current_match = 0;
    }

    pub fn get_selected_text(&self) -> String {
        if !self.selection.active {
            return String::new();
        }
        let (start, end) = self.selection.normalized();
        let buf = self.active_buffer();
        let mut result = String::new();
        for row_idx in start.0..=end.0.min(buf.num_rows - 1) {
            let row = &buf.rows[row_idx];
            let col_start = if row_idx == start.0 { start.1 } else { 0 };
            let col_end = if row_idx == end.0 {
                end.1 + 1
            } else {
                buf.cols
            };
            for col in col_start..col_end.min(buf.cols) {
                if self.selection.mode == SelectionMode::Block {
                    let min_c = start.1.min(end.1);
                    let max_c = start.1.max(end.1);
                    if col >= min_c && col <= max_c {
                        result.push(row.cells[col].ch);
                    }
                } else {
                    result.push(row.cells[col].ch);
                }
            }
            if row_idx < end.0 && !row.wrapped {
                result.push('\n');
            }
        }
        result
    }

    pub fn encode_mouse_event(
        &self,
        button: MouseButton,
        col: usize,
        row: usize,
        pressed: bool,
    ) -> Vec<u8> {
        let mut result = Vec::new();
        if self.modes.mouse_sgr_ext {
            let btn = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                MouseButton::WheelUp => 64,
                MouseButton::WheelDown => 65,
                MouseButton::Button4 => 128,
                MouseButton::Button5 => 129,
                MouseButton::Released => 3,
            };
            result.extend_from_slice(b"\x1B[<");
            push_decimal(&mut result, btn);
            result.push(b';');
            push_decimal(&mut result, (col + 1) as u32);
            result.push(b';');
            push_decimal(&mut result, (row + 1) as u32);
            result.push(if pressed { b'M' } else { b'm' });
        } else if self.modes.mouse_urxvt {
            let btn = match button {
                MouseButton::Left => 0u32,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                MouseButton::WheelUp => 64,
                MouseButton::WheelDown => 65,
                _ => 3,
            } + 32;
            result.extend_from_slice(b"\x1B[");
            push_decimal(&mut result, btn);
            result.push(b';');
            push_decimal(&mut result, (col + 1) as u32);
            result.push(b';');
            push_decimal(&mut result, (row + 1) as u32);
            result.push(b'M');
        } else {
            let btn = match button {
                MouseButton::Left => 0u8,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                MouseButton::WheelUp => 64,
                MouseButton::WheelDown => 65,
                _ => 3,
            };
            result.extend_from_slice(b"\x1B[M");
            result.push(btn + 32);
            result.push((col + 33).min(255) as u8);
            result.push((row + 33).min(255) as u8);
        }
        result
    }

    pub fn encode_key(&self, key: KeyCode, mods: Modifiers) -> Vec<u8> {
        let mut result = Vec::new();
        match key {
            KeyCode::Char(ch) => {
                if mods.ctrl && ch.is_ascii_alphabetic() {
                    let ctrl_ch = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                    result.push(ctrl_ch);
                } else if mods.alt && self.modes.alt_sends_escape {
                    result.push(0x1B);
                    let mut buf = [0u8; 4];
                    let s = ch.encode_utf8(&mut buf);
                    result.extend_from_slice(s.as_bytes());
                } else {
                    let mut buf = [0u8; 4];
                    let s = ch.encode_utf8(&mut buf);
                    result.extend_from_slice(s.as_bytes());
                }
            }
            KeyCode::Enter => result.push(0x0D),
            KeyCode::Tab => result.push(0x09),
            KeyCode::Backspace => result.push(0x7F),
            KeyCode::Escape => result.push(0x1B),
            KeyCode::Up => {
                if self.modes.cursor_keys_application {
                    result.extend_from_slice(b"\x1BOA");
                } else {
                    result.extend_from_slice(b"\x1B[A");
                }
            }
            KeyCode::Down => {
                if self.modes.cursor_keys_application {
                    result.extend_from_slice(b"\x1BOB");
                } else {
                    result.extend_from_slice(b"\x1B[B");
                }
            }
            KeyCode::Right => {
                if self.modes.cursor_keys_application {
                    result.extend_from_slice(b"\x1BOC");
                } else {
                    result.extend_from_slice(b"\x1B[C");
                }
            }
            KeyCode::Left => {
                if self.modes.cursor_keys_application {
                    result.extend_from_slice(b"\x1BOD");
                } else {
                    result.extend_from_slice(b"\x1B[D");
                }
            }
            KeyCode::Home => result.extend_from_slice(b"\x1B[H"),
            KeyCode::End => result.extend_from_slice(b"\x1B[F"),
            KeyCode::Insert => result.extend_from_slice(b"\x1B[2~"),
            KeyCode::Delete => result.extend_from_slice(b"\x1B[3~"),
            KeyCode::PageUp => result.extend_from_slice(b"\x1B[5~"),
            KeyCode::PageDown => result.extend_from_slice(b"\x1B[6~"),
            KeyCode::F(n) => {
                let seq: &[u8] = match n {
                    1 => b"\x1BOP",
                    2 => b"\x1BOQ",
                    3 => b"\x1BOR",
                    4 => b"\x1BOS",
                    5 => b"\x1B[15~",
                    6 => b"\x1B[17~",
                    7 => b"\x1B[18~",
                    8 => b"\x1B[19~",
                    9 => b"\x1B[20~",
                    10 => b"\x1B[21~",
                    11 => b"\x1B[23~",
                    12 => b"\x1B[24~",
                    13 => b"\x1B[25~",
                    14 => b"\x1B[26~",
                    15 => b"\x1B[28~",
                    16 => b"\x1B[29~",
                    17 => b"\x1B[31~",
                    18 => b"\x1B[32~",
                    19 => b"\x1B[33~",
                    20 => b"\x1B[34~",
                    _ => b"",
                };
                result.extend_from_slice(seq);
            }
        }
        result
    }

    pub fn apply_color_scheme(&mut self, scheme: ColorScheme) {
        self.profile.scheme = scheme;
        let colors = scheme.colors();
        for &(idx, val) in &colors.palette_overrides {
            if val != 0 {
                self.palette[idx] = val;
            }
        }
    }
}

// ── Key codes ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    };
}

// ── Tab ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Tab {
    pub id: u64,
    pub panes: Vec<Pane>,
    pub active_pane: usize,
    pub title: String,
    pub splits: Vec<SplitInfo>,
}

#[derive(Debug, Clone)]
pub struct SplitInfo {
    pub direction: SplitDirection,
    pub position: f32,
    pub pane_a: u64,
    pub pane_b: u64,
}

impl Tab {
    pub fn new(id: u64, cols: usize, rows: usize) -> Self {
        let pane = Pane::new(1, cols, rows);
        Self {
            id,
            panes: vec![pane],
            active_pane: 0,
            title: String::from("Terminal"),
            splits: Vec::new(),
        }
    }

    pub fn active_pane(&self) -> &Pane {
        &self.panes[self.active_pane]
    }

    pub fn active_pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active_pane]
    }

    pub fn split_horizontal(&mut self, next_pane_id: u64) {
        let cols = self.panes[self.active_pane].width;
        let rows = self.panes[self.active_pane].height / 2;
        self.panes[self.active_pane].resize(cols, rows);
        let mut new_pane = Pane::new(next_pane_id, cols, rows);
        new_pane.y = self.panes[self.active_pane].y + rows;
        let old_id = self.panes[self.active_pane].id;
        self.splits.push(SplitInfo {
            direction: SplitDirection::Horizontal,
            position: 0.5,
            pane_a: old_id,
            pane_b: next_pane_id,
        });
        self.panes.push(new_pane);
    }

    pub fn split_vertical(&mut self, next_pane_id: u64) {
        let cols = self.panes[self.active_pane].width / 2;
        let rows = self.panes[self.active_pane].height;
        self.panes[self.active_pane].resize(cols, rows);
        let mut new_pane = Pane::new(next_pane_id, cols, rows);
        new_pane.x = self.panes[self.active_pane].x + cols;
        let old_id = self.panes[self.active_pane].id;
        self.splits.push(SplitInfo {
            direction: SplitDirection::Vertical,
            position: 0.5,
            pane_a: old_id,
            pane_b: next_pane_id,
        });
        self.panes.push(new_pane);
    }

    pub fn close_pane(&mut self, pane_id: u64) {
        self.panes.retain(|p| p.id != pane_id);
        self.splits
            .retain(|s| s.pane_a != pane_id && s.pane_b != pane_id);
        if self.active_pane >= self.panes.len() {
            self.active_pane = self.panes.len().saturating_sub(1);
        }
    }

    pub fn focus_next_pane(&mut self) {
        if !self.panes.is_empty() {
            self.active_pane = (self.active_pane + 1) % self.panes.len();
        }
    }

    pub fn focus_prev_pane(&mut self) {
        if !self.panes.is_empty() {
            self.active_pane = self
                .active_pane
                .checked_sub(1)
                .unwrap_or(self.panes.len() - 1);
        }
    }

    pub fn zoom_pane(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.active_pane) {
            pane.zoomed = !pane.zoomed;
        }
    }
}

// ── Terminal emulator ────────────────────────────────────────────────────

pub struct TerminalEmulator {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub next_tab_id: u64,
    pub next_pane_id: u64,
    pub cols: usize,
    pub rows: usize,
    pub tab_bar_height: usize,
    pub default_profile: Profile,
    pub profiles: Vec<Profile>,
    pub clipboard: String,
    pub bell_visual: bool,
    pub bell_audio: bool,
    pub url_regex: String,
}

impl TerminalEmulator {
    pub fn new(cols: usize, rows: usize) -> Self {
        let tab = Tab::new(1, cols, rows.saturating_sub(1));
        Self {
            tabs: vec![tab],
            active_tab: 0,
            next_tab_id: 2,
            next_pane_id: 2,
            cols,
            rows,
            tab_bar_height: 1,
            default_profile: Profile::default_profile(),
            profiles: Vec::new(),
            clipboard: String::new(),
            bell_visual: true,
            bell_audio: false,
            url_regex: String::from(r"https?://[^\s<>]+"),
        }
    }

    pub fn new_tab(&mut self) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.next_pane_id += 1;
        let tab = Tab::new(id, self.cols, self.rows.saturating_sub(self.tab_bar_height));
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        id
    }

    pub fn close_tab(&mut self, tab_id: u64) {
        self.tabs.retain(|t| t.id != tab_id);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len().saturating_sub(1);
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab)
    }

    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
        }
    }

    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from < self.tabs.len() && to < self.tabs.len() && from != to {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
            self.active_tab = to;
        }
    }

    pub fn rename_tab(&mut self, tab_id: u64, name: &str) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.title.clear();
            tab.title.push_str(name);
        }
    }

    pub fn split_active_horizontal(&mut self) -> u64 {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.split_horizontal(id);
        }
        id
    }

    pub fn split_active_vertical(&mut self) -> u64 {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.split_vertical(id);
        }
        id
    }

    pub fn write_to_active(&mut self, data: &[u8]) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.active_pane_mut().write_input(data);
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        let pane_rows = rows.saturating_sub(self.tab_bar_height);
        for tab in &mut self.tabs {
            for pane in &mut tab.panes {
                pane.resize(cols, pane_rows);
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyCode, mods: Modifiers) -> Vec<u8> {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let pane = tab.active_pane_mut();
            pane.encode_key(key, mods)
        } else {
            Vec::new()
        }
    }

    pub fn handle_mouse(
        &mut self,
        button: MouseButton,
        col: usize,
        row: usize,
        pressed: bool,
    ) -> Vec<u8> {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let pane = tab.active_pane_mut();
            pane.mouse.col = col;
            pane.mouse.row = row;
            pane.mouse.pressed = pressed;
            pane.mouse.last_button = button;

            let has_tracking = pane.modes.mouse_normal
                || pane.modes.mouse_button_event
                || pane.modes.mouse_any_event
                || pane.modes.mouse_x10;

            if has_tracking {
                return pane.encode_mouse_event(button, col, row, pressed);
            }

            if button == MouseButton::Left {
                if pressed {
                    pane.selection.start(row, col, SelectionMode::Character);
                } else {
                    pane.selection.update(row, col);
                }
            }
        }
        Vec::new()
    }

    pub fn paste(&mut self, text: &str) -> Vec<u8> {
        let mut result = Vec::new();
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let pane = tab.active_pane();
            if pane.modes.bracketed_paste {
                result.extend_from_slice(b"\x1B[200~");
            }
            result.extend_from_slice(text.as_bytes());
            if pane.modes.bracketed_paste {
                result.extend_from_slice(b"\x1B[201~");
            }
        }
        result
    }

    pub fn copy_selection(&mut self) -> String {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            tab.active_pane().get_selected_text()
        } else {
            String::new()
        }
    }

    pub fn add_profile(&mut self, profile: Profile) {
        self.profiles.push(profile);
    }

    pub fn apply_profile_to_active(&mut self, name: &str) {
        let profile = self.profiles.iter().find(|p| p.name == name).cloned();
        if let Some(prof) = profile {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                let pane = tab.active_pane_mut();
                pane.profile = prof.clone();
                pane.apply_color_scheme(prof.scheme);
            }
        }
    }

    pub fn focus_report(&self, focused: bool) -> Vec<u8> {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if tab.active_pane().modes.focus_events {
                return if focused {
                    b"\x1B[I".to_vec()
                } else {
                    b"\x1B[O".to_vec()
                };
            }
        }
        Vec::new()
    }
}

impl TerminalEmulator {
    pub fn render(&self, canvas: &mut raegfx::Canvas, ox: usize, oy: usize, w: usize, h: usize) {
        canvas.fill_rect(ox, oy, w, h, TERM_BG);

        let tab_h = GLYPH_H + 8;
        let mut tx = ox;
        for (i, tab) in self.tabs.iter().enumerate() {
            let bg = if i == self.active_tab {
                TAB_ACTIVE
            } else {
                TAB_BG
            };
            let label_w = tab.title.len() * GLYPH_W + 16;
            canvas.fill_rect(tx, oy, label_w, tab_h, bg);
            let fg = if i == self.active_tab {
                TAB_ACCENT
            } else {
                TAB_FG
            };
            canvas.draw_text(tx + 8, oy + 4, &tab.title, fg, None);
            tx += label_w + 2;
        }
        canvas.draw_line(
            ox as i32,
            (oy + tab_h) as i32,
            (ox + w) as i32,
            (oy + tab_h) as i32,
            SPLIT_BORDER,
        );

        if let Some(tab) = self.tabs.get(self.active_tab) {
            let pane = tab.active_pane();
            let buf = pane.active_buffer();
            let palette = &pane.palette;
            let base_y = oy + tab_h + 2;
            let max_rows = (h.saturating_sub(tab_h + 2)) / GLYPH_H;
            let max_cols = w / GLYPH_W;

            for row_idx in 0..buf.num_rows.min(max_rows) {
                if row_idx >= buf.rows.len() {
                    break;
                }
                let row = &buf.rows[row_idx];
                let py = base_y + row_idx * GLYPH_H;
                for col_idx in 0..buf.cols.min(max_cols) {
                    if col_idx >= row.cells.len() {
                        break;
                    }
                    let cell = &row.cells[col_idx];
                    let (mut fg_color, mut bg_color) = Self::resolve_colors(&cell.attrs, palette);
                    if cell.attrs.reverse {
                        core::mem::swap(&mut fg_color, &mut bg_color);
                    }
                    let px = ox + col_idx * GLYPH_W;
                    if bg_color != TERM_BG {
                        canvas.fill_rect(px, py, GLYPH_W, GLYPH_H, bg_color);
                    }
                    if cell.ch != ' ' && !cell.attrs.hidden {
                        canvas.draw_glyph(px, py, cell.ch, fg_color, None);
                    }
                }
            }

            if buf.cursor.visible {
                let cx = ox + buf.cursor.col * GLYPH_W;
                let cy = base_y + buf.cursor.row * GLYPH_H;
                match pane.profile.cursor_shape {
                    CursorShape::Block => canvas.fill_rect(cx, cy, GLYPH_W, GLYPH_H, TERM_CURSOR),
                    CursorShape::Underline => {
                        canvas.fill_rect(cx, cy + GLYPH_H - 2, GLYPH_W, 2, TERM_CURSOR)
                    }
                    CursorShape::Bar => canvas.fill_rect(cx, cy, 2, GLYPH_H, TERM_CURSOR),
                }
            }
        }
    }

    fn resolve_colors(attrs: &CellAttrs, palette: &[u32; 256]) -> (u32, u32) {
        let fg = attrs.fg.to_argb(palette, TERM_FG);
        let bg = attrs.bg.to_argb(palette, TERM_BG);
        (fg, bg)
    }

    pub fn handle_char_input(&mut self, ch: u8) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.active_pane_mut().write_input(&[ch]);
        }
    }
}

fn push_decimal(buf: &mut Vec<u8>, mut n: u32) {
    if n == 0 {
        buf.push(b'0');
        return;
    }
    let start = buf.len();
    while n > 0 {
        buf.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    buf[start..].reverse();
}

// ── Global instance ──────────────────────────────────────────────────────

struct TerminalEmulatorHolder {
    inner: Option<TerminalEmulator>,
}

static mut TERMINAL_EMULATOR_HOLDER: TerminalEmulatorHolder =
    TerminalEmulatorHolder { inner: None };
static TERMINAL_EMULATOR_INIT: AtomicBool = AtomicBool::new(false);

pub fn init() {
    if TERMINAL_EMULATOR_INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        TERMINAL_EMULATOR_HOLDER.inner = Some(TerminalEmulator::new(120, 40));
    }
}

pub fn with_terminal_emulator<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut TerminalEmulator) -> R,
{
    if !TERMINAL_EMULATOR_INIT.load(Ordering::SeqCst) {
        return None;
    }
    unsafe { TERMINAL_EMULATOR_HOLDER.inner.as_mut().map(f) }
}

// ── Host KAT suite ───────────────────────────────────────────────────────
//
// FAIL-able host KATs for the VT/ANSI parser + grid model. Every test asserts
// a concrete post-condition on the grid/cursor per the xterm/ECMA-48 spec, so
// a wrong emulator makes the test go red. Tests tagged `#[ignore]` + `// BUG:`
// document confirmed spec divergences (see the BUG LIST in the hand-off) and
// flip to PASS once the emulator is fixed — do NOT weaken them to pass now.
//
// Run: `cargo test -p raeshell --lib` (per-crate — NEVER `--workspace`; the
// no_std kernel-side crates trip "duplicate lang item panic_impl"). The whole
// crate is `std` under `cfg(test)` so overflow checks are live.
#[cfg(test)]
mod terminal_emulator_kat {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────
    fn term(cols: usize, rows: usize) -> Pane {
        Pane::new(1, cols, rows)
    }

    fn feed(p: &mut Pane, bytes: &[u8]) {
        p.write_input(bytes);
    }

    /// Character at (row, col) of the active buffer.
    fn ch(p: &Pane, row: usize, col: usize) -> char {
        p.active_buffer().rows[row].cells[col].ch
    }

    fn attrs(p: &Pane, row: usize, col: usize) -> CellAttrs {
        p.active_buffer().rows[row].cells[col].attrs
    }

    /// (row, col) of the cursor in the active buffer.
    fn cur(p: &Pane) -> (usize, usize) {
        let b = p.active_buffer();
        (b.cursor.row, b.cursor.col)
    }

    // ── 1. Plain text ────────────────────────────────────────────────────
    #[test]
    fn plain_text_lands_in_cells_and_advances_cursor() {
        let mut p = term(10, 3);
        feed(&mut p, b"Hi");
        assert_eq!(ch(&p, 0, 0), 'H');
        assert_eq!(ch(&p, 0, 1), 'i');
        assert_eq!(cur(&p), (0, 2), "cursor advances one col per printed cell");
    }

    #[test]
    fn printed_char_carries_active_attrs() {
        let mut p = term(10, 3);
        feed(&mut p, b"\x1b[1mA"); // bold on, then 'A'
        assert!(
            attrs(&p, 0, 0).bold,
            "bold SGR must be captured on the cell"
        );
        // A subsequent reset must not retroactively touch already-printed cells.
        feed(&mut p, b"\x1b[0mB");
        assert!(attrs(&p, 0, 0).bold, "old cell keeps its attrs after reset");
        assert!(!attrs(&p, 0, 1).bold, "cell after reset is not bold");
    }

    // ── 2. Cursor movement + clamping ────────────────────────────────────
    #[test]
    fn cup_is_one_based_and_zero_indexed_internally() {
        let mut p = term(20, 10);
        feed(&mut p, b"\x1b[3;5H"); // row 3, col 5 (1-based)
        assert_eq!(cur(&p), (2, 4));
    }

    #[test]
    fn cup_bare_homes_cursor() {
        let mut p = term(20, 10);
        feed(&mut p, b"\x1b[5;5H\x1b[H");
        assert_eq!(cur(&p), (0, 0));
    }

    #[test]
    fn cup_missing_params_default_to_one() {
        let mut p = term(20, 10);
        feed(&mut p, b"\x1b[;5H"); // empty row => 1, col 5
        assert_eq!(cur(&p), (0, 4));
        feed(&mut p, b"\x1b[7;H"); // row 7, empty col => 1
        assert_eq!(cur(&p), (6, 0));
    }

    #[test]
    fn cup_clamps_out_of_range_to_grid_edge() {
        let mut p = term(10, 4);
        feed(&mut p, b"\x1b[99;99H");
        assert_eq!(cur(&p), (3, 9), "cursor never escapes the grid");
    }

    #[test]
    fn cursor_up_down_forward_back_clamp_at_edges() {
        let mut p = term(10, 4);
        // Up from home saturates at row 0.
        feed(&mut p, b"\x1b[5A");
        assert_eq!(cur(&p), (0, 0));
        // Down 100 clamps at last row.
        feed(&mut p, b"\x1b[100B");
        assert_eq!(cur(&p), (3, 0));
        // Forward 100 clamps at last col.
        feed(&mut p, b"\x1b[100C");
        assert_eq!(cur(&p), (3, 9));
        // Back 100 saturates at col 0.
        feed(&mut p, b"\x1b[100D");
        assert_eq!(cur(&p), (3, 0));
    }

    #[test]
    fn cursor_move_default_param_is_one() {
        let mut p = term(10, 4);
        feed(&mut p, b"\x1b[3;3H"); // (2,2)
        feed(&mut p, b"\x1b[A"); // up 1
        assert_eq!(cur(&p), (1, 2));
        feed(&mut p, b"\x1b[C"); // right 1
        assert_eq!(cur(&p), (1, 3));
    }

    #[test]
    fn cha_and_vpa_are_absolute() {
        let mut p = term(20, 10);
        feed(&mut p, b"\x1b[5;5H\x1b[9G"); // CHA -> col 9 (1-based) => col 8
        assert_eq!(cur(&p).1, 8);
        feed(&mut p, b"\x1b[4d"); // VPA -> row 4 (1-based) => row 3
        assert_eq!(cur(&p).0, 3);
    }

    // ── 3. SGR ───────────────────────────────────────────────────────────
    #[test]
    fn sgr_reset_clears_all_attrs() {
        let mut p = term(10, 2);
        // NB: avoid `4` mid-sequence (see BUG #4); test reset with bold+reverse.
        feed(&mut p, b"\x1b[1;7mX\x1b[0mY");
        let a = attrs(&p, 0, 0);
        assert!(a.bold && a.reverse);
        let b = attrs(&p, 0, 1);
        assert!(!b.bold && !b.reverse && b.underline == UnderlineStyle::None);
    }

    #[test]
    fn sgr_standalone_underline() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[4mA"); // 4 as the only/last param works correctly
        assert_eq!(attrs(&p, 0, 0).underline, UnderlineStyle::Single);
    }

    #[test]
    fn sgr_bare_m_is_reset() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[1m\x1b[mX");
        assert!(
            !attrs(&p, 0, 0).bold,
            "ESC[m with no params is a full reset"
        );
    }

    #[test]
    fn sgr_16_color_fg_and_bg() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[31;42mX");
        assert_eq!(attrs(&p, 0, 0).fg, Color::Indexed(1));
        assert_eq!(attrs(&p, 0, 0).bg, Color::Indexed(2));
    }

    #[test]
    fn sgr_bright_color_maps_to_high_palette() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[91;102mX");
        assert_eq!(attrs(&p, 0, 0).fg, Color::Indexed(9));
        assert_eq!(attrs(&p, 0, 0).bg, Color::Indexed(10));
    }

    #[test]
    fn sgr_256_color_indexed() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[38;5;196mX");
        assert_eq!(attrs(&p, 0, 0).fg, Color::Indexed(196));
    }

    #[test]
    fn sgr_truecolor_rgb() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[38;2;10;20;30mX");
        assert_eq!(attrs(&p, 0, 0).fg, Color::Rgb(10, 20, 30));
        feed(&mut p, b"\x1b[48;2;40;50;60mY");
        assert_eq!(attrs(&p, 0, 1).bg, Color::Rgb(40, 50, 60));
    }

    #[test]
    fn sgr_garbage_param_is_ignored_not_panicking() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[999mX"); // unknown SGR
        assert_eq!(attrs(&p, 0, 0), CellAttrs::default_attrs());
        assert_eq!(ch(&p, 0, 0), 'X');
    }

    #[test]
    fn sgr_bold_reverse_toggle_off() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[1;7mA\x1b[22;27mB");
        assert!(attrs(&p, 0, 0).bold && attrs(&p, 0, 0).reverse);
        assert!(!attrs(&p, 0, 1).bold && !attrs(&p, 0, 1).reverse);
    }

    // ── 4. Erase (ED / EL) ───────────────────────────────────────────────
    #[test]
    fn ed_2j_clears_whole_screen_with_current_bg() {
        let mut p = term(3, 2);
        feed(&mut p, b"abcdef"); // row0=abc, row1=def
        feed(&mut p, b"\x1b[41m"); // red bg becomes current
        feed(&mut p, b"\x1b[2J");
        for r in 0..2 {
            for c in 0..3 {
                assert_eq!(ch(&p, r, c), ' ', "cell ({r},{c}) cleared");
                assert_eq!(
                    attrs(&p, r, c).bg,
                    Color::Indexed(1),
                    "cleared with current bg"
                );
            }
        }
    }

    #[test]
    fn ed_0j_clears_cursor_to_end() {
        let mut p = term(3, 2);
        feed(&mut p, b"abcdef");
        feed(&mut p, b"\x1b[1;2H\x1b[0J"); // cursor (0,1)
        assert_eq!(ch(&p, 0, 0), 'a', "before cursor untouched");
        assert_eq!(ch(&p, 0, 1), ' ', "cursor cell cleared");
        assert_eq!(ch(&p, 0, 2), ' ');
        assert_eq!(ch(&p, 1, 0), ' ', "rows below cleared");
    }

    #[test]
    fn ed_1j_clears_start_to_cursor() {
        let mut p = term(3, 2);
        feed(&mut p, b"abcdef");
        feed(&mut p, b"\x1b[2;2H\x1b[1J"); // cursor (1,1)
        assert_eq!(ch(&p, 0, 0), ' ', "rows above cleared");
        assert_eq!(ch(&p, 1, 0), ' ');
        assert_eq!(ch(&p, 1, 1), ' ', "cursor cell cleared");
        assert_eq!(ch(&p, 1, 2), 'f', "after cursor untouched");
    }

    #[test]
    fn el_0_clears_cursor_to_eol() {
        let mut p = term(5, 2);
        feed(&mut p, b"abcde");
        feed(&mut p, b"\x1b[1;3H\x1b[K"); // cursor (0,2)
        assert_eq!(ch(&p, 0, 0), 'a');
        assert_eq!(ch(&p, 0, 1), 'b');
        assert_eq!(ch(&p, 0, 2), ' ');
        assert_eq!(ch(&p, 0, 4), ' ');
    }

    #[test]
    fn el_1_clears_start_to_cursor() {
        let mut p = term(5, 2);
        feed(&mut p, b"abcde");
        feed(&mut p, b"\x1b[1;3H\x1b[1K"); // cursor (0,2)
        assert_eq!(ch(&p, 0, 0), ' ');
        assert_eq!(ch(&p, 0, 2), ' ');
        assert_eq!(ch(&p, 0, 3), 'd', "after cursor untouched");
    }

    #[test]
    fn el_2_clears_whole_line() {
        let mut p = term(5, 2);
        feed(&mut p, b"abcde");
        feed(&mut p, b"\x1b[1;3H\x1b[2K");
        for c in 0..5 {
            assert_eq!(ch(&p, 0, c), ' ');
        }
    }

    #[test]
    fn ech_erases_n_chars_without_moving_cursor() {
        let mut p = term(5, 2);
        feed(&mut p, b"abcde");
        feed(&mut p, b"\x1b[1;2H\x1b[2X"); // cursor (0,1), erase 2
        assert_eq!(ch(&p, 0, 0), 'a');
        assert_eq!(ch(&p, 0, 1), ' ');
        assert_eq!(ch(&p, 0, 2), ' ');
        assert_eq!(ch(&p, 0, 3), 'd');
        assert_eq!(cur(&p), (0, 1), "ECH must not move the cursor");
    }

    // ── 5. Insert / delete lines & chars ─────────────────────────────────
    #[test]
    fn ich_inserts_blanks_shifting_right() {
        let mut p = term(5, 2);
        feed(&mut p, b"abcde");
        feed(&mut p, b"\x1b[1;2H\x1b[2@"); // insert 2 at col 1
        assert_eq!(ch(&p, 0, 0), 'a');
        assert_eq!(ch(&p, 0, 1), ' ');
        assert_eq!(ch(&p, 0, 2), ' ');
        assert_eq!(ch(&p, 0, 3), 'b');
        assert_eq!(ch(&p, 0, 4), 'c');
    }

    #[test]
    fn dch_deletes_chars_shifting_left() {
        let mut p = term(5, 2);
        feed(&mut p, b"abcde");
        feed(&mut p, b"\x1b[1;2H\x1b[2P"); // delete 2 at col 1
        assert_eq!(ch(&p, 0, 0), 'a');
        assert_eq!(ch(&p, 0, 1), 'd');
        assert_eq!(ch(&p, 0, 2), 'e');
        assert_eq!(ch(&p, 0, 3), ' ');
    }

    #[test]
    fn il_inserts_line_and_pushes_region_down() {
        let mut p = term(5, 4);
        // put a marker in col0 of each row
        feed(&mut p, b"\x1b[1;1Ha\x1b[2;1Hb\x1b[3;1Hc\x1b[4;1Hd");
        feed(&mut p, b"\x1b[2;1H\x1b[1L"); // IL at row1
        assert_eq!(ch(&p, 0, 0), 'a');
        assert_eq!(ch(&p, 1, 0), ' ', "inserted blank line");
        assert_eq!(ch(&p, 2, 0), 'b');
        assert_eq!(ch(&p, 3, 0), 'c', "d fell off the bottom");
    }

    #[test]
    fn dl_deletes_line_and_pulls_region_up() {
        let mut p = term(5, 4);
        feed(&mut p, b"\x1b[1;1Ha\x1b[2;1Hb\x1b[3;1Hc\x1b[4;1Hd");
        feed(&mut p, b"\x1b[2;1H\x1b[1M"); // DL at row1
        assert_eq!(ch(&p, 0, 0), 'a');
        assert_eq!(ch(&p, 1, 0), 'c');
        assert_eq!(ch(&p, 2, 0), 'd');
        assert_eq!(ch(&p, 3, 0), ' ', "blank pulled in at bottom");
    }

    // ── 6. Scroll region (DECSTBM) ───────────────────────────────────────
    #[test]
    fn newline_at_region_bottom_scrolls_only_within_region() {
        let mut p = term(5, 5);
        feed(&mut p, b"\x1b[2;4r"); // region rows 2..4 (1-based) => idx 1..3
                                    // DECSTBM homes the cursor; place a marker at col0 of every row.
        feed(
            &mut p,
            b"\x1b[1;1Ha\x1b[2;1Hb\x1b[3;1Hc\x1b[4;1Hd\x1b[5;1He",
        );
        feed(&mut p, b"\x1b[4;1H\n"); // cursor at region bottom, LF -> scroll region
        assert_eq!(ch(&p, 0, 0), 'a', "row above region untouched");
        assert_eq!(ch(&p, 4, 0), 'e', "row below region untouched");
        assert_eq!(ch(&p, 1, 0), 'c', "region scrolled up");
        assert_eq!(ch(&p, 2, 0), 'd');
        assert_eq!(ch(&p, 3, 0), ' ', "fresh blank at region bottom");
    }

    #[test]
    fn decstbm_homes_cursor() {
        let mut p = term(5, 5);
        feed(&mut p, b"\x1b[5;5H"); // move away
        feed(&mut p, b"\x1b[2;4r"); // set region
        assert_eq!(
            cur(&p),
            (0, 0),
            "DECSTBM homes cursor (non-origin => screen home)"
        );
    }

    #[test]
    fn decstbm_reset_restores_full_screen() {
        let mut p = term(5, 5);
        feed(&mut p, b"\x1b[2;4r\x1b[r"); // set then reset
        let b = p.active_buffer();
        assert_eq!(b.scroll_region.top, 0);
        assert_eq!(b.scroll_region.bottom, 4);
    }

    // ── 7. Line wrap / DECAWM ────────────────────────────────────────────
    #[test]
    fn autowrap_on_wraps_to_next_row() {
        let mut p = term(4, 3);
        feed(&mut p, b"abcde"); // 5 chars into 4 cols
        assert_eq!(ch(&p, 0, 0), 'a');
        assert_eq!(ch(&p, 0, 3), 'd');
        assert_eq!(ch(&p, 1, 0), 'e', "5th char wraps to next row");
        assert_eq!(cur(&p), (1, 1));
    }

    #[test]
    fn pending_wrap_latch_defers_until_next_print() {
        let mut p = term(4, 3);
        feed(&mut p, b"abcd"); // exactly fills row 0
        assert_eq!(
            cur(&p),
            (0, 3),
            "cursor latches at last col, not yet wrapped"
        );
        assert_eq!(ch(&p, 0, 3), 'd');
        feed(&mut p, b"e");
        assert_eq!(ch(&p, 1, 0), 'e');
    }

    #[test]
    fn autowrap_off_clamps_at_last_col() {
        let mut p = term(4, 3);
        feed(&mut p, b"\x1b[?7l"); // DECAWM off
        feed(&mut p, b"abcde");
        assert_eq!(ch(&p, 0, 3), 'e', "last char overwrites final cell");
        assert_eq!(cur(&p), (0, 3), "no wrap; cursor pinned to last col");
        assert_eq!(ch(&p, 1, 0), ' ', "next row stays blank");
    }

    // ── 8. Control chars: tab, CR, LF, BS ────────────────────────────────
    #[test]
    fn tab_advances_to_next_8_stop() {
        let mut p = term(20, 2);
        feed(&mut p, b"a\tb");
        assert_eq!(ch(&p, 0, 0), 'a');
        assert_eq!(ch(&p, 0, 8), 'b');
        assert_eq!(cur(&p), (0, 9));
    }

    #[test]
    fn carriage_return_moves_to_col0() {
        let mut p = term(5, 2);
        feed(&mut p, b"abc\rX");
        assert_eq!(ch(&p, 0, 0), 'X', "CR then print overwrites col 0");
        assert_eq!(ch(&p, 0, 1), 'b');
    }

    #[test]
    fn line_feed_moves_down_keeping_column() {
        let mut p = term(5, 3);
        feed(&mut p, b"a\n");
        assert_eq!(cur(&p), (1, 1), "LF keeps column (no implicit CR)");
    }

    #[test]
    fn backspace_moves_left_and_saturates() {
        let mut p = term(5, 2);
        feed(&mut p, b"ab\x08");
        assert_eq!(cur(&p), (0, 1));
        feed(&mut p, b"\x08\x08\x08"); // over-backspace
        assert_eq!(cur(&p), (0, 0), "backspace saturates at col 0");
    }

    // ── 9. Charset translation ───────────────────────────────────────────
    #[test]
    fn dec_special_graphics_maps_line_glyphs() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b(0q"); // G0 = DEC special, print 'q'
        assert_eq!(
            ch(&p, 0, 0),
            '─',
            "'q' -> horizontal line in DEC special graphics"
        );
        feed(&mut p, b"\x1b(Bx"); // back to ASCII
        assert_eq!(ch(&p, 0, 1), 'x', "ASCII passes through");
    }

    #[test]
    fn uk_charset_maps_hash_to_pound() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b(A#");
        assert_eq!(ch(&p, 0, 0), '£');
    }

    // ── 10. Save / restore cursor ────────────────────────────────────────
    #[test]
    fn decsc_decrc_round_trip() {
        let mut p = term(10, 5);
        feed(&mut p, b"\x1b[3;4H\x1b7"); // move (2,3), save
        feed(&mut p, b"\x1b[1;1H"); // move home
        feed(&mut p, b"\x1b8"); // restore
        assert_eq!(cur(&p), (2, 3));
    }

    // ── 11. DSR (device status report) response ──────────────────────────
    #[test]
    fn dsr_cursor_position_report() {
        let mut p = term(20, 10);
        feed(&mut p, b"\x1b[3;4H\x1b[6n");
        assert_eq!(p.pty.output_buffer, b"\x1b[3;4R", "CPR is 1-based row;col");
    }

    // ── 12. DEC mode visibility ──────────────────────────────────────────
    #[test]
    fn dectcem_toggles_cursor_visibility() {
        let mut p = term(10, 3);
        feed(&mut p, b"\x1b[?25l");
        assert!(!p.active_buffer().cursor.visible);
        feed(&mut p, b"\x1b[?25h");
        assert!(p.active_buffer().cursor.visible);
    }

    // ── 13. Resize ───────────────────────────────────────────────────────
    #[test]
    fn resize_keeps_cursor_in_bounds() {
        let mut p = term(10, 4);
        feed(&mut p, b"\x1b[4;10Hxyz"); // push cursor to bottom-right
        p.resize(6, 6);
        let (r, c) = cur(&p);
        assert!(r < 6 && c < 6, "cursor stays in bounds after grow");
        p.resize(20, 2);
        let (r, c) = cur(&p);
        assert!(r < 2 && c < 20, "cursor stays in bounds after shrink");
        // grid dimensions are consistent
        assert_eq!(p.active_buffer().num_rows, 2);
        for row in &p.active_buffer().rows {
            assert_eq!(row.cells.len(), 20);
        }
    }

    // ── 14. Hostile input fuzz (deterministic) ───────────────────────────
    #[test]
    fn fuzz_random_bytes_never_panics_and_keeps_invariants() {
        // Deterministic xorshift32 — no rand, no system entropy.
        let mut state: u32 = 0x1234_5678;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let mut p = term(24, 8);
        for _ in 0..40_000 {
            let byte = (next() & 0xFF) as u8;
            p.process_byte(byte);
            // Invariants that must hold after EVERY byte:
            let b = p.active_buffer();
            assert!(b.cursor.row < b.num_rows, "cursor row in bounds");
            assert!(b.cursor.col < b.cols, "cursor col in bounds");
        }
        // Grid shape must be preserved across arbitrary input.
        assert_eq!(p.active_buffer().num_rows, 8);
        for row in &p.active_buffer().rows {
            assert_eq!(row.cells.len(), 24, "row width invariant preserved");
        }
    }

    #[test]
    fn fuzz_structured_escape_sequences_never_panic() {
        // Bias the alphabet toward ESC/CSI-relevant bytes to hammer the state
        // machine harder than uniform random would.
        const ALPHABET: &[u8] = b"\x1b[];?0123456789ABCHJKLMPSTdfhlmnrABmM \n\r\t\x08\xe4\xb8\xad";
        let mut state: u32 = 0xDEAD_BEEF;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let mut p = term(16, 6);
        for _ in 0..40_000 {
            let byte = ALPHABET[(next() as usize) % ALPHABET.len()];
            p.process_byte(byte);
            let b = p.active_buffer();
            assert!(b.cursor.row < b.num_rows);
            assert!(b.cursor.col < b.cols);
        }
    }

    // ── BUG-documenting tests (assert CORRECT behavior; currently red) ────
    //
    // These are #[ignore]'d so the suite stays green. Each asserts the
    // xterm/ECMA-48-correct post-condition and will PASS once the emulator is
    // fixed. Run with `cargo test -p raeshell --lib -- --ignored` to reproduce.

    // BUG #1 (HIGH) — FIXED: integer-overflow panic on a long CSI numeric
    // parameter. `csi_param_byte`/`dcs_param_byte` used to accumulate
    // `v * 10 + digit` in u32 with no saturating guard, so a 10+ digit parameter
    // (e.g. `ESC[9999999999H`) overflowed u32 and panicked under overflow checks
    // — a remote-input DoS on a terminal parsing an arbitrary byte stream. Now
    // saturating_mul/saturating_add during accumulation; `finish_param` still
    // caps at u16::MAX. This test is the live regression guard.
    #[test]
    fn csi_huge_param_is_clamped_not_panicking() {
        let mut p = term(10, 4);
        feed(&mut p, b"\x1b[9999999999H"); // 10 digits -> saturates, no panic
                                           // Correct behavior: clamp and move cursor to the last row/col.
        assert_eq!(cur(&p), (3, 0));
    }

    // BUG #2 (MEDIUM) — FIXED: a double-width char at the right margin used to be
    // DROPPED. In `put_char`, when `col + width > cols` with autowrap on, it set
    // `wrap_pending = true` and `return`ed WITHOUT printing the glyph. Now the
    // wide-at-margin case folds into the wrap path (`wide_needs_wrap`) and falls
    // through to print the glyph on the next line, matching xterm. Live guard.
    #[test]
    fn wide_char_at_right_margin_wraps_not_dropped() {
        let mut p = term(4, 3);
        feed(&mut p, b"abc"); // cursor now at col 3 (last col)
        assert_eq!(cur(&p), (0, 3));
        feed(&mut p, "中".as_bytes()); // wide (width 2) — doesn't fit at col 3
                                       // Correct: it wraps to the next line and is printed there.
        assert_eq!(ch(&p, 1, 0), '中', "wide glyph must survive the wrap");
    }

    // BUG #3 (MEDIUM) — FIXED: ED 3 (ESC[3J) used to wipe the VISIBLE screen
    // instead of the scrollback. Per xterm, "P=3 => Erase Saved Lines" only
    // clears scrollback and must leave the on-screen grid intact. ED 3 is now
    // handled at the CSI `J` dispatch site (clears `scrollback` + resets
    // `scroll_offset`) and the `erase_display` grid path no longer touches mode
    // 3. Live regression guard.
    #[test]
    fn ed_3j_preserves_visible_screen() {
        let mut p = term(3, 2);
        feed(&mut p, b"abcdef"); // row0=abc, row1=def
        feed(&mut p, b"\x1b[3J"); // erase saved lines only
        assert_eq!(ch(&p, 0, 0), 'a', "ED 3 must NOT touch the visible screen");
        assert_eq!(ch(&p, 1, 2), 'f');
    }

    // BUG #4 (HIGH) — FIXED: SGR 4 (underline) used to greedily consume the
    // following parameter as an underline-style sub-parameter even when the
    // params were `;`-separated distinct attributes. `dispatch_sgr`'s `4` arm
    // read `params[i + 1]` and did `i += 1` unconditionally, and `csi_param_byte`
    // flattened `:` and `;` into one params vector — so `ESC[4;7m` was misread as
    // underline-style-7 and the `7` (reverse) was lost, corrupting common output
    // such as `ESC[4;34m` (underlined blue) and `ESC[4;7m` (underline + reverse).
    // Now the parser tracks a parallel `param_is_colon` flag and the `4` arm
    // consumes the next param as a sub-style ONLY when it was colon-joined
    // (ECMA-48 `4:3`); a `;`-separated param is left for the next iteration.
    // Live regression guard.
    #[test]
    fn sgr_underline_then_reverse_keeps_both() {
        let mut p = term(10, 2);
        feed(&mut p, b"\x1b[4;7mX");
        let a = attrs(&p, 0, 0);
        assert_eq!(
            a.underline,
            UnderlineStyle::Single,
            "4 is a standalone underline"
        );
        assert!(a.reverse, "the 7 after it must still set reverse");
    }
}
