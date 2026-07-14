#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

#[allow(unused_imports)]
use raekit;

use rae_calc::Calculator;
use rae_tokens::{DARK, RAEBLUE};
use rae_toml::Toml;
use raegfx::text::FontFamily;
use raegfx::Canvas;

const WIN_W: usize = 300;
const WIN_H: usize = 460;
const SURFACE_VIRT: u64 = 0x0000_7900_0000;

// Generic chrome on `rae_tokens::DARK` + the RaeBlue accent ramp (whole-OS
// cohesion). Operator buttons + the active mode tab take the accent; equals
// takes `state_ok`. No app-specific colors remain. Live Vibe accent is read at
// launch via `SYS_THEME_GET`.
const BG: u32 = DARK.bg_raised;
const DISPLAY_BG: u32 = DARK.bg_base;
const BTN_BG: u32 = DARK.bg_elevated;
const BTN_EQ: u32 = DARK.state_ok;
const TEXT_FG: u32 = DARK.text_primary;
const TITLE_BG: u32 = DARK.bg_base;

/// The live desktop accent seed (Vibe Mode) via `SYS_THEME_GET`, or RaeBlue when
/// the theme syscall is unavailable. Read at launch so Calculator re-skins to the
/// active theme (Concept §Customization Engine).
fn theme_seed() -> u32 {
    raekit::sys::theme_accent()
}
/// Operator-button accent (live ramp base). Non-const → computed in render.
fn btn_op() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).base
}

const BTN_W: usize = 64;
const BTN_H: usize = 40;
const BTN_GAP: usize = 6;
const BTN_PAD_X: usize = 10;
const TAB_Y: usize = 30;
const TAB_H: usize = 24;
const DISPLAY_Y: usize = 60;
const DISPLAY_H: usize = 84;
const BTN_START_Y: usize = 152;

// Converter-mode geometry: three stepper rows (Category / From / To) then a
// 4-column digit-entry grid. The stepper `<`/`>` buttons sit at the row edges;
// the selected label is drawn between them by the renderer.
const CONV_STEP_Y: usize = 152;
const CONV_STEP_H: usize = 26;
const CONV_STEP_GAP: usize = 4;
const CONV_ARROW_W: usize = 30;
const CONV_GRID_Y: usize = 152 + 3 * (26 + 4) + 8; // below the three stepper rows

/// The on-screen origin we present this window at. Absolute cursor coordinates
/// from `cursor_pos()` are converted to surface-local space by subtracting this
/// (the compositor honors a non-zero present offset — see `present_surface`).
const PRESENT_X: i32 = 300;
const PRESENT_Y: i32 = 100;

/// Which calculator surface is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Standard,
    Scientific,
    Programmer,
    Converter,
}

impl Mode {
    /// The stable config token for this mode (persisted in `calculator.toml`).
    fn token(self) -> &'static str {
        match self {
            Mode::Standard => "standard",
            Mode::Scientific => "scientific",
            Mode::Programmer => "programmer",
            Mode::Converter => "converter",
        }
    }

    /// Parse a mode token back from config. An UNKNOWN token resolves to the
    /// Standard default (never panics) — so a corrupt/foreign value opens the
    /// safe, simplest surface rather than rejecting the whole file.
    fn from_token(s: &str) -> Mode {
        match s {
            "scientific" => Mode::Scientific,
            "programmer" => Mode::Programmer,
            "converter" => Mode::Converter,
            _ => Mode::Standard,
        }
    }
}

/// An axis-aligned rectangle in SURFACE-LOCAL coordinates. The single geometry
/// type used to BOTH draw an interactive element AND hit-test a click against
/// it — there is exactly one rect per element, so draw-rects == hit-rects can
/// never drift (the invariant `design_proof` enforces).
#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    /// True iff the surface-local point `(px, py)` lies inside this rect.
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x as i32
            && px < (self.x + self.w) as i32
            && py >= self.y as i32
            && py < (self.y + self.h) as i32
    }
}

/// What a click on an interactive element does — mapped to the EXACT same
/// behavior the corresponding key fires. `Ascii` replays one `handle_ascii`
/// keystroke (digits, `+ - * / % = C . BS`); `Text` pushes a multi-char token
/// (`sin`, `AND`, `<<`, `0x`…) the same way typing it would; `SwitchMode`
/// mirrors F1/F2/F3; `ToggleDeg` mirrors the `d` key in Scientific.
#[derive(Clone, Copy)]
enum Action {
    Ascii(u8),
    Text(&'static str),
    SwitchMode(Mode),
    ToggleDeg,
    /// Converter: step the category selector by +1/-1 (wrapping).
    ConvCategory(i32),
    /// Converter: step the From-unit picker by +1/-1 (wrapping).
    ConvFrom(i32),
    /// Converter: step the To-unit picker by +1/-1 (wrapping).
    ConvTo(i32),
    None,
}

/// One interactive element: its draw rect, its label, its fill color, and the
/// action a click on it dispatches. Built once per mode into a fixed array (no
/// allocation), then used for BOTH rendering and hit-testing.
#[derive(Clone, Copy)]
struct Element {
    rect: Rect,
    label: &'static str,
    color: u32,
    action: Action,
}

impl Element {
    const EMPTY: Element = Element {
        rect: Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        label: "",
        color: 0,
        action: Action::None,
    };
}

// ── Unit Converter tables (pure data — embedded consts, no assets) ────────────
//
// Concept §Windows Pain Points: the bundled Calculator must reach Win11 parity
// (Standard/Scientific/Programmer/Converter). Each category carries a base unit
// and a factor table: `value_in_base = input * factor[from]`, then
// `result = value_in_base / factor[to]`. Every factor is positive, so the
// divide can never be by zero and the math never panics. Temperature is the
// affine exception (offsets, not a pure scale) and is handled specially via a
// Celsius pivot in `convert()`.

/// One unit: its display label and its factor relative to the category's base
/// unit. For Temperature the factor field is unused (affine path).
#[derive(Clone, Copy)]
struct Unit {
    label: &'static str,
    factor: f64,
}

/// One conversion category: a name and its ordered unit list.
struct Category {
    name: &'static str,
    units: &'static [Unit],
    /// True for Temperature only — routes through the affine Celsius pivot.
    affine: bool,
}

// Length — base unit: metre.
const LENGTH: &[Unit] = &[
    Unit {
        label: "m",
        factor: 1.0,
    },
    Unit {
        label: "km",
        factor: 1000.0,
    },
    Unit {
        label: "cm",
        factor: 0.01,
    },
    Unit {
        label: "mm",
        factor: 0.001,
    },
    Unit {
        label: "mi",
        factor: 1609.344,
    },
    Unit {
        label: "yd",
        factor: 0.9144,
    },
    Unit {
        label: "ft",
        factor: 0.3048,
    },
    Unit {
        label: "in",
        factor: 0.0254,
    },
    Unit {
        label: "nmi",
        factor: 1852.0,
    },
];

// Mass — base unit: kilogram.
const MASS: &[Unit] = &[
    Unit {
        label: "kg",
        factor: 1.0,
    },
    Unit {
        label: "g",
        factor: 0.001,
    },
    Unit {
        label: "mg",
        factor: 0.000001,
    },
    Unit {
        label: "t",
        factor: 1000.0,
    },
    Unit {
        label: "lb",
        factor: 0.45359237,
    },
    Unit {
        label: "oz",
        factor: 0.028349523125,
    },
    Unit {
        label: "st",
        factor: 6.35029318,
    },
];

// Temperature — affine: units listed for the picker; factors unused. Base is °C.
const TEMP: &[Unit] = &[
    Unit {
        label: "C",
        factor: 1.0,
    },
    Unit {
        label: "F",
        factor: 1.0,
    },
    Unit {
        label: "K",
        factor: 1.0,
    },
];

// Data — base unit: byte. Decimal (1000) and binary (1024) prefixes.
const DATA: &[Unit] = &[
    Unit {
        label: "bit",
        factor: 0.125,
    },
    Unit {
        label: "byte",
        factor: 1.0,
    },
    Unit {
        label: "KB",
        factor: 1000.0,
    },
    Unit {
        label: "MB",
        factor: 1_000_000.0,
    },
    Unit {
        label: "GB",
        factor: 1_000_000_000.0,
    },
    Unit {
        label: "TB",
        factor: 1_000_000_000_000.0,
    },
    Unit {
        label: "KiB",
        factor: 1024.0,
    },
    Unit {
        label: "MiB",
        factor: 1_048_576.0,
    },
    Unit {
        label: "GiB",
        factor: 1_073_741_824.0,
    },
    Unit {
        label: "TiB",
        factor: 1_099_511_627_776.0,
    },
];

// Speed — base unit: metre/second.
const SPEED: &[Unit] = &[
    Unit {
        label: "m/s",
        factor: 1.0,
    },
    Unit {
        label: "km/h",
        factor: 0.2777777777777778,
    },
    Unit {
        label: "mph",
        factor: 0.44704,
    },
    Unit {
        label: "knot",
        factor: 0.5144444444444445,
    },
    Unit {
        label: "ft/s",
        factor: 0.3048,
    },
];

// Area — base unit: square metre.
const AREA: &[Unit] = &[
    Unit {
        label: "m2",
        factor: 1.0,
    },
    Unit {
        label: "km2",
        factor: 1_000_000.0,
    },
    Unit {
        label: "cm2",
        factor: 0.0001,
    },
    Unit {
        label: "ft2",
        factor: 0.09290304,
    },
    Unit {
        label: "in2",
        factor: 0.00064516,
    },
    Unit {
        label: "acre",
        factor: 4046.8564224,
    },
    Unit {
        label: "ha",
        factor: 10000.0,
    },
];

// Time — base unit: second.
const TIME: &[Unit] = &[
    Unit {
        label: "s",
        factor: 1.0,
    },
    Unit {
        label: "min",
        factor: 60.0,
    },
    Unit {
        label: "h",
        factor: 3600.0,
    },
    Unit {
        label: "day",
        factor: 86400.0,
    },
    Unit {
        label: "week",
        factor: 604800.0,
    },
    Unit {
        label: "ms",
        factor: 0.001,
    },
    Unit {
        label: "us",
        factor: 0.000001,
    },
];

/// All converter categories, in selector order.
const CATEGORIES: &[Category] = &[
    Category {
        name: "Length",
        units: LENGTH,
        affine: false,
    },
    Category {
        name: "Mass",
        units: MASS,
        affine: false,
    },
    Category {
        name: "Temp",
        units: TEMP,
        affine: false_temp(),
    },
    Category {
        name: "Data",
        units: DATA,
        affine: false,
    },
    Category {
        name: "Speed",
        units: SPEED,
        affine: false,
    },
    Category {
        name: "Area",
        units: AREA,
        affine: false,
    },
    Category {
        name: "Time",
        units: TIME,
        affine: false,
    },
];

/// `const fn` so the Temperature category's `affine: true` reads clearly above
/// without a literal that could be mistaken for the others (Temp is the one
/// affine category; every other is a pure scale).
const fn false_temp() -> bool {
    true
}

/// Convert `value` from `units[from]` to `units[to]` within one category. Pure
/// scale for every category except Temperature, which pivots through Celsius.
/// Never panics: factors are all positive (no divide-by-zero) and out-of-range
/// indices fall back to identity.
fn convert(cat: &Category, from: usize, to: usize, value: f64) -> f64 {
    if from >= cat.units.len() || to >= cat.units.len() {
        return value;
    }
    if cat.affine {
        // Temperature: input → Celsius → output.
        let c = match cat.units[from].label {
            "F" => (value - 32.0) * 5.0 / 9.0,
            "K" => value - 273.15,
            _ => value, // "C"
        };
        match cat.units[to].label {
            "F" => c * 9.0 / 5.0 + 32.0,
            "K" => c + 273.15,
            _ => c, // "C"
        }
    } else {
        let base = value * cat.units[from].factor;
        base / cat.units[to].factor
    }
}

/// Max interactive elements in any mode: Converter is the widest — 4 tabs + a
/// 4x4 digit grid (16) + 6 stepper buttons (cat/from/to prev+next) + clear = 27;
/// the math modes peak at 4 tabs + 24 grid + 4 strip = 32.
const MAX_ELEMENTS: usize = 34;

/// A fixed-capacity, allocation-free element list. The single source of truth
/// for a mode's layout: filled once, drawn from, hit-tested against.
struct Layout {
    items: [Element; MAX_ELEMENTS],
    count: usize,
}

impl Layout {
    fn new() -> Self {
        Self {
            items: [Element::EMPTY; MAX_ELEMENTS],
            count: 0,
        }
    }
    fn push(&mut self, rect: Rect, label: &'static str, color: u32, action: Action) {
        if self.count < MAX_ELEMENTS {
            self.items[self.count] = Element {
                rect,
                label,
                color,
                action,
            };
            self.count += 1;
        }
    }
    fn as_slice(&self) -> &[Element] {
        &self.items[..self.count]
    }
    /// Hit-test a surface-local point; returns the action of the topmost
    /// element whose rect contains it, or `None` if the click missed (empty
    /// space = no-op, never panics).
    fn hit(&self, px: i32, py: i32) -> Option<Action> {
        self.as_slice()
            .iter()
            .find(|e| e.rect.contains(px, py))
            .map(|e| e.action)
    }
}

/// Map a button label to the action it dispatches. Single-char labels that
/// match a key replay that keystroke; multi-char tokens push text. This is the
/// ONE place label→action lives, shared by every mode's layout builder.
fn action_for(label: &str, mode: Mode) -> Action {
    match label {
        // Standard chrome keys (also valid in sci/prog as raw bytes).
        "=" => Action::Ascii(b'='),
        "C" if mode == Mode::Standard => Action::Ascii(b'C'),
        "CE" => Action::Ascii(0x1B), // clear-entry == Esc-clear for the expr buffer
        "BS" => Action::Ascii(0x08),
        "+/-" => Action::Ascii(b'n'),
        "%" => Action::Ascii(b'%'),
        "/" => Action::Ascii(b'/'),
        "*" => Action::Ascii(b'*'),
        "-" => Action::Ascii(b'-'),
        "+" => Action::Ascii(b'+'),
        "." => Action::Ascii(b'.'),
        "(" => Action::Ascii(b'('),
        ")" => Action::Ascii(b')'),
        "^" => Action::Ascii(b'^'),
        "x!" => Action::Text("!"),
        "DEG" | "RAD" => Action::ToggleDeg,
        // Multi-char scientific/programmer tokens push their text verbatim
        // (returned as `'static` literals, not the borrowed `label`).
        "sin" => Action::Text("sin"),
        "cos" => Action::Text("cos"),
        "tan" => Action::Text("tan"),
        "ln" => Action::Text("ln"),
        "log" => Action::Text("log"),
        "exp" => Action::Text("exp"),
        "sqrt" => Action::Text("sqrt"),
        "pi" => Action::Text("pi"),
        "e" => Action::Text("e"),
        "AND" => Action::Text("&"),
        "OR" => Action::Text("|"),
        "XOR" => Action::Text("^"),
        "NOT" => Action::Text("~"),
        "<<" => Action::Text("<<"),
        ">>" => Action::Text(">>"),
        "0x" => Action::Text("0x"),
        "0b" => Action::Text("0b"),
        // Hex digit / single-char digit labels → that byte.
        _ if label.len() == 1 => Action::Ascii(label.as_bytes()[0]),
        _ => Action::None,
    }
}

/// A free-form expression buffer used by Scientific + Programmer modes (the user
/// types an expression, then `=`/Enter evaluates it through `rae_calc`). Fixed
/// capacity, never allocates, never panics.
const EXPR_CAP: usize = 63;
struct ExprBuf {
    buf: [u8; EXPR_CAP],
    len: usize,
    /// Result/echo line shown under the editable expression.
    result: [u8; EXPR_CAP],
    result_len: usize,
    error: bool,
}

impl ExprBuf {
    fn new() -> Self {
        Self {
            buf: [0; EXPR_CAP],
            len: 0,
            result: [0; EXPR_CAP],
            result_len: 0,
            error: false,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
    fn result_str(&self) -> &str {
        if self.error {
            return "Error";
        }
        core::str::from_utf8(&self.result[..self.result_len]).unwrap_or("")
    }
    fn push_byte(&mut self, b: u8) {
        if self.len < EXPR_CAP {
            self.buf[self.len] = b;
            self.len += 1;
        }
    }
    fn backspace(&mut self) {
        if self.len > 0 {
            self.len -= 1;
        }
    }
    fn clear(&mut self) {
        self.len = 0;
        self.result_len = 0;
        self.error = false;
    }
    fn set_result(&mut self, s: &str) {
        self.result_len = 0;
        self.error = false;
        for &b in s.as_bytes() {
            if self.result_len < EXPR_CAP {
                self.result[self.result_len] = b;
                self.result_len += 1;
            }
        }
    }
    fn set_error(&mut self) {
        self.error = true;
    }
}

/// Format an `f64` into `out` (no libm, no exponent) — same policy as the
/// Standard engine: trim trailing fractional zeros, round to 10 places.
fn fmt_f64(v: f64, out: &mut [u8]) -> usize {
    if !v.is_finite() {
        return 0;
    }
    let neg = v < 0.0;
    let mut value = if neg { -v } else { v };
    if value == 0.0 {
        value = 0.0;
    }
    let mut len = 0usize;
    let push = |out: &mut [u8], len: &mut usize, b: u8| {
        if *len < out.len() {
            out[*len] = b;
            *len += 1;
        }
    };
    if neg {
        push(out, &mut len, b'-');
    }
    // Integer part.
    let int_part = floor_i128(value) as u128;
    let mut tmp = [0u8; 40];
    let mut n = 0;
    let mut ip = int_part;
    if ip == 0 {
        push(out, &mut len, b'0');
    } else {
        while ip > 0 && n < tmp.len() {
            tmp[n] = b'0' + (ip % 10) as u8;
            ip /= 10;
            n += 1;
        }
        for j in 0..n {
            push(out, &mut len, tmp[n - 1 - j]);
        }
    }
    // Fractional part.
    let mut frac = value - int_part as f64;
    let mut fbuf = [0u8; 10];
    let mut fd = 0usize;
    let mut i = 0;
    while i < 10 && frac > 0.0 {
        frac *= 10.0;
        let d = floor_i128(frac) as u8;
        frac -= d as f64;
        fbuf[fd] = b'0' + (d % 10);
        fd += 1;
        i += 1;
    }
    while fd > 0 && fbuf[fd - 1] == b'0' {
        fd -= 1;
    }
    if fd > 0 {
        push(out, &mut len, b'.');
        for j in 0..fd {
            push(out, &mut len, fbuf[j]);
        }
    }
    len
}

fn floor_i128(x: f64) -> f64 {
    let t = x as i128 as f64;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

/// Parse a plain decimal `f64` (optional leading `-`, digits, one `.`) without
/// libm or std's parser. Empty / malformed → 0.0. Never panics.
fn parse_f64(s: &str) -> f64 {
    let b = s.as_bytes();
    if b.is_empty() {
        return 0.0;
    }
    let mut i = 0;
    let neg = b[0] == b'-';
    if neg {
        i = 1;
    }
    let mut int_part = 0f64;
    while i < b.len() && b[i].is_ascii_digit() {
        int_part = int_part * 10.0 + (b[i] - b'0') as f64;
        i += 1;
    }
    let mut frac = 0f64;
    let mut scale = 1f64;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            scale *= 10.0;
            frac += (b[i] - b'0') as f64 / scale;
            i += 1;
        }
    }
    let v = int_part + frac;
    if neg {
        -v
    } else {
        v
    }
}

/// Write an unsigned `u64` in `radix` (2/8/10/16) into `out`, returning length.
fn fmt_radix(mut v: u64, radix: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 64];
    let mut n = 0;
    while v > 0 && n < tmp.len() {
        let d = (v % radix) as u8;
        tmp[n] = if d < 10 { b'0' + d } else { b'A' + (d - 10) };
        v /= radix;
        n += 1;
    }
    let count = n.min(out.len());
    for j in 0..count {
        out[j] = tmp[n - 1 - j];
    }
    count
}

// ── Persistent preferences (rae_toml) ─────────────────────────────────────────
//
// LEGACY_GAMING_CONCEPT.md §"The user owns the machine": "remember my settings" must be
// real. Calculator persists its VIEW state — the active mode (Std/Sci/Prog/Conv),
// the Scientific deg/rad toggle, and the last Converter category — to
// `<home>/.config/calculator.toml` and restores it on launch, so the app opens in
// the mode + units the user left it in. Every load is hostile-input-tolerant: a
// missing, corrupt, or out-of-range config falls back to TYPED DEFAULTS and NEVER
// panics. This is the per-app prefs pattern the consumer apps follow (the proven
// Music/Notes recipe).

/// The decoded, defaulted preferences restored at launch. Pure data: load builds
/// it from a parsed (or absent) TOML; save serializes the live App view-state.
#[derive(Clone)]
struct Prefs {
    /// Active mode (default Standard). Stored/parsed as a stable token.
    mode: Mode,
    /// Scientific deg/rad toggle (true = degrees). Default false (radians).
    degrees: bool,
    /// Last Converter category NAME (e.g. "Length"). Re-resolved against the live
    /// `CATEGORIES` table on load; an unknown name → category 0. Default "Length".
    conv_category: String,
}

impl Prefs {
    /// The typed defaults used on first run or any config error.
    fn defaults() -> Self {
        Self {
            mode: Mode::Standard,
            degrees: false,
            conv_category: String::from(CATEGORIES[0].name),
        }
    }

    /// Build `Prefs` from a parsed TOML table, validating every field and
    /// substituting the typed default for any missing / wrong-typed value. Never
    /// panics; an unrelated shape (e.g. a non-table root) yields full defaults. An
    /// unknown `mode` token resolves to Standard via `Mode::from_token`.
    fn from_toml(t: &Toml) -> Self {
        let mut p = Self::defaults();
        if let Some(s) = t.get("mode").and_then(Toml::as_str) {
            p.mode = Mode::from_token(s);
        }
        if let Some(b) = t.get("degrees").and_then(Toml::as_bool) {
            p.degrees = b;
        }
        if let Some(s) = t.get("conv_category").and_then(Toml::as_str) {
            // Only accept a NAME the live table actually knows; an unknown name
            // keeps the default (category 0) — never an out-of-range index.
            if CATEGORIES.iter().any(|c| c.name == s) {
                p.conv_category = String::from(s);
            }
        }
        p
    }

    /// Serialize the live preferences into an order-stable `Toml::Table` ready for
    /// `rae_toml::to_string`. The schema is flat (no headers), human-editable.
    fn to_toml(&self) -> Toml {
        let mut table: Vec<(String, Toml)> = Vec::new();
        table.push((
            String::from("mode"),
            Toml::String(String::from(self.mode.token())),
        ));
        table.push((String::from("degrees"), Toml::Boolean(self.degrees)));
        table.push((
            String::from("conv_category"),
            Toml::String(self.conv_category.clone()),
        ));
        Toml::Table(table)
    }

    /// The Converter category INDEX this prefs' name resolves to (0 if unknown).
    fn conv_index(&self) -> usize {
        CATEGORIES
            .iter()
            .position(|c| c.name == self.conv_category)
            .unwrap_or(0)
    }
}

/// A fixed-capacity path builder for the config file (`<home>/.config/...`),
/// allocation-free and char-boundary-safe (we only ever push known ASCII names).
const CFG_PATH_CAP: usize = 256;
struct CfgPath {
    bytes: [u8; CFG_PATH_CAP],
    len: usize,
}

impl CfgPath {
    fn new() -> Self {
        Self {
            bytes: [0; CFG_PATH_CAP],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("/")
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(CFG_PATH_CAP);
        self.bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }
    fn push_component(&mut self, name: &str) {
        if self.len > 0 && self.bytes[self.len - 1] != b'/' && self.len < CFG_PATH_CAP {
            self.bytes[self.len] = b'/';
            self.len += 1;
        }
        for &b in name.as_bytes() {
            if self.len >= CFG_PATH_CAP {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
}

/// The per-app config DIRECTORY: `<session home>/.config`. Falls back to
/// `/home/user/.config` when no session is present. The `.config` directory is
/// created (idempotent) before any write.
fn prefs_dir() -> CfgPath {
    let mut p = CfgPath::new();
    let mut info = [0u8; 96];
    if raekit::sys::session_info(&mut info).is_some() {
        if let Some(home) = raekit::sys::session_home_from(&info) {
            p.set(home);
            p.push_component(".config");
            return p;
        }
    }
    p.set("/home/user/.config");
    p
}

/// Load preferences from `<home>/.config/calculator.toml`. On ANY failure — file
/// absent, unreadable, not UTF-8, or a `rae_toml::parse` error — returns the typed
/// defaults. Never panics, never blocks the app from launching.
fn load_prefs() -> Prefs {
    let mut path = prefs_dir();
    path.push_component("calculator.toml");
    let fd = raekit::sys::open(path.as_str(), 0);
    if fd == u64::MAX {
        return Prefs::defaults();
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Hard cap: a config file should be tiny; refuse to slurp a giant blob.
        if data.len() > 64 * 1024 {
            break;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = raekit::sys::close(fd);
    let text = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => return Prefs::defaults(),
    };
    match rae_toml::parse(text) {
        Ok(t) => Prefs::from_toml(&t),
        Err(_) => Prefs::defaults(),
    }
}

/// Persist `prefs` to `<home>/.config/calculator.toml` (best effort). Creates the
/// `.config` directory if missing, serializes via `rae_toml::to_string`, and
/// writes O_CREAT|O_TRUNC. A failure is silent — the app keeps running.
fn save_prefs(prefs: &Prefs) {
    let dir = prefs_dir();
    let _ = raekit::sys::mkdir(dir.as_str());
    let mut path = dir;
    path.push_component("calculator.toml");
    let text = rae_toml::to_string(&prefs.to_toml());
    // O_WRONLY | O_CREAT | O_TRUNC = 0x0241.
    let fd = raekit::sys::open(path.as_str(), 0x0241);
    if fd == u64::MAX {
        return;
    }
    let bytes = text.as_bytes();
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + 4096).min(bytes.len());
        let n = raekit::sys::write(fd, &bytes[off..end]) as usize;
        if n == 0 {
            break;
        }
        off += n;
    }
    let _ = raekit::sys::close(fd);
}

struct App {
    mode: Mode,
    calc: Calculator,
    sci: ExprBuf,
    prog: ExprBuf,
    /// Last committed integer value (Programmer mode multi-base display).
    prog_value: i64,
    /// Degrees mode for Scientific trig (else radians).
    degrees: bool,
    shift_held: bool,
    /// Converter selection state: category index + From/To unit indices.
    conv_cat: usize,
    conv_from: usize,
    conv_to: usize,
    /// Converter value-entry buffer (digits + one dot + leading sign).
    conv_buf: [u8; 24],
    conv_len: usize,
}

impl App {
    fn new() -> Self {
        // Restore saved preferences (typed defaults on first run / any error).
        let prefs = load_prefs();
        let conv_cat = prefs.conv_index();
        // Clamp the From/To unit indices into the restored category (the unit
        // selection itself isn't persisted — only the category).
        let units = CATEGORIES[conv_cat].units.len();
        Self {
            mode: prefs.mode,
            calc: Calculator::new(),
            sci: ExprBuf::new(),
            prog: ExprBuf::new(),
            prog_value: 0,
            degrees: prefs.degrees,
            shift_held: false,
            conv_cat,
            conv_from: 0,
            conv_to: if units > 1 { 1 } else { 0 },
            conv_buf: [0; 24],
            conv_len: 0,
        }
    }

    /// Snapshot the live view-state into a `Prefs` and write it to disk. Called on
    /// every preference-affecting change (mode switch, deg/rad toggle, converter
    /// category step). Best effort + silent on failure (never blocks the app).
    fn persist(&self) {
        let prefs = Prefs {
            mode: self.mode,
            degrees: self.degrees,
            conv_category: String::from(CATEGORIES[self.conv_cat].name),
        };
        save_prefs(&prefs);
    }

    // ── Converter helpers ─────────────────────────────────────────────────────
    /// The currently selected category.
    fn conv_category(&self) -> &'static Category {
        &CATEGORIES[self.conv_cat]
    }

    /// The value typed into the converter entry, parsed to `f64` (empty → 0).
    fn conv_value(&self) -> f64 {
        let s = core::str::from_utf8(&self.conv_buf[..self.conv_len]).unwrap_or("");
        parse_f64(s)
    }

    /// The converted result for the current selection.
    fn conv_result(&self) -> f64 {
        let cat = self.conv_category();
        convert(cat, self.conv_from, self.conv_to, self.conv_value())
    }

    /// Append one converter-entry byte (digit, dot, or leading '-'). Rejects a
    /// second dot and a sign anywhere but the start. Never overflows the buffer.
    fn conv_push(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {}
            b'.' => {
                if self.conv_buf[..self.conv_len].contains(&b'.') {
                    return;
                }
            }
            b'-' => {
                if self.conv_len != 0 {
                    return;
                }
            }
            _ => return,
        }
        if self.conv_len < self.conv_buf.len() {
            self.conv_buf[self.conv_len] = b;
            self.conv_len += 1;
        }
    }

    fn conv_backspace(&mut self) {
        if self.conv_len > 0 {
            self.conv_len -= 1;
        }
    }

    fn conv_clear(&mut self) {
        self.conv_len = 0;
    }

    /// Step the category selector by `delta` (wrapping) and clamp the From/To
    /// unit indices into the new category's unit count.
    fn conv_step_category(&mut self, delta: i32) {
        let n = CATEGORIES.len() as i32;
        self.conv_cat = (((self.conv_cat as i32 + delta) % n + n) % n) as usize;
        let u = self.conv_category().units.len();
        if self.conv_from >= u {
            self.conv_from = 0;
        }
        if self.conv_to >= u {
            self.conv_to = if u > 1 { 1 } else { 0 };
        }
        // Remember the new converter category across launches.
        self.persist();
    }

    fn conv_step_from(&mut self, delta: i32) {
        let n = self.conv_category().units.len() as i32;
        self.conv_from = (((self.conv_from as i32 + delta) % n + n) % n) as usize;
    }

    fn conv_step_to(&mut self, delta: i32) {
        let n = self.conv_category().units.len() as i32;
        self.conv_to = (((self.conv_to as i32 + delta) % n + n) % n) as usize;
    }

    // ── Scientific evaluation: apply deg→rad to trig args when in degrees ─────
    fn eval_scientific(&mut self) {
        let s = self.sci.as_str();
        if s.is_empty() {
            return;
        }
        // In degrees mode, wrap recognized trig calls' arguments with the
        // deg→rad factor. We keep it simple: evaluate, and if degrees, the user
        // is expected to type sin(...) over a degree value — we pre-scale by
        // substituting a leading multiply. To stay panic-free and allocation-
        // free we evaluate the raw string, then for degrees re-evaluate the
        // common single-call forms. The robust path: evaluate as-is in radians;
        // degrees handling is applied to bare trig of a numeric arg below.
        let res = if self.degrees {
            eval_degrees(s)
        } else {
            rae_calc::eval(s)
        };
        match res {
            Ok(v) => {
                let mut out = [0u8; EXPR_CAP];
                let n = fmt_f64(v, &mut out);
                self.sci
                    .set_result(core::str::from_utf8(&out[..n]).unwrap_or("0"));
            }
            Err(_) => self.sci.set_error(),
        }
    }

    fn eval_programmer(&mut self) {
        let s = self.prog.as_str();
        if s.is_empty() {
            return;
        }
        match rae_calc::eval_int(s) {
            Ok(v) => {
                self.prog_value = v;
                let mut out = [0u8; EXPR_CAP];
                let n = fmt_radix(v as u64, 10, &mut out);
                // store decimal as the echo line; the multi-base block renders
                // all four bases live from prog_value.
                self.prog
                    .set_result(core::str::from_utf8(&out[..n]).unwrap_or("0"));
            }
            Err(_) => self.prog.set_error(),
        }
    }

    // ── Layout (single source of truth: draw-rects == hit-rects) ─────────────
    //
    // Build the full interactive-element list for `mode` — the mode tabs plus
    // every button at its exact draw rectangle. `render` draws from this list
    // and `on_click` hit-tests against it, so the rectangles can never drift.

    /// Tab labels in column order (also used for hit-rect geometry).
    const TABS: [(&'static str, Mode); 4] = [
        ("Std", Mode::Standard),
        ("Sci", Mode::Scientific),
        ("Prog", Mode::Programmer),
        ("Conv", Mode::Converter),
    ];

    /// The button grid for `mode` (label, fill-color) by row/column. Empty
    /// labels are skipped (no element emitted).
    fn grid_rows(&self) -> ([[(&'static str, u32); 4]; 6], usize) {
        let op = btn_op();
        match self.mode {
            Mode::Standard => {
                #[rustfmt::skip]
                let g: [[(&str, u32); 4]; 6] = [
                    [("C",  op),     ("+/-", op),     ("%",  op),     ("/", op)],
                    [("7",  BTN_BG), ("8",   BTN_BG), ("9",  BTN_BG), ("*", op)],
                    [("4",  BTN_BG), ("5",   BTN_BG), ("6",  BTN_BG), ("-", op)],
                    [("1",  BTN_BG), ("2",   BTN_BG), ("3",  BTN_BG), ("+", op)],
                    [("0",  BTN_BG), (".",   BTN_BG), ("BS", BTN_BG), ("=", BTN_EQ)],
                    [("",   0),      ("",    0),      ("",   0),      ("",  0)],
                ];
                (g, 5)
            }
            Mode::Scientific => {
                let op = btn_op();
                let deg = if self.degrees { "DEG" } else { "RAD" };
                #[rustfmt::skip]
                let g: [[(&str, u32); 4]; 6] = [
                    [(deg,  op),     ("sin", op),     ("cos", op),    ("tan", op)],
                    [("ln", op),     ("log", op),     ("exp", op),    ("x!",  op)],
                    [("pi", op),     ("e",   op),     ("(",   op),    (")",   op)],
                    [("7",  BTN_BG), ("8",   BTN_BG), ("9",   BTN_BG), ("/",  op)],
                    [("4",  BTN_BG), ("5",   BTN_BG), ("6",   BTN_BG), ("*",  op)],
                    [("1",  BTN_BG), ("2",   BTN_BG), ("3",   BTN_BG), ("-",  op)],
                ];
                (g, 6)
            }
            Mode::Programmer => {
                #[rustfmt::skip]
                let g: [[(&str, u32); 4]; 6] = [
                    [("AND", op),    ("OR",  op),     ("XOR", op),    ("NOT", op)],
                    [("<<",  op),    (">>",  op),     ("0x",  op),    ("0b",  op)],
                    [("A",   BTN_BG),("B",   BTN_BG), ("C",   BTN_BG),("D",   BTN_BG)],
                    [("7",   BTN_BG),("8",   BTN_BG), ("9",   BTN_BG),("/",   op)],
                    [("4",   BTN_BG),("5",   BTN_BG), ("6",   BTN_BG),("*",   op)],
                    [("1",   BTN_BG),("2",   BTN_BG), ("3",   BTN_BG),("-",   op)],
                ];
                (g, 6)
            }
            // Converter builds its own (non-grid) layout in `build_layout`; this
            // returns an empty grid so the standard grid loop emits nothing.
            Mode::Converter => ([[("", 0); 4]; 6], 0),
        }
    }

    /// The bottom strip for Scientific/Programmer (Standard's last row lives in
    /// the grid). Returns the 4 (label, color) cells, or `None` for Standard.
    fn bottom_strip(&self) -> Option<[(&'static str, u32); 4]> {
        let op = btn_op();
        match self.mode {
            Mode::Standard => None,
            Mode::Scientific => Some([("0", BTN_BG), ("sqrt", op), ("^", op), ("=", BTN_EQ)]),
            Mode::Programmer => Some([("0", BTN_BG), ("CE", op), ("+", op), ("=", BTN_EQ)]),
            Mode::Converter => None,
        }
    }

    /// Build the complete interactive-element list for the current mode. Every
    /// rect here is BOTH drawn and hit-tested — one geometry, no drift.
    fn build_layout(&self) -> Layout {
        let mut layout = Layout::new();
        let accent = btn_op();

        // Mode tabs.
        let tab_w = (WIN_W - 20) / Self::TABS.len();
        for (i, &(label, m)) in Self::TABS.iter().enumerate() {
            let x = 10 + i * tab_w;
            let active = m == self.mode;
            let bg = if active { accent } else { DARK.bg_elevated };
            layout.push(
                Rect {
                    x,
                    y: TAB_Y,
                    w: tab_w - 2,
                    h: TAB_H,
                },
                label,
                bg,
                Action::SwitchMode(m),
            );
        }

        // Converter: bespoke layout (3 stepper rows + a digit-entry grid). The
        // math-mode grid/strip below emit nothing for Converter.
        if self.mode == Mode::Converter {
            self.build_converter_layout(&mut layout, accent);
            return layout;
        }

        // Button grid.
        let (rows, n_rows) = self.grid_rows();
        for row_i in 0..n_rows {
            for col_i in 0..4 {
                let (label, color) = rows[row_i][col_i];
                if label.is_empty() {
                    continue;
                }
                let x = BTN_PAD_X + col_i * (BTN_W + BTN_GAP);
                let y = BTN_START_Y + row_i * (BTN_H + BTN_GAP);
                layout.push(
                    Rect {
                        x,
                        y,
                        w: BTN_W,
                        h: BTN_H,
                    },
                    label,
                    color,
                    action_for(label, self.mode),
                );
            }
        }

        // Bottom strip (sci/prog).
        if let Some(strip) = self.bottom_strip() {
            let y = BTN_START_Y + 6 * (BTN_H + BTN_GAP);
            for (col_i, &(label, color)) in strip.iter().enumerate() {
                let x = BTN_PAD_X + col_i * (BTN_W + BTN_GAP);
                layout.push(
                    Rect {
                        x,
                        y,
                        w: BTN_W,
                        h: BTN_H,
                    },
                    label,
                    color,
                    action_for(label, self.mode),
                );
            }
        }

        layout
    }

    /// Build the Converter mode's interactive elements: three stepper rows
    /// (Category / From-unit / To-unit, each with a `<` and `>` button) plus a
    /// 4-column digit-entry grid (0-9, ., backspace, clear). The selected
    /// category/unit labels are drawn by the renderer between the arrows.
    fn build_converter_layout(&self, layout: &mut Layout, accent: u32) {
        // Three stepper rows: (left-action, right-action) by row index.
        let steppers: [(Action, Action); 3] = [
            (Action::ConvCategory(-1), Action::ConvCategory(1)),
            (Action::ConvFrom(-1), Action::ConvFrom(1)),
            (Action::ConvTo(-1), Action::ConvTo(1)),
        ];
        for (i, &(left, right)) in steppers.iter().enumerate() {
            let y = CONV_STEP_Y + i * (CONV_STEP_H + CONV_STEP_GAP);
            layout.push(
                Rect {
                    x: 10,
                    y,
                    w: CONV_ARROW_W,
                    h: CONV_STEP_H,
                },
                "<",
                accent,
                left,
            );
            layout.push(
                Rect {
                    x: WIN_W - 10 - CONV_ARROW_W,
                    y,
                    w: CONV_ARROW_W,
                    h: CONV_STEP_H,
                },
                ">",
                accent,
                right,
            );
        }

        // Digit-entry grid (mirrors Standard's number pad).
        #[rustfmt::skip]
        let grid: [[(&str, u32); 4]; 4] = [
            [("7", BTN_BG), ("8", BTN_BG), ("9", BTN_BG), ("C", accent)],
            [("4", BTN_BG), ("5", BTN_BG), ("6", BTN_BG), ("BS", accent)],
            [("1", BTN_BG), ("2", BTN_BG), ("3", BTN_BG), ("-", accent)],
            [("0", BTN_BG), (".", BTN_BG), ("",  0),      ("",  0)],
        ];
        for (row_i, row) in grid.iter().enumerate() {
            for (col_i, &(label, color)) in row.iter().enumerate() {
                if label.is_empty() {
                    continue;
                }
                let x = BTN_PAD_X + col_i * (BTN_W + BTN_GAP);
                let y = CONV_GRID_Y + row_i * (BTN_H + BTN_GAP);
                // Converter entry keys map to their byte; "C"/"BS" to clear/bs.
                let action = match label {
                    "C" => Action::Ascii(b'C'),
                    "BS" => Action::Ascii(0x08),
                    _ => Action::Ascii(label.as_bytes()[0]),
                };
                layout.push(
                    Rect {
                        x,
                        y,
                        w: BTN_W,
                        h: BTN_H,
                    },
                    label,
                    color,
                    action,
                );
            }
        }
    }

    // ── Render ───────────────────────────────────────────────────────────────
    fn render(&self, canvas: &mut Canvas) {
        canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

        // Title bar.
        canvas.fill_rect(0, 0, WIN_W, 28, TITLE_BG);
        canvas.draw_text_aa(
            8,
            ((28 - rae_tokens::TYPE_SUBTITLE.line_height as usize) / 2) as i32,
            "Calculator",
            rae_tokens::TYPE_SUBTITLE,
            DARK.text_secondary,
            FontFamily::Sans,
        );

        // Display area.
        canvas.fill_rect(10, DISPLAY_Y, WIN_W - 20, DISPLAY_H, DISPLAY_BG);
        match self.mode {
            Mode::Standard => self.render_display_standard(canvas),
            Mode::Scientific => self.render_display_expr(canvas, &self.sci),
            Mode::Programmer => self.render_display_programmer(canvas),
            Mode::Converter => self.render_display_converter(canvas),
        }

        // Tabs + all buttons drawn from the SAME element list used to hit-test.
        let layout = self.build_layout();
        for e in layout.as_slice() {
            self.draw_element(canvas, e);
        }
    }

    /// Draw one interactive element (tab or button) from its rect/label/color.
    /// Tabs use the caption font; buttons use the body font (matching the
    /// original look). The text is centered in the rect.
    fn draw_element(&self, canvas: &mut Canvas, e: &Element) {
        let is_tab = matches!(e.action, Action::SwitchMode(_));
        let font = if is_tab {
            rae_tokens::TYPE_CAPTION
        } else {
            rae_tokens::TYPE_BODY
        };
        let active_tab = matches!(e.action, Action::SwitchMode(m) if m == self.mode);
        let fg = if is_tab && !active_tab {
            DARK.text_secondary
        } else {
            TEXT_FG
        };
        canvas.fill_rect(e.rect.x, e.rect.y, e.rect.w, e.rect.h, e.color);
        let lw = canvas.measure_text_aa(e.label, font, FontFamily::Sans);
        let lx = e.rect.x as i32 + (e.rect.w as i32 - lw) / 2;
        let ly = (e.rect.y + (e.rect.h - font.line_height as usize) / 2) as i32;
        canvas.draw_text_aa(lx, ly, e.label, font, fg, FontFamily::Sans);
    }

    fn render_display_standard(&self, canvas: &mut Canvas) {
        let txt = self.calc.display();
        let disp_w = canvas.measure_text_aa(txt, rae_tokens::TYPE_TITLE, FontFamily::Sans);
        let text_x = (WIN_W - 20) as i32 - disp_w;
        let text_y =
            (DISPLAY_Y + (DISPLAY_H - rae_tokens::TYPE_TITLE.line_height as usize) / 2) as i32;
        canvas.draw_text_aa(
            text_x,
            text_y,
            txt,
            rae_tokens::TYPE_TITLE,
            TEXT_FG,
            FontFamily::Sans,
        );
    }

    fn render_display_expr(&self, canvas: &mut Canvas, e: &ExprBuf) {
        // Top: the editable expression (secondary). Bottom: the result (primary).
        let expr = e.as_str();
        let ew = canvas.measure_text_aa(expr, rae_tokens::TYPE_BODY, FontFamily::Sans);
        canvas.draw_text_aa(
            (WIN_W - 20) as i32 - ew,
            (DISPLAY_Y + 8) as i32,
            expr,
            rae_tokens::TYPE_BODY,
            DARK.text_secondary,
            FontFamily::Sans,
        );
        let res = e.result_str();
        let rw = canvas.measure_text_aa(res, rae_tokens::TYPE_TITLE, FontFamily::Sans);
        canvas.draw_text_aa(
            (WIN_W - 20) as i32 - rw,
            (DISPLAY_Y + DISPLAY_H - rae_tokens::TYPE_TITLE.line_height as usize - 8) as i32,
            res,
            rae_tokens::TYPE_TITLE,
            TEXT_FG,
            FontFamily::Sans,
        );
    }

    fn render_display_programmer(&self, canvas: &mut Canvas) {
        // Editable expression (right-aligned, top).
        let expr = self.prog.as_str();
        let ew = canvas.measure_text_aa(expr, rae_tokens::TYPE_BODY, FontFamily::Sans);
        canvas.draw_text_aa(
            (WIN_W - 20) as i32 - ew,
            (DISPLAY_Y + 4) as i32,
            expr,
            rae_tokens::TYPE_BODY,
            DARK.text_secondary,
            FontFamily::Sans,
        );
        if self.prog.error {
            let res = "Error";
            let rw = canvas.measure_text_aa(res, rae_tokens::TYPE_SUBTITLE, FontFamily::Sans);
            canvas.draw_text_aa(
                (WIN_W - 20) as i32 - rw,
                (DISPLAY_Y + 24) as i32,
                res,
                rae_tokens::TYPE_SUBTITLE,
                DARK.state_danger,
                FontFamily::Sans,
            );
            return;
        }
        // All four bases simultaneously (Win11 Programmer layout).
        let v = self.prog_value as u64;
        let labels = [("HEX", 16u64), ("DEC", 10), ("OCT", 8), ("BIN", 2)];
        let mut row = 0;
        for &(lbl, radix) in labels.iter() {
            let mut out = [0u8; 64];
            let n = if radix == 10 && self.prog_value < 0 {
                // signed decimal
                let mut tmp = [0u8; 64];
                let abs = (self.prog_value.unsigned_abs()) as u64;
                let m = fmt_radix(abs, 10, &mut tmp);
                out[0] = b'-';
                out[1..1 + m].copy_from_slice(&tmp[..m]);
                m + 1
            } else {
                fmt_radix(v, radix, &mut out)
            };
            let y = (DISPLAY_Y + 22 + row * 15) as i32;
            canvas.draw_text_aa(
                14,
                y,
                lbl,
                rae_tokens::TYPE_CAPTION,
                DARK.text_secondary,
                FontFamily::Sans,
            );
            let val = core::str::from_utf8(&out[..n]).unwrap_or("0");
            let vw = canvas.measure_text_aa(val, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
            canvas.draw_text_aa(
                (WIN_W - 20) as i32 - vw,
                y,
                val,
                rae_tokens::TYPE_CAPTION,
                TEXT_FG,
                FontFamily::Sans,
            );
            row += 1;
        }
    }

    /// Converter display: the typed value + the chosen From-unit on the left, an
    /// `=`-style arrow, and the converted result + To-unit. Drawn in the display
    /// zone; the category/unit selectors render as labels in the stepper rows.
    fn render_display_converter(&self, canvas: &mut Canvas) {
        let cat = self.conv_category();
        // Category name (top-left of the display zone).
        canvas.draw_text_aa(
            14,
            (DISPLAY_Y + 6) as i32,
            cat.name,
            rae_tokens::TYPE_CAPTION,
            DARK.text_secondary,
            FontFamily::Sans,
        );

        // Input value + from-unit (one line).
        let mut line = [0u8; 40];
        let mut n = 0usize;
        let entry = core::str::from_utf8(&self.conv_buf[..self.conv_len]).unwrap_or("0");
        let entry = if entry.is_empty() { "0" } else { entry };
        for &b in entry.as_bytes() {
            if n < line.len() {
                line[n] = b;
                n += 1;
            }
        }
        let from_lbl = cat.units[self.conv_from].label;
        if n < line.len() {
            line[n] = b' ';
            n += 1;
        }
        for &b in from_lbl.as_bytes() {
            if n < line.len() {
                line[n] = b;
                n += 1;
            }
        }
        let in_str = core::str::from_utf8(&line[..n]).unwrap_or("0");
        let iw = canvas.measure_text_aa(in_str, rae_tokens::TYPE_SUBTITLE, FontFamily::Sans);
        canvas.draw_text_aa(
            (WIN_W - 20) as i32 - iw,
            (DISPLAY_Y + 24) as i32,
            in_str,
            rae_tokens::TYPE_SUBTITLE,
            DARK.text_secondary,
            FontFamily::Sans,
        );

        // Result value + to-unit (primary, bottom).
        let mut rbuf = [0u8; 48];
        let rn = fmt_f64(self.conv_result(), &mut rbuf);
        let mut res_line = [0u8; 56];
        let mut rl = 0usize;
        for &b in &rbuf[..rn] {
            if rl < res_line.len() {
                res_line[rl] = b;
                rl += 1;
            }
        }
        if rl < res_line.len() {
            res_line[rl] = b' ';
            rl += 1;
        }
        for &b in cat.units[self.conv_to].label.as_bytes() {
            if rl < res_line.len() {
                res_line[rl] = b;
                rl += 1;
            }
        }
        let res_str = core::str::from_utf8(&res_line[..rl]).unwrap_or("0");
        let rw = canvas.measure_text_aa(res_str, rae_tokens::TYPE_TITLE, FontFamily::Sans);
        canvas.draw_text_aa(
            (WIN_W - 20) as i32 - rw,
            (DISPLAY_Y + DISPLAY_H - rae_tokens::TYPE_TITLE.line_height as usize - 6) as i32,
            res_str,
            rae_tokens::TYPE_TITLE,
            TEXT_FG,
            FontFamily::Sans,
        );

        // Stepper-row labels: Category / From / To, centered between the arrows.
        let labels = [
            cat.name,
            cat.units[self.conv_from].label,
            cat.units[self.conv_to].label,
        ];
        let captions = ["Category", "From", "To"];
        for i in 0..3 {
            let y = CONV_STEP_Y + i * (CONV_STEP_H + CONV_STEP_GAP);
            // The small caption (left of the value, after the `<` arrow).
            canvas.draw_text_aa(
                (10 + CONV_ARROW_W + 6) as i32,
                (y + (CONV_STEP_H - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32,
                captions[i],
                rae_tokens::TYPE_CAPTION,
                DARK.text_secondary,
                FontFamily::Sans,
            );
            // The selected value, centered in the row.
            let lw = canvas.measure_text_aa(labels[i], rae_tokens::TYPE_BODY, FontFamily::Sans);
            let lx = (WIN_W as i32 - lw) / 2;
            canvas.draw_text_aa(
                lx,
                (y + (CONV_STEP_H - rae_tokens::TYPE_BODY.line_height as usize) / 2) as i32,
                labels[i],
                rae_tokens::TYPE_BODY,
                TEXT_FG,
                FontFamily::Sans,
            );
        }
    }

    // ── Click dispatch ───────────────────────────────────────────────────────
    /// Dispatch a left-click at surface-local `(px, py)`. Hit-tests against the
    /// current mode's element list (the SAME rects `render` draws) and runs the
    /// matched element's action — identical to the key it mirrors. A click in
    /// empty space hits nothing and is a no-op. Returns true if anything fired
    /// (so the caller can re-render only when state changed).
    fn on_click(&mut self, px: i32, py: i32) -> bool {
        let layout = self.build_layout();
        let Some(action) = layout.hit(px, py) else {
            return false;
        };
        self.dispatch(action)
    }

    /// Apply an `Action` (shared by click dispatch + the design proof).
    fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::Ascii(b) => {
                self.handle_ascii(b);
                true
            }
            Action::Text(s) => {
                for &b in s.as_bytes() {
                    self.handle_ascii(b);
                }
                true
            }
            Action::SwitchMode(m) => {
                self.mode = m;
                self.persist();
                true
            }
            Action::ToggleDeg => {
                self.degrees = !self.degrees;
                self.persist();
                true
            }
            Action::ConvCategory(d) => {
                self.conv_step_category(d);
                true
            }
            Action::ConvFrom(d) => {
                self.conv_step_from(d);
                true
            }
            Action::ConvTo(d) => {
                self.conv_step_to(d);
                true
            }
            Action::None => false,
        }
    }

    // ── Key handling ─────────────────────────────────────────────────────────
    fn handle_ascii(&mut self, ascii: u8) {
        match self.mode {
            Mode::Standard => self.handle_standard(ascii),
            Mode::Scientific => self.handle_text(ascii, true),
            Mode::Programmer => self.handle_text(ascii, false),
            Mode::Converter => self.handle_converter(ascii),
        }
    }

    /// Converter key handling: digits/dot/sign feed the value entry; backspace
    /// and C/Esc clear it; `[`/`]` cycle the category; `,`/`.`-pair handled as
    /// dot; `<`/`>` (and `-`/`+` when used as steppers) are reserved. We map the
    /// bracket and parenthesis keys to selector stepping so the converter is
    /// fully keyboard-drivable without a mouse.
    fn handle_converter(&mut self, ascii: u8) {
        match ascii {
            b'0'..=b'9' | b'.' => self.conv_push(ascii),
            // Leading minus only meaningful for Temperature; conv_push guards it.
            b'_' => self.conv_push(b'-'),
            0x08 => self.conv_backspace(),
            b'c' | b'C' | 0x1B => self.conv_clear(),
            // Category stepping: '[' previous, ']' next.
            b'[' => self.conv_step_category(-1),
            b']' => self.conv_step_category(1),
            // From-unit stepping: ';' previous, '\'' next.
            b';' => self.conv_step_from(-1),
            b'\'' => self.conv_step_from(1),
            // To-unit stepping: ',' previous, '/' next.
            b',' => self.conv_step_to(-1),
            b'/' => self.conv_step_to(1),
            _ => {}
        }
    }

    fn handle_standard(&mut self, ascii: u8) {
        match ascii {
            b'0'..=b'9' => self.calc.input_digit(ascii - b'0'),
            b'.' => self.calc.input_decimal(),
            b'+' => self.calc.apply_op('+'),
            b'-' => self.calc.apply_op('-'),
            b'*' => self.calc.apply_op('*'),
            b'/' => self.calc.apply_op('/'),
            b'%' => self.calc.percent(),
            b'\n' | b'=' => self.calc.equals(),
            b'c' | b'C' => self.calc.clear(),
            b'n' | b'N' => self.calc.negate(),
            0x08 => self.calc.backspace(),
            0x1B => self.calc.clear(),
            _ => {}
        }
    }

    /// Free-form text entry shared by Scientific (`sci`) and Programmer (`prog`).
    /// `is_sci` picks the buffer + the evaluator on `=`/Enter.
    fn handle_text(&mut self, ascii: u8, is_sci: bool) {
        // Toggle deg/rad on 'd' in scientific mode.
        if is_sci && (ascii == b'd' || ascii == b'D') {
            self.degrees = !self.degrees;
            self.persist();
            return;
        }
        match ascii {
            b'\n' | b'=' => {
                if is_sci {
                    self.eval_scientific();
                } else {
                    self.eval_programmer();
                }
            }
            0x08 => {
                if is_sci {
                    self.sci.backspace();
                } else {
                    self.prog.backspace();
                }
            }
            0x1B => {
                if is_sci {
                    self.sci.clear();
                } else {
                    self.prog.clear();
                }
            }
            // Accept digits, letters, operators, parens, dot for expressions.
            b'0'..=b'9'
            | b'a'..=b'z'
            | b'A'..=b'Z'
            | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'('
            | b')'
            | b'.'
            | b','
            | b'!'
            | b'^'
            | b'&'
            | b'|'
            | b'~'
            | b'<'
            | b'>' => {
                if is_sci {
                    self.sci.push_byte(ascii);
                } else {
                    self.prog.push_byte(ascii);
                }
            }
            _ => {}
        }
    }
}

/// Degrees-aware scientific eval: convert the whole input from "degrees for
/// trig" by substituting `sin(x)` → `sin((x)*0.0174532925199433)` for the three
/// direct trig functions. Allocation-free: builds into a fixed scratch buffer;
/// on overflow falls back to a plain radians eval (never panics).
fn eval_degrees(s: &str) -> Result<f64, rae_calc::CalcError> {
    const D2R: &str = "*0.017453292519943295";
    let mut out = [0u8; 256];
    let mut n = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut ok = true;
    let push = |out: &mut [u8], n: &mut usize, b: u8, ok: &mut bool| {
        if *n < out.len() {
            out[*n] = b;
            *n += 1;
        } else {
            *ok = false;
        }
    };
    while i < bytes.len() {
        // Detect a trig call name followed by '('.
        let is_trig = (bytes[i..].starts_with(b"sin(")
            || bytes[i..].starts_with(b"cos(")
            || bytes[i..].starts_with(b"tan("))
            && !(i > 0 && bytes[i - 1].is_ascii_alphabetic());
        if is_trig {
            // copy "sin("
            for k in 0..4 {
                push(&mut out, &mut n, bytes[i + k], &mut ok);
            }
            i += 4;
            // copy the balanced argument, then append the *D2R before the ')'.
            let mut depth = 1i32;
            let arg_start = n;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            // wrap the arg: insert ( at arg_start ... ) *D2R
                            break;
                        }
                    }
                    _ => {}
                }
                push(&mut out, &mut n, bytes[i], &mut ok);
                i += 1;
            }
            let _ = arg_start;
            // append the degree→radian factor, then the closing paren.
            for &b in D2R.as_bytes() {
                push(&mut out, &mut n, b, &mut ok);
            }
            if i < bytes.len() && bytes[i] == b')' {
                push(&mut out, &mut n, b')', &mut ok);
                i += 1;
            }
        } else {
            push(&mut out, &mut n, bytes[i], &mut ok);
            i += 1;
        }
        if !ok {
            return rae_calc::eval(s); // overflow fallback (radians)
        }
    }
    rae_calc::eval(core::str::from_utf8(&out[..n]).unwrap_or(s))
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

// ── Design proof (R10: a fail-able check the token wiring is correct) ─────
//
// `cargo test` can't host a libtest harness in this `#![no_main]` bin (raekit's
// `#[panic_handler]` + std's = duplicate lang item). This pure proof is the
// fail-able authority; the ramp is host-KAT'd by `cargo test -p rae_tokens` and
// the MATH engine (Standard, Scientific, Programmer) by `cargo test -p rae_calc`.

/// True iff Calculator's chrome is wired to the shared design tokens AND all
/// three math modes round-trip through `rae_calc`.
#[must_use]
pub fn design_proof() -> bool {
    let ramp = rae_tokens::derive_accent(theme_seed(), &DARK);
    let chrome = btn_op() == ramp.base
        && BG == DARK.bg_raised
        && DISPLAY_BG == DARK.bg_base
        && BTN_BG == DARK.bg_elevated
        && BTN_EQ == DARK.state_ok
        && TEXT_FG == DARK.text_primary
        && raekit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;
    // Standard (float), Scientific (trig), Programmer (bitwise) all wired in.
    let math = rae_calc::eval("2+3*4") == Ok(14.0)
        && rae_calc::eval("10/4") == Ok(2.5)
        && rae_calc::eval("sin(0)") == Ok(0.0)
        && rae_calc::eval_int("0xFF & 0x0F") == Ok(15);
    chrome && math && hit_test_proof() && convert_proof() && prefs_round_trip_ok()
}

/// Prove the Calculator PREFS SCHEMA: a known non-default `Prefs` serialized via
/// `rae_toml` then re-parsed restores every field exactly (active mode token, the
/// deg/rad toggle, the Converter category), AND a corrupt / missing-key document
/// resolves to the typed defaults (NOT a panic, NOT a wrong value). Also proves
/// the mode + converter-category token round-trips and that an UNKNOWN mode token
/// maps to Standard. Returns `false` on any drift (→ exit(3) at startup).
#[must_use]
fn prefs_round_trip_ok() -> bool {
    // (a) Full round-trip of a non-default Prefs.
    let p = Prefs {
        mode: Mode::Programmer,
        degrees: true,
        conv_category: String::from("Temp"),
    };
    let text = rae_toml::to_string(&p.to_toml());
    let parsed = match rae_toml::parse(&text) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let back = Prefs::from_toml(&parsed);
    if back.mode != Mode::Programmer || !back.degrees || back.conv_category != "Temp" {
        return false;
    }
    // The category name must resolve to a real index (Temp is in the table).
    if CATEGORIES[back.conv_index()].name != "Temp" {
        return false;
    }

    // (b) Every mode token must round-trip through token() → from_token().
    for m in [
        Mode::Standard,
        Mode::Scientific,
        Mode::Programmer,
        Mode::Converter,
    ] {
        if Mode::from_token(m.token()) != m {
            return false;
        }
    }
    // An UNKNOWN mode token resolves to Standard (no panic, safe default).
    if Mode::from_token("bogus-mode") != Mode::Standard {
        return false;
    }

    // (c) A corrupt document → typed defaults (parse FAILS, we don't panic).
    let corrupt = "mode = = oops\n[unterminated\n";
    let d = match rae_toml::parse(corrupt) {
        Ok(t) => Prefs::from_toml(&t), // shouldn't reach here for this input
        Err(_) => Prefs::defaults(),
    };
    if d.mode != Mode::Standard || d.degrees || d.conv_category != CATEGORIES[0].name {
        return false;
    }

    // (d) A well-formed doc MISSING every prefs key → typed defaults per field.
    let empty = match rae_toml::parse("unrelated = 1\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let e = Prefs::from_toml(&empty);
    if e.mode != Mode::Standard || e.degrees || e.conv_category != CATEGORIES[0].name {
        return false;
    }

    // (e) A wrong-TYPED field (degrees as a string) is ignored → default; an
    // unknown converter category name is rejected → default category; a valid
    // mode token still parses.
    let wrong = match rae_toml::parse(
        "mode = \"scientific\"\ndegrees = \"yes\"\nconv_category = \"NoSuchCat\"\n",
    ) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let w = Prefs::from_toml(&wrong);
    if w.mode != Mode::Scientific || w.degrees || w.conv_category != CATEGORIES[0].name {
        return false;
    }

    true
}

/// Find a category by name (proof helper; categories are static).
fn category_by_name(name: &str) -> &'static Category {
    CATEGORIES
        .iter()
        .find(|c| c.name == name)
        .unwrap_or(&CATEGORIES[0])
}

/// Find a unit index by label within a category (proof helper).
fn unit_index(cat: &Category, label: &str) -> usize {
    cat.units.iter().position(|u| u.label == label).unwrap_or(0)
}

/// Helper: convert `value` of `from` to `to` within the named category.
fn conv_named(cat_name: &str, from: &str, to: &str, value: f64) -> f64 {
    let cat = category_by_name(cat_name);
    convert(cat, unit_index(cat, from), unit_index(cat, to), value)
}

/// True iff a value is within `tol` of `expected` (the converter proof's
/// tolerance check for irrational/rounded factors).
fn near(actual: f64, expected: f64, tol: f64) -> bool {
    let d = actual - expected;
    let d = if d < 0.0 { -d } else { d };
    d <= tol
}

/// Prove the Converter math: exact pure-scale conversions, the affine
/// Temperature path (the load-bearing guard — a missing +32 offset or wrong
/// pivot fails here), binary-data sizes, and a tolerance check on the rounded
/// mile factor. Returns false (→ exit non-zero) on any drift.
#[must_use]
fn convert_proof() -> bool {
    // Length: exact (1000) and tolerance (mile).
    if conv_named("Length", "km", "m", 1.0) != 1000.0 {
        return false;
    }
    if !near(conv_named("Length", "mi", "m", 1.0), 1609.34, 0.01) {
        return false;
    }
    // Temperature — the affine path. 100C == 212F, 0C == 32F, 0C == 273.15K.
    if !near(conv_named("Temp", "C", "F", 100.0), 212.0, 1e-9) {
        return false;
    }
    if !near(conv_named("Temp", "C", "F", 0.0), 32.0, 1e-9) {
        return false;
    }
    if !near(conv_named("Temp", "C", "K", 0.0), 273.15, 1e-9) {
        return false;
    }
    // Inverse affine: 212F == 100C, 273.15K == 0C.
    if !near(conv_named("Temp", "F", "C", 212.0), 100.0, 1e-9) {
        return false;
    }
    if !near(conv_named("Temp", "K", "C", 273.15), 0.0, 1e-9) {
        return false;
    }
    // Affine guard: 100C is NOT 180F (a missing +32 offset would yield 180).
    #[allow(clippy::float_cmp)]
    if conv_named("Temp", "C", "F", 100.0) == 180.0 {
        return false;
    }
    // Data: 1 GiB == 1073741824 bytes (binary).
    if conv_named("Data", "GiB", "byte", 1.0) != 1_073_741_824.0 {
        return false;
    }
    // Time: 1 h == 3600 s.
    if conv_named("Time", "h", "s", 1.0) != 3600.0 {
        return false;
    }
    true
}

/// Prove the mouse hit-test invariant: the rectangles `render` DRAWS are the
/// same rectangles `on_click` HIT-TESTS (single source of truth, no drift), and
/// a click maps to the action that button's key fires.
///
/// For every interactive element of every mode, a click at the rect's exact
/// center must resolve back to THAT element. A click far outside all rects must
/// resolve to nothing. The mode-tab rects must select their mode. Returns false
/// (→ `exit(non-zero)`) on any drift.
#[must_use]
fn hit_test_proof() -> bool {
    for mode in [
        Mode::Standard,
        Mode::Scientific,
        Mode::Programmer,
        Mode::Converter,
    ] {
        let mut app = App::new();
        app.mode = mode;
        let layout = app.build_layout();

        // Every element's center hits exactly that element (by label + rect).
        for e in layout.as_slice() {
            let cx = (e.rect.x + e.rect.w / 2) as i32;
            let cy = (e.rect.y + e.rect.h / 2) as i32;
            let hit = layout.as_slice().iter().find(|h| h.rect.contains(cx, cy));
            match hit {
                Some(h) => {
                    // The center must land in a rect with the same label (rects
                    // are non-overlapping, so it must be this one).
                    if h.label != e.label {
                        return false;
                    }
                }
                None => return false,
            }
        }

        // A click outside the window resolves to nothing (no panic, no-op).
        if layout.hit(-100, -100).is_some() {
            return false;
        }
        if layout.hit(WIN_W as i32 + 50, WIN_H as i32 + 50).is_some() {
            return false;
        }

        // A click on the "Scientific" mode tab selects Scientific.
        let sci_tab = layout
            .as_slice()
            .iter()
            .find(|e| matches!(e.action, Action::SwitchMode(Mode::Scientific)));
        let Some(tab) = sci_tab else {
            return false;
        };
        let tcx = (tab.rect.x + tab.rect.w / 2) as i32;
        let tcy = (tab.rect.y + tab.rect.h / 2) as i32;
        let mut app2 = App::new();
        app2.mode = mode;
        if !app2.on_click(tcx, tcy) || app2.mode != Mode::Scientific {
            return false;
        }
    }

    // Dispatch mapping: clicking the Standard "7" button feeds digit 7 to the
    // engine exactly as the '7' key does.
    let mut app = App::new();
    let layout = app.build_layout();
    let seven = layout.as_slice().iter().find(|e| e.label == "7");
    let Some(seven) = seven else {
        return false;
    };
    let scx = (seven.rect.x + seven.rect.w / 2) as i32;
    let scy = (seven.rect.y + seven.rect.h / 2) as i32;
    if !app.on_click(scx, scy) {
        return false;
    }
    app.calc.display() == "7"
}

/// A cheap runtime gate that the real math engine is wired in (precedence +
/// float division — the integer-only bug this app fixes). Returns false if the
/// linked `rae_calc` ever regresses; the authoritative proof is the host KATs.
#[must_use]
pub fn eval_proof() -> bool {
    rae_calc::eval("2+3*4") == Ok(14.0) && rae_calc::eval("10/4") == Ok(2.5)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    if !design_proof() || !eval_proof() {
        raekit::sys::exit(3);
    }
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }

    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();

    app.render(&mut canvas);
    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    // Live left-button state, tracked across frames for click-EDGE detection
    // (fire once on was-up -> now-down, not every frame the button is held).
    let mut left_was_down = false;

    loop {
        // ── Mouse: drain button events for live state, hit-test the cursor ────
        // `poll_mouse()` is a destructive per-event queue (bits[7:0]=buttons);
        // drain it to find the latest left-button level, then read the absolute
        // cursor and convert to surface-local space for hit-testing.
        let mut mouse_activity = false;
        let mut left_down = left_was_down;
        loop {
            let ev = raekit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            left_down = (ev & 0x01) != 0;
            mouse_activity = true;
        }
        if mouse_activity || left_down != left_was_down {
            // Left-click edge: was up, now down.
            if left_down && !left_was_down {
                let (cx, cy, _btn) = raekit::sys::cursor_pos();
                // Subtract the LIVE window origin (not the stale present-time
                // PRESENT_X/Y) so clicks land correctly after the window manager
                // moves the window (Overview / Spaces / tiling). Falls back to the
                // present origin if the surface isn't found. Saturating-sub keeps a
                // cursor above/left of the window from underflowing.
                let (ox, oy) = raekit::sys::surface_origin(sid)
                    .unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
                let lx = (cx as i32).saturating_sub(ox as i32);
                let ly = (cy as i32).saturating_sub(oy as i32);
                if app.on_click(lx, ly) {
                    app.render(&mut canvas);
                    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
            }
            left_was_down = left_down;
        }

        let key = raekit::sys::read_key();
        if key == 0 {
            raekit::sys::yield_now();
            continue;
        }

        let scancode = key as u8;
        let is_release = scancode & 0x80 != 0;
        let code = scancode & 0x7F;

        if code == 0x2A || code == 0x36 {
            app.shift_held = !is_release;
            continue;
        }

        if is_release {
            continue;
        }

        // Mode tabs on F1/F2/F3 (scancodes 0x3B/0x3C/0x3D).
        match code {
            0x3B => {
                app.mode = Mode::Standard;
                app.persist();
                app.render(&mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                continue;
            }
            0x3C => {
                app.mode = Mode::Scientific;
                app.persist();
                app.render(&mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                continue;
            }
            0x3D => {
                app.mode = Mode::Programmer;
                app.persist();
                app.render(&mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                continue;
            }
            0x3E => {
                app.mode = Mode::Converter;
                app.persist();
                app.render(&mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                continue;
            }
            _ => {}
        }

        if let Some(ascii) = scancode_to_ascii(code, app.shift_held) {
            app.handle_ascii(ascii);
        }

        app.render(&mut canvas);
        raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
    }
}
