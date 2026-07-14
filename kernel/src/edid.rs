//! EDID parsing — log monitor capabilities from a 128-byte base block.
//!
//! Full kernel mode-setting is future work; this module lets bring-up logs
//! what the panel reports.

#![allow(dead_code)]

extern crate alloc;

/// Parsed fields from EDID base block (128 bytes).
#[derive(Debug, Clone, Copy, Default)]
pub struct EdidInfo {
    pub width_cm: u8,
    pub height_cm: u8,
    pub preferred_width: u16,
    pub preferred_height: u16,
    pub refresh_hz_x100: u16,
}

/// Parse EDID v1 base block. Returns `None` if header invalid.
pub fn parse_base(edid: &[u8]) -> Option<EdidInfo> {
    if edid.len() < 128 {
        return None;
    }
    if edid[0..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }

    let width_cm = edid[21];
    let height_cm = edid[22];

    let mut info = EdidInfo {
        width_cm,
        height_cm,
        ..Default::default()
    };

    // First detailed timing descriptor (bytes 54..71) if not used as monitor name.
    if edid[54] != 0 || edid[55] != 0 {
        // Per VESA E-EDID: byte 56 = h_active[7:0], byte 58 upper nibble =
        // h_active[11:8] (lower nibble is h_blank[11:8]); byte 59 = v_active[7:0],
        // byte 61 upper nibble = v_active[11:8]. The high nibble must be masked
        // and shifted to bits [11:8], NOT the whole byte shifted left 4.
        let h_active = (((edid[58] as u16) & 0xF0) << 4) | (edid[56] as u16);
        let v_active = (((edid[61] as u16) & 0xF0) << 4) | (edid[59] as u16);
        let pixclk_10khz = u16::from_le_bytes([edid[54], edid[55]]);
        if h_active > 0 && v_active > 0 && pixclk_10khz > 0 {
            info.preferred_width = h_active;
            info.preferred_height = v_active;
            // refresh ≈ pixel_clock / (h_total * v_total) — approximate from active only
            let refresh_x100 = (pixclk_10khz as u32)
                .saturating_mul(10_000)
                .saturating_div((h_active as u32).saturating_mul(v_active as u32).max(1))
                as u16;
            info.refresh_hz_x100 = refresh_x100;
        }
    }

    Some(info)
}

pub fn log_if_present(edid: &[u8]) {
    match parse_base(edid) {
        Some(i) => crate::serial_println!(
            "[edid] panel ~{}x{} cm, preferred {}x{} @ ~{}.{:02} Hz (GOP mode may differ)",
            i.width_cm,
            i.height_cm,
            i.preferred_width,
            i.preferred_height,
            i.refresh_hz_x100 / 100,
            i.refresh_hz_x100 % 100,
        ),
        None => crate::serial_println!("[edid] no valid base block (mode set not available)"),
    }
}

/// Build a minimal EDID 1.3 base block from GOP width/height (no DDC on QEMU).
pub fn synthesize_from_gop(width: u32, height: u32) -> [u8; 128] {
    let mut block = [0u8; 128];
    block[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    block[8..18].copy_from_slice(b"ATHENAOS   ");
    block[18..22].copy_from_slice(b"GOP ");
    // Detailed timing descriptor in the real VESA E-EDID layout so it decodes
    // back through parse_base correctly (active fields are 12-bit: low 8 in one
    // byte, high 4 in the upper nibble of another).
    let w = width.min(4095) as u16;
    let h = height.min(4095) as u16;
    // Pixel clock (bytes 54..56, LE, units of 10 kHz). Pick a clock that yields
    // ~60 Hz from the active-area approximation parse_base uses.
    let pixclk_10khz = (((width as u64) * (height as u64) * 60) / 10_000).clamp(1, 65_535) as u16;
    block[54..56].copy_from_slice(&pixclk_10khz.to_le_bytes());
    block[56] = (w & 0xFF) as u8; // h_active[7:0]
    block[57] = 0; // h_blank[7:0] (synthetic: none)
    block[58] = (((w >> 8) & 0x0F) << 4) as u8; // h_active[11:8] in upper nibble
    block[59] = (h & 0xFF) as u8; // v_active[7:0]
    block[60] = 0; // v_blank[7:0]
    block[61] = (((h >> 8) & 0x0F) << 4) as u8; // v_active[11:8] in upper nibble
    block[126] = 0; // extensions
    let mut sum: u8 = 0;
    for b in &block[0..127] {
        sum = sum.wrapping_add(*b);
    }
    block[127] = (256u16 - sum as u16) as u8;
    block
}

pub fn init() {
    crate::serial_println!("[ OK ] EDID parser ready (introspection only)");
}

pub fn run_boot_smoketest() {
    let (w, h) = crate::framebuffer::current_mode();
    let block = synthesize_from_gop(w, h);
    log_if_present(&block);
    let parsed = parse_base(&block).is_some();

    // Decode a REAL EDID detailed-timing descriptor (1920x1080 @ 148.5 MHz,
    // h_total 2200 / v_total 1125) with the bit layout off a shipping panel.
    // The previous smoketest only round-tripped our own synthetic block, so a
    // wrong decode masked itself; this fragment makes the test able to FAIL.
    let mut real = [0u8; 128];
    real[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    // pixel clock 14850 (0x3A02) -> LE 0x02,0x3A
    real[54] = 0x02;
    real[55] = 0x3A;
    real[56] = 0x80; // h_active[7:0]  (0x780 = 1920)
    real[57] = 0x18; // h_blank[7:0]
    real[58] = 0x71; // h_active[11:8]=0x7 (upper nibble), h_blank[11:8]=0x1
    real[59] = 0x38; // v_active[7:0]  (0x438 = 1080)
    real[60] = 0x2D; // v_blank[7:0]
    real[61] = 0x40; // v_active[11:8]=0x4 (upper nibble), v_blank[11:8]=0x0
    let decoded = parse_base(&real);
    let real_ok = matches!(
        decoded,
        Some(EdidInfo {
            preferred_width: 1920,
            preferred_height: 1080,
            ..
        })
    );

    let pass = parsed && real_ok;
    crate::selftest::record_smoketest("edid", pass);
    crate::serial_println!(
        "[edid] smoketest: gop={}x{} synthetic_block={} real_1080p_decode={:?} -> {}",
        w,
        h,
        parsed,
        decoded.map(|d| (d.preferred_width, d.preferred_height)),
        if pass { "PASS" } else { "FAIL" }
    );
}

pub fn dump_text() -> alloc::string::String {
    use alloc::string::String;
    let (w, h) = crate::framebuffer::current_mode();
    let block = synthesize_from_gop(w, h);
    let mut out = String::from("# EDID (GOP-derived synthetic base block)\n");
    if let Some(i) = parse_base(&block) {
        out.push_str(&alloc::format!(
            "gop: {}x{}\npreferred: {}x{} @ ~{}.{:02} Hz\n",
            w,
            h,
            i.preferred_width,
            i.preferred_height,
            i.refresh_hz_x100 / 100,
            i.refresh_hz_x100 % 100
        ));
    } else {
        out.push_str("parse: failed\n");
    }
    out
}
