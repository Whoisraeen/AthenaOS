//! USB HID boot-protocol keyboard driver.
//!
//! Concept §Architecture: "input subsystem [is] in-kernel hot path";
//! Concept §"User owns the machine" requires a working keyboard the
//! moment UEFI hands us the framebuffer.
//!
//! MasterChecklist Phase 2.1 — without HID, post-2015 laptops are mute
//! after `ExitBootServices`. This module owns the *report-to-event*
//! translation that lives between xHCI's interrupt-in endpoint delivery
//! and the kernel `input` event queue.
//!
//! ## Migration provenance (docs/REDOX_EXTRACTION_MAP.md row R05)
//!
//! Architectural pattern adapted from Redox OS `inputd` / `ps2d`
//! (`gitlab.redox-os.org/redox-os/base.git`, MIT). Redox runs these as
//! microkernel daemons that multiplex multiple input sources; AthenaOS
//! is hybrid and keeps the hot path in-kernel feeding the existing
//! `crate::input` queue. **No Redox source code is copied here** —
//! the byte-level report layout below is USB HID 1.11 Appendix B
//! ("Boot Interface Descriptors"), a public specification, and the
//! state-machine pattern of "diff against previous report → emit
//! KeyDown/KeyUp" is the standard idempotent boot-keyboard idiom that
//! every OS implements identically.
//!
//! When `base.git` is cloned locally for deep migration, this docstring
//! will be updated with the specific Redox file paths consulted.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use spin::Mutex;

use crate::input::{InputDeviceId, InputEventType, KeyCode, MouseButton};

// ─── USB HID Boot Keyboard Report (HID 1.11 §B.1) ─────────────────────────
//
// Boot Protocol Keyboard Input Report (Input, type=Report) is always
// 8 bytes:
//   byte 0:    Modifier bitmap (LCTRL=0x01, LSHIFT=0x02, LALT=0x04,
//              LGUI=0x08, RCTRL=0x10, RSHIFT=0x20, RALT=0x40, RGUI=0x80)
//   byte 1:    Reserved (typically 0)
//   byte 2..7: Up to 6 simultaneously-pressed HID Usage IDs from the
//              Keyboard/Keypad Page (0x07). Order is irrelevant. A
//              report of [0,0,0,0,0,0] in bytes 2..7 means "no keys
//              held"; if a key was held in the previous report and is
//              absent from this one, it's released.
//
// Apparent special case: usage 0x01 (ErrorRollOver) means the device
// can't report which keys are pressed because too many are held; we
// emit KeyUp for every previously-held key and stop tracking until
// the next clean report.

pub const BOOT_KEYBOARD_REPORT_LEN: usize = 8;
pub const HID_USAGE_ROLLOVER: u8 = 0x01;

#[derive(Debug, Clone, Copy, Default)]
pub struct BootKeyboardReport {
    pub modifiers: u8,
    pub keys: [u8; 6],
}

impl BootKeyboardReport {
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < BOOT_KEYBOARD_REPORT_LEN {
            return None;
        }
        Some(Self {
            modifiers: b[0],
            // b[1] is reserved per spec.
            keys: [b[2], b[3], b[4], b[5], b[6], b[7]],
        })
    }

    fn contains(&self, usage: u8) -> bool {
        usage != 0 && self.keys.iter().any(|&k| k == usage)
    }

    fn rollover(&self) -> bool {
        self.keys.iter().any(|&k| k == HID_USAGE_ROLLOVER)
    }
}

// ─── HID Usage Page 0x07 → input::KeyCode (HID 1.11 §10 + Usage Tables) ───
//
// Coverage chosen to map every key our `input::KeyCode` enum names
// without inventing variants. Anything we don't recognize → KeyCode::Unknown(u16).

fn hid_usage_to_keycode(usage: u8) -> KeyCode {
    use KeyCode::*;
    match usage {
        0x04 => A,
        0x05 => B,
        0x06 => C,
        0x07 => D,
        0x08 => E,
        0x09 => F,
        0x0A => G,
        0x0B => H,
        0x0C => I,
        0x0D => J,
        0x0E => K,
        0x0F => L,
        0x10 => M,
        0x11 => N,
        0x12 => O,
        0x13 => P,
        0x14 => Q,
        0x15 => R,
        0x16 => S,
        0x17 => T,
        0x18 => U,
        0x19 => V,
        0x1A => W,
        0x1B => X,
        0x1C => Y,
        0x1D => Z,
        0x1E => Key1,
        0x1F => Key2,
        0x20 => Key3,
        0x21 => Key4,
        0x22 => Key5,
        0x23 => Key6,
        0x24 => Key7,
        0x25 => Key8,
        0x26 => Key9,
        0x27 => Key0,
        0x28 => Enter,
        0x29 => Escape,
        0x2A => Backspace,
        0x2B => Tab,
        0x2C => Space,
        0x2D => Minus,
        0x2E => Equal,
        0x2F => LeftBracket,
        0x30 => RightBracket,
        0x31 => Backslash,
        0x33 => Semicolon,
        0x34 => Apostrophe,
        0x35 => Grave,
        0x36 => Comma,
        0x37 => Period,
        0x38 => Slash,
        0x39 => CapsLock,
        0x3A => F1,
        0x3B => F2,
        0x3C => F3,
        0x3D => F4,
        0x3E => F5,
        0x3F => F6,
        0x40 => F7,
        0x41 => F8,
        0x42 => F9,
        0x43 => F10,
        0x44 => F11,
        0x45 => F12,
        0x46 => PrintScreen,
        0x47 => ScrollLock,
        0x48 => Pause,
        0x49 => Insert,
        0x4A => Home,
        0x4B => PageUp,
        0x4C => Delete,
        0x4D => End,
        0x4E => PageDown,
        0x4F => Right,
        0x50 => Left,
        0x51 => Down,
        0x52 => Up,
        0x53 => NumLock,
        0x54 => NumpadDivide,
        0x55 => NumpadMultiply,
        0x56 => NumpadMinus,
        0x57 => NumpadPlus,
        0x58 => NumpadEnter,
        0x59 => Numpad1,
        0x5A => Numpad2,
        0x5B => Numpad3,
        0x5C => Numpad4,
        0x5D => Numpad5,
        0x5E => Numpad6,
        0x5F => Numpad7,
        0x60 => Numpad8,
        0x61 => Numpad9,
        0x62 => Numpad0,
        0x63 => NumpadDot,
        0x67 => NumpadEqual,
        0x65 => ContextMenu,
        0x66 => Power,
        // Modifiers when reported as usages (some keyboards do this in
        // addition to / instead of byte 0). We let the modifier byte
        // path own these; here we still translate so dedup is correct.
        0xE0 => LeftCtrl,
        0xE1 => LeftShift,
        0xE2 => LeftAlt,
        0xE3 => LeftMeta,
        0xE4 => RightCtrl,
        0xE5 => RightShift,
        0xE6 => RightAlt,
        0xE7 => RightMeta,
        other => Unknown(other as u16),
    }
}

// ─── Modifier bitmap → KeyCode ────────────────────────────────────────────

const MOD_BITS: [(u8, KeyCode, u8); 8] = [
    (0x01, KeyCode::LeftCtrl, 0x1D),
    (0x02, KeyCode::LeftShift, 0x2A),
    (0x04, KeyCode::LeftAlt, 0x38),
    (0x08, KeyCode::LeftMeta, 0x5B),
    (0x10, KeyCode::RightCtrl, 0x1D),
    (0x20, KeyCode::RightShift, 0x36),
    (0x40, KeyCode::RightAlt, 0x38),
    (0x80, KeyCode::RightMeta, 0x5C),
];

// ─── Per-device state + report processing ────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
struct DeviceState {
    last: BootKeyboardReport,
    valid: bool,
}

/// Single global slot for the (single) boot keyboard. Real systems may
/// have several HIDs; the multi-keyboard story comes after Phase 2.1.
static BOOT_KBD: Mutex<DeviceState> = Mutex::new(DeviceState {
    last: BootKeyboardReport {
        modifiers: 0,
        keys: [0; 6],
    },
    valid: false,
});

static REPORTS_TOTAL: spin::Mutex<u64> = spin::Mutex::new(0);
static KEYDOWNS_TOTAL: spin::Mutex<u64> = spin::Mutex::new(0);
static KEYUPS_TOTAL: spin::Mutex<u64> = spin::Mutex::new(0);
static ROLLOVERS_TOTAL: spin::Mutex<u64> = spin::Mutex::new(0);
static LEGACY_KEY_BRIDGED: spin::Mutex<u64> = spin::Mutex::new(0);
static LEGACY_MOUSE_BRIDGED: spin::Mutex<u64> = spin::Mutex::new(0);

/// Translate a HID Usage Page 0x07 keycode into a PS/2 *set-1* scancode for the
/// legacy `shell_runner::handle_key` bridge. Returns `(base_scancode, extended)`
/// where `extended == true` means the key is an `0xE0`-prefixed extended scancode
/// in set-1 (the entire navigation cluster — arrows, Home/End, PageUp/Down,
/// Insert/Delete). `shell_runner::handle_key` latches `0xE0` then evaluates the
/// next byte with `extended == true`; the Start-menu nav arms (`(true, 0x50)`
/// Down / `(true, 0x48)` Up, etc.) and every window-management arrow chord only
/// fire on the extended form, so a nav key MUST carry the prefix or the menu is
/// not keyboard-navigable (the SEV-2 bug: nav usages previously fell to `_ =>
/// None` and produced no scancode at all).
fn hid_usage_to_set1(usage: u8) -> Option<(u8, bool)> {
    // Navigation cluster — set-1 EXTENDED (0xE0-prefixed) scancodes. These are
    // the codes `shell_runner::handle_key` switches on with `extended == true`.
    match usage {
        0x52 => return Some((0x48, true)), // Up Arrow      -> E0 48
        0x51 => return Some((0x50, true)), // Down Arrow    -> E0 50
        0x50 => return Some((0x4B, true)), // Left Arrow    -> E0 4B
        0x4F => return Some((0x4D, true)), // Right Arrow   -> E0 4D
        0x4A => return Some((0x47, true)), // Home          -> E0 47
        0x4D => return Some((0x4F, true)), // End           -> E0 4F
        0x4B => return Some((0x49, true)), // Page Up       -> E0 49
        0x4E => return Some((0x51, true)), // Page Down     -> E0 51
        0x49 => return Some((0x52, true)), // Insert        -> E0 52
        0x4C => return Some((0x53, true)), // Delete        -> E0 53
        _ => {}
    }
    let sc = match usage {
        0x29 => Some(0x01), // Esc
        0x1E => Some(0x02),
        0x1F => Some(0x03),
        0x20 => Some(0x04),
        0x21 => Some(0x05),
        0x22 => Some(0x06),
        0x23 => Some(0x07),
        0x24 => Some(0x08),
        0x25 => Some(0x09),
        0x26 => Some(0x0A),
        0x27 => Some(0x0B),
        0x2D => Some(0x0C),
        0x2E => Some(0x0D),
        0x2A => Some(0x0E),
        0x2B => Some(0x0F),
        0x14 => Some(0x10),
        0x1A => Some(0x11),
        0x08 => Some(0x12),
        0x15 => Some(0x13),
        0x17 => Some(0x14),
        0x1C => Some(0x15),
        0x18 => Some(0x16),
        0x0C => Some(0x17),
        0x12 => Some(0x18),
        0x13 => Some(0x19),
        0x2F => Some(0x1A),
        0x30 => Some(0x1B),
        0x28 => Some(0x1C),
        0x04 => Some(0x1E),
        0x16 => Some(0x1F),
        0x07 => Some(0x20),
        0x09 => Some(0x21),
        0x0A => Some(0x22),
        0x0B => Some(0x23),
        0x0D => Some(0x24),
        0x0E => Some(0x25),
        0x0F => Some(0x26),
        0x33 => Some(0x27),
        0x34 => Some(0x28),
        0x35 => Some(0x29),
        0x31 => Some(0x2B),
        0x1D => Some(0x2C),
        0x1B => Some(0x2D),
        0x06 => Some(0x2E),
        0x19 => Some(0x2F),
        0x05 => Some(0x30),
        0x11 => Some(0x31),
        0x10 => Some(0x32),
        0x36 => Some(0x33),
        0x37 => Some(0x34),
        0x38 => Some(0x35),
        0x2C => Some(0x39),
        0x3A => Some(0x3B),
        0x3B => Some(0x3C),
        0x3C => Some(0x3D),
        0x3D => Some(0x3E),
        0x3E => Some(0x3F),
        0x3F => Some(0x40),
        0x40 => Some(0x41),
        0x41 => Some(0x42),
        0x42 => Some(0x43),
        0x43 => Some(0x44),
        0x44 => Some(0x57),
        0x45 => Some(0x58),
        0xE0 => Some(0x1D),
        0xE1 => Some(0x2A),
        0xE2 => Some(0x38),
        0xE3 => Some(0x5B),
        0xE4 => Some(0x1D),
        0xE5 => Some(0x36),
        0xE6 => Some(0x38),
        0xE7 => Some(0x5C),
        _ => None,
    };
    // Non-nav keys are all set-1 base scancodes (not extended).
    sc.map(|s| (s, false))
}

fn bridge_scancode(scancode: u8) {
    let msg = crate::ipc::Message {
        msg_type: 1,
        arg1: scancode as u64,
        arg2: 0,
        arg3: 0,
    };
    if let Some(mut ipc) = crate::ipc::IPC.try_lock() {
        let _ = ipc.send(crate::ipc::KEYBOARD_CHANNEL, msg);
    }
    crate::scheduler::unblock_receivers(crate::ipc::KEYBOARD_CHANNEL);
    if let Some(tid) = crate::compositor::focused_task_id() {
        crate::scheduler::with_task_by_id(tid, |task| task.push_key(scancode));
    }
    crate::shell_runner::handle_key(scancode);
    *LEGACY_KEY_BRIDGED.lock() += 1;
}

/// Bridge a translated set-1 key transition. For extended (`0xE0`-prefixed)
/// keys — the whole nav cluster — emit the `0xE0` prefix byte FIRST so
/// `shell_runner::handle_key` latches `extended` before the real key byte
/// arrives; without it the Start menu / window arrows never fire (SEV-2). The
/// release form is `base | 0x80` (the prefix is NOT OR'd with 0x80).
fn bridge_key(base: u8, extended: bool, is_release: bool) {
    if extended {
        bridge_scancode(0xE0);
    }
    let code = if is_release { base | 0x80 } else { base };
    bridge_scancode(code);
}

fn bridge_mouse_packet(dx: i32, dy: i32, buttons: u8) {
    let msg = crate::ipc::Message {
        msg_type: 2,
        arg1: dx as i64 as u64,
        arg2: dy as i64 as u64,
        arg3: buttons as u64,
    };
    if let Some(mut ipc) = crate::ipc::IPC.try_lock() {
        let _ = ipc.send(crate::ipc::MOUSE_CHANNEL, msg);
    }
    crate::scheduler::unblock_receivers(crate::ipc::MOUSE_CHANNEL);
    crate::compositor::move_cursor(dx, -dy);
    if let Some(tid) = crate::compositor::focused_task_id() {
        crate::scheduler::with_task_by_id(tid, |task| {
            task.push_mouse(dx as i16, -dy as i16, buttons);
        });
    }
    crate::shell_runner::handle_mouse(dx, -dy, buttons);
    *LEGACY_MOUSE_BRIDGED.lock() += 1;
}

/// Drive one boot-protocol report through the diff state machine and
/// push KeyDown / KeyUp events to the kernel input queue for each
/// transition. Idempotent: re-submitting the same report emits nothing.
pub fn dispatch_boot_report(device_id: InputDeviceId, raw: &[u8]) {
    let report = match BootKeyboardReport::from_bytes(raw) {
        Some(r) => r,
        None => return,
    };

    *REPORTS_TOTAL.lock() += 1;

    let mut state = BOOT_KBD.lock();
    let prev = if state.valid {
        state.last
    } else {
        BootKeyboardReport::default()
    };

    // Rollover error: release everything previously held, mark invalid
    // so the next clean report starts from zero. Per HID 1.11 §B.1.
    if report.rollover() {
        *ROLLOVERS_TOTAL.lock() += 1;
        if state.valid {
            for (bit, code, scancode) in MOD_BITS.iter() {
                if prev.modifiers & bit != 0 {
                    crate::input::push_event(device_id, InputEventType::KeyUp(*code));
                    bridge_scancode(*scancode | 0x80);
                    *KEYUPS_TOTAL.lock() += 1;
                }
            }
            for k in prev.keys.iter().copied().filter(|&k| k != 0) {
                crate::input::push_event(device_id, InputEventType::KeyUp(hid_usage_to_keycode(k)));
                if let Some((sc, ext)) = hid_usage_to_set1(k) {
                    bridge_key(sc, ext, true);
                }
                *KEYUPS_TOTAL.lock() += 1;
            }
        }
        state.valid = false;
        return;
    }

    // Accessibility key filters (Phase 19.3 — sticky/slow/bounce keys) sit
    // between the HID report diff and the input queue. With all filters off
    // (the default) `filter_key` is an exact passthrough, so this is invisible
    // until a user enables a feature from Settings. The `bridge_*` scancode path
    // (AthBridge/raw consumers) is intentionally NOT filtered — sticky keys is a
    // desktop affordance for input-queue consumers.
    let now_ms = crate::aurora::aurora_now_ms();

    // Modifier diff: each bit individually.
    for (bit, code, scancode) in MOD_BITS.iter() {
        let was = prev.modifiers & bit != 0;
        let now = report.modifiers & bit != 0;
        if !was && now {
            for (c, d) in crate::a11y_input::filter_key(*code, true, now_ms) {
                crate::input::push_event(
                    device_id,
                    if d {
                        InputEventType::KeyDown(c)
                    } else {
                        InputEventType::KeyUp(c)
                    },
                );
            }
            bridge_scancode(*scancode);
            *KEYDOWNS_TOTAL.lock() += 1;
        } else if was && !now {
            for (c, d) in crate::a11y_input::filter_key(*code, false, now_ms) {
                crate::input::push_event(
                    device_id,
                    if d {
                        InputEventType::KeyDown(c)
                    } else {
                        InputEventType::KeyUp(c)
                    },
                );
            }
            bridge_scancode(*scancode | 0x80);
            *KEYUPS_TOTAL.lock() += 1;
        }
    }

    // Released keys: in prev but not in current.
    for k in prev.keys.iter().copied().filter(|&k| k != 0) {
        if !report.contains(k) {
            let code = hid_usage_to_keycode(k);
            for (c, d) in crate::a11y_input::filter_key(code, false, now_ms) {
                crate::input::push_event(
                    device_id,
                    if d {
                        InputEventType::KeyDown(c)
                    } else {
                        InputEventType::KeyUp(c)
                    },
                );
            }
            if let Some((sc, ext)) = hid_usage_to_set1(k) {
                bridge_key(sc, ext, true);
            }
            *KEYUPS_TOTAL.lock() += 1;
        }
    }

    // Pressed keys: in current but not in prev.
    for k in report.keys.iter().copied().filter(|&k| k != 0) {
        if !prev.contains(k) {
            let code = hid_usage_to_keycode(k);
            for (c, d) in crate::a11y_input::filter_key(code, true, now_ms) {
                crate::input::push_event(
                    device_id,
                    if d {
                        InputEventType::KeyDown(c)
                    } else {
                        InputEventType::KeyUp(c)
                    },
                );
            }
            if let Some((sc, ext)) = hid_usage_to_set1(k) {
                bridge_key(sc, ext, false);
            }
            *KEYDOWNS_TOTAL.lock() += 1;
        }
    }

    // input→photon telemetry: a real key/modifier transition is user input
    // that should reflect on screen (char echo / UI). Skip unchanged repeat
    // reports so the latency metric only times input that changes state.
    if report.modifiers != prev.modifiers || report.keys != prev.keys {
        crate::perf::record_input_event();
    }

    state.last = report;
    state.valid = true;
}

// ─── USB HID Boot Mouse Report (HID 1.11 §B.2) ────────────────────────────
//
// Boot Protocol Mouse Input Report is 3 bytes (4 with optional wheel):
//   byte 0:    Button bitmap (bit 0 = Left, 1 = Right, 2 = Middle,
//              3..7 typically unused on boot protocol; some 5-button
//              mice surface Back=bit 3, Forward=bit 4)
//   byte 1:    X delta as signed 8-bit (-127..127, relative motion)
//   byte 2:    Y delta as signed 8-bit (positive = down, USB convention)
//   byte 3:    Wheel delta (signed 8-bit) — optional; absent on strict
//              boot protocol but most mice send it. We treat reports
//              of length 3 (no wheel) and 4 (with wheel) as valid.

pub const BOOT_MOUSE_REPORT_MIN: usize = 3;
pub const BOOT_MOUSE_REPORT_MAX: usize = 4;

#[derive(Debug, Clone, Copy, Default)]
pub struct BootMouseReport {
    pub buttons: u8,
    pub dx: i8,
    pub dy: i8,
    pub wheel: i8,
}

impl BootMouseReport {
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < BOOT_MOUSE_REPORT_MIN {
            return None;
        }
        Some(Self {
            buttons: b[0],
            dx: b[1] as i8,
            dy: b[2] as i8,
            wheel: if b.len() >= 4 { b[3] as i8 } else { 0 },
        })
    }
}

const MOUSE_BTNS: [(u8, MouseButton); 5] = [
    (0x01, MouseButton::Left),
    (0x02, MouseButton::Right),
    (0x04, MouseButton::Middle),
    (0x08, MouseButton::Back),
    (0x10, MouseButton::Forward),
];

#[derive(Debug, Clone, Copy, Default)]
struct MouseState {
    last_buttons: u8,
    valid: bool,
}

static BOOT_MOUSE: Mutex<MouseState> = Mutex::new(MouseState {
    last_buttons: 0,
    valid: false,
});

static MOUSE_REPORTS_TOTAL: spin::Mutex<u64> = spin::Mutex::new(0);
static MOUSE_BUTTONS_DOWN: spin::Mutex<u64> = spin::Mutex::new(0);
static MOUSE_BUTTONS_UP: spin::Mutex<u64> = spin::Mutex::new(0);
static MOUSE_MOTIONS_NONZERO: spin::Mutex<u64> = spin::Mutex::new(0);
static MOUSE_WHEELS_NONZERO: spin::Mutex<u64> = spin::Mutex::new(0);

/// Drive one boot-protocol mouse report through the diff state machine
/// and push button / motion / wheel events to the kernel input queue.
/// Motion + wheel are inherently deltas so they emit on every nonzero
/// report; buttons diff against the last seen state.
pub fn dispatch_boot_mouse(device_id: InputDeviceId, raw: &[u8]) {
    let report = match BootMouseReport::from_bytes(raw) {
        Some(r) => r,
        None => return,
    };
    // Boot protocol carries i8 deltas — widen to the full-resolution core.
    dispatch_mouse_core(
        device_id,
        report.buttons,
        report.dx as i32,
        report.dy as i32,
        report.wheel as i32,
    );
}

/// Dispatch a FULL-RESOLUTION pointer update (i32 deltas) — for report-protocol
/// mice decoded by `athhid` from a HID report descriptor. A gaming mouse (the
/// Athena Razer) sends 16-bit X/Y; routing it through the 4-byte BOOT report
/// would clamp each delta to ±127 and the cursor crawls (observed on iron). This
/// path preserves the native resolution end-to-end (the cursor consumer,
/// `compositor::move_cursor`, takes i32). `buttons` is the boot-layout bitmap
/// (bit0=left, bit1=right, bit2=middle), which is exactly athhid's low button bits.
pub fn dispatch_mouse_delta(device_id: InputDeviceId, dx: i32, dy: i32, wheel: i32, buttons: u8) {
    dispatch_mouse_core(device_id, buttons, dx, dy, wheel);
}

/// Shared pointer-dispatch core: button-edge diff + motion/wheel events + the
/// legacy IPC/cursor bridge. Single-sourced by both the boot and report paths.
fn dispatch_mouse_core(device_id: InputDeviceId, buttons: u8, dx: i32, dy: i32, wheel: i32) {
    *MOUSE_REPORTS_TOTAL.lock() += 1;

    let mut state = BOOT_MOUSE.lock();
    let prev_buttons = if state.valid { state.last_buttons } else { 0 };

    // Button diff
    for (bit, btn) in MOUSE_BTNS.iter() {
        let was = prev_buttons & bit != 0;
        let now = buttons & bit != 0;
        if !was && now {
            crate::input::push_event(device_id, InputEventType::MouseButtonDown(*btn));
            *MOUSE_BUTTONS_DOWN.lock() += 1;
        } else if was && !now {
            crate::input::push_event(device_id, InputEventType::MouseButtonUp(*btn));
            *MOUSE_BUTTONS_UP.lock() += 1;
        }
    }

    // Motion delta — emit only on nonzero so idle reports don't flood the queue
    if dx != 0 || dy != 0 {
        crate::input::push_event(device_id, InputEventType::MouseMove { dx, dy });
        *MOUSE_MOTIONS_NONZERO.lock() += 1;
    }

    // Wheel delta
    if wheel != 0 {
        crate::input::push_event(device_id, InputEventType::MouseScroll { dx: 0, dy: wheel });
        *MOUSE_WHEELS_NONZERO.lock() += 1;
    }

    // Mirror the PS/2 IRQ routing path so USB HID is a drop-in source for
    // existing task polling, IPC consumers, cursor movement, and shell UI.
    let btns = buttons & 0x07;
    let has_delta = dx != 0 || dy != 0 || wheel != 0;
    let button_changed = state.valid && prev_buttons != buttons;
    if has_delta || button_changed {
        bridge_mouse_packet(dx, dy, btns);
        // input→photon telemetry: a pointer move/click is user input that
        // should reach the screen (cursor/UI). Arm the latency clock. Idle
        // (zero-delta, no-button) reports are skipped so the metric only times
        // input that actually produces a visible change. /proc/athena/perf.
        crate::perf::record_input_event();
    }

    state.last_buttons = buttons;
    state.valid = true;
}

// ─── R10 4-artifact contract ─────────────────────────────────────────────

pub fn init() {
    // No hardware probe here — the xHCI driver discovers HID-class
    // devices and calls back into `dispatch_boot_report` (keyboards)
    // or `dispatch_boot_mouse` (mice). We only own the report parsers
    // + state machines.
    crate::serial_println!("[ OK ] USB HID boot-keyboard + boot-mouse parsers (R05)");
}

/// FAIL-able pure model of the HID-usage -> set-1 nav-cluster mapping (SEV-2).
/// Every arrow / Home / End / PageUp/Down / Insert / Delete usage MUST map to
/// its standard set-1 EXTENDED scancode, or the Start menu and window-management
/// arrow chords are not keyboard-navigable. Asserts the EXACT (scancode,
/// extended) pairs `shell_runner::handle_key` switches on — so a regression that
/// drops a mapping (back to `_ => None`) or forgets the `0xE0`-prefix flag fails
/// loudly on every boot. Returns Err(reason) on the first mismatch.
fn hid_nav_keymap_check() -> Result<(), &'static str> {
    // (HID usage, expected set-1 base scancode, expected extended).
    const NAV: [(u8, u8, bool); 10] = [
        (0x52, 0x48, true), // Up
        (0x51, 0x50, true), // Down
        (0x50, 0x4B, true), // Left
        (0x4F, 0x4D, true), // Right
        (0x4A, 0x47, true), // Home
        (0x4D, 0x4F, true), // End
        (0x4B, 0x49, true), // PageUp
        (0x4E, 0x51, true), // PageDown
        (0x49, 0x52, true), // Insert
        (0x4C, 0x53, true), // Delete
    ];
    for (usage, want_sc, want_ext) in NAV.iter().copied() {
        match hid_usage_to_set1(usage) {
            Some((sc, ext)) if sc == want_sc && ext == want_ext => {}
            Some((sc, ext)) => {
                let _ = (sc, ext);
                return Err("nav usage maps to wrong set-1 scancode/extended flag");
            }
            None => return Err("nav usage unmapped (would not navigate the Start menu)"),
        }
    }
    // A plain letter must remain a NON-extended base scancode (regression guard
    // that the extended branch didn't leak into ordinary keys).
    match hid_usage_to_set1(0x04) {
        Some((0x1E, false)) => {}
        _ => return Err("letter 'A' regressed (must be set-1 0x1E, non-extended)"),
    }
    Ok(())
}

pub fn run_boot_smoketest() {
    let device_id: InputDeviceId = 0xB007_C0DE_u64;

    // Nav-cluster keymap model — proves arrow/Home/End/PageUp-Down/Insert/Delete
    // reach shell_runner as EXTENDED set-1 scancodes (the SEV-2 fix).
    match hid_nav_keymap_check() {
        Ok(()) => crate::serial_println!(
            "[usb-hid] nav-keymap model: 10 nav usages -> set-1 extended, letters base -> PASS"
        ),
        Err(reason) => crate::serial_println!(
            "[usb-hid] nav-keymap model -> FAIL: {} (Start menu not keyboard-navigable)",
            reason
        ),
    }

    // ── Keyboard ────────────────────────────────────────────────────────
    //
    // Frame 1: nothing pressed.
    // Frame 2: LeftShift + 'A' pressed.
    // Frame 3: everything released.
    // Expected deltas: reports+=3, keydowns+=2 (LShift, A), keyups+=2.
    let before_kb_reports = *REPORTS_TOTAL.lock();
    let before_kb_downs = *KEYDOWNS_TOTAL.lock();
    let before_kb_ups = *KEYUPS_TOTAL.lock();
    let before_key_bridge = *LEGACY_KEY_BRIDGED.lock();

    let kf1: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 0];
    let kf2: [u8; 8] = [0x02, 0, 0x04, 0, 0, 0, 0, 0]; // LSHIFT bit + 'a'
    let kf3: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 0];

    dispatch_boot_report(device_id, &kf1);
    dispatch_boot_report(device_id, &kf2);
    dispatch_boot_report(device_id, &kf3);

    let kdr = *REPORTS_TOTAL.lock() - before_kb_reports;
    let kdd = *KEYDOWNS_TOTAL.lock() - before_kb_downs;
    let kdu = *KEYUPS_TOTAL.lock() - before_kb_ups;

    // ── Mouse ───────────────────────────────────────────────────────────
    //
    // Frame 1: idle (no buttons, no motion).
    // Frame 2: left button pressed + dx=+5, dy=-2, wheel=+1.
    // Frame 3: button released, motion stop, wheel zero.
    // Expected deltas:
    //   reports+=3
    //   button_downs+=1 (Left)
    //   button_ups+=1   (Left)
    //   motions_nonzero+=1
    //   wheels_nonzero+=1
    let before_m_reports = *MOUSE_REPORTS_TOTAL.lock();
    let before_m_down = *MOUSE_BUTTONS_DOWN.lock();
    let before_m_up = *MOUSE_BUTTONS_UP.lock();
    let before_m_motion = *MOUSE_MOTIONS_NONZERO.lock();
    let before_m_wheel = *MOUSE_WHEELS_NONZERO.lock();
    let before_mouse_bridge = *LEGACY_MOUSE_BRIDGED.lock();

    let mf1: [u8; 4] = [0x00, 0, 0, 0];
    let mf2: [u8; 4] = [0x01, 5, (-2_i8) as u8, 1]; // Left button + motion + wheel
    let mf3: [u8; 4] = [0x00, 0, 0, 0];

    dispatch_boot_mouse(device_id, &mf1);
    dispatch_boot_mouse(device_id, &mf2);
    dispatch_boot_mouse(device_id, &mf3);

    let mdr = *MOUSE_REPORTS_TOTAL.lock() - before_m_reports;
    let mdd = *MOUSE_BUTTONS_DOWN.lock() - before_m_down;
    let mdu = *MOUSE_BUTTONS_UP.lock() - before_m_up;
    let mdm = *MOUSE_MOTIONS_NONZERO.lock() - before_m_motion;
    let mdw = *MOUSE_WHEELS_NONZERO.lock() - before_m_wheel;
    let key_bridge = *LEGACY_KEY_BRIDGED.lock() - before_key_bridge;
    let mouse_bridge = *LEGACY_MOUSE_BRIDGED.lock() - before_mouse_bridge;

    let kb_pass = kdr == 3 && kdd == 2 && kdu == 2;
    let mouse_pass = mdr == 3 && mdd == 1 && mdu == 1 && mdm == 1 && mdw == 1;
    let bridge_pass = key_bridge == 4 && mouse_bridge == 2;

    crate::serial_println!(
        "[usb-hid] smoketest kbd:   reports+={} keydowns+={} keyups+={} -> {}",
        kdr,
        kdd,
        kdu,
        if kb_pass { "PASS" } else { "FAIL" },
    );
    crate::serial_println!(
        "[usb-hid] smoketest mouse: reports+={} btn_dn+={} btn_up+={} motion+={} wheel+={} -> {}",
        mdr,
        mdd,
        mdu,
        mdm,
        mdw,
        if mouse_pass { "PASS" } else { "FAIL" },
    );
    crate::serial_println!(
        "[usb-hid] smoketest legacy bridge: key_packets+={} mouse_packets+={} -> {}",
        key_bridge,
        mouse_bridge,
        if bridge_pass { "PASS" } else { "FAIL" },
    );
}

pub fn dump_text() -> String {
    alloc::format!(
        "# USB HID boot keyboard\n\
         kbd_reports:    {}\n\
         kbd_keydowns:   {}\n\
         kbd_keyups:     {}\n\
         kbd_rollovers:  {}\n\
         \n\
         # USB HID boot mouse\n\
         mouse_reports:  {}\n\
         mouse_btn_down: {}\n\
         mouse_btn_up:   {}\n\
         mouse_motion:   {}\n\
         mouse_wheel:    {}\n\
         legacy_key_bridge:   {}\n\
         legacy_mouse_bridge: {}\n",
        *REPORTS_TOTAL.lock(),
        *KEYDOWNS_TOTAL.lock(),
        *KEYUPS_TOTAL.lock(),
        *ROLLOVERS_TOTAL.lock(),
        *MOUSE_REPORTS_TOTAL.lock(),
        *MOUSE_BUTTONS_DOWN.lock(),
        *MOUSE_BUTTONS_UP.lock(),
        *MOUSE_MOTIONS_NONZERO.lock(),
        *MOUSE_WHEELS_NONZERO.lock(),
        *LEGACY_KEY_BRIDGED.lock(),
        *LEGACY_MOUSE_BRIDGED.lock(),
    )
}
