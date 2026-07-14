//! RaeenOS Music — *"play my music"* (RaeenOS_Concept.md §creators/media).
//!
//! The daily-driver music player that completes the "play my music" parity with
//! Windows Media Player / macOS Music on day one: a **track list** of a Music
//! directory (`.wav` files), **transport controls** (play/pause/next/prev/stop),
//! a **seek/progress indicator**, and keyboard control. Built on the proven
//! from-scratch `raemedia::wav` decoder and the wired audio path
//! `raekit::audio_submit` → SYS_AUDIO_SUBMIT (267) → AudioMixer → AUDIO_RING → HDA.
//!
//! Standalone userspace ELF (`exec_path = "music"`). Chrome is on the shared
//! `rae_tokens` design language; the live desktop accent comes through
//! `SYS_THEME_GET` (raekit::sys::theme_accent) at launch so Music matches the
//! desktop 1:1 (whole-OS cohesion).
//!
//! AUDIO CONTRACT: the mixer takes interleaved 48 kHz i16 **stereo** PCM, at most
//! `AUDIO_SUBMIT_MAX_FRAMES` (512) frames per `audio_submit` call. So on play we
//! decode the WAV, then in the event loop stream it in bounded chunks: any
//! sample-rate ≠ 48000 is linearly resampled and mono is upmixed to stereo before
//! submission. Submission is paced by the mixer's accepted-frame return value so
//! we never overrun the ring (back off + retry the remainder next loop tick).
//!
//! HOSTILE-INPUT POSTURE: a media file is untrusted data. Every decode goes
//! through the host-KAT'd `raemedia::wav` decoder which returns `Err` (never
//! panics) on malformed input; a decode failure skips the track + shows an error
//! and the app stays alive. The whole pipeline never panics.
//!
//! PROOF: this ELF can't run `cargo test`, so `design_proof()` (a fail-able
//! runtime gate at `_start`) decodes a built-in tiny WAV fixture, asserts exact
//! samples, and asserts the mono→stereo + 24kHz→48kHz resample helper produces
//! the expected frames — exit(3) on any drift. `raemedia`'s WAV host KATs are the
//! decode-logic proof.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

#[allow(unused_imports)]
use raekit;

use alloc::string::String;
use rae_tokens::{DARK, RAEBLUE};
use rae_toml::Toml;
use raegfx::text::FontFamily;
use raegfx::Canvas;
use raemedia::wav::{decode_wav, DecodedAudio};

// ── Window geometry ─────────────────────────────────────────────────────

const WIN_W: usize = 720;
const WIN_H: usize = 520;
const SURFACE_VIRT: u64 = 0x0000_7C00_0000;

const TITLE_H: usize = 28;
const TOOLBAR_H: usize = 34;
const TRANSPORT_H: usize = 72;
const STATUS_H: usize = 22;

const ROW_H: usize = 30;

/// The on-screen origin we present this window at. Absolute cursor coordinates
/// from `cursor_pos()` are converted to surface-local space by subtracting this.
const PRESENT_X: i32 = 200;
const PRESENT_Y: i32 = 80;

// ── Mouse hit-testing (single source of truth: draw-rects == hit-rects) ───
//
// Each clickable element computes its rect from the SAME constants `render`
// draws with, so the draw geometry and the hit geometry can never drift. A
// click dispatches to the EXACT action the matching key fires; an empty-space
// click resolves to `Action::None` (no-op, never panics).

/// What a left-click maps to — each mirrors a keyboard action 1:1.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Select (and load+play) track row `i` in the list.
    SelectTrack(usize),
    PlayPause,
    Next,
    Prev,
    Stop,
    /// Seek to a 0..=1000 permille position along the progress bar.
    SeekPermille(u32),
    /// Cycle the repeat mode (Off → One → All).
    ToggleRepeat,
    /// Toggle shuffle on/off.
    ToggleShuffle,
    /// Nudge volume down one step.
    VolDown,
    /// Nudge volume up one step.
    VolUp,
    Close,
    None,
}

/// An axis-aligned rect in surface-local coordinates.
#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x as i32
            && px < (self.x + self.w) as i32
            && py >= self.y as i32
            && py < (self.y + self.h) as i32
    }
}

// Transport-button layout constants — shared by draw + hit so they can't drift.
const XBTN_W: usize = 20;
const TRANSPORT_BTN_W: usize = 56;
const TRANSPORT_BTN_H: usize = 28;
const TRANSPORT_BTN_GAP: usize = 8;
/// The five transport buttons, left to right, with the action each fires.
const TRANSPORT_BTNS: [(&str, Action); 5] = [
    ("Prev", Action::Prev),
    ("Play", Action::PlayPause),
    ("Next", Action::Next),
    ("Stop", Action::Stop),
    ("|<", Action::SeekPermille(0)),
];

/// X of transport button `i` (0..5). Right-aligned cluster in the transport bar.
fn transport_btn_x(i: usize) -> usize {
    let cluster_w =
        TRANSPORT_BTNS.len() * TRANSPORT_BTN_W + (TRANSPORT_BTNS.len() - 1) * TRANSPORT_BTN_GAP;
    let start = WIN_W - 16 - cluster_w;
    start + i * (TRANSPORT_BTN_W + TRANSPORT_BTN_GAP)
}

/// Y of the transport button row (centered on the now-playing title line).
fn transport_btn_y() -> usize {
    WIN_H - STATUS_H - TRANSPORT_H + 6
}

/// The progress/seek bar rect — the SAME geometry `render_transport` draws.
fn seek_bar_rect() -> Rect {
    let tr_y = WIN_H - STATUS_H - TRANSPORT_H;
    Rect {
        x: 16,
        y: tr_y + 40,
        w: WIN_W - 32,
        h: 8,
    }
}

/// The window-close (X) rect in the title bar.
fn close_rect() -> Rect {
    Rect {
        x: WIN_W - 28,
        y: 4,
        w: XBTN_W,
        h: 20,
    }
}

// ── Toolbar preference controls (repeat / shuffle / volume) ────────────────
//
// A right-aligned cluster in the toolbar, drawn + hit-tested from the SAME
// constants so geometry can't drift. Order, right→left: [Vol+][vol][Vol-]
// [Shuffle][Repeat].
const TB_CTL_H: usize = 22;
const TB_CTL_GAP: usize = 6;
const TB_REPEAT_W: usize = 86;
const TB_SHUFFLE_W: usize = 88;
const TB_VOL_BTN_W: usize = 24;
const TB_VOL_LABEL_W: usize = 52;

/// Y of the toolbar control row (centered in the toolbar band).
fn tb_ctl_y() -> usize {
    TITLE_H + (TOOLBAR_H - TB_CTL_H) / 2
}

/// Repeat-mode toggle rect (leftmost of the right-aligned cluster).
fn repeat_ctl_rect() -> Rect {
    let total = TB_REPEAT_W
        + TB_CTL_GAP
        + TB_SHUFFLE_W
        + TB_CTL_GAP
        + TB_VOL_BTN_W
        + TB_CTL_GAP
        + TB_VOL_LABEL_W
        + TB_CTL_GAP
        + TB_VOL_BTN_W;
    let start = WIN_W.saturating_sub(12 + total);
    Rect {
        x: start,
        y: tb_ctl_y(),
        w: TB_REPEAT_W,
        h: TB_CTL_H,
    }
}

/// Shuffle toggle rect (right of repeat).
fn shuffle_ctl_rect() -> Rect {
    let r = repeat_ctl_rect();
    Rect {
        x: r.x + TB_REPEAT_W + TB_CTL_GAP,
        y: r.y,
        w: TB_SHUFFLE_W,
        h: TB_CTL_H,
    }
}

/// Volume-down button rect.
fn vol_down_rect() -> Rect {
    let r = shuffle_ctl_rect();
    Rect {
        x: r.x + TB_SHUFFLE_W + TB_CTL_GAP,
        y: r.y,
        w: TB_VOL_BTN_W,
        h: TB_CTL_H,
    }
}

/// Volume readout rect (between the - and + buttons; not clickable itself).
fn vol_label_rect() -> Rect {
    let r = vol_down_rect();
    Rect {
        x: r.x + TB_VOL_BTN_W + TB_CTL_GAP,
        y: r.y,
        w: TB_VOL_LABEL_W,
        h: TB_CTL_H,
    }
}

/// Volume-up button rect.
fn vol_up_rect() -> Rect {
    let r = vol_label_rect();
    Rect {
        x: r.x + TB_VOL_LABEL_W + TB_CTL_GAP,
        y: r.y,
        w: TB_VOL_BTN_W,
        h: TB_CTL_H,
    }
}

// ── Audio contract ───────────────────────────────────────────────────────
//
// The mixer is fixed 48 kHz i16 stereo and accepts at most 512 frames per
// `audio_submit` call (raekit::sys::audio_submit doc). We hold a decoded,
// resampled, stereo-interleaved PCM buffer and stream a window of it per tick.

const MIX_RATE: u32 = 48_000;
const AUDIO_SUBMIT_MAX_FRAMES: usize = 512;

// ── Palette (rae_tokens, docs/design/design-language.md) ──────────────────

const BG: u32 = DARK.bg_raised;
const TITLE_BG: u32 = DARK.bg_base;
const TOOLBAR_BG: u32 = DARK.bg_overlay;
const LIST_BG: u32 = DARK.bg_raised;
const ROW_ALT_BG: u32 = DARK.bg_base;
const TRANSPORT_BG: u32 = DARK.bg_overlay;
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_MUTED: u32 = DARK.text_secondary;
const STATUS_BG: u32 = DARK.bg_base;
const STROKE_HL: u32 = DARK.stroke_strong;
const TRACK_BG: u32 = DARK.bg_base; // progress trough

fn theme_seed() -> u32 {
    raekit::sys::theme_accent()
}

/// Accent base, derived through the shared ramp from the live theme seed.
fn accent() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).base
}

/// Opaque selection fill: the accent's pressed/active shade.
fn sel_fill() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).active
}

// ═══════════════════════════════════════════════════════════════════════════
// Audio helpers (host-proven shape via design_proof; pure integer/float math)
// ═══════════════════════════════════════════════════════════════════════════

/// Prepare a decoded WAV for the mixer: upmix mono→stereo and linearly resample
/// to 48 kHz, producing interleaved i16 L,R pairs ready for `audio_submit`.
/// Never panics; returns an empty buffer for an empty/degenerate input.
fn prepare_for_mixer(dec: &DecodedAudio) -> Vec<i16> {
    let ch = dec.channels.max(1) as usize;
    let in_frames = if ch == 0 { 0 } else { dec.samples.len() / ch };
    if in_frames == 0 {
        return Vec::new();
    }

    // Collapse/expand to a stereo (L,R) view per source frame first.
    // For mono: L=R=sample. For >=2 channels: take channels 0 and 1.
    let stereo_at = |frame: usize| -> (i32, i32) {
        let base = frame * ch;
        let l = dec.samples[base] as i32;
        let r = if ch >= 2 {
            dec.samples[base + 1] as i32
        } else {
            l
        };
        (l, r)
    };

    if dec.sample_rate == MIX_RATE {
        // Fast path: just upmix/downselect to stereo, no rate conversion.
        let mut out = Vec::with_capacity(in_frames * 2);
        for f in 0..in_frames {
            let (l, r) = stereo_at(f);
            out.push(l as i16);
            out.push(r as i16);
        }
        return out;
    }

    // Linear resample to 48 kHz. out_frames = in_frames * MIX_RATE / sample_rate.
    let sr = dec.sample_rate.max(1) as u64;
    let out_frames = ((in_frames as u64) * MIX_RATE as u64 / sr) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for o in 0..out_frames {
        // Source position in input-frame units, fixed-point via u64.
        let src_pos = (o as u64) * sr; // scaled by MIX_RATE
        let idx = (src_pos / MIX_RATE as u64) as usize;
        let frac_num = src_pos % MIX_RATE as u64;
        let (l0, r0) = stereo_at(idx.min(in_frames - 1));
        let (l1, r1) = stereo_at((idx + 1).min(in_frames - 1));
        // Linear interpolation: s = s0 + (s1 - s0) * frac.
        let lerp = |a: i32, b: i32| -> i16 {
            let v = a as i64 + ((b - a) as i64) * frac_num as i64 / MIX_RATE as i64;
            if v > i16::MAX as i64 {
                i16::MAX
            } else if v < i16::MIN as i64 {
                i16::MIN
            } else {
                v as i16
            }
        };
        out.push(lerp(l0, l1));
        out.push(lerp(r0, r1));
    }
    out
}

// ── Music directory enumeration ────────────────────────────────────────────

const PATH_CAP: usize = 256;
const NAME_CAP: usize = 64;
const MAX_TRACKS: usize = 128;
/// Hard cap on a single WAV slurp (matches the decoder's own size posture).
const DECODE_CAP: usize = 64 * 1024 * 1024; // 64 MiB

#[derive(Clone, Copy)]
struct PathBuf {
    bytes: [u8; PATH_CAP],
    len: usize,
}

impl PathBuf {
    fn new() -> Self {
        Self {
            bytes: [0; PATH_CAP],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("/")
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(PATH_CAP);
        self.bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }
    fn push_component(&mut self, name: &str) {
        if self.len > 0 && self.bytes[self.len - 1] != b'/' && self.len < PATH_CAP {
            self.bytes[self.len] = b'/';
            self.len += 1;
        }
        for &b in name.as_bytes() {
            if self.len >= PATH_CAP {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
}

/// One track entry: a file name. PCM is decoded on demand at play time (we do not
/// hold every track's full PCM resident).
struct Track {
    name: [u8; NAME_CAP],
    name_len: usize,
}

impl Track {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// True if a file name ends with `.wav`/`.wave` (case-insensitive).
fn is_wav_name(name: &str) -> bool {
    let lower_ends = |suf: &str| -> bool {
        let nb = name.as_bytes();
        let sb = suf.as_bytes();
        if nb.len() < sb.len() {
            return false;
        }
        let tail = &nb[nb.len() - sb.len()..];
        tail.iter()
            .zip(sb.iter())
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
    };
    lower_ends(".wav") || lower_ends(".wave")
}

// ── App state ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Playback {
    Stopped,
    Playing,
    Paused,
}

/// Repeat mode for end-of-track behavior. Persisted across launches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Repeat {
    /// At track end, advance to the next track (or stop on the last with a
    /// single track). The default — matches every desktop player's "off".
    Off,
    /// At track end, replay the SAME track from the start (loop one).
    One,
    /// At track end, advance and wrap from the last track back to the first
    /// (loop the whole list).
    All,
}

impl Repeat {
    /// Cycle Off → One → All → Off (the toolbar toggle order).
    fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::One,
            Repeat::One => Repeat::All,
            Repeat::All => Repeat::Off,
        }
    }
    /// Stable token persisted in the prefs file.
    fn as_token(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::One => "one",
            Repeat::All => "all",
        }
    }
    /// Parse the persisted token; unknown / missing → the typed default (`Off`).
    fn from_token(s: &str) -> Self {
        match s {
            "one" => Repeat::One,
            "all" => Repeat::All,
            _ => Repeat::Off,
        }
    }
    /// Short on-screen badge for the toolbar control.
    fn badge(self) -> &'static str {
        match self {
            Repeat::Off => "Repeat: off",
            Repeat::One => "Repeat: one",
            Repeat::All => "Repeat: all",
        }
    }
}

// ── Persistent preferences (rae_toml) ─────────────────────────────────────────
//
// RaeenOS_Concept.md §"The user owns the machine": "remember my settings" must be
// real. Music persists its user-visible state to `<home>/.config/music.toml` and
// restores it on launch. Every load is hostile-input-tolerant: a missing, corrupt,
// or out-of-range config falls back to TYPED DEFAULTS and NEVER panics — the app
// always starts. This is the per-app prefs pattern the other consumer apps follow.

/// Volume scale floor/ceiling (percent). 0 = silent, 100 = unity (no scaling).
const VOL_DEFAULT: u32 = 80;
const VOL_MAX: u32 = 100;
/// Volume nudge per Up/Down-volume keypress / scroll.
const VOL_STEP: u32 = 5;

/// The decoded, defaulted preferences restored at launch. Pure data: load builds
/// it from a parsed (or absent) TOML, save serializes the live App state into it.
#[derive(Clone)]
struct Prefs {
    /// Playback volume, 0..=100 (scales submitted PCM; 100 = unity).
    volume: u32,
    repeat: Repeat,
    shuffle: bool,
    /// Last-played / selected track FILE NAME (re-resolved against the live scan;
    /// a renamed/deleted file simply fails to match → selection 0). Empty = none.
    last_track: String,
    /// Last scanned library folder (absolute path). Empty = use the default
    /// `<home>/Music`.
    last_folder: String,
}

impl Prefs {
    /// The typed defaults used on first run or any config error.
    fn defaults() -> Self {
        Self {
            volume: VOL_DEFAULT,
            repeat: Repeat::Off,
            shuffle: false,
            last_track: String::new(),
            last_folder: String::new(),
        }
    }

    /// Build `Prefs` from a parsed TOML table, clamping/validating every field and
    /// substituting the typed default for any missing or out-of-range value. Never
    /// panics; an unrelated shape (e.g. a non-table root) yields full defaults.
    fn from_toml(t: &Toml) -> Self {
        let mut p = Self::defaults();
        if let Some(v) = t.get("volume").and_then(Toml::as_i64) {
            // Clamp into 0..=100 rather than rejecting — a stale/garbage number
            // still yields a usable, in-range volume.
            p.volume = v.clamp(0, VOL_MAX as i64) as u32;
        }
        if let Some(s) = t.get("repeat").and_then(Toml::as_str) {
            p.repeat = Repeat::from_token(s);
        }
        if let Some(b) = t.get("shuffle").and_then(Toml::as_bool) {
            p.shuffle = b;
        }
        if let Some(s) = t.get("last_track").and_then(Toml::as_str) {
            // Cap the stored name; a pathological length can't blow anything up
            // because it's only ever compared against scanned file names.
            p.last_track = String::from(truncate_on_char_boundary(s, PATH_CAP));
        }
        if let Some(s) = t.get("last_folder").and_then(Toml::as_str) {
            p.last_folder = String::from(truncate_on_char_boundary(s, PATH_CAP));
        }
        p
    }

    /// Serialize the live preferences into an order-stable `Toml::Table` ready for
    /// `rae_toml::to_string`. The schema is flat (no headers) so a round-trip is
    /// trivial and human-editable.
    fn to_toml(&self) -> Toml {
        let mut table: alloc::vec::Vec<(String, Toml)> = alloc::vec::Vec::new();
        table.push((String::from("volume"), Toml::Integer(self.volume as i64)));
        table.push((
            String::from("repeat"),
            Toml::String(String::from(self.repeat.as_token())),
        ));
        table.push((String::from("shuffle"), Toml::Boolean(self.shuffle)));
        table.push((
            String::from("last_track"),
            Toml::String(self.last_track.clone()),
        ));
        table.push((
            String::from("last_folder"),
            Toml::String(self.last_folder.clone()),
        ));
        Toml::Table(table)
    }
}

/// The per-app config path: `<session home>/.config/music.toml`. Falls back to the
/// same `/home/user` default the Music directory uses when no session is present.
/// The `.config` directory is created (idempotent) before any write.
fn prefs_path() -> PathBuf {
    let mut p = PathBuf::new();
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

/// Load preferences from `<home>/.config/music.toml`. On ANY failure — file
/// absent, unreadable, not UTF-8, or a `rae_toml::parse` error — returns the typed
/// defaults. Never panics, never blocks the app from launching.
fn load_prefs() -> Prefs {
    let mut dir = prefs_path();
    dir.push_component("music.toml");
    let fd = raekit::sys::open(dir.as_str(), 0);
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

/// Persist `prefs` to `<home>/.config/music.toml` (best effort). Creates the
/// `.config` directory if missing, serializes via `rae_toml::to_string`, and
/// writes O_CREAT|O_TRUNC. A failure is silent — the app keeps running.
fn save_prefs(prefs: &Prefs) {
    let dir = prefs_path();
    let _ = raekit::sys::mkdir(dir.as_str());
    let mut path = dir;
    path.push_component("music.toml");
    let text = rae_toml::to_string(&prefs.to_toml());
    // O_WRONLY | O_CREAT | O_TRUNC = 0x0241 (matches the Notes/Text Editor path).
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

/// Return a prefix of `s` no longer than `max` bytes, cut on a UTF-8 char
/// boundary so the result is always valid (never panics on a multi-byte split).
fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

struct App {
    dir: PathBuf,
    tracks: Vec<Track>,
    selected: usize,
    scroll_row: usize,
    state: Playback,
    // The currently-loaded track's mixer-ready PCM (48 kHz i16 stereo) + cursor.
    pcm: Vec<i16>,
    cursor_frames: usize, // submitted-up-to position, in frames
    now_playing: usize,   // index of the loaded track
    toast: [u8; 64],
    toast_len: usize,
    // ── Persisted preferences (mirrored to <home>/.config/music.toml) ──
    /// Playback volume 0..=100; scales every PCM frame before `audio_submit`.
    volume: u32,
    repeat: Repeat,
    shuffle: bool,
    /// xorshift RNG state for shuffle's next-track pick (seeded from the clock).
    rng: u64,
}

impl App {
    fn music_dir() -> PathBuf {
        // Prefer <session home>/Music; fall back to a system bucket.
        let mut info = [0u8; 96];
        if raekit::sys::session_info(&mut info).is_some() {
            if let Some(home) = raekit::sys::session_home_from(&info) {
                let mut p = PathBuf::new();
                p.set(home);
                p.push_component("Music");
                return p;
            }
        }
        let mut p = PathBuf::new();
        p.set("/home/user/Music");
        p
    }

    fn new() -> Self {
        // Restore saved preferences (typed defaults on first run / any error).
        let prefs = load_prefs();

        // Last folder wins if it was persisted and non-empty, else the default
        // `<home>/Music`. A stale/deleted folder simply scans to zero tracks.
        let dir = if prefs.last_folder.is_empty() {
            Self::music_dir()
        } else {
            let mut p = PathBuf::new();
            p.set(&prefs.last_folder);
            p
        };
        let _ = raekit::sys::mkdir(dir.as_str());

        // Seed the shuffle RNG from the clock so successive launches differ; a
        // zero seed is replaced (xorshift fixed-points at 0).
        let mut seed = raekit::sys::time_ns();
        if seed == 0 {
            seed = 0x9E37_79B9_7F4A_7C15;
        }

        let mut app = Self {
            dir,
            tracks: Vec::new(),
            selected: 0,
            scroll_row: 0,
            state: Playback::Stopped,
            pcm: Vec::new(),
            cursor_frames: 0,
            now_playing: usize::MAX,
            toast: [0; 64],
            toast_len: 0,
            volume: prefs.volume,
            repeat: prefs.repeat,
            shuffle: prefs.shuffle,
            rng: seed,
        };
        app.scan();
        // Re-resolve the last-played track against the freshly scanned list.
        if !prefs.last_track.is_empty() {
            for (i, t) in app.tracks.iter().enumerate() {
                if t.name() == prefs.last_track.as_str() {
                    app.selected = i;
                    break;
                }
            }
            app.ensure_visible();
        }
        app
    }

    /// Snapshot the live persistable state into a `Prefs` and write it to disk.
    /// Called on every preference-affecting change. Best effort + silent on
    /// failure (the app never blocks on the config write).
    fn persist(&self) {
        let last_track = if self.selected < self.tracks.len() {
            String::from(self.tracks[self.selected].name())
        } else {
            String::new()
        };
        let prefs = Prefs {
            volume: self.volume.min(VOL_MAX),
            repeat: self.repeat,
            shuffle: self.shuffle,
            last_track,
            last_folder: String::from(self.dir.as_str()),
        };
        save_prefs(&prefs);
    }

    /// Advance the xorshift64 RNG and return a value in `0..n` (n>0).
    fn rand_below(&mut self, n: usize) -> usize {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x % n as u64) as usize
    }

    /// Cycle the repeat mode (Off → One → All) and persist it.
    fn cycle_repeat(&mut self) {
        self.repeat = self.repeat.next();
        self.set_toast(self.repeat.badge());
        self.persist();
    }

    /// Toggle shuffle and persist it.
    fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        self.set_toast(if self.shuffle {
            "Shuffle: on"
        } else {
            "Shuffle: off"
        });
        self.persist();
    }

    /// Nudge volume by `delta` (clamped 0..=100), scale future submissions, and
    /// persist. The change applies to the next pumped chunk.
    fn adjust_volume(&mut self, delta: i32) {
        let v = self.volume as i32 + delta;
        self.volume = v.clamp(0, VOL_MAX as i32) as u32;
        self.persist();
    }

    fn set_toast(&mut self, s: &str) {
        let n = s.as_bytes().len().min(64);
        self.toast[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.toast_len = n;
    }

    fn toast_str(&self) -> &str {
        core::str::from_utf8(&self.toast[..self.toast_len]).unwrap_or("")
    }

    /// Enumerate the Music directory and build the track list.
    fn scan(&mut self) {
        self.tracks.clear();
        let mut buf = [0u8; 8192];
        let mut dirbuf = [0u8; PATH_CAP];
        let dn = self.dir.as_str().as_bytes().len().min(PATH_CAP);
        dirbuf[..dn].copy_from_slice(&self.dir.as_str().as_bytes()[..dn]);
        let dir = core::str::from_utf8(&dirbuf[..dn]).unwrap_or("/");

        let count = raekit::sys::readdir_at(dir, &mut buf) as usize;
        let mut off = 0usize;
        for _ in 0..count {
            if off + 6 > buf.len() || self.tracks.len() >= MAX_TRACKS {
                break;
            }
            let name_len = u16::from_le_bytes([buf[off], buf[off + 1]]) as usize;
            off += 6;
            if off + name_len > buf.len() {
                break;
            }
            let raw = &buf[off..off + name_len];
            off += name_len;
            let name = core::str::from_utf8(raw).unwrap_or("");
            if !is_wav_name(name) {
                continue;
            }
            let mut nbuf = [0u8; NAME_CAP];
            let n = raw.len().min(NAME_CAP);
            nbuf[..n].copy_from_slice(&raw[..n]);
            self.tracks.push(Track {
                name: nbuf,
                name_len: n,
            });
        }
        if self.selected >= self.tracks.len() {
            self.selected = self.tracks.len().saturating_sub(1);
        }
    }

    /// Read a whole file under the Music dir into a heap buffer (capped).
    fn read_track_bytes(&self, idx: usize) -> Option<Vec<u8>> {
        let t = self.tracks.get(idx)?;
        let mut path = PathBuf::new();
        path.set(self.dir.as_str());
        path.push_component(t.name());
        let fd = raekit::sys::open(path.as_str(), 0);
        if fd == u64::MAX {
            return None;
        }
        let mut data: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            if data.len() > DECODE_CAP {
                let _ = raekit::sys::close(fd);
                return None;
            }
            let n = raekit::sys::read(fd, &mut chunk) as usize;
            if n == 0 || n > chunk.len() {
                break;
            }
            data.extend_from_slice(&chunk[..n]);
        }
        let _ = raekit::sys::close(fd);
        if data.is_empty() {
            None
        } else {
            Some(data)
        }
    }

    /// Decode `idx` and load mixer-ready PCM. Returns true on success. On a
    /// decode failure: skip the track + show an error, app stays alive.
    fn load_track(&mut self, idx: usize) -> bool {
        let bytes = match self.read_track_bytes(idx) {
            Some(b) => b,
            None => {
                self.set_toast("Can't open this track");
                return false;
            }
        };
        match decode_wav(&bytes) {
            Ok(dec) => {
                let pcm = prepare_for_mixer(&dec);
                if pcm.is_empty() {
                    self.set_toast("Empty / unsupported audio");
                    return false;
                }
                self.pcm = pcm;
                self.cursor_frames = 0;
                self.now_playing = idx;
                self.toast_len = 0;
                true
            }
            Err(_) => {
                self.set_toast("Can't decode this track (skipped)");
                false
            }
        }
    }

    fn total_frames(&self) -> usize {
        self.pcm.len() / 2
    }

    /// Toggle play/pause on the selected track. Loads on first play.
    fn play_pause(&mut self) {
        if self.tracks.is_empty() {
            return;
        }
        match self.state {
            Playback::Playing => self.state = Playback::Paused,
            Playback::Paused => {
                self.state = Playback::Playing;
            }
            Playback::Stopped => {
                if self.now_playing != self.selected || self.pcm.is_empty() {
                    if !self.load_track(self.selected) {
                        return;
                    }
                }
                self.state = Playback::Playing;
            }
        }
    }

    fn stop(&mut self) {
        self.state = Playback::Stopped;
        self.cursor_frames = 0;
    }

    /// Advance to the next/previous track and start it.
    fn nav(&mut self, delta: i32) {
        if self.tracks.is_empty() {
            return;
        }
        let n = self.tracks.len() as i32;
        let mut idx = self.selected as i32 + delta;
        if idx < 0 {
            idx = n - 1;
        }
        if idx >= n {
            idx = 0;
        }
        self.selected = idx as usize;
        self.ensure_visible();
        if self.load_track(self.selected) {
            self.state = Playback::Playing;
        }
        // Remember the now-selected track across launches.
        self.persist();
    }

    /// Seek by `delta_frames` (clamped). Re-aligns the submit cursor; already-
    /// queued audio drains then the new region streams.
    fn seek(&mut self, delta_frames: i64) {
        if self.pcm.is_empty() {
            return;
        }
        let total = self.total_frames() as i64;
        let mut c = self.cursor_frames as i64 + delta_frames;
        if c < 0 {
            c = 0;
        }
        if c > total {
            c = total;
        }
        self.cursor_frames = c as usize;
    }

    fn move_sel(&mut self, delta: i32) {
        if self.tracks.is_empty() {
            return;
        }
        let n = self.tracks.len() as i32;
        let mut idx = self.selected as i32 + delta;
        if idx < 0 {
            idx = 0;
        }
        if idx >= n {
            idx = n - 1;
        }
        self.selected = idx as usize;
        self.ensure_visible();
        // Remember the highlighted track across launches.
        self.persist();
    }

    fn visible_rows(&self) -> usize {
        let h = WIN_H - TITLE_H - TOOLBAR_H - TRANSPORT_H - STATUS_H;
        (h / ROW_H).max(1)
    }

    fn ensure_visible(&mut self) {
        let vis = self.visible_rows();
        if self.selected < self.scroll_row {
            self.scroll_row = self.selected;
        }
        if self.selected >= self.scroll_row + vis {
            self.scroll_row = self.selected + 1 - vis;
        }
    }

    /// Pump audio to the mixer for one event-loop tick. Submits up to a small
    /// budget of chunks (each ≤512 frames), paced by the mixer's accepted-frame
    /// return value: if the ring is full (accepted < submitted) we stop for this
    /// tick and retry the remainder next time — never overrun. Returns true if
    /// the play cursor moved (caller re-renders the progress bar).
    fn pump_audio(&mut self) -> bool {
        if self.state != Playback::Playing || self.pcm.is_empty() {
            return false;
        }
        let total = self.total_frames();
        if self.cursor_frames >= total {
            // Track finished. Honor repeat/shuffle:
            //  - Repeat::One  → replay the same track from the start.
            //  - shuffle      → jump to a random other track.
            //  - Repeat::All  → advance, wrapping last→first.
            //  - Repeat::Off  → advance if more tracks remain, else stop.
            self.advance_after_track_end();
            return true;
        }
        let mut moved = false;
        // Bound the per-tick work so the UI thread stays responsive: at most a
        // handful of 512-frame chunks before yielding back to the event loop.
        for _ in 0..8 {
            if self.cursor_frames >= total {
                break;
            }
            let remaining = total - self.cursor_frames;
            let chunk_frames = remaining.min(AUDIO_SUBMIT_MAX_FRAMES);
            let start = self.cursor_frames * 2;
            let end = start + chunk_frames * 2;
            // Apply the persisted volume (0..=100) by scaling each sample. 100 =
            // unity (submit the slice as-is, zero copy). Anything less builds a
            // scaled scratch buffer so the saved volume is audible.
            let accepted = if self.volume >= VOL_MAX {
                raekit::sys::audio_submit(&self.pcm[start..end]) as usize
            } else {
                let mut scratch: Vec<i16> = Vec::with_capacity(chunk_frames * 2);
                let vol = self.volume as i32;
                for &s in &self.pcm[start..end] {
                    scratch.push(((s as i32 * vol) / VOL_MAX as i32) as i16);
                }
                raekit::sys::audio_submit(&scratch) as usize
            };
            if accepted == 0 {
                // Ring full or audio unavailable: back off, retry next tick.
                break;
            }
            self.cursor_frames += accepted;
            moved = true;
            if accepted < chunk_frames {
                // Partial accept = ring is filling; yield for this tick.
                break;
            }
        }
        moved
    }

    /// Decide what plays when the current track reaches its end, per the persisted
    /// repeat + shuffle modes. Never persists (these modes were already saved when
    /// toggled; end-of-track is not a user change of preference).
    fn advance_after_track_end(&mut self) {
        if self.tracks.is_empty() {
            self.stop();
            return;
        }
        // Repeat-one: replay the same loaded track from the start.
        if self.repeat == Repeat::One && !self.pcm.is_empty() {
            self.cursor_frames = 0;
            self.state = Playback::Playing;
            return;
        }
        // Shuffle: pick a random OTHER track (or replay the only one).
        if self.shuffle && self.tracks.len() > 1 {
            let mut idx = self.rand_below(self.tracks.len());
            if idx == self.selected {
                idx = (idx + 1) % self.tracks.len();
            }
            self.selected = idx;
            self.ensure_visible();
            if self.load_track(self.selected) {
                self.state = Playback::Playing;
            }
            return;
        }
        // Sequential advance. Repeat::All wraps last→first; Repeat::Off stops at
        // the end of the list.
        if self.selected + 1 < self.tracks.len() {
            self.nav(1);
        } else if self.repeat == Repeat::All && self.tracks.len() > 1 {
            self.selected = 0;
            self.ensure_visible();
            if self.load_track(self.selected) {
                self.state = Playback::Playing;
            }
        } else {
            self.stop();
        }
    }

    /// The surface-local rect of visible track row `i`, or `None` if `i` is not
    /// currently scrolled into view. Uses the SAME metrics `render_list` draws.
    fn track_row_rect(&self, i: usize) -> Option<Rect> {
        let vis = self.visible_rows();
        let start = self.scroll_row;
        let end = (start + vis).min(self.tracks.len());
        if i < start || i >= end {
            return None;
        }
        let list_y = TITLE_H + TOOLBAR_H;
        let rel = i - start;
        Some(Rect {
            x: 0,
            y: list_y + rel * ROW_H,
            w: WIN_W,
            h: ROW_H,
        })
    }

    /// Hit-test a surface-local click. Returns the action of the topmost element
    /// containing the point (close button, transport button, seek bar, then a
    /// track row), or `Action::None` for an empty-space click. Pure: builds the
    /// SAME rects `render` draws.
    fn hit(&self, px: i32, py: i32) -> Action {
        if close_rect().contains(px, py) {
            return Action::Close;
        }
        // Toolbar preference controls (repeat / shuffle / volume).
        if repeat_ctl_rect().contains(px, py) {
            return Action::ToggleRepeat;
        }
        if shuffle_ctl_rect().contains(px, py) {
            return Action::ToggleShuffle;
        }
        if vol_down_rect().contains(px, py) {
            return Action::VolDown;
        }
        if vol_up_rect().contains(px, py) {
            return Action::VolUp;
        }
        // Transport buttons.
        let by = transport_btn_y();
        for (i, (_label, action)) in TRANSPORT_BTNS.iter().enumerate() {
            let r = Rect {
                x: transport_btn_x(i),
                y: by,
                w: TRANSPORT_BTN_W,
                h: TRANSPORT_BTN_H,
            };
            if r.contains(px, py) {
                return *action;
            }
        }
        // Seek bar: map click-x along the bar to a 0..=1000 permille position.
        let bar = seek_bar_rect();
        // Widen the clickable band vertically so the 8px bar is easy to hit.
        let band = Rect {
            x: bar.x,
            y: bar.y.saturating_sub(6),
            w: bar.w,
            h: bar.h + 12,
        };
        if band.contains(px, py) {
            let rel = (px - bar.x as i32).clamp(0, bar.w as i32) as u32;
            let permille = if bar.w == 0 {
                0
            } else {
                rel * 1000 / bar.w as u32
            };
            return Action::SeekPermille(permille);
        }
        // Track rows.
        for i in 0..self.tracks.len() {
            if let Some(r) = self.track_row_rect(i) {
                if r.contains(px, py) {
                    return Action::SelectTrack(i);
                }
            }
        }
        Action::None
    }

    /// Apply an `Action` (shared by click dispatch + the hit-test proof).
    /// Returns true if anything changed (so the caller re-renders). `Close`
    /// exits the process. Each branch mirrors the matching key exactly.
    fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::SelectTrack(i) => {
                if i >= self.tracks.len() {
                    return false;
                }
                // Click a row = select + play it from the start (like Enter).
                self.selected = i;
                self.ensure_visible();
                self.stop();
                if self.load_track(self.selected) {
                    self.state = Playback::Playing;
                }
                // Remember the newly-selected track across launches.
                self.persist();
                true
            }
            Action::PlayPause => {
                self.play_pause();
                true
            }
            Action::Next => {
                self.nav(1);
                true
            }
            Action::Prev => {
                self.nav(-1);
                true
            }
            Action::Stop => {
                self.stop();
                true
            }
            Action::SeekPermille(permille) => {
                let total = self.total_frames();
                if total == 0 {
                    return false;
                }
                let target = (total as u64 * permille as u64 / 1000) as usize;
                let delta = target as i64 - self.cursor_frames as i64;
                self.seek(delta);
                true
            }
            Action::ToggleRepeat => {
                self.cycle_repeat();
                true
            }
            Action::ToggleShuffle => {
                self.toggle_shuffle();
                true
            }
            Action::VolDown => {
                self.adjust_volume(-(VOL_STEP as i32));
                true
            }
            Action::VolUp => {
                self.adjust_volume(VOL_STEP as i32);
                true
            }
            Action::Close => raekit::sys::exit(0),
            Action::None => false,
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

fn render(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar
    canvas.fill_rect_gradient(0, 0, WIN_W, TITLE_H, DARK.bg_elevated, TITLE_BG);
    canvas.draw_text_aa(
        12,
        ((TITLE_H.saturating_sub(rae_tokens::TYPE_SUBTITLE.line_height as usize)) / 2) as i32,
        "Music",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.fill_rounded_rect(
        WIN_W - 28,
        4,
        20,
        20,
        rae_tokens::RADIUS_XS as usize,
        DARK.state_danger,
    );
    let x_w = canvas.measure_text_aa("X", rae_tokens::TYPE_LABEL, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 18) as i32 - x_w / 2,
        (4 + (20 - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
        "X",
        rae_tokens::TYPE_LABEL,
        0xFF_FF_FF_FF,
        FontFamily::Sans,
    );

    // Toolbar (library path)
    let tb_y = TITLE_H;
    canvas.fill_rect(0, tb_y, WIN_W, TOOLBAR_H, TOOLBAR_BG);
    let tb_ty = (tb_y
        + (TOOLBAR_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    let lbl_w = canvas.draw_text_aa(
        12,
        tb_ty,
        "Library:",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    // Clip the path so it can't run under the right-aligned preference controls.
    let path_x = lbl_w + 18;
    let path_max = repeat_ctl_rect().x.saturating_sub(path_x as usize + 12);
    draw_label_clipped(
        canvas,
        app.dir.as_str(),
        path_x as usize,
        tb_ty as usize,
        path_max,
        accent(),
    );
    render_toolbar_controls(app, canvas);

    render_list(app, canvas);
    render_transport(app, canvas);

    // Status bar
    let st_y = WIN_H - STATUS_H;
    canvas.fill_rect(0, st_y, WIN_W, STATUS_H, STATUS_BG);
    let st_ty = (st_y
        + (STATUS_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    if !app.toast_str().is_empty() {
        canvas.draw_text_aa(
            12,
            st_ty,
            app.toast_str(),
            rae_tokens::TYPE_CAPTION,
            DARK.state_danger,
            FontFamily::Sans,
        );
    } else {
        let mut buf = [0u8; 48];
        let n = fmt_count(app.tracks.len(), &mut buf);
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            canvas.draw_text_aa(
                12,
                st_ty,
                s,
                rae_tokens::TYPE_CAPTION,
                TEXT_MUTED,
                FontFamily::Sans,
            );
        }
    }
    let hint =
        "Space:play  Up/Dn:select  L/R:seek  N/P:next/prev  T:repeat  H:shuffle  [/]:vol  Esc:quit";
    let hw = canvas.measure_text_aa(hint, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 12) as i32 - hw,
        st_ty,
        hint,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
}

/// Draw the right-aligned toolbar preference controls (repeat / shuffle / volume)
/// from the SAME rects `App::hit` tests. Active toggles use the accent fill so the
/// persisted state is visible at a glance.
fn render_toolbar_controls(app: &App, canvas: &mut Canvas) {
    let draw_pill = |canvas: &mut Canvas, r: Rect, label: &str, active: bool| {
        let fill = if active { sel_fill() } else { DARK.bg_elevated };
        canvas.fill_rounded_rect(r.x, r.y, r.w, r.h, rae_tokens::RADIUS_XS as usize, fill);
        let lw = canvas.measure_text_aa(label, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
        let ly = (r.y + (r.h - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32;
        canvas.draw_text_aa(
            r.x as i32 + (r.w as i32 - lw) / 2,
            ly,
            label,
            rae_tokens::TYPE_CAPTION,
            if active { TEXT_FG } else { TEXT_MUTED },
            FontFamily::Sans,
        );
    };

    // Repeat (active when not Off).
    draw_pill(
        canvas,
        repeat_ctl_rect(),
        app.repeat.badge(),
        app.repeat != Repeat::Off,
    );
    // Shuffle.
    draw_pill(
        canvas,
        shuffle_ctl_rect(),
        if app.shuffle {
            "Shuffle: on"
        } else {
            "Shuffle: off"
        },
        app.shuffle,
    );
    // Volume - / readout / +.
    draw_pill(canvas, vol_down_rect(), "-", false);
    draw_pill(canvas, vol_up_rect(), "+", false);
    let vr = vol_label_rect();
    let mut buf = [0u8; 8];
    let mut n = fmt_u64(app.volume as u64, &mut buf);
    if n < buf.len() {
        buf[n] = b'%';
        n += 1;
    }
    if let Ok(s) = core::str::from_utf8(&buf[..n]) {
        let lw = canvas.measure_text_aa(s, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
        let ly = (vr.y + (vr.h - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32;
        canvas.draw_text_aa(
            vr.x as i32 + (vr.w as i32 - lw) / 2,
            ly,
            s,
            rae_tokens::TYPE_CAPTION,
            accent(),
            FontFamily::Sans,
        );
    }
}

fn render_list(app: &App, canvas: &mut Canvas) {
    let list_y = TITLE_H + TOOLBAR_H;
    let list_h = WIN_H - list_y - TRANSPORT_H - STATUS_H;
    canvas.fill_rect(0, list_y, WIN_W, list_h, LIST_BG);

    if app.tracks.is_empty() {
        canvas.draw_text_aa(
            24,
            (list_y + 24) as i32,
            "No .wav tracks in your Music library yet.",
            rae_tokens::TYPE_BODY,
            TEXT_MUTED,
            FontFamily::Sans,
        );
        return;
    }

    let vis = app.visible_rows();
    let start = app.scroll_row;
    let end = (start + vis).min(app.tracks.len());
    for i in start..end {
        let rel = i - start;
        let ry = list_y + rel * ROW_H;
        let selected = i == app.selected;
        let playing = i == app.now_playing && app.state != Playback::Stopped;

        if selected {
            canvas.fill_rect(0, ry, WIN_W, ROW_H, sel_fill());
        } else if rel % 2 == 1 {
            canvas.fill_rect(0, ry, WIN_W, ROW_H, ROW_ALT_BG);
        }

        // Play marker for the loaded track.
        let ty = (ry + (ROW_H - rae_tokens::TYPE_BODY.line_height as usize) / 2) as i32;
        let marker = match (playing, app.state) {
            (true, Playback::Playing) => ">",
            (true, Playback::Paused) => "=",
            _ => " ",
        };
        canvas.draw_text_aa(
            12,
            ty,
            marker,
            rae_tokens::TYPE_BODY,
            accent(),
            FontFamily::Sans,
        );

        let name = app.tracks[i].name();
        let fg = if selected { TEXT_FG } else { TEXT_MUTED };
        draw_label_clipped(canvas, name, 34, ry + (ROW_H - 14) / 2, WIN_W - 48, fg);
    }
    canvas.draw_rect_outline(0, list_y, WIN_W, list_h, STROKE_HL);
}

fn render_transport(app: &App, canvas: &mut Canvas) {
    let tr_y = WIN_H - STATUS_H - TRANSPORT_H;
    canvas.fill_rect(0, tr_y, WIN_W, TRANSPORT_H, TRANSPORT_BG);
    canvas.fill_rect(0, tr_y, WIN_W, 1, STROKE_HL);

    // Now-playing title.
    let title = if app.now_playing < app.tracks.len() && app.state != Playback::Stopped {
        app.tracks[app.now_playing].name()
    } else {
        "—"
    };
    canvas.draw_text_aa(
        16,
        (tr_y + 10) as i32,
        title,
        rae_tokens::TYPE_BODY,
        TEXT_FG,
        FontFamily::Sans,
    );

    // Transport buttons (clickable; same rects as `App::hit`). The Play button
    // shows "Pause" while playing so the affordance matches the action.
    let by = transport_btn_y();
    for (i, (label, action)) in TRANSPORT_BTNS.iter().enumerate() {
        let bx = transport_btn_x(i);
        let shown = if matches!(action, Action::PlayPause) && app.state == Playback::Playing {
            "Pause"
        } else {
            *label
        };
        let active = matches!(action, Action::PlayPause) && app.state == Playback::Playing;
        let fill = if active { sel_fill() } else { DARK.bg_elevated };
        canvas.fill_rounded_rect(
            bx,
            by,
            TRANSPORT_BTN_W,
            TRANSPORT_BTN_H,
            rae_tokens::RADIUS_XS as usize,
            fill,
        );
        let lw = canvas.measure_text_aa(shown, rae_tokens::TYPE_LABEL, FontFamily::Sans);
        let ly = (by + (TRANSPORT_BTN_H - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32;
        canvas.draw_text_aa(
            bx as i32 + (TRANSPORT_BTN_W as i32 - lw) / 2,
            ly,
            shown,
            rae_tokens::TYPE_LABEL,
            if active { TEXT_FG } else { TEXT_MUTED },
            FontFamily::Sans,
        );
    }

    // State label (left of the button cluster).
    let state_lbl = match app.state {
        Playback::Playing => "Playing",
        Playback::Paused => "Paused",
        Playback::Stopped => "Stopped",
    };
    let sw = canvas.measure_text_aa(state_lbl, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (transport_btn_x(0) as i32 - 12) - sw,
        (tr_y + 12) as i32,
        state_lbl,
        rae_tokens::TYPE_CAPTION,
        accent(),
        FontFamily::Sans,
    );

    // Progress bar (seek indicator).
    let bar_x = 16usize;
    let bar_y = tr_y + 40;
    let bar_w = WIN_W - 32;
    let bar_h = 8usize;
    canvas.fill_rounded_rect(bar_x, bar_y, bar_w, bar_h, 4, TRACK_BG);
    let total = app.total_frames();
    if total > 0 {
        let filled = (app.cursor_frames.min(total) * bar_w) / total;
        if filled > 0 {
            canvas.fill_rounded_rect(bar_x, bar_y, filled, bar_h, 4, accent());
        }
        // Playhead time mm:ss / mm:ss at 48 kHz.
        let mut buf = [0u8; 32];
        let n = fmt_time(app.cursor_frames, total, &mut buf);
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            canvas.draw_text_aa(
                bar_x as i32,
                (bar_y + bar_h + 4) as i32,
                s,
                rae_tokens::TYPE_CAPTION,
                TEXT_MUTED,
                FontFamily::Sans,
            );
        }
    }
}

/// Draw a name clipped to `max_w` pixels (ellipsis when it overflows).
fn draw_label_clipped(canvas: &mut Canvas, name: &str, x: usize, y: usize, max_w: usize, fg: u32) {
    let full_w = canvas.measure_text_aa(name, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    if (full_w as usize) <= max_w {
        canvas.draw_text_aa(
            x as i32,
            y as i32,
            name,
            rae_tokens::TYPE_CAPTION,
            fg,
            FontFamily::Sans,
        );
        return;
    }
    let bytes = name.as_bytes();
    let mut take = bytes.len();
    while take > 0 {
        let mut buf = [0u8; NAME_CAP + 2];
        let t = take.min(NAME_CAP);
        buf[..t].copy_from_slice(&bytes[..t]);
        buf[t] = b'.';
        buf[t + 1] = b'.';
        if let Ok(s) = core::str::from_utf8(&buf[..t + 2]) {
            let w = canvas.measure_text_aa(s, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
            if (w as usize) <= max_w {
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    s,
                    rae_tokens::TYPE_CAPTION,
                    fg,
                    FontFamily::Sans,
                );
                return;
            }
        }
        take -= 1;
    }
}

fn fmt_count(n: usize, out: &mut [u8]) -> usize {
    let mut len = fmt_u64(n as u64, out);
    let suffix: &[u8] = if n == 1 { b" track" } else { b" tracks" };
    for &b in suffix {
        out[len] = b;
        len += 1;
    }
    len
}

/// Format "m:ss / m:ss" from frame positions at 48 kHz.
fn fmt_time(cur_frames: usize, total_frames: usize, out: &mut [u8]) -> usize {
    let cur_s = cur_frames / MIX_RATE as usize;
    let tot_s = total_frames / MIX_RATE as usize;
    let mut n = fmt_mmss(cur_s, out);
    for &b in b" / " {
        out[n] = b;
        n += 1;
    }
    n += fmt_mmss(tot_s, &mut out[n..]);
    n
}

fn fmt_mmss(secs: usize, out: &mut [u8]) -> usize {
    let m = secs / 60;
    let s = secs % 60;
    let mut n = fmt_u64(m as u64, out);
    out[n] = b':';
    n += 1;
    if s < 10 {
        out[n] = b'0';
        n += 1;
    }
    n += fmt_u64(s as u64, &mut out[n..]);
    n
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

// ── Design proof (R10: a fail-able check the token wiring + audio path are right) ─

/// True iff Music's chrome is wired to the shared design tokens, the audio
/// pipeline (decode → mono→stereo upmix → 48 kHz resample) produces the expected
/// frames, the mouse hit-test invariant holds, AND the persistent-preferences
/// schema round-trips through `rae_toml` (volume/repeat/shuffle/last-track) while
/// a corrupt/missing config resolves to the typed defaults (never a panic).
/// Deliberately fail-able: a regression in token wiring, the WAV decoder, the
/// upmix, the resample math, the prefs schema, or the defaulting flips this to
/// `false` (exit code 3 at startup). `raemedia`'s WAV host KATs prove the decode
/// logic and `rae_toml`'s KATs prove the parser; this proves Music's own wiring.
#[must_use]
pub fn design_proof() -> bool {
    let ramp = rae_tokens::derive_accent(theme_seed(), &DARK);
    let tokens_ok = accent() == ramp.base
        && sel_fill() == ramp.active
        && BG == DARK.bg_raised
        && TITLE_BG == DARK.bg_base
        && TOOLBAR_BG == DARK.bg_overlay
        && TEXT_FG == DARK.text_primary
        && TEXT_MUTED == DARK.text_secondary
        && STROKE_HL == DARK.stroke_strong
        && raekit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;

    let audio_ok = audio_pipeline_ok();

    tokens_ok && audio_ok && hit_test_proof()
}

/// Prove the mouse hit-test invariant: a click on a known element's rect-center
/// resolves to that element's action (the SAME rects `render` draws), and an
/// out-of-bounds click resolves to `Action::None`. Returns `false` on any drift
/// (→ exit(3) at startup). Builds a synthetic 3-track app so row geometry exists
/// without touching the real Music directory.
#[must_use]
fn hit_test_proof() -> bool {
    let mut app = App {
        dir: PathBuf::new(),
        tracks: Vec::new(),
        selected: 0,
        scroll_row: 0,
        state: Playback::Stopped,
        pcm: Vec::new(),
        cursor_frames: 0,
        now_playing: usize::MAX,
        toast: [0; 64],
        toast_len: 0,
        volume: VOL_DEFAULT,
        repeat: Repeat::Off,
        shuffle: false,
        rng: 0x1234_5678_9ABC_DEF0,
    };
    for label in [b"a.wav".as_slice(), b"b.wav", b"c.wav"] {
        let mut name = [0u8; NAME_CAP];
        name[..label.len()].copy_from_slice(label);
        app.tracks.push(Track {
            name,
            name_len: label.len(),
        });
    }

    // (1) A click at track-row 1's center hits SelectTrack(1).
    let r1 = match app.track_row_rect(1) {
        Some(r) => r,
        None => return false,
    };
    let cx = (r1.x + r1.w / 2) as i32;
    let cy = (r1.y + r1.h / 2) as i32;
    if app.hit(cx, cy) != Action::SelectTrack(1) {
        return false;
    }

    // (2) Each transport button's center hits its own action.
    let by = transport_btn_y();
    for (i, (_label, action)) in TRANSPORT_BTNS.iter().enumerate() {
        let bx = transport_btn_x(i) + TRANSPORT_BTN_W / 2;
        let byc = (by + TRANSPORT_BTN_H / 2) as i32;
        if app.hit(bx as i32, byc) != *action {
            return false;
        }
    }

    // (3) The Play button specifically maps to PlayPause.
    let play_idx = TRANSPORT_BTNS
        .iter()
        .position(|(_, a)| *a == Action::PlayPause);
    let Some(pi) = play_idx else { return false };
    let pbx = (transport_btn_x(pi) + TRANSPORT_BTN_W / 2) as i32;
    let pby = (by + TRANSPORT_BTN_H / 2) as i32;
    if app.hit(pbx, pby) != Action::PlayPause {
        return false;
    }

    // (4) A click at the seek bar's far-left maps near permille 0, far-right
    // near 1000 (monotone left→right mapping along the bar).
    let bar = seek_bar_rect();
    let lefty = (bar.y + bar.h / 2) as i32;
    let left = app.hit(bar.x as i32, lefty);
    let right = app.hit((bar.x + bar.w - 1) as i32, lefty);
    match (left, right) {
        (Action::SeekPermille(l), Action::SeekPermille(r)) => {
            if l > 20 || r < 980 {
                return false;
            }
        }
        _ => return false,
    }

    // (5) An out-of-bounds click resolves to None.
    if app.hit(-100, -100) != Action::None {
        return false;
    }
    if app.hit(WIN_W as i32 + 50, WIN_H as i32 + 50) != Action::None {
        return false;
    }

    // (6) The toolbar preference controls hit their own actions (the SAME rects
    // `render_toolbar_controls` draws).
    let rc = repeat_ctl_rect();
    if app.hit((rc.x + rc.w / 2) as i32, (rc.y + rc.h / 2) as i32) != Action::ToggleRepeat {
        return false;
    }
    let sc = shuffle_ctl_rect();
    if app.hit((sc.x + sc.w / 2) as i32, (sc.y + sc.h / 2) as i32) != Action::ToggleShuffle {
        return false;
    }
    let vd = vol_down_rect();
    if app.hit((vd.x + vd.w / 2) as i32, (vd.y + vd.h / 2) as i32) != Action::VolDown {
        return false;
    }
    let vu = vol_up_rect();
    if app.hit((vu.x + vu.w / 2) as i32, (vu.y + vu.h / 2) as i32) != Action::VolUp {
        return false;
    }

    // (7) Repeat/shuffle/volume STATE transitions (pure, no disk I/O). These
    // mirror what the dispatch arms do, without `persist()`'s file side effect.
    app.repeat = Repeat::Off;
    app.repeat = app.repeat.next();
    if app.repeat != Repeat::One {
        return false;
    }
    let was = app.shuffle;
    app.shuffle = !app.shuffle;
    if app.shuffle == was {
        return false;
    }
    app.volume = 50;
    let v = (app.volume as i32 + VOL_STEP as i32).clamp(0, VOL_MAX as i32) as u32;
    if v != 55 {
        return false;
    }

    prefs_round_trip_ok()
}

/// Prove the Music PREFS SCHEMA: a known `Prefs` serialized via `rae_toml` then
/// re-parsed restores every field exactly, AND a corrupt / missing-key document
/// resolves to the typed defaults (NOT a panic, NOT a wrong value). This proves
/// the per-app prefs contract on top of `rae_toml`'s own parser KATs. Returns
/// `false` on any drift (→ exit(3) at startup).
#[must_use]
fn prefs_round_trip_ok() -> bool {
    // (a) Full round-trip of a non-default Prefs.
    let p = Prefs {
        volume: 37,
        repeat: Repeat::All,
        shuffle: true,
        last_track: String::from("song two.wav"),
        last_folder: String::from("/home/rae/Music"),
    };
    let text = rae_toml::to_string(&p.to_toml());
    let parsed = match rae_toml::parse(&text) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let back = Prefs::from_toml(&parsed);
    if back.volume != 37
        || back.repeat != Repeat::All
        || !back.shuffle
        || back.last_track != "song two.wav"
        || back.last_folder != "/home/rae/Music"
    {
        return false;
    }

    // (b) Repeat token round-trips through its stable string form.
    for r in [Repeat::Off, Repeat::One, Repeat::All] {
        if Repeat::from_token(r.as_token()) != r {
            return false;
        }
    }

    // (c) A corrupt document → typed defaults (parse FAILS, we don't panic).
    let corrupt = "volume = = oops\n[unterminated\n";
    let d = match rae_toml::parse(corrupt) {
        Ok(t) => Prefs::from_toml(&t), // shouldn't reach here for this input
        Err(_) => Prefs::defaults(),
    };
    if d.volume != VOL_DEFAULT || d.repeat != Repeat::Off || d.shuffle {
        return false;
    }

    // (d) A well-formed doc MISSING every prefs key → typed defaults per field.
    let empty = match rae_toml::parse("unrelated = 1\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let e = Prefs::from_toml(&empty);
    if e.volume != VOL_DEFAULT
        || e.repeat != Repeat::Off
        || e.shuffle
        || !e.last_track.is_empty()
        || !e.last_folder.is_empty()
    {
        return false;
    }

    // (e) An out-of-range volume is CLAMPED, not rejected (stays usable).
    let oor = match rae_toml::parse("volume = 9999\nrepeat = \"bogus\"\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let c = Prefs::from_toml(&oor);
    if c.volume != VOL_MAX || c.repeat != Repeat::Off {
        return false;
    }

    true
}

/// Build a tiny 24 kHz MONO 16-bit WAV with known samples, decode it through
/// `raemedia::wav`, then assert:
///   (1) the decoded samples are exact (mono, 24 kHz),
///   (2) `prepare_for_mixer` upmixes mono→stereo (L==R per frame) AND resamples
///       24 kHz → 48 kHz (output frame count = 2x input, anchors preserved).
/// Returns `false` on any drift. Self-contained (no fixture file): the WAV is
/// assembled in-line, the construction the decoder's host KATs use.
fn audio_pipeline_ok() -> bool {
    // 4 mono frames at 24 kHz: 100, 200, 300, 400.
    let samples: [i16; 4] = [100, 200, 300, 400];
    let wav = build_mono_wav(24_000, &samples);

    let dec = match decode_wav(&wav) {
        Ok(d) => d,
        Err(_) => return false,
    };
    if dec.sample_rate != 24_000 || dec.channels != 1 {
        return false;
    }
    if dec.samples.as_slice() != &samples[..] {
        return false;
    }

    // prepare_for_mixer: 24k → 48k doubles the frame count; mono → stereo.
    let pcm = prepare_for_mixer(&dec);
    let out_frames = pcm.len() / 2;
    // out_frames = in_frames * 48000 / 24000 = 4 * 2 = 8.
    if out_frames != 8 {
        return false;
    }
    // Stereo invariant: L == R for every frame (mono upmix).
    for f in 0..out_frames {
        if pcm[f * 2] != pcm[f * 2 + 1] {
            return false;
        }
    }
    // Anchor: frame 0 must equal source sample 0 (start of stream).
    if pcm[0] != 100 {
        return false;
    }
    // Even output frames land exactly on source samples (frac == 0): frame 2 ->
    // source 1 (200), frame 4 -> source 2 (300), frame 6 -> source 3 (400).
    if pcm[2 * 2] != 200 || pcm[4 * 2] != 300 || pcm[6 * 2] != 400 {
        return false;
    }
    // Odd output frame 1 is the linear midpoint of sources 0 and 1: (100+200)/2.
    if pcm[1 * 2] != 150 {
        return false;
    }
    true
}

/// Assemble a mono 16-bit PCM WAV (RIFF/WAVE + fmt + data) from i16 samples.
/// Mirrors the decoder's own test writer; used only by `audio_pipeline_ok`.
fn build_mono_wav(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
    let channels: u16 = 1;
    let bits: u16 = 16;
    let block_align = channels * (bits / 8);
    let byte_rate = sample_rate * block_align as u32;
    let mut body: Vec<u8> = Vec::new();
    for s in samples {
        body.extend_from_slice(&s.to_le_bytes());
    }
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"RIFF");
    let riff_size = (4 + (8 + 16) + (8 + body.len())) as u32;
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

// ── Entry point ───────────────────────────────────────────────────────────

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
    render(&app, &mut canvas);
    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut extended = false;
    // Live left-button state for click-EDGE detection (fire on up->down).
    let mut left_was_down = false;

    loop {
        let key = raekit::sys::read_key();
        let mut dirty = false;

        // ── Mouse: drain button events, hit-test the cursor on a click edge ──
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
                if app.dispatch(app.hit(lx, ly)) {
                    dirty = true;
                }
            }
            left_was_down = left_down;
        }

        if key != 0 {
            let sc = key as u8;
            if sc == 0xE0 {
                extended = true;
                // Wait for the next byte (the actual code).
                continue;
            }
            let ext = core::mem::replace(&mut extended, false);
            let release = sc & 0x80 != 0;
            let code = sc & 0x7F;
            if !release {
                match (ext, code) {
                    (true, 0x48) => {
                        app.move_sel(-1);
                        dirty = true;
                    } // Up
                    (true, 0x50) => {
                        app.move_sel(1);
                        dirty = true;
                    } // Down
                    (true, 0x4B) => {
                        // Left = seek back ~2s (96000 frames @ 48k).
                        app.seek(-(2 * MIX_RATE as i64));
                        dirty = true;
                    }
                    (true, 0x4D) => {
                        // Right = seek forward ~2s.
                        app.seek(2 * MIX_RATE as i64);
                        dirty = true;
                    }
                    (false, 0x39) => {
                        app.play_pause();
                        dirty = true;
                    } // Space = play/pause
                    (false, 0x1C) => {
                        // Enter = play the selected track from the start.
                        app.stop();
                        if app.load_track(app.selected) {
                            app.state = Playback::Playing;
                        }
                        dirty = true;
                    }
                    (false, 0x31) => {
                        app.nav(1);
                        dirty = true;
                    } // 'n' = next
                    (false, 0x19) => {
                        app.nav(-1);
                        dirty = true;
                    } // 'p' = prev
                    (false, 0x1F) => {
                        app.stop();
                        dirty = true;
                    } // 's' = stop
                    (false, 0x13) => {
                        app.scan();
                        dirty = true;
                    } // 'r' = rescan
                    (false, 0x14) => {
                        app.cycle_repeat();
                        dirty = true;
                    } // 't' = cycle repeat mode
                    (false, 0x23) => {
                        app.toggle_shuffle();
                        dirty = true;
                    } // 'h' = toggle shuffle
                    (false, 0x1A) => {
                        app.adjust_volume(-(VOL_STEP as i32));
                        dirty = true;
                    } // '[' = volume down
                    (false, 0x1B) => {
                        app.adjust_volume(VOL_STEP as i32);
                        dirty = true;
                    } // ']' = volume up
                    (false, 0x01) => {
                        raekit::sys::exit(0);
                    } // Esc = quit
                    _ => {}
                }
            }
        }

        // Pump audio every tick (the heart of playback pacing).
        if app.pump_audio() {
            dirty = true;
        }

        if dirty {
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }

        // Yield when idle so we don't spin a core; the audio pump rate is fine at
        // event-loop cadence because each pump submits up to 8*512 frames.
        if key == 0 {
            raekit::sys::yield_now();
        }
    }
}
