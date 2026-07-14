//! Generic HID gamepad support — report-descriptor-driven (Concept §GameOS:
//! "every controller just works — first-party pads get the deluxe path,
//! everything else gets correct generic HID handling, never nothing").
//! MasterChecklist Phase 12.2 — "Generic HID gamepads".
//!
//! `input.rs` already models pads (DualSense/Xbox get dedicated decoders by
//! VID/PID; everything else is `GamepadState::Generic`). What was missing is
//! the HID half: a *report descriptor* parser, so an arbitrary pad's report
//! layout (where X/Y live, how many buttons, where the hat is) is learned
//! from the device itself instead of hardcoded. This module parses the
//! descriptor (HID 1.11 short items: Global/Local/Main state machine,
//! usage ranges, constant padding) into a [`GamepadLayout`] and decodes raw
//! input reports through it into normalized axes + a button bitmap.
//!
//! The smoketest parses a spec-correct 4-axis/12-button gamepad descriptor
//! and decodes a known report through the learned layout — deterministic,
//! identical on QEMU and iron. Binding to live xHCI interrupt-IN endpoints
//! (report-protocol devices) is the iron half (same checklist item).

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// HID usage pages / usages we care about.
const PAGE_GENERIC_DESKTOP: u16 = 0x01;
const PAGE_BUTTON: u16 = 0x09;
const USAGE_X: u16 = 0x30;
const USAGE_Y: u16 = 0x31;
const USAGE_Z: u16 = 0x32;
const USAGE_RX: u16 = 0x33;
const USAGE_RY: u16 = 0x34;
const USAGE_RZ: u16 = 0x35;
const USAGE_HAT: u16 = 0x39;
const USAGE_GAMEPAD: u16 = 0x05;
const USAGE_JOYSTICK: u16 = 0x04;

static DESCRIPTORS_PARSED: AtomicU64 = AtomicU64::new(0);
static REPORTS_DECODED: AtomicU64 = AtomicU64::new(0);

/// One field carved out of the input report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub bit_offset: u32,
    pub bit_size: u32,
    pub logical_min: i32,
    pub logical_max: i32,
}

/// The learned shape of a gamepad's input report.
#[derive(Debug, Clone, Default)]
pub struct GamepadLayout {
    pub report_id: Option<u8>,
    pub x: Option<Field>,
    pub y: Option<Field>,
    pub z: Option<Field>,
    pub rx: Option<Field>,
    pub ry: Option<Field>,
    pub rz: Option<Field>,
    pub hat: Option<Field>,
    pub buttons: Vec<Field>,
    /// Total input-report length in bits (incl. constant padding).
    pub report_bits: u32,
    /// True when the descriptor's application collection is a gamepad or
    /// joystick (Generic Desktop usage 0x05/0x04).
    pub is_gamepad: bool,
}

impl GamepadLayout {
    pub fn report_bytes(&self) -> usize {
        ((self.report_bits + 7) / 8) as usize
    }
}

/// Decoded, normalized snapshot of one input report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadInput {
    /// Axes normalized to -32768..32767 (i16 range), 0 = center.
    pub x: i16,
    pub y: i16,
    pub z: i16,
    pub rx: i16,
    pub ry: i16,
    pub rz: i16,
    /// Hat switch as 0-7 (N, NE, E, ...), 8 = centered/none.
    pub hat: u8,
    /// Bit N = button N+1 pressed.
    pub buttons: u32,
}

/// Parse a HID report descriptor into the gamepad's input-report layout.
/// Handles short items only (long items don't occur on real pads), usage
/// queues, usage ranges (buttons), constant padding, and per-report-ID
/// layouts (the FIRST input report's id is used — pads expose one).
pub fn parse_descriptor(desc: &[u8]) -> Option<GamepadLayout> {
    // Global state (HID 1.11 §6.2.2.7) + local usage queue (§6.2.2.8).
    let mut usage_page: u16 = 0;
    let mut logical_min: i32 = 0;
    let mut logical_max: i32 = 0;
    let mut report_size: u32 = 0;
    let mut report_count: u32 = 0;
    let mut report_id: Option<u8> = None;

    let mut usages: Vec<(u16, u16)> = Vec::new(); // (page, usage)
    let mut usage_range: Option<(u16, u16, u16)> = None; // (page, min, max)

    let mut layout = GamepadLayout::default();
    let mut bit_cursor: u32 = 0;

    let mut i = 0usize;
    while i < desc.len() {
        let prefix = desc[i];
        i += 1;
        if prefix == 0xFE {
            // Long item: skip per spec (never used by pads).
            if i + 1 >= desc.len() {
                return None;
            }
            let data_len = desc[i] as usize;
            i += 2 + data_len;
            continue;
        }
        let size = match prefix & 0x03 {
            0 => 0usize,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        if i + size > desc.len() {
            return None;
        }
        let mut data_u: u32 = 0;
        for (k, &b) in desc[i..i + size].iter().enumerate() {
            data_u |= (b as u32) << (8 * k);
        }
        // Sign-extended view for logical min/max.
        let data_i: i32 = match size {
            1 => data_u as u8 as i8 as i32,
            2 => data_u as u16 as i16 as i32,
            _ => data_u as i32,
        };
        i += size;

        let item_type = (prefix >> 2) & 0x03;
        let tag = prefix >> 4;
        match (item_type, tag) {
            // ── Global items ──
            (1, 0x0) => usage_page = data_u as u16,
            (1, 0x1) => logical_min = data_i,
            (1, 0x2) => logical_max = data_i,
            (1, 0x7) => report_size = data_u,
            (1, 0x9) => report_count = data_u,
            (1, 0x8) => {
                let id = data_u as u8;
                match layout.report_id {
                    None => {
                        layout.report_id = Some(id);
                        // The report id byte itself precedes the payload on
                        // the wire; field offsets below are payload-relative.
                    }
                    Some(current) if current != id => {
                        // A second report begins — pads carry their input in
                        // the first one; stop here.
                        break;
                    }
                    _ => {}
                }
                report_id = Some(id);
            }
            // ── Local items ──
            (2, 0x0) => usages.push((usage_page, data_u as u16)),
            (2, 0x1) => usage_range = Some((usage_page, data_u as u16, 0)),
            (2, 0x2) => {
                if let Some((p, min, _)) = usage_range {
                    usage_range = Some((p, min, data_u as u16));
                }
            }
            // ── Main items ──
            (0, 0xA) => {
                // Collection: note application usage (gamepad/joystick).
                if let Some(&(page, usage)) = usages.first() {
                    if page == PAGE_GENERIC_DESKTOP
                        && (usage == USAGE_GAMEPAD || usage == USAGE_JOYSTICK)
                    {
                        layout.is_gamepad = true;
                    }
                }
                usages.clear();
                usage_range = None;
            }
            (0, 0xC) => {
                // End collection.
                usages.clear();
                usage_range = None;
            }
            (0, 0x8) => {
                // Input item: allocate report_count fields of report_size bits.
                let constant = data_u & 0x01 != 0;
                if constant {
                    bit_cursor += report_size * report_count;
                } else {
                    // Expand a usage range (buttons) into individual usages.
                    let mut expanded: Vec<(u16, u16)> = usages.clone();
                    if let Some((page, min, max)) = usage_range {
                        for u in min..=max {
                            expanded.push((page, u));
                        }
                    }
                    for n in 0..report_count {
                        let field = Field {
                            bit_offset: bit_cursor,
                            bit_size: report_size,
                            logical_min,
                            logical_max,
                        };
                        bit_cursor += report_size;
                        // HID repeats the LAST usage when count > usages.
                        let (page, usage) = expanded
                            .get(n as usize)
                            .or_else(|| expanded.last())
                            .copied()
                            .unwrap_or((0, 0));
                        match (page, usage) {
                            (PAGE_GENERIC_DESKTOP, USAGE_X) => layout.x = Some(field),
                            (PAGE_GENERIC_DESKTOP, USAGE_Y) => layout.y = Some(field),
                            (PAGE_GENERIC_DESKTOP, USAGE_Z) => layout.z = Some(field),
                            (PAGE_GENERIC_DESKTOP, USAGE_RX) => layout.rx = Some(field),
                            (PAGE_GENERIC_DESKTOP, USAGE_RY) => layout.ry = Some(field),
                            (PAGE_GENERIC_DESKTOP, USAGE_RZ) => layout.rz = Some(field),
                            (PAGE_GENERIC_DESKTOP, USAGE_HAT) => layout.hat = Some(field),
                            (PAGE_BUTTON, _) => layout.buttons.push(field),
                            _ => {} // vendor/unknown usage: bits consumed above
                        }
                    }
                }
                usages.clear();
                usage_range = None;
            }
            _ => {
                // Output/Feature items and other tags: consume, no input bits.
                usages.clear();
                usage_range = None;
            }
        }
    }

    layout.report_bits = bit_cursor;
    if layout.is_gamepad && (layout.x.is_some() || !layout.buttons.is_empty()) {
        DESCRIPTORS_PARSED.fetch_add(1, Ordering::Relaxed);
        Some(layout)
    } else {
        None
    }
}

/// Extract `field` from a raw report (little-endian bit order per HID spec).
fn extract(report: &[u8], field: &Field) -> u32 {
    let mut val: u32 = 0;
    for bit in 0..field.bit_size.min(32) {
        let abs = field.bit_offset + bit;
        let byte = (abs / 8) as usize;
        if byte >= report.len() {
            break;
        }
        if report[byte] >> (abs % 8) & 1 != 0 {
            val |= 1 << bit;
        }
    }
    val
}

/// Normalize a raw axis sample to the i16 range, 0 = logical center.
fn normalize_axis(raw: u32, field: &Field) -> i16 {
    let min = field.logical_min as i64;
    let max = field.logical_max as i64;
    if max <= min {
        return 0;
    }
    // Sign-aware raw view: descriptors with negative logical_min report
    // signed samples.
    let raw_i = if min < 0 {
        let bits = field.bit_size.min(31);
        let sign = 1u32 << (bits - 1);
        if raw & sign != 0 {
            (raw as i64) - (1i64 << bits)
        } else {
            raw as i64
        }
    } else {
        raw as i64
    };
    let span = max - min;
    let centered = (raw_i - min) * 65535 / span - 32768;
    centered.clamp(-32768, 32767) as i16
}

/// Decode a raw input report through the learned layout. `report` excludes
/// the report-id byte (the caller strips it when `layout.report_id` is set).
pub fn decode_report(layout: &GamepadLayout, report: &[u8]) -> PadInput {
    let mut out = PadInput {
        hat: 8,
        ..PadInput::default()
    };
    let mut axis = |f: &Option<Field>| -> i16 {
        f.as_ref()
            .map(|f| normalize_axis(extract(report, f), f))
            .unwrap_or(0)
    };
    out.x = axis(&layout.x);
    out.y = axis(&layout.y);
    out.z = axis(&layout.z);
    out.rx = axis(&layout.rx);
    out.ry = axis(&layout.ry);
    out.rz = axis(&layout.rz);
    if let Some(h) = &layout.hat {
        let raw = extract(report, h) as i32;
        // Hats report logical_min..logical_max for N..NW; out-of-range = idle.
        out.hat = if raw >= h.logical_min && raw <= h.logical_max {
            (raw - h.logical_min) as u8
        } else {
            8
        };
    }
    for (n, f) in layout.buttons.iter().take(32).enumerate() {
        if extract(report, f) != 0 {
            out.buttons |= 1 << n;
        }
    }
    REPORTS_DECODED.fetch_add(1, Ordering::Relaxed);
    out
}

pub fn init() {
    crate::serial_println!(
        "[hid-pad] generic HID gamepad decoder ready (report-descriptor-driven)"
    );
}

/// Deterministic proof: parse a spec-correct 4-axis / 12-button / hat
/// gamepad report descriptor (the classic DirectInput-pad shape) and decode
/// a known report through the LEARNED layout — axes land where the
/// descriptor said, buttons map by usage range, padding is skipped.
pub fn run_boot_smoketest() {
    // Usage Page (Generic Desktop), Usage (Game Pad), Collection (App),
    //   4 × 8-bit axes X/Y/Z/Rz (0..255),
    //   hat switch (4 bits, logical 0..7) + 4 bits constant padding,
    //   12 × 1-bit buttons + 4 bits constant padding,
    // End Collection.
    const DESC: &[u8] = &[
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x05, // Usage (Game Pad)
        0xA1, 0x01, // Collection (Application)
        0x15, 0x00, //   Logical Minimum (0)
        0x26, 0xFF, 0x00, //   Logical Maximum (255)
        0x75, 0x08, //   Report Size (8)
        0x95, 0x04, //   Report Count (4)
        0x09, 0x30, //   Usage (X)
        0x09, 0x31, //   Usage (Y)
        0x09, 0x32, //   Usage (Z)
        0x09, 0x35, //   Usage (Rz)
        0x81, 0x02, //   Input (Data,Var,Abs)
        0x15, 0x00, //   Logical Minimum (0)
        0x25, 0x07, //   Logical Maximum (7)
        0x75, 0x04, //   Report Size (4)
        0x95, 0x01, //   Report Count (1)
        0x09, 0x39, //   Usage (Hat switch)
        0x81, 0x42, //   Input (Data,Var,Abs,Null)
        0x75, 0x04, //   Report Size (4)
        0x95, 0x01, //   Report Count (1)
        0x81, 0x03, //   Input (Const) — pad to byte
        0x05, 0x09, //   Usage Page (Button)
        0x19, 0x01, //   Usage Minimum (Button 1)
        0x29, 0x0C, //   Usage Maximum (Button 12)
        0x15, 0x00, //   Logical Minimum (0)
        0x25, 0x01, //   Logical Maximum (1)
        0x75, 0x01, //   Report Size (1)
        0x95, 0x0C, //   Report Count (12)
        0x81, 0x02, //   Input (Data,Var,Abs)
        0x75, 0x01, //   Report Size (1)
        0x95, 0x04, //   Report Count (4)
        0x81, 0x03, //   Input (Const) — pad to byte
        0xC0, // End Collection
    ];

    let Some(layout) = parse_descriptor(DESC) else {
        crate::serial_println!("[hid-pad] smoketest: descriptor rejected -> FAIL");
        return;
    };

    let layout_ok = layout.is_gamepad
        && layout.x.map(|f| f.bit_offset) == Some(0)
        && layout.y.map(|f| f.bit_offset) == Some(8)
        && layout.z.map(|f| f.bit_offset) == Some(16)
        && layout.rz.map(|f| f.bit_offset) == Some(24)
        && layout.hat.map(|f| (f.bit_offset, f.bit_size)) == Some((32, 4))
        && layout.buttons.len() == 12
        && layout.buttons[0].bit_offset == 40
        && layout.report_bytes() == 7;

    // Report: X full right (255), Y centered (128), Z=0, Rz=64,
    // hat = 2 (East), buttons 1+3 down (byte 5 bits 0,2), buttons 10+12
    // down (byte 6 bits 1,3).
    let report = [0xFFu8, 0x80, 0x00, 0x40, 0x02, 0b0000_0101, 0b0000_1010];
    let pad = decode_report(&layout, &report);
    let buttons_expected = (1 << 0) | (1 << 2) | (1 << 9) | (1 << 11);
    let decode_ok = pad.x == 32767 // full right
        && (-300..=300).contains(&pad.y) // centered
        && pad.z == -32768 // full left/up
        && pad.hat == 2
        && pad.buttons == buttons_expected;

    let pass = layout_ok && decode_ok;
    crate::serial_println!(
        "[hid-pad] smoketest: layout(axes@0/8/16/24,hat@32,12btns@40,7B)={} decode(x={},y={},z={},hat={},btns={:#06x})={} -> {}",
        layout_ok,
        pad.x,
        pad.y,
        pad.z,
        pad.hat,
        pad.buttons,
        decode_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/hid_pad` — generic gamepad decoder state.
pub fn dump_text() -> String {
    alloc::format!(
        "# generic HID gamepad decoder (report-descriptor-driven)\ndescriptors_parsed: {}\nreports_decoded: {}\n",
        DESCRIPTORS_PARSED.load(Ordering::Relaxed),
        REPORTS_DECODED.load(Ordering::Relaxed),
    )
}
