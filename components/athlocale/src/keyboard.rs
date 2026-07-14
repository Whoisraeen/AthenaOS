//! Keyboard-layout data + pure lookup API for AthenaOS.
//!
//! # Concept alignment
//! `LEGACY_GAMING_CONCEPT.md` calls for an OS that can "rival Windows + macOS"
//! globally — those ship dozens of keyboard layouts so a French or German
//! user can actually type. Today AthenaOS's scancode→char mapping is
//! hardcoded US-QWERTY (`kernel/src/shell_runner.rs::lock_scancode_to_ascii`),
//! so any non-US switcher types garbage. This module is the locale-owned
//! *data + lookup* half of the fix (parity gap #5): a registry of layouts and
//! a never-panic `(layout, scancode, modifiers) -> char` resolver. The
//! kernel-side wiring that consumes `bridge_scancode` is a deferred,
//! thin "consume-the-table" change in a later kernel slot.
//!
//! # Scancode representation
//! Keys are **IBM PC/XT "Set 1" make codes** (the classic 0x00..=0x58 scancode
//! set), NOT USB HID usage IDs. This deliberately matches the kernel input
//! path: `kernel/src/usb_hid.rs` translates raw HID usages to Set 1 via
//! `hid_usage_to_set1()` *before* calling `bridge_scancode(scancode)`, and the
//! existing US-QWERTY consumer `lock_scancode_to_ascii(code, shift)` indexes a
//! 58-entry Set 1 table. So a future kernel slot can swap that hardcoded array
//! for `KEYBOARD_LAYOUTS.resolve(active_id, scancode, mods)` with no change to
//! what value is fed in. Break codes (the 0x80 release bit) are the caller's
//! concern; this API only maps make codes.
//!
//! # Scope of this slice
//! Base (unshifted) + shift planes for the rows that actually diverge across
//! Latin layouts: the number row, the three letter rows, and the common
//! punctuation keys. AltGr / dead-key composition (é, ñ, €, the AZERTY `²`
//! AltGr plane) is a documented follow-up — see `LayoutPlanes` doc.

#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::vec::Vec;

/// A resolved keysym. We only need printable characters for this slice;
/// non-printing keys (Esc, Enter, Tab, Backspace) resolve to their ASCII
/// control codes exactly like the legacy US table, so a drop-in swap is exact.
pub type KeySym = char;

/// Modifier state relevant to layout resolution. Only Shift selects between
/// the two planes this slice ships; AltGr is carried for the future plane and
/// currently ignored by `resolve` (documented follow-up).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub caps_lock: bool,
    /// Right-Alt / AltGr. Reserved for the AltGr plane follow-up; ignored now.
    pub altgr: bool,
}

impl Modifiers {
    pub const fn none() -> Self {
        Self {
            shift: false,
            caps_lock: false,
            altgr: false,
        }
    }
    pub const fn shift() -> Self {
        Self {
            shift: true,
            caps_lock: false,
            altgr: false,
        }
    }
    /// Effective shift for *letter* keys: Shift XOR CapsLock.
    /// Caps lock only affects alphabetic keys, so callers must decide; this
    /// helper exists for the kernel consumer to compute the letter plane.
    pub const fn letter_shift(self) -> bool {
        self.shift ^ self.caps_lock
    }
}

/// The two character planes for a single physical key (a Set 1 scancode).
/// `base` = unshifted, `shift` = shifted. `None` means "this layout does not
/// produce a printable char for this scancode" (e.g. a key the layout leaves
/// dead or that is non-printing in this layout).
///
/// AltGr follow-up: add `altgr: Option<KeySym>` + `shift_altgr` here and a
/// matching arm in `resolve`; the table format already leaves room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPlanes {
    pub scancode: u8,
    pub base: Option<KeySym>,
    pub shift: Option<KeySym>,
    /// True for alphabetic keys, where CapsLock participates in plane choice.
    pub is_letter: bool,
}

impl KeyPlanes {
    const fn letter(scancode: u8, base: char, shift: char) -> Self {
        Self {
            scancode,
            base: Some(base),
            shift: Some(shift),
            is_letter: true,
        }
    }
    const fn sym(scancode: u8, base: char, shift: char) -> Self {
        Self {
            scancode,
            base: Some(base),
            shift: Some(shift),
            is_letter: false,
        }
    }
    /// A key whose base and shift are the same non-letter char (e.g. space).
    const fn same(scancode: u8, ch: char) -> Self {
        Self {
            scancode,
            base: Some(ch),
            shift: Some(ch),
            is_letter: false,
        }
    }
}

/// Stable identifier for a layout. Strings keep the registry open-ended and
/// match how a settings UI / locale would name a layout ("fr", "de", "dvorak").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutId {
    UsQwerty,
    FrenchAzerty,
    GermanQwertz,
    Dvorak,
}

impl LayoutId {
    /// Short machine id (matches a settings selector / xkb-style short name).
    pub const fn short_name(self) -> &'static str {
        match self {
            LayoutId::UsQwerty => "us",
            LayoutId::FrenchAzerty => "fr",
            LayoutId::GermanQwertz => "de",
            LayoutId::Dvorak => "dvorak",
        }
    }
    /// Human-readable name for a UI list.
    pub const fn display_name(self) -> &'static str {
        match self {
            LayoutId::UsQwerty => "English (US, QWERTY)",
            LayoutId::FrenchAzerty => "French (AZERTY)",
            LayoutId::GermanQwertz => "German (QWERTZ)",
            LayoutId::Dvorak => "English (Dvorak)",
        }
    }
    /// Resolve a layout from its short id/name, case-insensitive on ASCII.
    pub fn from_name(name: &str) -> Option<LayoutId> {
        // Manual ASCII-lowercase compare to stay no_std + alloc-light.
        const ALL: [LayoutId; 4] = [
            LayoutId::UsQwerty,
            LayoutId::FrenchAzerty,
            LayoutId::GermanQwertz,
            LayoutId::Dvorak,
        ];
        for id in ALL {
            if ascii_eq_ignore_case(name, id.short_name()) {
                return Some(id);
            }
        }
        None
    }
}

fn ascii_eq_ignore_case(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .all(|(x, y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

/// A complete layout: an id plus its key planes. Lookup is a linear scan over
/// a small fixed table (≤ ~50 keys) — cheap, allocation-free, no_std.
#[derive(Debug, Clone, Copy)]
pub struct KeyboardLayout {
    pub id: LayoutId,
    keys: &'static [KeyPlanes],
}

impl KeyboardLayout {
    /// Resolve `(scancode, modifiers) -> char`. Never panics; returns `None`
    /// for any scancode this layout does not map.
    pub fn resolve(&self, scancode: u8, mods: Modifiers) -> Option<KeySym> {
        let key = self.keys.iter().find(|k| k.scancode == scancode)?;
        let use_shift = if key.is_letter {
            mods.letter_shift()
        } else {
            mods.shift
        };
        if use_shift {
            key.shift
        } else {
            key.base
        }
    }

    /// All scancodes this layout maps (for diagnostics / coverage tests).
    pub fn scancodes(&self) -> impl Iterator<Item = u8> + '_ {
        self.keys.iter().map(|k| k.scancode)
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Layout tables. All keys are Set 1 make codes. The number row + the three
// letter rows + the common punctuation keys are covered — the keys that
// actually differ between these Latin layouts. Non-printing keys (Esc 0x01,
// Backspace 0x0E, Tab 0x0F, Enter 0x1C) and Space (0x39) are included so the
// resolver is a faithful drop-in for the legacy 58-entry table.
// ───────────────────────────────────────────────────────────────────────────

// Shared non-printing / structural keys identical across all Latin layouts.
const ESC: KeyPlanes = KeyPlanes::same(0x01, '\u{1b}');
const BKSP: KeyPlanes = KeyPlanes::same(0x0e, '\u{8}');
const TAB: KeyPlanes = KeyPlanes::same(0x0f, '\t');
const ENTER: KeyPlanes = KeyPlanes::same(0x1c, '\n');
const SPACE: KeyPlanes = KeyPlanes::same(0x39, ' ');

/// US QWERTY — the reference. Mirrors `lock_scancode_to_ascii`'s unshifted
/// plane exactly, and adds the correct shift plane (which the legacy code
/// never had).
const US_QWERTY: &[KeyPlanes] = &[
    ESC,
    // number row 0x02..=0x0d
    KeyPlanes::sym(0x02, '1', '!'),
    KeyPlanes::sym(0x03, '2', '@'),
    KeyPlanes::sym(0x04, '3', '#'),
    KeyPlanes::sym(0x05, '4', '$'),
    KeyPlanes::sym(0x06, '5', '%'),
    KeyPlanes::sym(0x07, '6', '^'),
    KeyPlanes::sym(0x08, '7', '&'),
    KeyPlanes::sym(0x09, '8', '*'),
    KeyPlanes::sym(0x0a, '9', '('),
    KeyPlanes::sym(0x0b, '0', ')'),
    KeyPlanes::sym(0x0c, '-', '_'),
    KeyPlanes::sym(0x0d, '=', '+'),
    BKSP,
    TAB,
    // top letter row 0x10..=0x1b
    KeyPlanes::letter(0x10, 'q', 'Q'),
    KeyPlanes::letter(0x11, 'w', 'W'),
    KeyPlanes::letter(0x12, 'e', 'E'),
    KeyPlanes::letter(0x13, 'r', 'R'),
    KeyPlanes::letter(0x14, 't', 'T'),
    KeyPlanes::letter(0x15, 'y', 'Y'),
    KeyPlanes::letter(0x16, 'u', 'U'),
    KeyPlanes::letter(0x17, 'i', 'I'),
    KeyPlanes::letter(0x18, 'o', 'O'),
    KeyPlanes::letter(0x19, 'p', 'P'),
    KeyPlanes::sym(0x1a, '[', '{'),
    KeyPlanes::sym(0x1b, ']', '}'),
    ENTER,
    // home letter row 0x1e..=0x28 (+0x29 backtick)
    KeyPlanes::letter(0x1e, 'a', 'A'),
    KeyPlanes::letter(0x1f, 's', 'S'),
    KeyPlanes::letter(0x20, 'd', 'D'),
    KeyPlanes::letter(0x21, 'f', 'F'),
    KeyPlanes::letter(0x22, 'g', 'G'),
    KeyPlanes::letter(0x23, 'h', 'H'),
    KeyPlanes::letter(0x24, 'j', 'J'),
    KeyPlanes::letter(0x25, 'k', 'K'),
    KeyPlanes::letter(0x26, 'l', 'L'),
    KeyPlanes::sym(0x27, ';', ':'),
    KeyPlanes::sym(0x28, '\'', '"'),
    KeyPlanes::sym(0x29, '`', '~'),
    KeyPlanes::sym(0x2b, '\\', '|'),
    // bottom letter row 0x2c..=0x35
    KeyPlanes::letter(0x2c, 'z', 'Z'),
    KeyPlanes::letter(0x2d, 'x', 'X'),
    KeyPlanes::letter(0x2e, 'c', 'C'),
    KeyPlanes::letter(0x2f, 'v', 'V'),
    KeyPlanes::letter(0x30, 'b', 'B'),
    KeyPlanes::letter(0x31, 'n', 'N'),
    KeyPlanes::letter(0x32, 'm', 'M'),
    KeyPlanes::sym(0x33, ',', '<'),
    KeyPlanes::sym(0x34, '.', '>'),
    KeyPlanes::sym(0x35, '/', '?'),
    SPACE,
];

/// French AZERTY. Physical-key reassignments vs QWERTY:
///   - 0x10 (QWERTY 'q') -> 'a'
///   - 0x1e (QWERTY 'a') -> 'q'
///   - 0x15 (QWERTY 'y') -> 'y' (unchanged) ; 0x2c (QWERTY 'z') -> 'w'
///   - 0x11 (QWERTY 'w') -> 'z'
///   - 0x32 (QWERTY 'm') -> ',' ; 0x33 (QWERTY ',') -> ';' ; 0x27 -> 'm'
/// Number row is shifted-by-default to digits (the unshifted plane is the
/// accented/symbol row); base letters of digit keys per the standard fr layout.
const FRENCH_AZERTY: &[KeyPlanes] = &[
    ESC,
    // number row: base = symbol/accent, shift = digit (standard AZERTY)
    KeyPlanes::sym(0x02, '&', '1'),
    KeyPlanes::sym(0x03, 'é', '2'),
    KeyPlanes::sym(0x04, '"', '3'),
    KeyPlanes::sym(0x05, '\'', '4'),
    KeyPlanes::sym(0x06, '(', '5'),
    KeyPlanes::sym(0x07, '-', '6'),
    KeyPlanes::sym(0x08, 'è', '7'),
    KeyPlanes::sym(0x09, '_', '8'),
    KeyPlanes::sym(0x0a, 'ç', '9'),
    KeyPlanes::sym(0x0b, 'à', '0'),
    KeyPlanes::sym(0x0c, ')', '°'),
    KeyPlanes::sym(0x0d, '=', '+'),
    BKSP,
    TAB,
    // top row: AZERTY -> a z e r t y u i o p
    KeyPlanes::letter(0x10, 'a', 'A'),
    KeyPlanes::letter(0x11, 'z', 'Z'),
    KeyPlanes::letter(0x12, 'e', 'E'),
    KeyPlanes::letter(0x13, 'r', 'R'),
    KeyPlanes::letter(0x14, 't', 'T'),
    KeyPlanes::letter(0x15, 'y', 'Y'),
    KeyPlanes::letter(0x16, 'u', 'U'),
    KeyPlanes::letter(0x17, 'i', 'I'),
    KeyPlanes::letter(0x18, 'o', 'O'),
    KeyPlanes::letter(0x19, 'p', 'P'),
    KeyPlanes::sym(0x1a, '^', '¨'),
    KeyPlanes::sym(0x1b, '$', '£'),
    ENTER,
    // home row: q s d f g h j k l m
    KeyPlanes::letter(0x1e, 'q', 'Q'),
    KeyPlanes::letter(0x1f, 's', 'S'),
    KeyPlanes::letter(0x20, 'd', 'D'),
    KeyPlanes::letter(0x21, 'f', 'F'),
    KeyPlanes::letter(0x22, 'g', 'G'),
    KeyPlanes::letter(0x23, 'h', 'H'),
    KeyPlanes::letter(0x24, 'j', 'J'),
    KeyPlanes::letter(0x25, 'k', 'K'),
    KeyPlanes::letter(0x26, 'l', 'L'),
    KeyPlanes::letter(0x27, 'm', 'M'),
    KeyPlanes::sym(0x28, 'ù', '%'),
    KeyPlanes::sym(0x29, '²', '~'),
    KeyPlanes::sym(0x2b, '*', 'µ'),
    // bottom row: w x c v b n , ; : !
    KeyPlanes::letter(0x2c, 'w', 'W'),
    KeyPlanes::letter(0x2d, 'x', 'X'),
    KeyPlanes::letter(0x2e, 'c', 'C'),
    KeyPlanes::letter(0x2f, 'v', 'V'),
    KeyPlanes::letter(0x30, 'b', 'B'),
    KeyPlanes::letter(0x31, 'n', 'N'),
    KeyPlanes::sym(0x32, ',', '?'),
    KeyPlanes::sym(0x33, ';', '.'),
    KeyPlanes::sym(0x34, ':', '/'),
    KeyPlanes::sym(0x35, '!', '§'),
    SPACE,
];

/// German QWERTZ. Differences vs QWERTY:
///   - 0x15 (QWERTY 'y') -> 'z'
///   - 0x2c (QWERTY 'z') -> 'y'
///   - umlauts replace [ ] ; ' : 0x1a->ü 0x1b->+ 0x27->ö 0x28->ä 0x29->^
///   - 0x2b -> '#' ; ß on the number row (0x0c).
const GERMAN_QWERTZ: &[KeyPlanes] = &[
    ESC,
    KeyPlanes::sym(0x02, '1', '!'),
    KeyPlanes::sym(0x03, '2', '"'),
    KeyPlanes::sym(0x04, '3', '§'),
    KeyPlanes::sym(0x05, '4', '$'),
    KeyPlanes::sym(0x06, '5', '%'),
    KeyPlanes::sym(0x07, '6', '&'),
    KeyPlanes::sym(0x08, '7', '/'),
    KeyPlanes::sym(0x09, '8', '('),
    KeyPlanes::sym(0x0a, '9', ')'),
    KeyPlanes::sym(0x0b, '0', '='),
    KeyPlanes::sym(0x0c, 'ß', '?'),
    KeyPlanes::sym(0x0d, '´', '`'),
    BKSP,
    TAB,
    // top row: q w e r t z u i o p ü +
    KeyPlanes::letter(0x10, 'q', 'Q'),
    KeyPlanes::letter(0x11, 'w', 'W'),
    KeyPlanes::letter(0x12, 'e', 'E'),
    KeyPlanes::letter(0x13, 'r', 'R'),
    KeyPlanes::letter(0x14, 't', 'T'),
    KeyPlanes::letter(0x15, 'z', 'Z'),
    KeyPlanes::letter(0x16, 'u', 'U'),
    KeyPlanes::letter(0x17, 'i', 'I'),
    KeyPlanes::letter(0x18, 'o', 'O'),
    KeyPlanes::letter(0x19, 'p', 'P'),
    KeyPlanes::letter(0x1a, 'ü', 'Ü'),
    KeyPlanes::sym(0x1b, '+', '*'),
    ENTER,
    // home row: a s d f g h j k l ö ä #
    KeyPlanes::letter(0x1e, 'a', 'A'),
    KeyPlanes::letter(0x1f, 's', 'S'),
    KeyPlanes::letter(0x20, 'd', 'D'),
    KeyPlanes::letter(0x21, 'f', 'F'),
    KeyPlanes::letter(0x22, 'g', 'G'),
    KeyPlanes::letter(0x23, 'h', 'H'),
    KeyPlanes::letter(0x24, 'j', 'J'),
    KeyPlanes::letter(0x25, 'k', 'K'),
    KeyPlanes::letter(0x26, 'l', 'L'),
    KeyPlanes::letter(0x27, 'ö', 'Ö'),
    KeyPlanes::letter(0x28, 'ä', 'Ä'),
    KeyPlanes::sym(0x29, '^', '°'),
    KeyPlanes::sym(0x2b, '#', '\''),
    // bottom row: y x c v b n m , . -
    KeyPlanes::letter(0x2c, 'y', 'Y'),
    KeyPlanes::letter(0x2d, 'x', 'X'),
    KeyPlanes::letter(0x2e, 'c', 'C'),
    KeyPlanes::letter(0x2f, 'v', 'V'),
    KeyPlanes::letter(0x30, 'b', 'B'),
    KeyPlanes::letter(0x31, 'n', 'N'),
    KeyPlanes::letter(0x32, 'm', 'M'),
    KeyPlanes::sym(0x33, ',', ';'),
    KeyPlanes::sym(0x34, '.', ':'),
    KeyPlanes::sym(0x35, '-', '_'),
    SPACE,
];

/// Dvorak (US). Same physical Set 1 scancodes, drastically different letters.
/// e.g. 0x10 (QWERTY 'q') -> '\''(quote); 0x12 (QWERTY 'e') -> '.';
/// 0x1e (QWERTY 'a') stays 'a'; 0x1f (QWERTY 's') -> 'o'.
const DVORAK: &[KeyPlanes] = &[
    ESC,
    // number row identical to US QWERTY in Dvorak
    KeyPlanes::sym(0x02, '1', '!'),
    KeyPlanes::sym(0x03, '2', '@'),
    KeyPlanes::sym(0x04, '3', '#'),
    KeyPlanes::sym(0x05, '4', '$'),
    KeyPlanes::sym(0x06, '5', '%'),
    KeyPlanes::sym(0x07, '6', '^'),
    KeyPlanes::sym(0x08, '7', '&'),
    KeyPlanes::sym(0x09, '8', '*'),
    KeyPlanes::sym(0x0a, '9', '('),
    KeyPlanes::sym(0x0b, '0', ')'),
    KeyPlanes::sym(0x0c, '[', '{'),
    KeyPlanes::sym(0x0d, ']', '}'),
    BKSP,
    TAB,
    // top row: ' , . p y f g c r l / =
    KeyPlanes::sym(0x10, '\'', '"'),
    KeyPlanes::sym(0x11, ',', '<'),
    KeyPlanes::sym(0x12, '.', '>'),
    KeyPlanes::letter(0x13, 'p', 'P'),
    KeyPlanes::letter(0x14, 'y', 'Y'),
    KeyPlanes::letter(0x15, 'f', 'F'),
    KeyPlanes::letter(0x16, 'g', 'G'),
    KeyPlanes::letter(0x17, 'c', 'C'),
    KeyPlanes::letter(0x18, 'r', 'R'),
    KeyPlanes::letter(0x19, 'l', 'L'),
    KeyPlanes::sym(0x1a, '/', '?'),
    KeyPlanes::sym(0x1b, '=', '+'),
    ENTER,
    // home row: a o e u i d h t n s -
    KeyPlanes::letter(0x1e, 'a', 'A'),
    KeyPlanes::letter(0x1f, 'o', 'O'),
    KeyPlanes::letter(0x20, 'e', 'E'),
    KeyPlanes::letter(0x21, 'u', 'U'),
    KeyPlanes::letter(0x22, 'i', 'I'),
    KeyPlanes::letter(0x23, 'd', 'D'),
    KeyPlanes::letter(0x24, 'h', 'H'),
    KeyPlanes::letter(0x25, 't', 'T'),
    KeyPlanes::letter(0x26, 'n', 'N'),
    KeyPlanes::letter(0x27, 's', 'S'),
    KeyPlanes::sym(0x28, '-', '_'),
    KeyPlanes::sym(0x29, '`', '~'),
    KeyPlanes::sym(0x2b, '\\', '|'),
    // bottom row: ; q j k x b m w v z
    KeyPlanes::sym(0x2c, ';', ':'),
    KeyPlanes::letter(0x2d, 'q', 'Q'),
    KeyPlanes::letter(0x2e, 'j', 'J'),
    KeyPlanes::letter(0x2f, 'k', 'K'),
    KeyPlanes::letter(0x30, 'x', 'X'),
    KeyPlanes::letter(0x31, 'b', 'B'),
    KeyPlanes::letter(0x32, 'm', 'M'),
    KeyPlanes::letter(0x33, 'w', 'W'),
    KeyPlanes::letter(0x34, 'v', 'V'),
    KeyPlanes::letter(0x35, 'z', 'Z'),
    SPACE,
];

const fn layout_table(id: LayoutId) -> &'static [KeyPlanes] {
    match id {
        LayoutId::UsQwerty => US_QWERTY,
        LayoutId::FrenchAzerty => FRENCH_AZERTY,
        LayoutId::GermanQwertz => GERMAN_QWERTZ,
        LayoutId::Dvorak => DVORAK,
    }
}

/// All layouts the registry knows about, in display order.
pub const ALL_LAYOUTS: [LayoutId; 4] = [
    LayoutId::UsQwerty,
    LayoutId::FrenchAzerty,
    LayoutId::GermanQwertz,
    LayoutId::Dvorak,
];

/// The registry / selector: list available layouts and resolve by id/name.
/// Pure: holds no state, every method is a lookup over the static tables.
#[derive(Debug, Clone, Copy, Default)]
pub struct LayoutRegistry;

impl LayoutRegistry {
    pub const fn new() -> Self {
        LayoutRegistry
    }

    /// All available layouts as `(short_name, display_name)` pairs.
    pub fn list(&self) -> Vec<(LayoutId, &'static str, &'static str)> {
        ALL_LAYOUTS
            .iter()
            .map(|&id| (id, id.short_name(), id.display_name()))
            .collect()
    }

    /// Fetch a layout by id.
    pub fn get(&self, id: LayoutId) -> KeyboardLayout {
        KeyboardLayout {
            id,
            keys: layout_table(id),
        }
    }

    /// Resolve a layout by short name ("fr"), case-insensitive.
    pub fn by_name(&self, name: &str) -> Option<KeyboardLayout> {
        LayoutId::from_name(name).map(|id| self.get(id))
    }

    /// One-shot resolve: `(layout, scancode, modifiers) -> char`. Never panics;
    /// returns `None` for an unknown layout-key combination.
    pub fn resolve(&self, id: LayoutId, scancode: u8, mods: Modifiers) -> Option<KeySym> {
        self.get(id).resolve(scancode, mods)
    }
}

/// Convenience top-level resolver mirroring the eventual kernel call site.
/// `kernel/src/shell_runner.rs::lock_scancode_to_ascii` becomes a thin wrapper
/// around this once the active layout id is threaded through.
pub fn resolve_key(id: LayoutId, scancode: u8, mods: Modifiers) -> Option<KeySym> {
    LayoutRegistry::new().resolve(id, scancode, mods)
}

/// Render the resolved char as a `String` (handy for catalog/UI glue that
/// wants an owned value). `None` -> empty string is intentionally avoided;
/// callers get `None` so they can distinguish "no mapping".
pub fn resolve_key_string(id: LayoutId, scancode: u8, mods: Modifiers) -> Option<String> {
    resolve_key(id, scancode, mods).map(|c| {
        let mut s = String::new();
        s.push(c);
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Layout-divergent keys. Each assertion is FAIL-able: a US-only mapper
    //    (the legacy `lock_scancode_to_ascii`) returns the QWERTY char for the
    //    SAME scancode, so it would produce the value on the right of the
    //    "would return" comment and FAIL these assertions. ──────────────────

    #[test]
    fn azerty_q_scancode_is_a() {
        // 0x10 = QWERTY 'q'. A US-only mapper would return 'q' -> FAILs this.
        let r = LayoutRegistry::new();
        assert_eq!(
            r.resolve(LayoutId::FrenchAzerty, 0x10, Modifiers::none()),
            Some('a')
        );
        assert_eq!(
            r.resolve(LayoutId::FrenchAzerty, 0x10, Modifiers::shift()),
            Some('A')
        );
    }

    #[test]
    fn azerty_a_scancode_is_q() {
        // 0x1e = QWERTY 'a'. US-only would return 'a' -> FAIL.
        assert_eq!(
            resolve_key(LayoutId::FrenchAzerty, 0x1e, Modifiers::none()),
            Some('q')
        );
    }

    #[test]
    fn azerty_number_row_needs_shift_for_digits() {
        // 0x03 base is 'é' on AZERTY, '2' only with shift.
        // US-only would return '2' unshifted -> FAIL.
        assert_eq!(
            resolve_key(LayoutId::FrenchAzerty, 0x03, Modifiers::none()),
            Some('é')
        );
        assert_eq!(
            resolve_key(LayoutId::FrenchAzerty, 0x03, Modifiers::shift()),
            Some('2')
        );
    }

    #[test]
    fn qwertz_y_scancode_is_z() {
        // 0x15 = QWERTY 'y'. US-only would return 'y' -> FAIL.
        assert_eq!(
            resolve_key(LayoutId::GermanQwertz, 0x15, Modifiers::none()),
            Some('z')
        );
        assert_eq!(
            resolve_key(LayoutId::GermanQwertz, 0x15, Modifiers::shift()),
            Some('Z')
        );
    }

    #[test]
    fn qwertz_z_scancode_is_y() {
        // 0x2c = QWERTY 'z'. US-only would return 'z' -> FAIL.
        assert_eq!(
            resolve_key(LayoutId::GermanQwertz, 0x2c, Modifiers::none()),
            Some('y')
        );
    }

    #[test]
    fn qwertz_umlaut_on_bracket_key() {
        // 0x1a = QWERTY '['. US-only would return '[' -> FAIL.
        assert_eq!(
            resolve_key(LayoutId::GermanQwertz, 0x1a, Modifiers::none()),
            Some('ü')
        );
        assert_eq!(
            resolve_key(LayoutId::GermanQwertz, 0x1a, Modifiers::shift()),
            Some('Ü')
        );
    }

    #[test]
    fn dvorak_divergence() {
        // 0x1f = QWERTY 's' -> Dvorak 'o'. US-only would return 's' -> FAIL.
        assert_eq!(
            resolve_key(LayoutId::Dvorak, 0x1f, Modifiers::none()),
            Some('o')
        );
        // 0x12 = QWERTY 'e' -> Dvorak '.'. US-only would return 'e' -> FAIL.
        assert_eq!(
            resolve_key(LayoutId::Dvorak, 0x12, Modifiers::none()),
            Some('.')
        );
        // 0x10 = QWERTY 'q' -> Dvorak '\''. US-only would return 'q' -> FAIL.
        assert_eq!(
            resolve_key(LayoutId::Dvorak, 0x10, Modifiers::none()),
            Some('\'')
        );
    }

    #[test]
    fn us_qwerty_unchanged() {
        // The reference must match the legacy unshifted table exactly.
        let r = LayoutRegistry::new();
        assert_eq!(
            r.resolve(LayoutId::UsQwerty, 0x10, Modifiers::none()),
            Some('q')
        );
        assert_eq!(
            r.resolve(LayoutId::UsQwerty, 0x1e, Modifiers::none()),
            Some('a')
        );
        assert_eq!(
            r.resolve(LayoutId::UsQwerty, 0x15, Modifiers::none()),
            Some('y')
        );
        assert_eq!(
            r.resolve(LayoutId::UsQwerty, 0x2c, Modifiers::none()),
            Some('z')
        );
        assert_eq!(
            r.resolve(LayoutId::UsQwerty, 0x39, Modifiers::none()),
            Some(' ')
        );
    }

    #[test]
    fn us_qwerty_shift_plane() {
        // Legacy table had NO shift plane; this proves we added it correctly.
        assert_eq!(
            resolve_key(LayoutId::UsQwerty, 0x02, Modifiers::shift()),
            Some('!')
        );
        assert_eq!(
            resolve_key(LayoutId::UsQwerty, 0x10, Modifiers::shift()),
            Some('Q')
        );
    }

    #[test]
    fn caps_lock_affects_letters_not_symbols() {
        let caps = Modifiers {
            shift: false,
            caps_lock: true,
            altgr: false,
        };
        // Letter: caps -> uppercase.
        assert_eq!(resolve_key(LayoutId::UsQwerty, 0x10, caps), Some('Q'));
        // Symbol: caps alone does NOT shift it.
        assert_eq!(resolve_key(LayoutId::UsQwerty, 0x02, caps), Some('1'));
        // Shift + caps on a letter -> back to lowercase (XOR).
        let shift_caps = Modifiers {
            shift: true,
            caps_lock: true,
            altgr: false,
        };
        assert_eq!(resolve_key(LayoutId::UsQwerty, 0x10, shift_caps), Some('q'));
    }

    #[test]
    fn unknown_scancode_returns_none() {
        // Never-panic contract: out-of-table scancodes -> None on every layout.
        for &id in ALL_LAYOUTS.iter() {
            assert_eq!(resolve_key(id, 0x7f, Modifiers::none()), None);
            assert_eq!(resolve_key(id, 0xff, Modifiers::shift()), None);
            // 0x1d (LeftCtrl, non-printing) is intentionally unmapped.
            assert_eq!(resolve_key(id, 0x1d, Modifiers::none()), None);
        }
    }

    #[test]
    fn registry_lists_all_layouts() {
        let r = LayoutRegistry::new();
        let list = r.list();
        assert_eq!(list.len(), 4);
        assert!(list.iter().any(|(_, short, _)| *short == "fr"));
        assert!(list.iter().any(|(_, short, _)| *short == "dvorak"));
    }

    #[test]
    fn registry_resolves_by_name_case_insensitive() {
        let r = LayoutRegistry::new();
        assert_eq!(r.by_name("FR").map(|l| l.id), Some(LayoutId::FrenchAzerty));
        assert_eq!(r.by_name("de").map(|l| l.id), Some(LayoutId::GermanQwertz));
        assert_eq!(r.by_name("Dvorak").map(|l| l.id), Some(LayoutId::Dvorak));
        assert_eq!(r.by_name("us").map(|l| l.id), Some(LayoutId::UsQwerty));
        assert_eq!(r.by_name("nonexistent").map(|l| l.id), None);
    }

    #[test]
    fn non_printing_keys_consistent_across_layouts() {
        // Esc/Backspace/Tab/Enter/Space identical everywhere — drop-in safety.
        for &id in ALL_LAYOUTS.iter() {
            assert_eq!(resolve_key(id, 0x01, Modifiers::none()), Some('\u{1b}'));
            assert_eq!(resolve_key(id, 0x0e, Modifiers::none()), Some('\u{8}'));
            assert_eq!(resolve_key(id, 0x0f, Modifiers::none()), Some('\t'));
            assert_eq!(resolve_key(id, 0x1c, Modifiers::none()), Some('\n'));
            assert_eq!(resolve_key(id, 0x39, Modifiers::none()), Some(' '));
        }
    }

    #[test]
    fn resolve_key_string_roundtrip() {
        assert_eq!(
            resolve_key_string(LayoutId::FrenchAzerty, 0x10, Modifiers::none()).as_deref(),
            Some("a")
        );
        assert_eq!(
            resolve_key_string(LayoutId::UsQwerty, 0x7f, Modifiers::none()),
            None
        );
    }
}
