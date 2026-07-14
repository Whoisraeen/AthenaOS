//! Host-render preview of the OOBE card + soft shadow — verifies the
//! `fill_rounded_rect_shadow` primitive visually without QEMU (the headless
//! QMP screendump has a stride artifact). Writes target/shadow_preview.ppm.
use raegfx::Canvas;
use std::fs::File;
use std::io::Write;

#[test]
fn render_shadow_preview() {
    let (w, h) = (1280usize, 720usize);
    let mut buf = vec![0u8; w * h * 4];
    let mut canvas = unsafe { Canvas::new(buf.as_mut_ptr(), w, h, 4) };

    // Match the OOBE backdrop (setup_ui): blue gradient + translucent depth blob.
    canvas.fill_rect_gradient(0, 0, w, h, 0xFF_E6_EF_FC, 0xFF_3E_7C_E4);
    canvas.fill_circle(w / 6, h / 5, (w / 7).max(120), 0x33_FF_FF_FF);

    // The card, centered.
    let (cw, ch) = (580usize, 480usize);
    let (cx, cy) = ((w - cw) / 2, (h - ch) / 2);
    let cr = 24usize;
    canvas.fill_rounded_rect_shadow(cx, cy, cw, ch, cr, 0x0A_10_1C, 44, 18);
    canvas.fill_rounded_rect(cx, cy, cw, ch, cr, 0xF2_F8_FB_FF);
    canvas.draw_rounded_rect_outline(cx, cy, cw, ch, cr, 0x40_FF_FF_FF);
    canvas.draw_rounded_rect_outline(cx, cy, cw, ch - cr, cr, 0xB0_FF_FF_FF);

    // Dump P6 PPM. Canvas writes ARGB u32 → little-endian bytes [B,G,R,A].
    let mut rgb = Vec::with_capacity(w * h * 3);
    for i in 0..w * h {
        rgb.push(buf[i * 4 + 2]); // R
        rgb.push(buf[i * 4 + 1]); // G
        rgb.push(buf[i * 4]); // B
    }
    // Write to the gitignored workspace target/ (not committed). View with:
    //   py -c "from PIL import Image; \
    //          Image.open('target/shadow_preview.ppm').save('out.png')"
    let out_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/shadow_preview.ppm"
    );
    let mut f = File::create(out_path).expect("create ppm");
    write!(f, "P6\n{} {}\n255\n", w, h).unwrap();
    f.write_all(&rgb).unwrap();

    // Sanity: a feathered shadow exists to the LEFT of the card (darker than the
    // bare gradient there) and fades — i.e. it is NOT a hard uniform band.
    let probe_y = cy + ch / 2;
    let near = buf[(probe_y * w + (cx - 4)) * 4 + 2] as i32; // R, 4px out
    let far = buf[(probe_y * w + (cx - 30)) * 4 + 2] as i32; // R, 30px out
    assert!(
        near < far,
        "shadow should be darkest near the card and feather out"
    );
}

fn dump_ppm(buf: &[u8], w: usize, h: usize, name: &str) {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for i in 0..w * h {
        rgb.push(buf[i * 4 + 2]); // R
        rgb.push(buf[i * 4 + 1]); // G
        rgb.push(buf[i * 4]); // B
    }
    let path = format!("{}/../../target/{}", env!("CARGO_MANIFEST_DIR"), name);
    let mut f = File::create(&path).expect("create ppm");
    write!(f, "P6\n{} {}\n255\n", w, h).unwrap();
    f.write_all(&rgb).unwrap();
}

/// Host-render preview of the upgraded DARK login card (rounded + soft shadow +
/// circular avatar + rounded field with focus ring), mirroring login_ui::render.
#[test]
fn render_login_preview() {
    let (w, h) = (1280usize, 720usize);
    let mut buf = vec![0u8; w * h * 4];
    let mut canvas = unsafe { Canvas::new(buf.as_mut_ptr(), w, h, 4) };

    // Dark vertical gradient backdrop (approx DARK palette bg.base -> bg.overlay).
    canvas.fill_rect_gradient(0, 0, w, h, 0xFF_12_1A_2C, 0xFF_07_0C_18);

    let (cw, ch) = (360usize, 320usize);
    let (cx0, cy0) = ((w - cw) / 2, (h - ch) / 2);
    let cr = 24usize;
    canvas.fill_rounded_rect_shadow(cx0, cy0, cw, ch, cr, 0x04_06_0C, 40, 16);
    canvas.fill_rounded_rect(cx0, cy0, cw, ch, cr, 0xFF_1B_24_36); // bg.raised
    canvas.draw_rounded_rect_outline(cx0, cy0, cw, ch, cr, 0x24_FF_FF_FF);
    canvas.draw_rounded_rect_outline(cx0, cy0, cw, ch - cr, cr, 0x55_FF_FF_FF);

    let scx = w / 2;
    let (accent, accent_dim) = (0xFF_2E_5C_C8u32, 0xFF_24_46_98u32);
    // Circular avatar with accent ring.
    let avy = cy0 + 32;
    let asz = 64usize;
    let acy = avy + asz / 2;
    canvas.fill_circle(scx, acy, asz / 2, accent_dim);
    canvas.fill_circle(scx, acy, asz / 2 - 2, accent);

    // Rounded field with a focus ring.
    let (fw, fh) = (280usize, 32usize);
    let fx = scx - fw / 2;
    let fy = avy + asz + 52;
    let fr = 8usize;
    canvas.fill_rounded_rect(fx, fy, fw, fh, fr, 0xFF_0E_16_26); // bg.elevated
    canvas.draw_rounded_rect_outline(fx, fy, fw, fh, fr, 0x40_FF_FF_FF);
    canvas.draw_rounded_rect_outline(fx, fy, fw, fh, fr, accent); // focused

    dump_ppm(&buf, w, h, "login_preview.ppm");

    // Sanity: the card is brighter than the dark backdrop beside it.
    let cyy = cy0 + ch / 2;
    let card_r = buf[(cyy * w + (cx0 + cw / 2)) * 4 + 2] as i32;
    let bg_r = buf[(cyy * w + 40) * 4 + 2] as i32;
    assert!(
        card_r > bg_r,
        "card should be lighter than the dark backdrop"
    );
}
