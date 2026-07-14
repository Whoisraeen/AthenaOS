//! AthenaOS HID report-descriptor decoding (Concept §Input — "Gaming isn't a
//! mode": gaming mice/keyboards must work, and they frequently do NOT speak the
//! USB HID *boot* protocol — they report through a report descriptor with
//! vendor layouts, 16-bit X/Y, report IDs, extra buttons, etc.).
//!
//! Our in-kernel xHCI path currently parses only the fixed boot-protocol layout
//! (`kernel/src/usb_hid.rs`). This crate adds report-descriptor-driven decoding
//! by wrapping the `hidreport` crate (MIT) — the *same* parser Redox OS relies
//! on (`usbhidd` -> `rehid` -> `hidreport`). We harvest the parser only; the
//! USB transport stays AthenaOS's in-kernel xHCI driver. See
//! `docs/REDOX_EXTRACTION_MAP.md`.
//!
//! `#![no_std]` + `alloc` so the kernel can depend on this once the mouse test
//! confirms boot-protocol decoding is insufficient. Pure logic — host-KAT'd
//! (`cargo test -p raehid`), no hardware.

#![no_std]

extern crate alloc;

/// Gamepad / controller decoding (DualSense, Xbox, generic HID gamepad) →
/// normalized [`gamepad::GamepadState`]. Concept §GameOS: "gaming isn't a mode".
pub mod gamepad;

/// Mouse polling-rate control (125 Hz … 8000 Hz) — rate ↔ USB `bInterval`
/// conversion, speed-aware, validated against the real Razer DeathAdder.
pub mod polling;

use alloc::vec::Vec;
use hidreport::{Field, Report, ReportDescriptor};

/// USB HID usage pages we care about for pointing/keying devices.
const PAGE_GENERIC_DESKTOP: u16 = 0x01;
const PAGE_KEYBOARD: u16 = 0x07;
const PAGE_BUTTON: u16 = 0x09;
/// Generic Desktop usages.
const USAGE_X: u16 = 0x30;
const USAGE_Y: u16 = 0x31;
const USAGE_WHEEL: u16 = 0x38;

/// A decoded pointer report: relative deltas + a button bitmap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseDelta {
    pub dx: i32,
    pub dy: i32,
    pub wheel: i32,
    /// Button bitmap, bit 0 = button 1 (left), bit 1 = button 2 (right), …
    pub buttons: u16,
}

/// A decoded keyboard report: modifier bitmap + the set of pressed key usages
/// (HID Keyboard/Keypad page codes — the same values a boot keyboard reports in
/// its keycode array).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyboardState {
    /// Modifier bitmap (left/right ctrl/shift/alt/gui), HID usages 0xE0..=0xE7.
    pub modifiers: u8,
    /// Pressed key usage codes (non-zero entries of the keycode array).
    pub keys: Vec<u8>,
}

/// What kind of input device a report descriptor describes (for routing into
/// the boot-protocol input pipeline). Determined from the declared usages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidKind {
    Mouse,
    Keyboard,
    Other,
}

impl MouseDelta {
    /// Pack into the 4-byte USB HID *boot* mouse report layout
    /// (`[buttons, dx, dy, wheel]`, dx/dy/wheel as signed i8) so report-protocol
    /// devices can reuse the kernel's existing boot-mouse dispatch path. High-res
    /// 16-bit deltas are clamped to the i8 boot range — fine for cursor motion;
    /// precision-sensitive paths can read the raw `MouseDelta` instead.
    pub fn to_boot_report(&self) -> [u8; 4] {
        let clamp = |v: i32| v.clamp(-127, 127) as i8 as u8;
        [
            (self.buttons & 0xFF) as u8,
            clamp(self.dx),
            clamp(self.dy),
            clamp(self.wheel),
        ]
    }
}

impl KeyboardState {
    /// Pack into the 8-byte USB HID *boot* keyboard report layout
    /// (`[modifiers, reserved, key0..key5]`) so report-protocol keyboards reuse
    /// the kernel's existing boot-keyboard dispatch. Extra keys beyond 6 are
    /// dropped (boot-report limit; matches real boot keyboards' rollover).
    pub fn to_boot_report(&self) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0] = self.modifiers;
        for (slot, &k) in out[2..].iter_mut().zip(self.keys.iter()) {
            *slot = k;
        }
        out
    }
}

/// A parsed HID report descriptor, ready to decode raw input reports.
pub struct HidDevice {
    rdesc: ReportDescriptor,
}

impl core::fmt::Debug for HidDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The full ReportDescriptor isn't Debug (and is large); summarise by kind
        // so containing structs can derive Debug.
        f.debug_struct("HidDevice")
            .field("kind", &self.kind())
            .finish()
    }
}

impl HidDevice {
    /// Parse a raw HID report descriptor (descriptor type 0x22 bytes). Returns
    /// `None` if the descriptor is malformed.
    pub fn parse(descriptor_bytes: &[u8]) -> Option<Self> {
        ReportDescriptor::try_from(descriptor_bytes)
            .ok()
            .map(|rdesc| Self { rdesc })
    }

    /// Select the input report matching this raw report. Devices that declare
    /// report IDs prefix each report with its ID (byte 0); devices without
    /// report IDs have a single input report and no prefix byte.
    pub(crate) fn report_for<'a>(&'a self, report: &[u8]) -> Option<&'a dyn ReportRef> {
        // NEVER-PANIC: `find_input_report` indexes `report[0]` unchecked, so an
        // empty buffer (a hostile/zero-length USB read) would panic. Bail out to
        // the no-report path before touching it.
        if report.is_empty() {
            return None;
        }
        // `find_input_report` keys off byte 0 (the report ID). For report-ID-less
        // devices it still resolves the single report; if not, fall back to the
        // first declared input report.
        if let Some(r) = self.rdesc.find_input_report(report) {
            return Some(r as &dyn ReportRef);
        }
        self.rdesc
            .input_reports()
            .first()
            .map(|r| r as &dyn ReportRef)
    }

    /// Decode a raw input report as a pointer (mouse). Returns `None` only if no
    /// input report layout applies; an all-zero report still yields a zeroed
    /// `MouseDelta` (no movement), which is a valid result.
    pub fn extract_mouse(&self, report: &[u8]) -> Option<MouseDelta> {
        let r = self.report_for(report)?;
        let mut md = MouseDelta::default();
        for field in r.fields() {
            let Field::Variable(v) = field else { continue };
            let page: u16 = u16::from(v.usage.usage_page);
            let id: u16 = u16::from(v.usage.usage_id);
            let Ok(fv) = v.extract(report) else { continue };
            let val: i32 = i32::from(&fv);
            match (page, id) {
                (PAGE_GENERIC_DESKTOP, USAGE_X) => md.dx = val,
                (PAGE_GENERIC_DESKTOP, USAGE_Y) => md.dy = val,
                (PAGE_GENERIC_DESKTOP, USAGE_WHEEL) => md.wheel = val,
                (PAGE_BUTTON, b) if (1..=16).contains(&b) => {
                    if val != 0 {
                        md.buttons |= 1 << (b - 1);
                    }
                }
                _ => {}
            }
        }
        Some(md)
    }

    /// Classify the device from its declared usages so the kernel knows whether
    /// to route reports through the mouse or keyboard pipeline. A descriptor with
    /// Generic-Desktop X/Y variable fields is a pointer; one with Keyboard-page
    /// fields is a keyboard. (Some composite devices declare both — pointer wins,
    /// matching how the boot path already prioritises mice.)
    pub fn kind(&self) -> HidKind {
        let mut has_pointer = false;
        let mut has_keyboard = false;
        for r in self.rdesc.input_reports() {
            for field in Report::fields(r) {
                if let Field::Variable(v) = field {
                    let page: u16 = u16::from(v.usage.usage_page);
                    let id: u16 = u16::from(v.usage.usage_id);
                    if page == PAGE_GENERIC_DESKTOP && (id == USAGE_X || id == USAGE_Y) {
                        has_pointer = true;
                    }
                    if page == PAGE_KEYBOARD {
                        has_keyboard = true;
                    }
                }
            }
        }
        if has_pointer {
            HidKind::Mouse
        } else if has_keyboard {
            HidKind::Keyboard
        } else {
            HidKind::Other
        }
    }

    /// Decode a raw input report as a keyboard. Modifier keys arrive as variable
    /// bit fields (usage page 0x07, usages 0xE0..=0xE7); the main keycodes arrive
    /// as an array field whose values are HID key usages.
    pub fn extract_keyboard(&self, report: &[u8]) -> Option<KeyboardState> {
        let r = self.report_for(report)?;
        let mut ks = KeyboardState::default();
        for field in r.fields() {
            match field {
                Field::Variable(v) => {
                    let page: u16 = u16::from(v.usage.usage_page);
                    let id: u16 = u16::from(v.usage.usage_id);
                    if page == PAGE_KEYBOARD && (0xE0..=0xE7).contains(&id) {
                        if let Ok(fv) = v.extract(report) {
                            if i32::from(&fv) != 0 {
                                ks.modifiers |= 1 << (id - 0xE0);
                            }
                        }
                    }
                }
                Field::Array(a) => {
                    if let Ok(values) = a.extract(report) {
                        for fv in values.iter() {
                            let code = u32::from(fv);
                            if code != 0 {
                                ks.keys.push(code as u8);
                            }
                        }
                    }
                }
                Field::Constant(_) => {}
            }
        }
        Some(ks)
    }
}

/// Object-safe view over `hidreport`'s `impl Report` so `report_for` can return
/// a single type from both the `find_input_report` and `input_reports().first()`
/// branches.
pub(crate) trait ReportRef {
    fn fields(&self) -> &[Field];
    /// Minimum raw-report length (bytes) the declared fields cover. A report
    /// shorter than this must NOT be handed to field extraction — `hidreport`'s
    /// `extract` slices the buffer unchecked and panics on a short read, which is
    /// unacceptable for untrusted USB-device input. Callers guard with this.
    fn size_in_bytes(&self) -> usize;
}

impl<T: Report> ReportRef for T {
    fn fields(&self) -> &[Field] {
        Report::fields(self)
    }
    fn size_in_bytes(&self) -> usize {
        Report::size_in_bytes(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    /// Standard USB HID boot-mouse report descriptor (no report ID): 3 button
    /// bits + 5 pad bits + X(8) + Y(8) + Wheel(8) → a 4-byte report.
    const BOOT_MOUSE_RDESC: &[u8] = &[
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x02, // Usage (Mouse)
        0xA1, 0x01, // Collection (Application)
        0x09, 0x01, //   Usage (Pointer)
        0xA1, 0x00, //   Collection (Physical)
        0x05, 0x09, //     Usage Page (Button)
        0x19, 0x01, //     Usage Minimum (1)
        0x29, 0x03, //     Usage Maximum (3)
        0x15, 0x00, //     Logical Minimum (0)
        0x25, 0x01, //     Logical Maximum (1)
        0x95, 0x03, //     Report Count (3)
        0x75, 0x01, //     Report Size (1)
        0x81, 0x02, //     Input (Data,Var,Abs)
        0x95, 0x01, //     Report Count (1)
        0x75, 0x05, //     Report Size (5)
        0x81, 0x03, //     Input (Cnst,Var,Abs) — padding
        0x05, 0x01, //     Usage Page (Generic Desktop)
        0x09, 0x30, //     Usage (X)
        0x09, 0x31, //     Usage (Y)
        0x09, 0x38, //     Usage (Wheel)
        0x15, 0x81, //     Logical Minimum (-127)
        0x25, 0x7F, //     Logical Maximum (127)
        0x75, 0x08, //     Report Size (8)
        0x95, 0x03, //     Report Count (3)
        0x81, 0x06, //     Input (Data,Var,Rel)
        0xC0, //   End Collection
        0xC0, // End Collection
    ];

    #[test]
    fn parses_boot_mouse_descriptor() {
        assert!(
            HidDevice::parse(BOOT_MOUSE_RDESC).is_some(),
            "boot-mouse report descriptor must parse"
        );
        // A descriptor that ends mid-item must NOT parse as valid.
        assert!(HidDevice::parse(&[0x05]).is_none());
    }

    #[test]
    fn extracts_mouse_movement_and_buttons() {
        let dev = HidDevice::parse(BOOT_MOUSE_RDESC).expect("parse");
        // byte0 = buttons (0b001 = left), byte1 = X(+5), byte2 = Y(-5), byte3 = wheel(+1)
        let report = [0x01u8, 0x05, 0xFB, 0x01];
        let md = dev.extract_mouse(&report).expect("decode");
        assert_eq!(md.dx, 5, "X delta");
        assert_eq!(md.dy, -5, "Y delta (signed)");
        assert_eq!(md.wheel, 1, "wheel delta");
        assert_eq!(md.buttons & 0b1, 0b1, "left button pressed");
        assert_eq!(md.buttons & 0b10, 0, "right button not pressed");
    }

    #[test]
    fn idle_mouse_report_is_zeroed_not_failed() {
        let dev = HidDevice::parse(BOOT_MOUSE_RDESC).expect("parse");
        let md = dev.extract_mouse(&[0u8, 0, 0, 0]).expect("decode idle");
        assert_eq!(md, MouseDelta::default(), "idle report = no movement");
    }

    /// Standard USB HID boot-keyboard report descriptor (no report ID): 8-bit
    /// modifier byte + 1 reserved byte + 6-byte keycode array → an 8-byte report.
    const BOOT_KBD_RDESC: &[u8] = &[
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x06, // Usage (Keyboard)
        0xA1, 0x01, // Collection (Application)
        0x05, 0x07, //   Usage Page (Keyboard/Keypad)
        0x19, 0xE0, //   Usage Minimum (0xE0 = LeftControl)
        0x29, 0xE7, //   Usage Maximum (0xE7 = Right GUI)
        0x15, 0x00, //   Logical Minimum (0)
        0x25, 0x01, //   Logical Maximum (1)
        0x75, 0x01, //   Report Size (1)
        0x95, 0x08, //   Report Count (8)
        0x81, 0x02, //   Input (Data,Var,Abs) — modifier byte
        0x95, 0x01, //   Report Count (1)
        0x75, 0x08, //   Report Size (8)
        0x81, 0x03, //   Input (Cnst,Var,Abs) — reserved byte
        0x95, 0x06, //   Report Count (6)
        0x75, 0x08, //   Report Size (8)
        0x15, 0x00, //   Logical Minimum (0)
        0x25, 0x65, //   Logical Maximum (101)
        0x05, 0x07, //   Usage Page (Keyboard/Keypad)
        0x19, 0x00, //   Usage Minimum (0)
        0x29, 0x65, //   Usage Maximum (101)
        0x81, 0x00, //   Input (Data,Array) — 6-byte keycode array
        0xC0, // End Collection
    ];

    #[test]
    fn extracts_keyboard_modifier_and_keys() {
        let dev = HidDevice::parse(BOOT_KBD_RDESC).expect("parse keyboard rdesc");
        // modifier 0x02 = LeftShift (usage 0xE1 -> bit 1); keycode 0x04 = 'a'.
        let report = [0x02u8, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
        let ks = dev.extract_keyboard(&report).expect("decode keyboard");
        assert_eq!(ks.modifiers & 0b10, 0b10, "LeftShift modifier set");
        assert!(
            ks.keys.contains(&0x04),
            "key 'a' (0x04) present, got {:?}",
            ks.keys
        );
    }

    #[test]
    fn idle_keyboard_report_has_no_keys() {
        let dev = HidDevice::parse(BOOT_KBD_RDESC).expect("parse");
        let ks = dev.extract_keyboard(&[0u8; 8]).expect("decode idle");
        assert_eq!(ks.modifiers, 0, "no modifiers held");
        assert!(ks.keys.is_empty(), "no keys held, got {:?}", ks.keys);
    }

    #[test]
    fn classifies_device_kind() {
        assert_eq!(
            HidDevice::parse(BOOT_MOUSE_RDESC).unwrap().kind(),
            HidKind::Mouse
        );
        assert_eq!(
            HidDevice::parse(BOOT_KBD_RDESC).unwrap().kind(),
            HidKind::Keyboard
        );
    }

    #[test]
    fn mouse_delta_to_boot_report_clamps() {
        let md = MouseDelta {
            dx: 5,
            dy: -5,
            wheel: 1,
            buttons: 0b101,
        };
        assert_eq!(md.to_boot_report(), [0b101, 5, (-5i8) as u8, 1]);
        // 16-bit high-res deltas clamp into the i8 boot range.
        let hi = MouseDelta {
            dx: 4000,
            dy: -4000,
            wheel: 0,
            buttons: 0,
        };
        assert_eq!(hi.to_boot_report(), [0, 127, (-127i8) as u8, 0]);
    }

    #[test]
    fn keyboard_state_to_boot_report() {
        let ks = KeyboardState {
            modifiers: 0x02,
            keys: alloc::vec![0x04, 0x05],
        };
        assert_eq!(ks.to_boot_report(), [0x02, 0, 0x04, 0x05, 0, 0, 0, 0]);
    }
}
