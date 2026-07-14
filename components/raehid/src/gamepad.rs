//! RaeenOS gamepad / controller HID decoding (Concept §GameOS — "gaming isn't a
//! mode": a controller that *just works* is core to the OS thesis, not a bolt-on).
//!
//! This module decodes the raw USB-HID *input* reports of the major controller
//! lineages into one normalized [`GamepadState`] — the cross-controller
//! abstraction the rest of RaeenOS (GameOS shell navigation, per-game profiles,
//! in-game overlay) consumes without caring which pad is plugged in.
//!
//! Three layouts are decoded:
//!   * the generic USB-HID gamepad (report-descriptor driven, via `hidreport`),
//!   * the Sony DualSense / DualShock-style fixed report,
//!   * the Xbox-style (XInput-derived) fixed report.
//!
//! Pure logic, no hardware: the USB transport + output (rumble) transfer stay in
//! the kernel xHCI path (deferred). Every parser is **never-panic on hostile or
//! truncated reports** — a controller is untrusted USB-device input, and a
//! short/garbage report must yield a partial/neutral state, never a fault.
//!
//! Host-KAT'd (`cargo test -p raehid`): concrete byte buffers decode to concrete
//! buttons/sticks/triggers; a truncated report returns a neutral state instead of
//! panicking.

use hidreport::Field;

use crate::HidDevice;

/// Generic Desktop usages used by HID gamepads (in addition to X/Y from the
/// pointing-device set already in `lib.rs`).
const PAGE_GENERIC_DESKTOP: u16 = 0x01;
const PAGE_BUTTON: u16 = 0x09;
const USAGE_X: u16 = 0x30; // left stick X
const USAGE_Y: u16 = 0x31; // left stick Y
const USAGE_Z: u16 = 0x32; // right stick X (common) / sometimes L2
const USAGE_RX: u16 = 0x33; // right stick X (alt) / L2 analog
const USAGE_RY: u16 = 0x34; // right stick Y (alt) / R2 analog
const USAGE_RZ: u16 = 0x35; // right stick Y (common)
const USAGE_HAT: u16 = 0x39; // hat switch (D-pad)

/// The normalized digital buttons, as a bitmap in [`GamepadState::buttons`].
///
/// The names follow the Xbox/abstract convention (`A/B/X/Y`); the
/// PlayStation face buttons map positionally: Cross→A, Circle→B, Square→X,
/// Triangle→Y. This is the cross-controller contract every consumer codes
/// against — a single bit layout regardless of the physical pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Button {
    /// South face button (Xbox A / PS Cross).
    A = 1 << 0,
    /// East face button (Xbox B / PS Circle).
    B = 1 << 1,
    /// West face button (Xbox X / PS Square).
    X = 1 << 2,
    /// North face button (Xbox Y / PS Triangle).
    Y = 1 << 3,
    /// Left shoulder bumper (L1 / LB).
    L1 = 1 << 4,
    /// Right shoulder bumper (R1 / RB).
    R1 = 1 << 5,
    /// Left trigger crossed its digital threshold (L2 / LT full-pull).
    L2 = 1 << 6,
    /// Right trigger crossed its digital threshold (R2 / RT full-pull).
    R2 = 1 << 7,
    /// Left stick click (L3 / LS).
    L3 = 1 << 8,
    /// Right stick click (R3 / RS).
    R3 = 1 << 9,
    /// Select / Back / Create / Share / View.
    Select = 1 << 10,
    /// Start / Menu / Options.
    Start = 1 << 11,
    /// Guide / Home / PS / Xbox button.
    Guide = 1 << 12,
    /// Touchpad click (DualSense/DualShock 4); 0 on pads without one.
    Touchpad = 1 << 13,
    /// D-pad up (decoded from the hat switch).
    DpadUp = 1 << 14,
    /// D-pad down.
    DpadDown = 1 << 15,
    /// D-pad left.
    DpadLeft = 1 << 16,
    /// D-pad right.
    DpadRight = 1 << 17,
}

/// The eight hat-switch directions plus neutral, decoded from the 4-bit hat
/// nibble that every HID gamepad reports (0..=7 clockwise from up, 8/0xF =
/// centered depending on the device's logical range).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Hat {
    Up,
    UpRight,
    Right,
    DownRight,
    Down,
    DownLeft,
    Left,
    UpLeft,
    #[default]
    Neutral,
}

impl Hat {
    /// Decode a raw hat nibble. HID hat switches report 0..=7 clockwise starting
    /// at "up"; any out-of-range value (typically 8 or 0xF) means "centered".
    /// Never panics — an arbitrary `u8` maps cleanly to a variant.
    pub fn from_nibble(n: u8) -> Self {
        match n & 0x0F {
            0 => Hat::Up,
            1 => Hat::UpRight,
            2 => Hat::Right,
            3 => Hat::DownRight,
            4 => Hat::Down,
            5 => Hat::DownLeft,
            6 => Hat::Left,
            7 => Hat::UpLeft,
            _ => Hat::Neutral,
        }
    }

    /// The set of D-pad button bits this hat direction implies (a diagonal sets
    /// two bits). Used to fold the hat into the unified button bitmap.
    fn dpad_bits(self) -> u32 {
        let u = Button::DpadUp as u32;
        let d = Button::DpadDown as u32;
        let l = Button::DpadLeft as u32;
        let r = Button::DpadRight as u32;
        match self {
            Hat::Up => u,
            Hat::UpRight => u | r,
            Hat::Right => r,
            Hat::DownRight => d | r,
            Hat::Down => d,
            Hat::DownLeft => d | l,
            Hat::Left => l,
            Hat::UpLeft => u | l,
            Hat::Neutral => 0,
        }
    }
}

/// The normalized cross-controller state. This is the single model the GameOS
/// shell, controller-as-navigation, and per-game profiles read — independent of
/// the physical controller layout.
///
/// Sticks are normalized to `i16` with `0` at center, `i16::MIN` at full
/// negative deflection (left / up) and `i16::MAX` at full positive (right /
/// down). Triggers are `u8`, `0` released … `255` fully pulled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GamepadState {
    /// Bitmap of pressed [`Button`]s (OR of `Button as u32`).
    pub buttons: u32,
    /// Left stick X: negative = left, positive = right.
    pub left_x: i16,
    /// Left stick Y: negative = up, positive = down.
    pub left_y: i16,
    /// Right stick X: negative = left, positive = right.
    pub right_x: i16,
    /// Right stick Y: negative = up, positive = down.
    pub right_y: i16,
    /// Left analog trigger (L2 / LT), 0..=255.
    pub left_trigger: u8,
    /// Right analog trigger (R2 / RT), 0..=255.
    pub right_trigger: u8,
    /// Decoded hat direction (also folded into the D-pad button bits).
    pub hat: Hat,
}

impl GamepadState {
    /// True if the given button is pressed.
    pub fn pressed(&self, b: Button) -> bool {
        self.buttons & (b as u32) != 0
    }

    fn set(&mut self, b: Button) {
        self.buttons |= b as u32;
    }

    /// Apply a radial deadzone to both sticks: any axis whose magnitude is
    /// within `deadzone` of center is snapped to `0`. Trigger/button state is
    /// untouched. Returns `self` for chaining. Never panics.
    pub fn with_deadzone(mut self, deadzone: i16) -> Self {
        let dz = u32::from(deadzone.unsigned_abs());
        let clamp_axis = |v: i16| -> i16 {
            if u32::from(v.unsigned_abs()) <= dz {
                0
            } else {
                v
            }
        };
        self.left_x = clamp_axis(self.left_x);
        self.left_y = clamp_axis(self.left_y);
        self.right_x = clamp_axis(self.right_x);
        self.right_y = clamp_axis(self.right_y);
        self
    }

    /// Fold the hat direction into the D-pad button bits (idempotent).
    fn apply_hat(&mut self) {
        self.buttons |= self.hat.dpad_bits();
    }
}

/// Normalize an unsigned 8-bit axis sample (0..=255, 0x80 ≈ center — the byte
/// layout DualSense and most HID gamepads use) to the signed `i16` stick range.
/// Never panics.
pub fn norm_u8_axis(raw: u8) -> i16 {
    // Map 0..=255 to roughly -32768..=32512: (raw - 128) * 256.
    ((raw as i32 - 128) * 256).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// Normalize a signed little-endian 16-bit axis sample (already centered at 0,
/// the layout XInput-style pads use) — a pass-through that exists so callers
/// route every axis through one helper. Never panics.
pub fn norm_i16_axis(raw: i16) -> i16 {
    raw
}

/// Normalize an 8-bit trigger sample (0..=255) — identity, named for symmetry.
pub fn norm_u8_trigger(raw: u8) -> u8 {
    raw
}

/// Normalize a 10-bit trigger sample (0..=1023, the XInput/GIP range) down to
/// the common 0..=255 scale. Never panics.
pub fn norm_u10_trigger(raw: u16) -> u8 {
    ((raw.min(1023) as u32 * 255) / 1023) as u8
}

/// The digital threshold (out of 255) at which an analog trigger also latches
/// its digital `L2`/`R2` button bit — matches the ~3/4-pull most pads use.
const TRIGGER_DIGITAL_THRESHOLD: u8 = 192;

// ---------------------------------------------------------------------------
// DualSense / DualShock-style fixed report
// ---------------------------------------------------------------------------

/// Decode a Sony DualSense USB input report into [`GamepadState`].
///
/// USB-mode DualSense input report (report ID `0x01`) byte layout used here
/// (the documented stable prefix):
/// ```text
///   [0]  report id (0x01)
///   [1]  left stick X   (0..255, 0x80 center)
///   [2]  left stick Y
///   [3]  right stick X
///   [4]  right stick Y
///   [5]  L2 analog trigger (0..255)
///   [6]  R2 analog trigger
///   [7]  sequence/counter (ignored)
///   [8]  buttons0: bits 4..7 = Square/Cross/Circle/Triangle,
///                  bits 0..3 = hat/D-pad nibble
///   [9]  buttons1: L1,R1,L2,R2,Create(Share),Options,L3,R3 (bits 0..7)
///   [10] buttons2: bit0 = PS(Guide), bit1 = Touchpad click
/// ```
/// DualShock 4 shares this prefix (its report ID is also `0x01`), so this
/// decoder serves both. Truncated reports decode whatever prefix is present and
/// leave the rest neutral — never panics.
pub fn decode_dualsense(report: &[u8]) -> GamepadState {
    let mut st = GamepadState::default();
    // Tolerate the optional report-ID prefix: if byte 0 is the DualSense report
    // ID (0x01) treat the sticks as starting at index 1; otherwise assume the ID
    // was already stripped by the transport and sticks start at index 0.
    let base = if report.first() == Some(&0x01) { 1 } else { 0 };
    let at = |i: usize| report.get(base + i).copied();

    if let Some(v) = at(0) {
        st.left_x = norm_u8_axis(v);
    }
    if let Some(v) = at(1) {
        st.left_y = norm_u8_axis(v);
    }
    if let Some(v) = at(2) {
        st.right_x = norm_u8_axis(v);
    }
    if let Some(v) = at(3) {
        st.right_y = norm_u8_axis(v);
    }
    if let Some(v) = at(4) {
        st.left_trigger = norm_u8_trigger(v);
    }
    if let Some(v) = at(5) {
        st.right_trigger = norm_u8_trigger(v);
    }

    if let Some(b0) = at(7) {
        st.hat = Hat::from_nibble(b0 & 0x0F);
        if b0 & 0x10 != 0 {
            st.set(Button::X); // Square -> X
        }
        if b0 & 0x20 != 0 {
            st.set(Button::A); // Cross -> A
        }
        if b0 & 0x40 != 0 {
            st.set(Button::B); // Circle -> B
        }
        if b0 & 0x80 != 0 {
            st.set(Button::Y); // Triangle -> Y
        }
    }

    if let Some(b1) = at(8) {
        if b1 & 0x01 != 0 {
            st.set(Button::L1);
        }
        if b1 & 0x02 != 0 {
            st.set(Button::R1);
        }
        if b1 & 0x04 != 0 {
            st.set(Button::L2);
        }
        if b1 & 0x08 != 0 {
            st.set(Button::R2);
        }
        if b1 & 0x10 != 0 {
            st.set(Button::Select); // Create / Share
        }
        if b1 & 0x20 != 0 {
            st.set(Button::Start); // Options
        }
        if b1 & 0x40 != 0 {
            st.set(Button::L3);
        }
        if b1 & 0x80 != 0 {
            st.set(Button::R3);
        }
    }

    if let Some(b2) = at(9) {
        if b2 & 0x01 != 0 {
            st.set(Button::Guide); // PS button
        }
        if b2 & 0x02 != 0 {
            st.set(Button::Touchpad);
        }
    }

    // Latch digital L2/R2 from the analog pull as well, so a pad that only sends
    // analog still reports the button.
    if st.left_trigger >= TRIGGER_DIGITAL_THRESHOLD {
        st.set(Button::L2);
    }
    if st.right_trigger >= TRIGGER_DIGITAL_THRESHOLD {
        st.set(Button::R2);
    }

    st.apply_hat();
    st
}

// ---------------------------------------------------------------------------
// Xbox-style (XInput-derived) fixed report
// ---------------------------------------------------------------------------

/// Decode an Xbox-style input report into [`GamepadState`].
///
/// The byte layout follows the XInput/GIP gamepad mapping commonly surfaced over
/// HID (no report-ID prefix):
/// ```text
///   [0..2]  buttons bitmap (LE u16):
///             bit0 D-Up bit1 D-Down bit2 D-Left bit3 D-Right
///             bit4 Start bit5 Back(Select) bit6 L3 bit7 R3
///             bit8 L1(LB) bit9 R1(RB) bit10 Guide bit11 reserved
///             bit12 A bit13 B bit14 X bit15 Y
///   [2]     left trigger  (0..255)
///   [3]     right trigger (0..255)
///   [4..6]  left stick X  (LE i16, signed, 0 center)
///   [6..8]  left stick Y  (LE i16)
///   [8..10] right stick X (LE i16)
///   [10..12] right stick Y (LE i16)
/// ```
/// XInput's sticks have +Y = up; we flip Y so the normalized model's "+Y =
/// down" contract holds across all decoders. Truncated reports leave missing
/// fields neutral — never panics.
pub fn decode_xbox(report: &[u8]) -> GamepadState {
    let mut st = GamepadState::default();
    let le16 = |i: usize| -> Option<u16> {
        Some(u16::from_le_bytes([*report.get(i)?, *report.get(i + 1)?]))
    };
    let le_i16 = |i: usize| -> Option<i16> { le16(i).map(|v| v as i16) };

    if let Some(b) = le16(0) {
        if b & (1 << 0) != 0 {
            st.hat = Hat::Up;
        }
        // D-pad arrives as discrete bits; build the hat from the combination so
        // diagonals decode too.
        let up = b & (1 << 0) != 0;
        let down = b & (1 << 1) != 0;
        let left = b & (1 << 2) != 0;
        let right = b & (1 << 3) != 0;
        st.hat = match (up, down, left, right) {
            (true, _, false, false) => Hat::Up,
            (true, _, true, false) => Hat::UpLeft,
            (true, _, false, true) => Hat::UpRight,
            (false, true, false, false) => Hat::Down,
            (false, true, true, false) => Hat::DownLeft,
            (false, true, false, true) => Hat::DownRight,
            (false, false, true, false) => Hat::Left,
            (false, false, false, true) => Hat::Right,
            _ => Hat::Neutral,
        };
        if b & (1 << 4) != 0 {
            st.set(Button::Start);
        }
        if b & (1 << 5) != 0 {
            st.set(Button::Select); // Back / View
        }
        if b & (1 << 6) != 0 {
            st.set(Button::L3);
        }
        if b & (1 << 7) != 0 {
            st.set(Button::R3);
        }
        if b & (1 << 8) != 0 {
            st.set(Button::L1);
        }
        if b & (1 << 9) != 0 {
            st.set(Button::R1);
        }
        if b & (1 << 10) != 0 {
            st.set(Button::Guide);
        }
        if b & (1 << 12) != 0 {
            st.set(Button::A);
        }
        if b & (1 << 13) != 0 {
            st.set(Button::B);
        }
        if b & (1 << 14) != 0 {
            st.set(Button::X);
        }
        if b & (1 << 15) != 0 {
            st.set(Button::Y);
        }
    }

    if let Some(v) = report.get(2).copied() {
        st.left_trigger = norm_u8_trigger(v);
    }
    if let Some(v) = report.get(3).copied() {
        st.right_trigger = norm_u8_trigger(v);
    }
    if let Some(v) = le_i16(4) {
        st.left_x = norm_i16_axis(v);
    }
    if let Some(v) = le_i16(6) {
        // XInput +Y = up; flip to the model's +Y = down. saturating_neg avoids
        // the i16::MIN overflow panic.
        st.left_y = v.saturating_neg();
    }
    if let Some(v) = le_i16(8) {
        st.right_x = norm_i16_axis(v);
    }
    if let Some(v) = le_i16(10) {
        st.right_y = v.saturating_neg();
    }

    if st.left_trigger >= TRIGGER_DIGITAL_THRESHOLD {
        st.set(Button::L2);
    }
    if st.right_trigger >= TRIGGER_DIGITAL_THRESHOLD {
        st.set(Button::R2);
    }

    st.apply_hat();
    st
}

// ---------------------------------------------------------------------------
// Generic HID gamepad (report-descriptor driven)
// ---------------------------------------------------------------------------

impl HidDevice {
    /// Decode a raw input report from a *generic* HID gamepad using its parsed
    /// report descriptor. This covers third-party / Steam-Input-lineage pads that
    /// expose a standard Generic-Desktop gamepad layout (X/Y/Z/Rz sticks, a hat,
    /// and a Button-page bitmap) rather than a known vendor fixed report.
    ///
    /// Returns `None` only if no input report layout applies; an all-neutral
    /// report yields a centered/zero [`GamepadState`]. Never panics on a
    /// truncated/garbage report — `hidreport`'s field extraction is bounds-checked
    /// and a failed extract leaves that field neutral.
    pub fn extract_gamepad(&self, report: &[u8]) -> Option<GamepadState> {
        let r = self.report_for(report)?;
        let mut st = GamepadState::default();
        let mut button_index: u32 = 0;

        // NEVER-PANIC guard: `hidreport`'s field `extract` indexes the report
        // buffer unchecked and panics on a short read. A controller is untrusted
        // USB input — a truncated/garbage report must yield a neutral state, not
        // a fault. If the buffer can't cover the declared layout, return neutral.
        if report.len() < r.size_in_bytes() {
            return Some(st);
        }

        for field in r.fields() {
            match field {
                Field::Variable(v) => {
                    let page: u16 = u16::from(v.usage.usage_page);
                    let id: u16 = u16::from(v.usage.usage_id);
                    let Ok(fv) = v.extract(report) else { continue };
                    let val: i32 = i32::from(&fv);
                    // Logical range, to normalize an axis to the i16 model range.
                    let lmin: i32 = i32::from(&v.logical_minimum);
                    let lmax: i32 = i32::from(&v.logical_maximum);

                    match (page, id) {
                        (PAGE_GENERIC_DESKTOP, USAGE_X) => st.left_x = scale_axis(val, lmin, lmax),
                        (PAGE_GENERIC_DESKTOP, USAGE_Y) => st.left_y = scale_axis(val, lmin, lmax),
                        (PAGE_GENERIC_DESKTOP, USAGE_Z) | (PAGE_GENERIC_DESKTOP, USAGE_RX) => {
                            st.right_x = scale_axis(val, lmin, lmax)
                        }
                        (PAGE_GENERIC_DESKTOP, USAGE_RZ) | (PAGE_GENERIC_DESKTOP, USAGE_RY) => {
                            st.right_y = scale_axis(val, lmin, lmax)
                        }
                        (PAGE_GENERIC_DESKTOP, USAGE_HAT) => {
                            // The hat's logical 0 may be offset; normalize to a
                            // 0-based nibble relative to its logical minimum.
                            let n = (val - lmin).max(0) as u8;
                            st.hat = Hat::from_nibble(n);
                        }
                        (PAGE_BUTTON, _) => {
                            // Each Button-page variable field is one button, in
                            // declaration order; map the first 14 to our bitmap.
                            if val != 0 {
                                map_generic_button(&mut st, button_index);
                            }
                            button_index += 1;
                        }
                        _ => {}
                    }
                }
                Field::Array(a) => {
                    // Some pads declare buttons as an array of pressed usages.
                    if let Ok(values) = a.extract(report) {
                        for fv in values.iter() {
                            let code = u32::from(fv);
                            if code >= 1 {
                                map_generic_button(&mut st, code - 1);
                            }
                        }
                    }
                }
                Field::Constant(_) => {}
            }
        }

        st.apply_hat();
        Some(st)
    }
}

/// Scale a raw axis from its declared logical range to the model's `i16` range,
/// center at the logical midpoint. Never panics (guards a zero-width range).
fn scale_axis(val: i32, lmin: i32, lmax: i32) -> i16 {
    if lmax <= lmin {
        return 0;
    }
    let span = (lmax - lmin) as i64;
    let centered = (val as i64 - lmin as i64) * 2 - span; // -span..=+span
    let scaled = centered * (i16::MAX as i64) / span;
    scaled.clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

/// Map a 0-based generic button index to the normalized button bitmap, following
/// the de-facto HID gamepad ordering (A,B,X,Y,L1,R1,L2,R2,Select,Start,Guide,
/// L3,R3,Touchpad).
fn map_generic_button(st: &mut GamepadState, index: u32) {
    const ORDER: [Button; 14] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::L1,
        Button::R1,
        Button::L2,
        Button::R2,
        Button::Select,
        Button::Start,
        Button::Guide,
        Button::L3,
        Button::R3,
        Button::Touchpad,
    ];
    if let Some(b) = ORDER.get(index as usize) {
        st.set(*b);
    }
}

// ---------------------------------------------------------------------------
// Output report (rumble) — encoding only; the USB transfer is kernel-deferred.
// ---------------------------------------------------------------------------

/// A rumble command for the dual force-feedback motors found on modern pads.
/// The actual USB output-report transfer lives in the kernel xHCI path; this
/// struct + its encoders define the *wire format* so the kernel only has to push
/// the bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RumbleCommand {
    /// Strong / low-frequency (left) motor, 0..=255.
    pub strong: u8,
    /// Weak / high-frequency (right) motor, 0..=255.
    pub weak: u8,
}

impl RumbleCommand {
    /// Encode a DualSense USB output report (report ID `0x02`) carrying just the
    /// rumble motors. The remaining feature flags are left zero (no LED / trigger
    /// effect changes). Returns a 48-byte buffer — the documented USB output
    /// report length.
    pub fn to_dualsense_output(&self) -> [u8; 48] {
        let mut out = [0u8; 48];
        out[0] = 0x02; // report ID
        out[1] = 0x03; // flags: enable rumble (bit0 | bit1)
        out[3] = self.weak; // right / high-freq motor
        out[4] = self.strong; // left / low-freq motor
        out
    }

    /// Encode an Xbox-style rumble output report (`[id, strong, weak]`). The GIP
    /// transport wraps more framing, but the motor magnitudes are these two
    /// bytes; the kernel adds the transport header.
    pub fn to_xbox_output(&self) -> [u8; 3] {
        [0x00, self.strong, self.weak]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    // ----- Hat switch: all 8 directions + neutral -----

    #[test]
    fn hat_all_eight_directions_and_neutral() {
        assert_eq!(Hat::from_nibble(0), Hat::Up);
        assert_eq!(Hat::from_nibble(1), Hat::UpRight);
        assert_eq!(Hat::from_nibble(2), Hat::Right);
        assert_eq!(Hat::from_nibble(3), Hat::DownRight);
        assert_eq!(Hat::from_nibble(4), Hat::Down);
        assert_eq!(Hat::from_nibble(5), Hat::DownLeft);
        assert_eq!(Hat::from_nibble(6), Hat::Left);
        assert_eq!(Hat::from_nibble(7), Hat::UpLeft);
        assert_eq!(Hat::from_nibble(8), Hat::Neutral);
        assert_eq!(Hat::from_nibble(0x0F), Hat::Neutral);
    }

    #[test]
    fn hat_folds_into_dpad_bits() {
        // UpLeft must light both up and left D-pad bits.
        let mut st = GamepadState {
            hat: Hat::UpLeft,
            ..Default::default()
        };
        st.apply_hat();
        assert!(st.pressed(Button::DpadUp));
        assert!(st.pressed(Button::DpadLeft));
        assert!(!st.pressed(Button::DpadDown));
        assert!(!st.pressed(Button::DpadRight));
    }

    // ----- Normalization helpers -----

    #[test]
    fn u8_axis_neutral_is_center() {
        // 0x80 is the neutral byte; must land at (or very near) 0.
        assert_eq!(norm_u8_axis(0x80), 0);
    }

    #[test]
    fn u8_axis_extremes() {
        assert_eq!(norm_u8_axis(0x00), i16::MIN); // full negative
        assert!(norm_u8_axis(0xFF) > 32000); // near full positive
    }

    #[test]
    fn u10_trigger_scales_to_byte() {
        assert_eq!(norm_u10_trigger(0), 0);
        assert_eq!(norm_u10_trigger(1023), 255);
        // overflow input is clamped, not panicking.
        assert_eq!(norm_u10_trigger(5000), 255);
    }

    #[test]
    fn deadzone_clamps_small_values_to_zero() {
        let st = GamepadState {
            left_x: 1000,
            left_y: -800,
            right_x: 20000,
            right_y: -50,
            ..Default::default()
        }
        .with_deadzone(2000);
        assert_eq!(st.left_x, 0, "small left_x within deadzone -> 0");
        assert_eq!(st.left_y, 0, "small left_y within deadzone -> 0");
        assert_eq!(st.right_x, 20000, "large right_x preserved");
        assert_eq!(st.right_y, 0, "tiny right_y within deadzone -> 0");
    }

    // ----- DualSense decode -----

    #[test]
    fn dualsense_decodes_buttons_sticks_triggers() {
        // report id 0x01, LX=0xFF(right max) LY=0x00(up max) RX=0x80 RY=0x80,
        // L2=0x10 R2=0xFF, seq=0,
        // b0 = 0x20 | 0x04 = Cross(A) + hat=4(Down)
        // b1 = 0x01 | 0x20 = L1 + Options(Start)
        // b2 = 0x02 = Touchpad click
        let report = [
            0x01u8, 0xFF, 0x00, 0x80, 0x80, 0x10, 0xFF, 0x00, 0x24, 0x21, 0x02,
        ];
        let st = decode_dualsense(&report);
        assert_eq!(st.left_x, norm_u8_axis(0xFF), "LX full right");
        assert_eq!(st.left_y, i16::MIN, "LY full up");
        assert_eq!(st.right_x, 0, "RX center");
        assert_eq!(st.right_y, 0, "RY center");
        assert_eq!(st.left_trigger, 0x10);
        assert_eq!(st.right_trigger, 0xFF);
        assert!(st.pressed(Button::A), "Cross -> A");
        assert!(!st.pressed(Button::B));
        assert_eq!(st.hat, Hat::Down);
        assert!(st.pressed(Button::DpadDown), "hat Down -> DpadDown bit");
        assert!(st.pressed(Button::L1));
        assert!(st.pressed(Button::Start), "Options -> Start");
        assert!(st.pressed(Button::Touchpad));
        // Full R2 pull also latches the digital R2 button.
        assert!(st.pressed(Button::R2), "analog R2 full -> digital R2");
        // L2 analog was low, so no digital L2 from b1 (bit not set) nor threshold.
        assert!(!st.pressed(Button::L2));
    }

    #[test]
    fn dualsense_face_buttons_map_positionally() {
        // b0 nibble hi = all four face buttons; hat neutral (0x0F).
        let report = [0x01u8, 0x80, 0x80, 0x80, 0x80, 0, 0, 0, 0xF0 | 0x0F, 0, 0];
        let st = decode_dualsense(&report);
        assert!(st.pressed(Button::X), "Square -> X");
        assert!(st.pressed(Button::A), "Cross -> A");
        assert!(st.pressed(Button::B), "Circle -> B");
        assert!(st.pressed(Button::Y), "Triangle -> Y");
        assert_eq!(st.hat, Hat::Neutral);
    }

    // ----- Xbox decode -----

    #[test]
    fn xbox_decodes_buttons_sticks_triggers() {
        // buttons LE u16 = A(bit12)|B(bit13)|R1(bit9)|Start(bit4)|D-Right(bit3)
        let btn: u16 = (1 << 12) | (1 << 13) | (1 << 9) | (1 << 4) | (1 << 3);
        let lt = 0x20u8;
        let rt = 0xFFu8;
        let lx: i16 = 16000;
        let ly: i16 = 16000; // XInput +Y up -> model should flip to -16000
        let rx: i16 = -16000;
        let ry: i16 = 0;
        let mut report: Vec<u8> = Vec::new();
        report.extend_from_slice(&btn.to_le_bytes());
        report.push(lt);
        report.push(rt);
        report.extend_from_slice(&lx.to_le_bytes());
        report.extend_from_slice(&ly.to_le_bytes());
        report.extend_from_slice(&rx.to_le_bytes());
        report.extend_from_slice(&ry.to_le_bytes());
        // NOTE: byte[2..4] are reused as triggers above; rebuild cleanly:
        // layout: [0..2]=btn,[2]=lt,[3]=rt,[4..6]=lx,[6..8]=ly,[8..10]=rx,[10..12]=ry
        let st = decode_xbox(&report);
        assert!(st.pressed(Button::A));
        assert!(st.pressed(Button::B));
        assert!(st.pressed(Button::R1));
        assert!(st.pressed(Button::Start));
        assert!(!st.pressed(Button::X));
        assert_eq!(st.hat, Hat::Right, "D-Right bit -> hat Right");
        assert!(st.pressed(Button::DpadRight));
        assert_eq!(st.left_trigger, 0x20);
        assert_eq!(st.right_trigger, 0xFF);
        assert!(st.pressed(Button::R2), "full RT -> digital R2");
        assert_eq!(st.left_x, 16000);
        assert_eq!(st.left_y, -16000, "Y flipped to model +down convention");
        assert_eq!(st.right_x, -16000);
        assert_eq!(st.right_y, 0);
    }

    // ----- Generic HID gamepad (report-descriptor driven) -----

    /// A minimal standard HID gamepad descriptor: X,Y,Z,Rz (8-bit, 0..255) +
    /// a 4-bit hat + 4-bit padding + 8 buttons -> a 6-byte report:
    /// [X, Y, Z, Rz, hat|pad, buttons].
    const GAMEPAD_RDESC: &[u8] = &[
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x05, // Usage (Game Pad)
        0xA1, 0x01, // Collection (Application)
        0x05, 0x01, //   Usage Page (Generic Desktop)
        0x09, 0x30, //   Usage (X)
        0x09, 0x31, //   Usage (Y)
        0x09, 0x32, //   Usage (Z)
        0x09, 0x35, //   Usage (Rz)
        0x15, 0x00, //   Logical Minimum (0)
        0x26, 0xFF, 0x00, // Logical Maximum (255)
        0x75, 0x08, //   Report Size (8)
        0x95, 0x04, //   Report Count (4)
        0x81, 0x02, //   Input (Data,Var,Abs) — X,Y,Z,Rz
        0x09, 0x39, //   Usage (Hat switch)
        0x15, 0x00, //   Logical Minimum (0)
        0x25, 0x07, //   Logical Maximum (7)
        0x35, 0x00, //   Physical Minimum (0)
        0x46, 0x3B, 0x01, // Physical Maximum (315)
        0x65, 0x14, //   Unit (degrees)
        0x75, 0x04, //   Report Size (4)
        0x95, 0x01, //   Report Count (1)
        0x81, 0x42, //   Input (Data,Var,Abs,Null) — hat
        0x65, 0x00, //   Unit (none)
        0x75, 0x04, //   Report Size (4)
        0x95, 0x01, //   Report Count (1)
        0x81, 0x03, //   Input (Cnst,Var,Abs) — 4-bit pad
        0x05, 0x09, //   Usage Page (Button)
        0x19, 0x01, //   Usage Minimum (1)
        0x29, 0x08, //   Usage Maximum (8)
        0x15, 0x00, //   Logical Minimum (0)
        0x25, 0x01, //   Logical Maximum (1)
        0x75, 0x01, //   Report Size (1)
        0x95, 0x08, //   Report Count (8)
        0x81, 0x02, //   Input (Data,Var,Abs) — 8 buttons
        0xC0, // End Collection
    ];

    #[test]
    fn generic_gamepad_descriptor_parses() {
        assert!(
            HidDevice::parse(GAMEPAD_RDESC).is_some(),
            "standard HID gamepad descriptor must parse"
        );
    }

    #[test]
    fn generic_gamepad_decodes_sticks_hat_buttons() {
        let dev = HidDevice::parse(GAMEPAD_RDESC).expect("parse gamepad rdesc");
        // X=255(right) Y=0(up) Z=128(center) Rz=128(center),
        // hat nibble=2(Right) + pad, buttons=0b0000_0011 (button1=A, button2=B).
        let report = [0xFFu8, 0x00, 0x80, 0x80, 0x02, 0x03];
        let st = dev.extract_gamepad(&report).expect("decode gamepad");
        assert!(st.left_x > 32000, "X near full right, got {}", st.left_x);
        assert!(st.left_y < -32000, "Y near full up, got {}", st.left_y);
        assert!(st.right_x.abs() < 300, "Z center, got {}", st.right_x);
        assert!(st.right_y.abs() < 300, "Rz center, got {}", st.right_y);
        assert_eq!(st.hat, Hat::Right);
        assert!(st.pressed(Button::DpadRight), "hat Right -> DpadRight");
        assert!(st.pressed(Button::A), "button1 -> A");
        assert!(st.pressed(Button::B), "button2 -> B");
        assert!(!st.pressed(Button::X), "button3 not pressed");
    }

    #[test]
    fn generic_gamepad_idle_is_centered() {
        let dev = HidDevice::parse(GAMEPAD_RDESC).expect("parse");
        // neutral sticks 0x80, hat=0x0F (centered/null), no buttons.
        let report = [0x80u8, 0x80, 0x80, 0x80, 0x0F, 0x00];
        let st = dev.extract_gamepad(&report).expect("decode idle");
        assert!(st.left_x.abs() < 300, "LX center");
        assert!(st.left_y.abs() < 300, "LY center");
        assert_eq!(st.hat, Hat::Neutral);
        assert_eq!(st.buttons, 0, "no buttons pressed");
    }

    // ----- Never-panic on hostile / truncated input -----

    #[test]
    fn truncated_dualsense_does_not_panic() {
        // Only the report ID + one stick byte — must decode a partial neutral
        // state, never panic.
        let st = decode_dualsense(&[0x01u8, 0xFF]);
        assert_eq!(st.left_x, norm_u8_axis(0xFF));
        assert_eq!(st.right_trigger, 0, "missing bytes stay neutral");
        // Empty buffer is also safe.
        let _ = decode_dualsense(&[]);
        // Garbage of odd length.
        let _ = decode_dualsense(&[0xDE, 0xAD, 0xBE, 0xEF, 0x12]);
    }

    #[test]
    fn truncated_xbox_does_not_panic() {
        let st = decode_xbox(&[0x10u8, 0x00]); // buttons only (Start)
        assert!(st.pressed(Button::Start));
        assert_eq!(st.left_trigger, 0, "missing trigger byte stays 0");
        let _ = decode_xbox(&[]);
        let _ = decode_xbox(&[0xFF; 3]); // not enough for sticks
    }

    #[test]
    fn garbage_generic_report_does_not_panic() {
        let dev = HidDevice::parse(GAMEPAD_RDESC).expect("parse");
        // Report far too short for the declared 6-byte layout.
        let _ = dev.extract_gamepad(&[0x00]);
        let _ = dev.extract_gamepad(&[]);
        // Over-long garbage.
        let _ = dev.extract_gamepad(&[0xFF; 64]);
    }

    // ----- Rumble output encoding -----

    #[test]
    fn rumble_dualsense_output_encodes_motors() {
        let cmd = RumbleCommand {
            strong: 0xAA,
            weak: 0x55,
        };
        let out = cmd.to_dualsense_output();
        assert_eq!(out[0], 0x02, "report id");
        assert_eq!(out[3], 0x55, "weak / high-freq motor byte");
        assert_eq!(out[4], 0xAA, "strong / low-freq motor byte");
    }

    #[test]
    fn rumble_xbox_output_encodes_motors() {
        let cmd = RumbleCommand {
            strong: 0x11,
            weak: 0x22,
        };
        assert_eq!(cmd.to_xbox_output(), [0x00, 0x11, 0x22]);
    }
}
