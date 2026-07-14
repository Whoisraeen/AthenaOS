//! ui_screenshot — HOST-render UI screenshot harness (ADR 0004, host-render path).
//!
//! *"Built for people who care about how things feel."* — `LEGACY_GAMING_CONCEPT.md`
//! §AthUI. Goal #1 requires the UI be "proven by raeen-visual-qa screenshots."
//! The live desktop is unreachable in headless CI and the QEMU
//! `screendump`->PPM pipeline stripes (project memory: a 24bpp-read-as-32bpp /
//! stride capture artifact — NOT the real render; iron + HOST-RENDER are clean).
//! This is the clean path: render representative UI surfaces on a host-backed
//! `raegfx::Canvas` (the EXACT software rasterizer the kernel composites with)
//! and encode real, spec-valid PNGs with raemedia's from-scratch encoder.
//!
//! Two tiers of capture:
//!   1. Design ATOMS — the soft-ambient drop shadow (proving a feathered
//!      penumbra, not a hard block), a glassmorphic panel, rounded-rects +
//!      gradients, raefont grayscale-AA text across the type ramp, circles, and
//!      the focus ring (normal + high-contrast).
//!   2. Real shipped SURFACES whose `render(&mut Canvas)` is host-callable with
//!      no kernel state: the Control Center, the LIVE Files window (apps/files::
//!      render_preview, not the dead raeshell twin), and
//!      notification toasts. (Surfaces whose render lives in `shell_runner.rs`
//!      and is wound through `DesktopShell` kernel state are NOT host-callable;
//!      see the REPORT gap list.)
//!
//! Output: PNGs + `manifest.txt` in `docs/design/screenshots/`.
//! Run: `cargo run --release` from `tools/ui_screenshot/`.

use raegfx::text::FontFamily;
use raegfx::Canvas;
use raemedia::png_encode::{encode_argb8888, ColorType};
use rae_tokens::{
    TYPE_BODY, TYPE_CAPTION, TYPE_DISPLAY, TYPE_LABEL, TYPE_SUBTITLE, TYPE_TITLE,
};
use std::fs;
use std::path::PathBuf;

/// A host framebuffer: a `Vec<u32>` of ARGB8888 (`0xAARRGGBB`) pixels. The
/// Canvas is constructed over the SAME bytes via a `*mut u8` cast; because the
/// kernel's bpp=4 path does `*(p as *mut u32) = color`, reading the `Vec<u32>`
/// back yields the literal ARGB values the encoder wants — no byte reshuffle.
struct HostFb {
    px: Vec<u32>,
    w: usize,
    h: usize,
}

impl HostFb {
    fn new(w: usize, h: usize) -> Self {
        HostFb {
            px: vec![0u32; w * h],
            w,
            h,
        }
    }

    /// Borrow a `Canvas` over this framebuffer for the duration of `f`.
    fn canvas(&mut self) -> Canvas {
        // SAFETY: the buffer is `w*h*4` bytes, writable, and outlives the Canvas
        // (which is dropped before `self.px` is read back in `save`).
        unsafe { Canvas::new(self.px.as_mut_ptr() as *mut u8, self.w, self.h, 4) }
    }

    /// Encode to a real PNG (alpha preserved) and write under the screenshots dir.
    fn save(&self, dir: &PathBuf, name: &str, manifest: &mut Vec<(String, usize, String)>, desc: &str) {
        let bytes = encode_argb8888(&self.px, self.w as u32, self.h as u32, ColorType::Rgba)
            .expect("PNG encode failed");
        let path = dir.join(name);
        fs::write(&path, &bytes).expect("write png");
        println!(
            "  wrote {:<34} {}x{}  {} bytes",
            name, self.w, self.h, bytes.len()
        );
        manifest.push((name.to_string(), bytes.len(), desc.to_string()));
    }
}

fn screenshots_dir() -> PathBuf {
    // tools/ui_screenshot/ -> repo root -> docs/design/screenshots/
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    d.pop(); // tools/
    d.pop(); // repo root
    d.push("docs");
    d.push("design");
    d.push("screenshots");
    fs::create_dir_all(&d).expect("create screenshots dir");
    d
}

// ── Backdrops shared by the atom captures (mirror the OOBE / dark desktop) ──

fn light_backdrop(c: &mut Canvas, w: usize, h: usize) {
    c.fill_rect_gradient(0, 0, w, h, 0xFF_E6_EF_FC, 0xFF_3E_7C_E4);
    c.fill_circle(w / 6, h / 5, (w / 7).max(120), 0x33_FF_FF_FF);
    c.fill_circle(
        w.saturating_sub(w / 6),
        h.saturating_sub(h / 7),
        (w / 6).max(150),
        0x26_24_50_B8,
    );
}

fn dark_backdrop(c: &mut Canvas, w: usize, h: usize) {
    c.fill_rect_gradient(0, 0, w, h, 0xFF_12_1A_2C, 0xFF_07_0C_18);
    c.fill_circle(w / 5, h / 4, (w / 8).max(90), 0x1A_2E_5C_C8);
}

/// The NEW signature desktop backdrop: the procedural Aurora Mesh (IDENTITY.md
/// §3) instead of the flat navy void. Surfaces re-render over THIS so the
/// before/after on the "flat void" defect is immediate. `phase=0` is the still
/// frame the live wallpaper would advance over time.
fn aurora_backdrop(c: &mut Canvas, w: usize, h: usize) {
    raegfx::glass::render_aurora_dark(c, 0, 0, w, h, 0);
}

fn main() {
    let dir = screenshots_dir();
    println!("ui_screenshot — host-render UI capture -> {}", dir.display());

    // Build the raefont AA text engine (embedded Inter + JetBrains Mono). The
    // kernel does this at boot; on host we must do it explicitly or every
    // `draw_text_aa` silently falls back to the blocky 8x8 bitmap font — which
    // would make these captures LIE about text crispness. Assert it loaded so
    // the harness FAILS loudly instead of shipping bitmap text.
    let aa_ready = raegfx::text::ensure_init();
    assert!(
        aa_ready && raegfx::text::families_loaded() == 2,
        "raefont AA engine did not load on host (ready={aa_ready}, families={}) — \
         captures would fall back to the 8x8 bitmap font",
        raegfx::text::families_loaded()
    );
    println!(
        "raefont AA engine ready: {} families (RaeSans + RaeMono)",
        raegfx::text::families_loaded()
    );

    println!("[atoms]");

    let mut manifest: Vec<(String, usize, String)> = Vec::new();

    render_drop_shadow_atom(&dir, &mut manifest);
    render_glass_panel_atom(&dir, &mut manifest);
    render_primitives_atom(&dir, &mut manifest);
    render_type_ramp_atom(&dir, &mut manifest);
    render_focus_rings_atom(&dir, &mut manifest);
    render_icon_sheet_atom(&dir, &mut manifest);

    println!("[identity]");
    render_aurora_wallpaper(&dir, &mut manifest);
    render_glass_tiers_over_aurora(&dir, &mut manifest);
    render_iridescent_edge_3x(&dir, &mut manifest);

    println!("[surfaces]");
    render_control_center_surface(&dir, &mut manifest);
    render_files_surface(&dir, &mut manifest);
    render_notification_toasts_surface(&dir, &mut manifest);
    render_taskbar_surface(&dir, &mut manifest);
    render_start_menu_surface(&dir, &mut manifest);
    render_context_menu_surface(&dir, &mut manifest);
    render_settings_surface(&dir, &mut manifest);
    render_lock_screen_surface(&dir, &mut manifest);
    render_command_palette_surface(&dir, &mut manifest);
    render_snap_layout_surface(&dir, &mut manifest);
    render_snap_assist_surface(&dir, &mut manifest);

    // Manifest: which surface each PNG shows.
    let mut man = String::new();
    man.push_str("# ui_screenshot manifest — host-rendered UI captures (ADR 0004)\n");
    man.push_str("# Regenerate: cd tools/ui_screenshot && cargo run --release\n");
    man.push_str("# Each line: <png>  <bytes>  <what it shows>\n\n");
    for (name, bytes, desc) in &manifest {
        man.push_str(&format!("{:<40} {:>8}  {}\n", name, bytes, desc));
    }
    fs::write(dir.join("manifest.txt"), man).expect("write manifest");
    println!(
        "\nwrote {} PNGs + manifest.txt to {}",
        manifest.len(),
        dir.display()
    );
}

// ════════════════════════════════════════════════════════════════════════
// Atoms
// ════════════════════════════════════════════════════════════════════════

/// The soft-ambient drop shadow in isolation, on a FLAT neutral backdrop so the
/// penumbra is unambiguous: a card with `fill_rounded_rect_shadow` (the macOS-
/// grade SDF feather) and, beside it, the OLD hard offset-silhouette look for
/// contrast. Also runs the FAIL-able assertion that the shadow ramps (darkest at
/// the edge, fading out) rather than being a hard uniform block.
fn render_drop_shadow_atom(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    let (w, h) = (900usize, 460usize);
    let mut fb = HostFb::new(w, h);
    {
        let mut c = fb.canvas();
        // Flat light-gray field — a colored gradient would mask the penumbra.
        c.fill_rect(0, 0, w, h, 0xFF_D6_DA_E2);

        // LEFT: the shipped soft ambient shadow (neutral dark, 44px blur, 18 dy).
        let (cw, ch) = (300usize, 240usize);
        let (lx, ly) = (110usize, 120usize);
        let cr = 24usize;
        c.fill_rounded_rect_shadow(lx, ly, cw, ch, cr, 0x0A_10_1C, 44, 18);
        c.fill_rounded_rect(lx, ly, cw, ch, cr, 0xFF_F4_F7_FB);
        c.draw_rounded_rect_outline(lx, ly, cw, ch, cr, 0x30_00_00_00);
        c.draw_text_aa(
            (lx + 24) as i32,
            (ly + 24) as i32,
            "Soft ambient",
            TYPE_SUBTITLE,
            0xFF_12_18_24,
            FontFamily::Sans,
        );
        c.draw_text_aa(
            (lx + 24) as i32,
            (ly + 56) as i32,
            "feathered penumbra",
            TYPE_CAPTION,
            0xFF_55_5E_6E,
            FontFamily::Sans,
        );

        // RIGHT: the OLD hard offset silhouette (one translucent rect nudged
        // down-right) — the "90s bevel ledge" the SDF shadow replaced. Side by
        // side, the difference (soft ramp vs hard block) is obvious to the eye.
        let (rx, ry) = (490usize, 120usize);
        c.fill_rounded_rect(rx + 8, ry + 8, cw, ch, cr, 0x55_0A_10_1C); // hard block
        c.fill_rounded_rect(rx, ry, cw, ch, cr, 0xFF_F4_F7_FB);
        c.draw_rounded_rect_outline(rx, ry, cw, ch, cr, 0x30_00_00_00);
        c.draw_text_aa(
            (rx + 24) as i32,
            (ry + 24) as i32,
            "Old offset block",
            TYPE_SUBTITLE,
            0xFF_12_18_24,
            FontFamily::Sans,
        );
        c.draw_text_aa(
            (rx + 24) as i32,
            (ry + 56) as i32,
            "hard ledge (rejected)",
            TYPE_CAPTION,
            0xFF_55_5E_6E,
            FontFamily::Sans,
        );
    }

    // FAIL-able penumbra proof: sample the red channel of the LEFT card's cast
    // shadow on the FLAT backdrop. Near the card edge it must be DARKER than the
    // bare field, and it must FEATHER (near darker than far), i.e. a soft ramp.
    let (lx, ly, cw, ch) = (110usize, 120usize, 300usize, 240usize);
    let probe_y = ly + ch / 2 + 18; // along the down-cast band
    let bg = (fb.px[probe_y * w + (lx + cw + 90)] >> 16) & 0xFF; // bare field R
    let near = (fb.px[probe_y * w + (lx + cw + 6)] >> 16) & 0xFF; // 6px out
    let far = (fb.px[probe_y * w + (lx + cw + 40)] >> 16) & 0xFF; // 40px out
    assert!(
        near < bg,
        "shadow must darken the field near the card (near={near} bg={bg})"
    );
    assert!(
        near < far && far <= bg,
        "shadow must FEATHER, not be a hard block (near={near} far={far} bg={bg})"
    );
    println!("  shadow penumbra: near={near} < far={far} <= field={bg}  -> SOFT RAMP");

    fb.save(
        dir,
        "atom-drop-shadow.png",
        manifest,
        "Soft ambient drop shadow (SDF feather) vs the rejected hard offset block; FAIL-able penumbra ramp proven",
    );
}

/// A glassmorphic panel over a colorful backdrop: translucent fill blends the
/// depth blobs through it, top-edge highlight, hairline stroke.
fn render_glass_panel_atom(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    let (w, h) = (760usize, 480usize);
    let mut fb = HostFb::new(w, h);
    {
        let mut c = fb.canvas();
        light_backdrop(&mut c, w, h);
        let (pw, ph) = (520usize, 320usize);
        let (px, py) = ((w - pw) / 2, (h - ph) / 2);
        let r = 24usize;
        // Soft shadow under the glass.
        c.fill_rounded_rect_shadow(px, py, pw, ph, r, 0x0A_10_1C, 40, 16);
        // Translucent glass tint — the backdrop blobs show THROUGH this.
        c.fill_rounded_rect(px, py, pw, ph, r, 0xB8_1B_24_36);
        c.draw_rounded_rect_outline(px, py, pw, ph, r, 0x30_FF_FF_FF);
        c.draw_rounded_rect_outline(px, py, pw, ph - r, r, 0x90_FF_FF_FF); // top highlight
        c.draw_text_aa(
            (px + 28) as i32,
            (py + 28) as i32,
            "Glassmorphic panel",
            TYPE_TITLE,
            0xFF_F4_F7_FB,
            FontFamily::Sans,
        );
        c.draw_text_aa(
            (px + 28) as i32,
            (py + 70) as i32,
            "translucent material.glass over a blurred backdrop",
            TYPE_BODY,
            0xCC_D6_DA_E2,
            FontFamily::Sans,
        );
        // A small accent pill inside, to show layered translucency.
        c.fill_rounded_rect(px + 28, py + 120, 160, 40, 12, 0xFF_2E_5C_C8);
        c.draw_text_aa(
            (px + 56) as i32,
            (py + 132) as i32,
            "Primary",
            TYPE_LABEL,
            0xFF_FF_FF_FF,
            FontFamily::Sans,
        );
    }
    fb.save(
        dir,
        "atom-glass-panel.png",
        manifest,
        "Glassmorphic panel: translucent glass tint over colorful backdrop, top-edge highlight + hairline stroke",
    );
}

/// Rounded-rects (varied radii), vertical gradients, and AA circles — the
/// geometric primitives the chrome is built from.
fn render_primitives_atom(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    let (w, h) = (760usize, 420usize);
    let mut fb = HostFb::new(w, h);
    {
        let mut c = fb.canvas();
        c.fill_rect(0, 0, w, h, 0xFF_1B_20_2C);

        // Row of rounded-rects, increasing radius.
        for (i, r) in [0usize, 6, 14, 24, 40].iter().enumerate() {
            let x = 40 + i * 140;
            c.fill_rounded_rect(x, 50, 110, 90, *r, 0xFF_2E_5C_C8);
        }
        c.draw_text_aa(40, 16, "rounded-rect radii: 0 / 6 / 14 / 24 / 40", TYPE_LABEL, 0xFF_C8_D0_E0, FontFamily::Sans);

        // Gradients.
        c.draw_text_aa(40, 170, "vertical gradients", TYPE_LABEL, 0xFF_C8_D0_E0, FontFamily::Sans);
        c.fill_rect_gradient(40, 200, 200, 90, 0xFF_3E_7C_E4, 0xFF_12_1A_2C);
        c.fill_rect_gradient(260, 200, 200, 90, 0xFF_E0_5C_8A, 0xFF_2A_12_2C);
        c.fill_rect_gradient(480, 200, 200, 90, 0xFF_4C_C8_8A, 0xFF_10_2C_22);

        // AA circles (status dots / avatars).
        c.draw_text_aa(40, 312, "anti-aliased circles", TYPE_LABEL, 0xFF_C8_D0_E0, FontFamily::Sans);
        for (i, rr) in [10usize, 18, 28, 40].iter().enumerate() {
            let cx = 70 + i * 150;
            c.fill_circle(cx, 370, *rr, 0xFF_4C_C8_8A);
        }
    }
    fb.save(
        dir,
        "atom-primitives.png",
        manifest,
        "Core primitives: rounded-rects (radii 0..40), vertical gradients, anti-aliased circles",
    );
}

/// The raefont grayscale-AA type ramp (§6) — DISPLAY..CAPTION, Sans + Mono — so
/// visual-qa can judge text crispness (the "basic / not crisp" 8x8-font fix).
fn render_type_ramp_atom(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    let (w, h) = (820usize, 560usize);
    let mut fb = HostFb::new(w, h);
    {
        let mut c = fb.canvas();
        c.fill_rect(0, 0, w, h, 0xFF_0E_12_1C);
        let fg = 0xFF_ED_F1_F8u32;
        let dim = 0xFF_8A_93_A6u32;
        let mut y = 36i32;
        let ramp: [(&str, rae_tokens::TypeStyle); 6] = [
            ("Display — RaeSans", TYPE_DISPLAY),
            ("Title — RaeSans", TYPE_TITLE),
            ("Subtitle — RaeSans", TYPE_SUBTITLE),
            ("Body — the quick brown fox jumps over the lazy dog", TYPE_BODY),
            ("Label — RaeSans", TYPE_LABEL),
            ("Caption — RaeSans", TYPE_CAPTION),
        ];
        for (s, style) in ramp.iter() {
            c.draw_text_aa(40, y, s, *style, fg, FontFamily::Sans);
            y += style.line_height as i32 + 14;
        }
        y += 10;
        c.draw_text_aa(40, y, "RaeMono — JetBrains Mono", TYPE_LABEL, dim, FontFamily::Sans);
        y += TYPE_LABEL.line_height as i32 + 8;
        c.draw_text_aa(40, y, "fn main() { let x: u32 = 0xCAFE; }", TYPE_BODY, fg, FontFamily::Mono);
        y += TYPE_BODY.line_height as i32 + 6;
        c.draw_text_aa(40, y, "0123456789 +-*/= () [] {} <> 0O 1lI", TYPE_BODY, fg, FontFamily::Mono);
    }
    fb.save(
        dir,
        "atom-type-ramp.png",
        manifest,
        "raefont grayscale-AA type ramp (Display..Caption, RaeSans + RaeMono) — text-crispness reference",
    );
}

/// The single focus-ring renderer (`raeui::draw_focus_ring`) in BOTH modes:
/// normal accent (left) and high-contrast forced-colors (right). HC is the live
/// `rae_tokens::set_high_contrast` toggle — proving the ring auto-swaps to the
/// HC cyan without the surface knowing.
fn render_focus_rings_atom(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    let (w, h) = (760usize, 320usize);
    let mut fb = HostFb::new(w, h);
    {
        let mut c = fb.canvas();
        c.fill_rect(0, 0, w, h, 0xFF_12_18_24);
        let accent = 0xFF_2E_5C_C8u32;

        // LEFT: a focused button, normal mode.
        rae_tokens::set_high_contrast(false);
        let (bx, by, bw, bh) = (90usize, 110usize, 220usize, 64usize);
        c.fill_rounded_rect(bx, by, bw, bh, 14, 0xFF_24_2E_42);
        raeui::accessibility::draw_focus_ring(&mut c, bx, by, bw, bh, 14, accent);
        c.draw_text_aa((bx + 28) as i32, (by + 22) as i32, "Focused (normal)", TYPE_LABEL, 0xFF_ED_F1_F8, FontFamily::Sans);
        c.draw_text_aa(90, 60, "draw_focus_ring — accent", TYPE_CAPTION, 0xFF_8A_93_A6, FontFamily::Sans);

        // RIGHT: the SAME call under forced-colors HC — auto-swaps to HC cyan.
        rae_tokens::set_high_contrast(true);
        let (hx, hy) = (460usize, 110usize);
        c.fill_rounded_rect(hx, hy, bw, bh, 14, 0xFF_00_00_00);
        raeui::accessibility::draw_focus_ring(&mut c, hx, hy, bw, bh, 14, accent);
        c.draw_text_aa((hx + 28) as i32, (hy + 22) as i32, "Focused (HC)", TYPE_LABEL, 0xFF_FF_FF_FF, FontFamily::Sans);
        c.draw_text_aa(460, 60, "high-contrast forced-colors", TYPE_CAPTION, 0xFF_8A_93_A6, FontFamily::Sans);
        rae_tokens::set_high_contrast(false); // restore global
    }
    fb.save(
        dir,
        "atom-focus-ring.png",
        manifest,
        "raeui::draw_focus_ring: normal accent (left) + auto-swapped high-contrast cyan (right)",
    );
}

/// The AthGFX line-icon set (`raegfx::icon::Icon`) — the real glyphs that
/// replace the W/B/F/N/A/X/G/R/P + H/D/L/M LETTER placeholders visual-QA flagged
/// (`visual-qa-critique-2026-06-21.md` #1/#2). Renders every icon:
///   * Row block 1 — at Control-Center TILE size (28px), text.secondary tint,
///     each in a glass tile so it reads exactly as it will in the panel.
///   * Row block 2 — at a large HERO size (72px) so visual-QA can confirm the
///     vector strokes stay crisp when scaled (NOT a bitmap, NOT a letter).
///   * A tinted strip — the same icons in accent + file-type token colors,
///     proving monochrome icons take any token color.
/// Runs a FAIL-able assertion that every icon painted real ink at tile size.
fn render_icon_sheet_atom(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raegfx::icon::Icon;

    let all = Icon::ALL;
    let cols = 8usize;
    let rows = all.len().div_ceil(cols);

    // Layout metrics.
    let tile = 56usize; // glass tile box
    let icon_sz = 28i32; // CC tile icon size
    let gap = 16usize;
    let pad = 32usize;
    let label_h = 18usize;
    let hero = 72i32; // large scaling-proof size
    let hero_box = 96usize;

    let grid_w = cols * tile + (cols - 1) * gap;
    let w = pad * 2 + grid_w;
    // height: title + tile grid (+labels) + hero row + tint strip
    let grid_h = rows * (tile + label_h) + (rows - 1) * gap;
    let hero_cols = all.len().min(8);
    let hero_w = hero_cols * hero_box + (hero_cols - 1) * gap;
    let h = pad
        + 40 // title
        + grid_h
        + 48 // hero heading
        + hero_box
        + 48 // tint heading
        + tile
        + pad;
    let w = w.max(pad * 2 + hero_w);

    let mut fb = HostFb::new(w, h);
    let txt_secondary = 0xFF_AE_B4_C6u32;
    let txt_primary = 0xFF_F0_F2_F8u32;
    let txt_tertiary = 0xFF_6E_76_8Cu32;
    let accent = 0xFF_4E_9C_FFu32;

    {
        let mut c = fb.canvas();
        c.fill_rect(0, 0, w, h, 0xFF_0E_12_1C);

        c.draw_text_aa(
            pad as i32,
            (pad - 4) as i32,
            "AthGFX line icons — crisp vector glyphs (NOT letters, NOT bitmaps)",
            TYPE_SUBTITLE,
            txt_primary,
            FontFamily::Sans,
        );

        // ── Block 1: every icon at CC tile size, inside a glass tile + label ──
        let grid_top = pad + 40;
        for (i, icon) in all.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            let tx = pad + col * (tile + gap);
            let ty = grid_top + row * (tile + label_h + gap);
            // glass tile
            c.fill_rounded_rect(tx, ty, tile, tile, 12, 0xFF_1A_1E_2E);
            c.draw_rounded_rect_outline(tx, ty, tile, tile, 12, 0x33_FF_FF_FF);
            // centered icon
            let ix = (tx + (tile - icon_sz as usize) / 2) as i32;
            let iy = (ty + (tile - icon_sz as usize) / 2) as i32;
            c.draw_icon(*icon, ix, iy, icon_sz, txt_secondary);
            // name label under the tile
            c.draw_text_aa(
                tx as i32,
                (ty + tile + 2) as i32,
                icon.name(),
                TYPE_CAPTION,
                txt_tertiary,
                FontFamily::Sans,
            );
        }

        // ── Block 2: hero size (scaling proof) ──
        let hero_head_y = grid_top + grid_h + 16;
        c.draw_text_aa(
            pad as i32,
            hero_head_y as i32,
            "Same icons at 72px — vector strokes stay crisp (scaling proof)",
            TYPE_LABEL,
            txt_secondary,
            FontFamily::Sans,
        );
        let hero_top = hero_head_y + 28;
        for (i, icon) in all.iter().take(hero_cols).enumerate() {
            let hx = pad + i * (hero_box + gap);
            c.fill_rounded_rect(hx, hero_top, hero_box, hero_box, 16, 0xFF_16_1A_28);
            let ix = (hx + (hero_box - hero as usize) / 2) as i32;
            let iy = (hero_top + (hero_box - hero as usize) / 2) as i32;
            c.draw_icon(*icon, ix, iy, hero, txt_primary);
        }

        // ── Tint strip: a row tinted with accent + file-type tokens ──
        let tint_head_y = hero_top + hero_box + 16;
        c.draw_text_aa(
            pad as i32,
            tint_head_y as i32,
            "Monochrome — tints with any token color (accent / file-type)",
            TYPE_LABEL,
            txt_secondary,
            FontFamily::Sans,
        );
        let tint_top = tint_head_y + 28;
        let tints: [(Icon, u32); 8] = [
            (Icon::Folder, accent),                // ftype.dir = accent
            (Icon::Code, accent),                  // ftype.code = accent
            (Icon::Exec, 0xFF_3F_BF_7F),           // ftype.exec
            (Icon::Media, 0xFF_C0_7C_FF),          // ftype.media
            (Icon::Doc, 0xFF_F0_C8_5C),            // ftype.doc
            (Icon::Archive, 0xFF_F0_A0_3C),        // ftype.archive
            (Icon::GameController, accent),        // gaming accent
            (Icon::Performance, 0xFF_E8_B5_4B),    // warn-ish
        ];
        for (i, (icon, color)) in tints.iter().enumerate() {
            let tx = pad + i * (tile + gap);
            c.fill_rounded_rect(tx, tint_top, tile, tile, 12, 0xFF_1A_1E_2E);
            c.draw_rounded_rect_outline(tx, tint_top, tile, tile, 12, 0x33_FF_FF_FF);
            let ix = (tx + (tile - icon_sz as usize) / 2) as i32;
            let iy = (tint_top + (tile - icon_sz as usize) / 2) as i32;
            c.draw_icon(*icon, ix, iy, icon_sz, *color);
        }
    }

    // FAIL-able proof: re-render each icon onto a clean buffer at tile size and
    // assert it painted non-trivial ink (not empty, not a stray dot, not the 8x8
    // letter fallback). If draw_icon regresses to nothing, the harness FAILS.
    let probe = 40usize;
    for icon in all.iter() {
        let mut sfb = HostFb::new(probe, probe);
        {
            let mut sc = sfb.canvas();
            sc.draw_icon(*icon, 4, 4, 32, 0xFF_FF_FF_FF);
        }
        let painted = sfb.px.iter().filter(|&&p| p != 0).count();
        assert!(
            painted >= 30,
            "icon {} painted only {} px — empty/degenerate, would look like a placeholder",
            icon.name(),
            painted
        );
    }
    println!(
        "  icon sheet: {} icons, all paint real ink at tile size  -> CRISP, NOT LETTERS",
        all.len()
    );

    fb.save(
        dir,
        "atom-icons.png",
        manifest,
        "AthGFX line-icon set: CC-tile size in glass tiles + 72px scaling proof + token-tinted strip (replaces letter placeholders)",
    );
}

// ════════════════════════════════════════════════════════════════════════
// Shipped surfaces (real render fns, host-callable, no kernel state)
// ════════════════════════════════════════════════════════════════════════

/// The Control Center panel via the SHIPPED `ControlCenter::render(&mut Canvas)`
/// — the same code the kernel composites. Populated with representative state
/// (game mode on, media playing, an expanded Wi-Fi list) over a desktop backdrop.
fn render_control_center_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::control_center::{ControlCenter, ExpandRow, TileKind};
    let (w, h) = (1280usize, 800usize);
    let taskbar = 44usize;
    let mut fb = HostFb::new(w, h);

    let mut cc = ControlCenter::new(w, h, taskbar);
    cc.visible = true;
    cc.toggle_tile(TileKind::GameMode);
    cc.set_media(true, "Midnight City", "M83");
    cc.toggle_expand(TileKind::WiFi);
    cc.set_expand_rows(vec![
        ExpandRow { name: String::from("Raeen-5G"), signal: 4, secured: true, connected: true },
        ExpandRow { name: String::from("Cafe Guest"), signal: 2, secured: false, connected: false },
        ExpandRow { name: String::from("Neighbor"), signal: 1, secured: true, connected: false },
    ]);

    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        // A faux taskbar strip so the panel sits in context.
        c.fill_rect(0, h - taskbar, w, taskbar, 0xE6_10_14_1E);
        cc.render(&mut c);
    }
    fb.save(
        dir,
        "surface-control-center.png",
        manifest,
        "SHIPPED ControlCenter::render OVER the Aurora Mesh backdrop — glass panel, tile grid, sliders, media card, expanded Wi-Fi list (void-defect before/after)",
    );
}

/// The LIVE Files window via `files::render_preview(canvas, &FilesViewState)` —
/// the real `apps/files` render path (host-renderable since the lib/bin split +
/// the raekit `host` feature), NOT the quarantined `raeshell::file_manager` dead
/// twin (retired from this harness). Rendered over the signature aurora backdrop.
fn render_files_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    let (w, h) = (1100usize, 720usize);
    let mut fb = HostFb::new(w, h);
    let state = files::preview_state_demo();
    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        files::render_preview(&mut c, &state);
    }
    fb.save(
        dir,
        "surface-files.png",
        manifest,
        "LIVE apps/files::render_preview — real Files window (Home demo state) over the aurora",
    );
}

/// Notification toasts via the SHIPPED `NotificationDaemon::render_toasts`.
fn render_notification_toasts_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::notifications_daemon::{NotificationDaemon, NotificationHints, Urgency};
    let (w, h) = (1280usize, 800usize);
    let mut fb = HostFb::new(w, h);

    let mut nd = NotificationDaemon::new(w as u32, h as u32);
    let now = 1_000u64;
    nd.notify(
        "Messages", 0, "chat",
        "Aria Chen", "Are we still on for tonight? 🎮",
        &[("reply", "Reply"), ("mute", "Mute")],
        NotificationHints::new().with_urgency(Urgency::Normal),
        -1, now, 21, 14,
    );
    nd.notify(
        "AthStore", 0, "download",
        "Update ready", "Celeste 1.4 finished downloading.",
        &[("install", "Install")],
        NotificationHints::new().with_urgency(Urgency::Low),
        -1, now + 1, 21, 14,
    );
    nd.notify(
        "Battery", 0, "warning",
        "Low battery", "12% remaining — plug in soon.",
        &[],
        NotificationHints::new().with_urgency(Urgency::Critical),
        -1, now + 2, 21, 15,
    );

    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        nd.render_toasts(&mut c);
    }
    fb.save(
        dir,
        "surface-notifications.png",
        manifest,
        "SHIPPED NotificationDaemon::render_toasts OVER the Aurora Mesh backdrop — stacked glass toast cards (normal/low/critical urgency)",
    );
}

/// The SHIPPED `Taskbar::render(&mut Canvas)` over the Aurora Mesh — the
/// `glass.chrome` tier (IDENTITY.md §7): a translucent frosted bar the aurora
/// reads through, frosted app pills, an accent-filled active app with dark
/// on-accent ink, token-tinted tray icons. The first time the taskbar is
/// critiquable. Rendered as a 56px edge-docked bar at the bottom over the aurora.
fn render_taskbar_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::taskbar::{Taskbar, TaskButtonState};
    let (w, h) = (1280usize, 800usize);
    let bar_h = 56usize;
    let mut fb = HostFb::new(w, h);

    let mut tb = Taskbar::new(w as u32, h as u32);
    tb.width = w as u32;
    tb.height = bar_h as u32;
    // Representative running apps: one focused (active = accent pill), one
    // hovered, one normal, one urgent — exercising every pill state.
    tb.add_task(1, "files", "Files", 'F');
    tb.add_task(2, "browser", "RaeBrowser", 'W');
    tb.add_task(3, "terminal", "Terminal", 'T');
    tb.add_task(4, "messages", "Messages", 'M');
    // Exercise every pill state; the REAL layout centers the cluster (Win11).
    let states = [
        TaskButtonState::Focused,
        TaskButtonState::Hovered,
        TaskButtonState::Normal,
        TaskButtonState::Urgent,
    ];
    for (i, btn) in tb.task_buttons.iter_mut().enumerate() {
        btn.width = 150;
        btn.state = states[i.min(states.len() - 1)];
    }
    tb.task_buttons[3].badge_count = 3;
    tb.relayout();
    // Tray icons (net / volume / battery) — the live shell init adds these;
    // without them the tray corner critiques as empty.
    tb.system_tray.add_icon("network", "Network: Connected", 'N');
    tb.system_tray.add_icon("volume", "Volume: 75%", 'V');
    tb.system_tray.add_icon("battery", "Battery: 100%", 'B');

    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        // Render the bar into a bottom-docked sub-canvas region by offsetting the
        // framebuffer pointer to the bar's top-left row.
        let mut bar_px = vec![0u32; w * bar_h];
        // copy the aurora strip under the bar so the glass composites over it.
        for y in 0..bar_h {
            for x in 0..w {
                bar_px[y * w + x] = fb.px[(h - bar_h + y) * w + x];
            }
        }
        {
            let mut bc = unsafe {
                Canvas::new(bar_px.as_mut_ptr() as *mut u8, w, bar_h, 4)
            };
            tb.render(&mut bc);
        }
        for y in 0..bar_h {
            for x in 0..w {
                c.draw_pixel(x, h - bar_h + y, bar_px[y * w + x]);
            }
        }
    }
    fb.save(
        dir,
        "surface-taskbar.png",
        manifest,
        "SHIPPED Taskbar::render OVER the Aurora Mesh — glass.chrome tier (see-through frosted bar + iridescent rim), frosted app pills, accent-filled active app with dark on-accent ink, token-tinted tray (first critiquable taskbar)",
    );
}

/// The SHIPPED `StartMenu::render(&mut Canvas)` over the Aurora Mesh — the
/// `glass.popover` tier (IDENTITY.md §7): a frosted flyout the aurora reads
/// through, app tiles as frosted cards, the selected row accent-filled with dark
/// on-accent ink. Retires the old opaque `StartMenuTheme` navy palette.
fn render_start_menu_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::start_menu::{AppCategory, RecommendedItem, StartMenu};
    let (w, h) = (1280usize, 800usize);
    let mut fb = HostFb::new(w, h);

    let mut sm = StartMenu::new(w as u32, h as u32);
    for (name, id, exec, icon, cat) in [
        ("Files", "com.raeos.files", "/usr/bin/raefiles", 'F', AppCategory::Utilities),
        ("Browser", "com.raeos.browser", "/usr/bin/raebrowser", 'W', AppCategory::Web),
        ("Terminal", "com.raeos.terminal", "/usr/bin/raeterminal", 'T', AppCategory::Utilities),
        ("Settings", "com.raeos.settings", "/usr/bin/raesettings", 'S', AppCategory::System),
        ("Editor", "com.raeos.editor", "/usr/bin/raeeditor", 'E', AppCategory::Productivity),
        ("RaeGames", "com.raeos.games", "/usr/bin/raegames", 'G', AppCategory::Games),
    ] {
        let appid = sm.add_app(name, id, exec, icon, cat);
        sm.pin_app(appid);
    }
    // A second populated row — Recommended/recents — so the menu isn't a single
    // sparse row (Round-7 "ALSO"). Mirrors what the live `start_menu::init` adds.
    sm.add_recommended(RecommendedItem::new_file(
        "report.pdf",
        "/home/user/Documents/report.pdf",
        1000,
    ));
    sm.add_recommended(RecommendedItem::new_file(
        "main.rs",
        "/home/user/Projects/main.rs",
        980,
    ));
    sm.add_recommended(RecommendedItem::new_app("RaeGames", "com.raeos.games", 940));
    sm.open();

    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        // A faux taskbar strip so the anchored menu sits in context (same as
        // the Control Center surface).
        c.fill_rect(0, h - 44, w, 44, 0xE6_10_14_1E);
        // `StartMenu::render` anchors ITSELF at self.x/self.y on a full-screen
        // canvas (panel_origin). The old menu-sized sub-canvas here predated
        // that fix and clipped the panel at its own right edge.
        sm.render(&mut c);
    }
    fb.save(
        dir,
        "surface-start-menu.png",
        manifest,
        "SHIPPED StartMenu::render OVER the Aurora Mesh — glass.popover tier (frosted flyout + iridescent rim), app tiles as frosted cards, accent-filled selection with dark on-accent ink (retires the opaque StartMenuTheme navy)",
    );
}

/// The SHIPPED `ContextMenu::render(&mut Canvas, x, y)` over the Aurora Mesh —
/// the `glass.popover` tier (IDENTITY.md §7) applied to the highest-frequency
/// surface: the right-click flyout. A frosted card the aurora reads through with
/// the iridescent rim + a soft ambient drop shadow so it floats; RaeSans
/// `type.label` rows with a leading `raegfx` line-icon, an optional right-aligned
/// `type.caption` shortcut hint, hairline `stroke.subtle` separators between
/// groups, a hovered row as an accent wash with dark on-accent ink, and a
/// disabled row in `text.tertiary`. Retires the old flat ad-hoc styling.
fn render_context_menu_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::start_menu::{ContextAction, ContextMenu, ContextMenuItem};
    let (w, h) = (640usize, 480usize);
    let mut fb = HostFb::new(w, h);

    // A representative Files-style right-click menu: Open / Open file location /
    // —— / Run as administrator / App settings / —— / Pin to taskbar (disabled) /
    // Uninstall. Built on the shipped item shapes (action-keyed icons + the same
    // separator + shortcut fields the live constructors use). One item is disabled
    // so the text.tertiary disabled style is in the shot.
    let mut menu = ContextMenu::new();
    menu.items = vec![
        ContextMenuItem {
            action: ContextAction::Open,
            label: String::from("Open"),
            icon_char: 'O',
            enabled: true,
            separator_after: false,
            shortcut: Some(String::from("Enter")),
        },
        ContextMenuItem {
            action: ContextAction::OpenFileLocation,
            label: String::from("Open file location"),
            icon_char: 'L',
            enabled: true,
            separator_after: true,
            shortcut: None,
        },
        ContextMenuItem {
            action: ContextAction::RunAsAdmin,
            label: String::from("Run as administrator"),
            icon_char: 'A',
            enabled: true,
            separator_after: false,
            shortcut: Some(String::from("Ctrl+Shift+Enter")),
        },
        ContextMenuItem {
            action: ContextAction::AppSettings,
            label: String::from("App settings"),
            icon_char: 'S',
            enabled: true,
            separator_after: true,
            shortcut: None,
        },
        ContextMenuItem {
            action: ContextAction::PinToTaskbar,
            label: String::from("Pin to taskbar"),
            icon_char: 'T',
            enabled: false, // disabled → text.tertiary style
            separator_after: false,
            shortcut: None,
        },
        ContextMenuItem {
            action: ContextAction::Uninstall,
            label: String::from("Uninstall"),
            icon_char: 'X',
            enabled: true,
            separator_after: false,
            shortcut: Some(String::from("Del")),
        },
    ];
    menu.visible = true;
    menu.selected = 2; // hover the "Run as administrator" row (accent wash)
    menu.relayout();

    let (mx, my) = (60i32, 60i32);
    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        menu.render(&mut c, mx, my);
    }

    // FAIL-able (Round-9 visual-QA — SHIP-GATE a11y + warm-amber identity), measured on
    // the rendered PNG:
    //   (1) white text.primary must clear WCAG AA 4.5:1 EVERYWHERE on the popover glass,
    //       including the lower third over the bright-aurora bleed (the regression was
    //       3.7–3.9:1: the mean-channel luma cap did not guarantee real WCAG contrast for
    //       saturated interior pixels). We measure the worst CR of white text.primary
    //       against the GLASS the text sits over (the popover surface re-rendered without
    //       menu content, so AA glyph-fringe blends don't pollute the background measure).
    //   (2) the warm-amber rim stop must be VISIBLE as warm/gold on the menu's bottom
    //       edge over the REAL aurora (the defect: 0 warm px across the real surfaces).
    {
        let mw = menu.width as usize;
        let mh = menu.height as usize;
        let radius = rae_tokens::RADIUS_MD as usize; // matches the menu's own surface radius
        let text = rae_tokens::active_palette().text_primary;

        // (1) Glass-only re-render at the SAME rect over the SAME aurora — the background
        // the white rows sit on. Worst interior CR must clear AA over the bright bleed.
        let mut gfb = HostFb::new(w, h);
        {
            let mut c = gfb.canvas();
            aurora_backdrop(&mut c, w, h);
            raegfx::glass::draw_glass_surface(
                &mut c,
                mx as usize,
                my as usize,
                mw,
                mh,
                radius,
                rae_tokens::GLASS_POPOVER_DARK,
            );
        }
        let mut worst = f32::MAX;
        // Skip the 3px iridescent rim band + the top highlight (additive light over the
        // capped interior); body text sits inset past them. Inset 8px from the rect edge
        // clears the band everywhere except the rounded corners, which carry no text.
        let band = rae_tokens::GLASS_EDGE_BAND_PX as usize;
        for yy in (my as usize + band + 5)..(my as usize + mh - band - 5).min(h) {
            for xx in (mx as usize + band + 5)..(mx as usize + mw - band - 5).min(w) {
                let p = gfb.px[yy * w + xx] | 0xFF00_0000;
                let cr = rae_tokens::contrast_ratio(text, p);
                if cr < worst {
                    worst = cr;
                }
            }
        }
        println!(
            "  context menu legibility: white text.primary worst CR over popover glass = {:.2}  -> {}",
            worst,
            if worst >= 4.5 { "AA PASS" } else { "AA FAIL" }
        );
        assert!(
            worst >= 4.5,
            "context-menu popover glass fails WCAG AA: white text.primary worst CR {worst:.2} < 4.5 \
             (the SHIP-GATE a11y regression — the lower third over the bright aurora bleed)"
        );

        // (2) warm-amber must read warm/gold on the bottom edge over the real aurora.
        let mut warm = 0u32;
        for xx in (mx as usize + radius)..(mx as usize + mw - radius).min(w) {
            for d in 1..=band {
                let yy = (my as usize + mh).saturating_sub(d);
                let p = fb.px[yy * w + xx];
                let r = ((p >> 16) & 0xFF) as i64;
                let g = ((p >> 8) & 0xFF) as i64;
                let b = (p & 0xFF) as i64;
                // warm/gold: red & green decisively above blue (the GLASS_EDGE_WARM read).
                if r >= b + 24 && g >= b + 12 {
                    warm += 1;
                }
            }
        }
        // OBSIDIAN (IDENTITY-OBSIDIAN.md §2): the surface rim is retired — the
        // menu edge must carry essentially ZERO warm rim pixels now.
        println!(
            "  context menu edge: bottom warm/gold px (over real aurora) = {}  -> {}",
            warm,
            if warm < 20 { "CLEAN (no rim)" } else { "RIM REGRESSED" }
        );
        assert!(
            warm < 20,
            "obsidian surfaces carry no warm rim — context-menu bottom edge shows \
             {warm} warm px (the retired iridescent rim regressed back in)"
        );
    }

    // FAIL-able: the rendered menu must (a) paint an accent wash on the hovered
    // row and (b) not be a flat field (glass + ink give a real luma spread).
    let a = rae_tokens::derive_accent(raeshell::active_accent(), raeshell::active_palette());
    let (ar, ag, ab) = (
        ((a.base >> 16) & 0xFF) as i32,
        ((a.base >> 8) & 0xFF) as i32,
        (a.base & 0xFF) as i32,
    );
    let mut accent_px = 0u32;
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    for &p in fb.px.iter() {
        let r = ((p >> 16) & 0xFF) as i32;
        let g = ((p >> 8) & 0xFF) as i32;
        let b = (p & 0xFF) as i32;
        if (r - ar).abs() < 40 && (g - ag).abs() < 40 && (b - ab).abs() < 40 {
            accent_px += 1;
        }
        let l = (r + g + b) as u32;
        lo = lo.min(l);
        hi = hi.max(l);
    }
    assert!(
        accent_px > 200,
        "context menu hover row must paint an accent wash (accent-like px={accent_px})"
    );
    assert!(
        hi - lo > 80,
        "context menu reads as a flat field (luma spread {} too small) — not glass + ink",
        hi - lo
    );
    println!("  context menu: accent-wash px={accent_px}, luma spread {}  -> GLASS + ACCENT HOVER", hi - lo);

    fb.save(
        dir,
        "surface-context-menu.png",
        manifest,
        "SHIPPED ContextMenu::render OVER the Aurora Mesh — glass.popover flyout (frosted + iridescent rim + soft shadow), RaeSans rows with leading line-icons, right-aligned shortcut hints, stroke.subtle separators, an accent-wash hover row with dark on-accent ink, a disabled text.tertiary row (glassifies the highest-frequency right-click surface)",
    );
}

/// The SHIPPED `ControlPanel::render(&mut Canvas, ox,oy,w,h)` over the Aurora
/// Mesh — the `glass.panel` tier (IDENTITY.md §7): a frosted Settings window the
/// aurora reads through, a frosted sidebar (panel tier) + a de-tinted SOLID
/// content area (readable, NOT a dark box), accent on the selected nav row.
fn render_settings_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::control_panel::ControlPanel;
    let (w, h) = (1280usize, 800usize);
    let mut fb = HostFb::new(w, h);

    let mut cp = ControlPanel::new();
    // `show()` lands on the Appearance & Vibe → Colors page (a populated detail
    // pane + a lit nav-row icon), not the old empty landing (visual-QA Round-7 #2).
    cp.show();

    let (wx, wy, ww, wh) = (140usize, 90usize, 1000usize, 620usize);
    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        cp.render(&mut c, wx, wy, ww, wh);
    }
    fb.save(
        dir,
        "surface-settings.png",
        manifest,
        "SHIPPED ControlPanel::render OVER the Aurora Mesh — glass.panel tier (frosted Settings window + iridescent rim), frosted sidebar + de-tinted SOLID content area (readable, not a dark box), accent on the selected nav row",
    );
}

/// The SHIPPED `LockScreen::render(&mut [u32], stride)` — the login/unlock
/// moment, glassified to the Liquid Glass identity (IDENTITY.md §3 Aurora +
/// §7 popover tier). Driven through the EXACT raw-buffer path the kernel's
/// `shell_runner::render_lock` calls (the lock screen wraps the slice in a
/// `raegfx::Canvas` internally), so this capture exercises the shipped code,
/// not a host-only preview. Shows the aurora backdrop + a centered frosted
/// glass card holding the clock, avatar, display name, and password pill with
/// the iridescent rim.
fn render_lock_screen_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::lock_screen::LockScreen;
    let (w, h) = (1280usize, 800usize);
    let mut fb = HostFb::new(w, h);

    let mut ls = LockScreen::new(w as u32, h as u32);
    ls.lock();
    ls.set_display_name("Aria Chen");
    // Type a few chars so the password pill shows filled dots (not the empty
    // placeholder) — the realistic mid-auth frame.
    for &k in b"hunter2".iter() {
        ls.handle_input(k);
    }

    // The exact kernel call: render straight into the raw ARGB framebuffer with
    // stride == width. The lock screen builds its own Canvas over this slice.
    ls.render(&mut fb.px, w);

    // FAIL-able: the frame must NOT be a flat field — the Aurora Mesh + glass
    // card give a wide luma spread (the old hardcoded-hex render was near-flat).
    let mut lo = u32::MAX;
    let mut hi = 0u32;
    for &p in fb.px.iter() {
        let l = ((p >> 16) & 0xFF) + ((p >> 8) & 0xFF) + (p & 0xFF);
        lo = lo.min(l);
        hi = hi.max(l);
    }
    assert!(
        hi - lo > 80,
        "lock screen reads as a flat field (luma spread {} too small) — not aurora + glass",
        hi - lo
    );
    println!("  lock screen: luma spread {} (lo={lo} hi={hi})  -> AURORA + GLASS CARD", hi - lo);

    fb.save(
        dir,
        "surface-lock-screen.png",
        manifest,
        "SHIPPED LockScreen::render OVER the Aurora Mesh — glass.popover card (frosted + iridescent rim) holding the clock, avatar, display name, and password pill; token colors + RaeSans (retires the flat hardcoded-hex lock render)",
    );
}

/// The SHIPPED `CommandPalette::render(&mut Canvas)` over the Aurora Mesh — the
/// `glass.popover` tier (IDENTITY.md §7) applied to the global launcher: a
/// frosted search/command flyout the aurora reads through, with the iridescent
/// rim + a soft ambient drop shadow so it floats (macOS Spotlight / Win11
/// PowerToys Run quality). A representative query ("fir") returns a few ranked
/// rows: the selected (top) row is an accent wash with dark on-accent ink, each
/// row leads with a real `raegfx` line-icon, titles in RaeSans `type.body`,
/// paths/hints in `text.secondary`, a right-aligned `text.tertiary` category tag.
fn render_command_palette_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::command_palette::CommandPalette;
    let (w, h) = (1280usize, 800usize);
    let mut fb = HostFb::new(w, h);

    let mut pal = CommandPalette::new(w, h);
    // Representative app/setting catalog so "fir" lands a handful of ranked rows
    // (an app, a settings-action, plus a file) — enough to show the row stack,
    // the icons, and the accent-highlighted selection.
    for (name, exec, desc, kw) in [
        ("Firefox", "firefox", "Web browser", &["web", "internet", "browser"][..]),
        ("Files", "files", "File manager", &["explorer", "folder"][..]),
        ("Find My Device", "findmy", "Locate this device", &["find", "locate", "device"][..]),
        ("Terminal", "terminal", "Command line", &["shell", "console"][..]),
    ] {
        pal.index_app(name, exec, desc, kw);
    }
    pal.index_file("/home/user/Documents/firmware-notes.txt");
    pal.open();
    for c in "fir".chars() {
        pal.push_char(c);
    }

    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        // A faux taskbar strip so the flyout sits in desktop context.
        let taskbar = 44usize;
        c.fill_rect(0, h - taskbar, w, taskbar, 0xE6_10_14_1E);
        pal.render(&mut c);
    }

    // FAIL-able: the flyout must (a) paint an accent wash on the selected row and
    // (b) read as glass + ink (a real luma spread), not a flat field.
    let a = rae_tokens::derive_accent(raeshell::active_accent(), raeshell::active_palette());
    let (ar, ag, ab) = (
        ((a.base >> 16) & 0xFF) as i32,
        ((a.base >> 8) & 0xFF) as i32,
        (a.base & 0xFF) as i32,
    );
    let mut accent_px = 0u32;
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    for &p in fb.px.iter() {
        let r = ((p >> 16) & 0xFF) as i32;
        let g = ((p >> 8) & 0xFF) as i32;
        let b = (p & 0xFF) as i32;
        if (r - ar).abs() < 40 && (g - ag).abs() < 40 && (b - ab).abs() < 40 {
            accent_px += 1;
        }
        let l = (r + g + b) as u32;
        lo = lo.min(l);
        hi = hi.max(l);
    }
    assert!(
        accent_px > 200,
        "command palette selected row must paint an accent wash (accent-like px={accent_px})"
    );
    assert!(
        hi - lo > 80,
        "command palette reads as a flat field (luma spread {} too small) — not glass + ink",
        hi - lo
    );
    println!(
        "  command palette: accent-wash px={accent_px}, luma spread {}  -> GLASS POPOVER + ACCENT SELECTION",
        hi - lo
    );

    fb.save(
        dir,
        "surface-command-palette.png",
        manifest,
        "SHIPPED CommandPalette::render OVER the Aurora Mesh — glass.popover flyout (frosted + iridescent rim + soft ambient shadow), RaeSans search field with a real magnifier icon, result rows with leading raegfx line-icons, an accent-wash selected row with dark on-accent ink, secondary path text + right-aligned category tags (retires the deprecated GLASS_TINT_DARK alias fill)",
    );
}

fn render_snap_layout_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::snap_layouts::SnapOverlay;
    use raeshell::tiling_wm::Rect;
    let (w, h) = (1280usize, 800usize);
    let mut fb = HostFb::new(w, h);

    // Work area = full screen minus a 44px taskbar (where the layouts tile).
    let taskbar = 44usize;
    let work = Rect::new(0, 0, w as u32, (h - taskbar) as u32);
    let mut overlay = SnapOverlay::new(w, h, work);
    overlay.open();
    // Highlight the "large left" template's left zone so the accent-fill state
    // (the Win11 hover preview) is in the shot, not just the subtle idle tiles.
    overlay.set_hover(Some((1, 0)));

    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        // A faux taskbar strip so the flyout reads in desktop context.
        c.fill_rect(0, h - taskbar, w, taskbar, 0xE6_10_14_1E);
        overlay.render(&mut c);
    }

    // FAIL-able: the flyout must (a) paint an accent-fill preview on the hovered
    // zone and (b) read as glass + ink (a real luma spread), not a flat field.
    let a = rae_tokens::derive_accent(raeshell::active_accent(), raeshell::active_palette());
    let (ar, ag, ab) = (
        ((a.base >> 16) & 0xFF) as i32,
        ((a.base >> 8) & 0xFF) as i32,
        (a.base & 0xFF) as i32,
    );
    let mut accent_px = 0u32;
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    for &p in fb.px.iter() {
        let r = ((p >> 16) & 0xFF) as i32;
        let g = ((p >> 8) & 0xFF) as i32;
        let b = (p & 0xFF) as i32;
        if (r - ar).abs() < 40 && (g - ag).abs() < 40 && (b - ab).abs() < 40 {
            accent_px += 1;
        }
        let l = (r + g + b) as u32;
        lo = lo.min(l);
        hi = hi.max(l);
    }
    assert!(
        accent_px > 300,
        "snap layout hovered zone must paint an accent-fill preview (accent-like px={accent_px})"
    );
    assert!(
        hi - lo > 80,
        "snap layout flyout reads as a flat field (luma spread {} too small) — not glass + ink",
        hi - lo
    );
    println!(
        "  snap layouts: accent-fill px={accent_px}, luma spread {}  -> GLASS + ACCENT PREVIEW",
        hi - lo
    );

    fb.save(
        dir,
        "surface-snap-layouts.png",
        manifest,
        "SnapOverlay::render OVER the Aurora Mesh — the NEW Win11-style Snap Layouts flyout (glass.popover): a 3x2 grid of layout templates (two-even, large-left, left+stack, quadrants, thirds, wide-center), each a scaled preview of its exact work-area tiling, with an accent-fill hover preview on the selected zone (dark on-accent). Opened with the Rae key + Z; clicking a zone snaps the focused window to that exact region",
    );
}

fn render_snap_assist_surface(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use raeshell::snap_assist::{Candidate, SnapAssist};
    use raeshell::snap_layouts::SnapTemplate;
    use raeshell::tiling_wm::Rect;
    let (w, h) = (1280usize, 800usize);
    let mut fb = HostFb::new(w, h);

    // A window is already snapped into the LEFT half; Snap Assist offers the
    // other three windows to fill the empty RIGHT half.
    let taskbar = 44usize;
    let work = Rect::new(0, 0, w as u32, (h - taskbar) as u32);
    let zones = SnapTemplate::TwoEven.zones(work);
    let candidates = vec![
        Candidate { id: 2, title: String::from("RaeBrowser") },
        Candidate { id: 3, title: String::from("Terminal") },
        Candidate { id: 4, title: String::from("Files") },
    ];
    let mut assist = SnapAssist::new(zones, 0, 1, candidates, w, h);
    assist.set_hover(Some(0)); // light the first candidate tile

    {
        let mut c = fb.canvas();
        aurora_backdrop(&mut c, w, h);
        c.fill_rect(0, h - taskbar, w, taskbar, 0xE6_10_14_1E);
        assist.render(&mut c);
    }

    // FAIL-able: the hovered candidate tile must paint an accent highlight and
    // the picker must read as glass + ink (a real luma spread).
    let a = rae_tokens::derive_accent(raeshell::active_accent(), raeshell::active_palette());
    let (ar, ag, ab) = (
        ((a.base >> 16) & 0xFF) as i32,
        ((a.base >> 8) & 0xFF) as i32,
        (a.base & 0xFF) as i32,
    );
    let mut accent_px = 0u32;
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    for &p in fb.px.iter() {
        let r = ((p >> 16) & 0xFF) as i32;
        let g = ((p >> 8) & 0xFF) as i32;
        let b = (p & 0xFF) as i32;
        if (r - ar).abs() < 48 && (g - ag).abs() < 48 && (b - ab).abs() < 48 {
            accent_px += 1;
        }
        let l = (r + g + b) as u32;
        lo = lo.min(l);
        hi = hi.max(l);
    }
    assert!(
        accent_px > 150,
        "snap assist hovered tile must paint an accent highlight (accent-like px={accent_px})"
    );
    assert!(
        hi - lo > 80,
        "snap assist reads as a flat field (luma spread {} too small)",
        hi - lo
    );
    println!(
        "  snap assist: accent px={accent_px}, luma spread {}  -> GLASS + ACCENT TILE",
        hi - lo
    );

    fb.save(
        dir,
        "surface-snap-assist.png",
        manifest,
        "SnapAssist::render OVER the Aurora Mesh — after a window snaps to the LEFT half, Snap Assist fills the empty RIGHT zone with a glass picker of the other windows' tiles (accent-outlined on hover, RaeSans labels), the occupied zone outlined. Click a tile to fill the zone; advances zone-by-zone until the layout is full (Win11 parity)",
    );
}

// ════════════════════════════════════════════════════════════════════════
// Identity captures — the three new Liquid Glass primitives (IDENTITY.md §11)
// ════════════════════════════════════════════════════════════════════════

/// Step 2 (§3.2): the full-screen Aurora Mesh wallpaper, no windows — the living
/// blue-violet-teal mesh that kills the flat navy void. The single highest-impact
/// change. Proves `raegfx::glass::render_aurora_dark` directly.
fn render_aurora_wallpaper(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    let (w, h) = (1280usize, 800usize);
    let mut fb = HostFb::new(w, h);
    {
        let mut c = fb.canvas();
        raegfx::glass::render_aurora_dark(&mut c, 0, 0, w, h, 0);
    }
    // FAIL-able: the wallpaper must NOT be a flat void — assert a real luma spread
    // across the frame (the old navy gradient had almost none in the lower band).
    let mut lo = u32::MAX;
    let mut hi = 0u32;
    for &p in fb.px.iter() {
        let l = ((p >> 16) & 0xFF) + ((p >> 8) & 0xFF) + (p & 0xFF);
        lo = lo.min(l);
        hi = hi.max(l);
    }
    assert!(
        hi - lo > 80,
        "aurora wallpaper reads as a flat void (luma spread {} too small)",
        hi - lo
    );
    println!("  aurora wallpaper: luma spread {} (lo={lo} hi={hi})  -> LIVING MESH", hi - lo);
    fb.save(
        dir,
        "wallpaper-aurora-dark.png",
        manifest,
        "raegfx::glass::render_aurora_dark — full-screen procedural Aurora Mesh (blue/violet/teal blobs, soft falloff, vignette); replaces the flat navy void",
    );
}

/// Step 3 (§2): the three glass tiers (chrome / panel / popover) stacked over the
/// aurora so the increasing opacity left→right and the backdrop reading through
/// each are visible. Uses the SHIPPED `glass::draw_glass_surface` (tier + luma
/// auto-adjust + edge stack), the exact call the kernel + shell-apps make.
fn render_glass_tiers_over_aurora(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use rae_tokens::{GLASS_CHROME_DARK, GLASS_PANEL_DARK, GLASS_POPOVER_DARK};
    let (w, h) = (1180usize, 520usize);
    let mut fb = HostFb::new(w, h);
    {
        let mut c = fb.canvas();
        raegfx::glass::render_aurora_dark(&mut c, 0, 0, w, h, 0);

        let pw = 320usize;
        let ph = 340usize;
        let py = 110usize;
        let gap = 40usize;
        let total = 3 * pw + 2 * gap;
        let x0 = (w - total) / 2;
        let tiers = [
            ("chrome  25%", GLASS_CHROME_DARK),
            ("panel  45%", GLASS_PANEL_DARK),
            ("popover  60%", GLASS_POPOVER_DARK),
        ];
        for (i, (label, tier)) in tiers.iter().enumerate() {
            let px = x0 + i * (pw + gap);
            // soft ambient shadow so the glass reads as floating over the aurora.
            c.fill_rounded_rect_shadow(px, py, pw, ph, 16, 0x0A_10_1C, 40, 16);
            // the SHIPPED tiered-glass draw — tint + luma-adjust + edge stack.
            raegfx::glass::draw_glass_surface(&mut c, px, py, pw, ph, 16, *tier);
            c.draw_text_aa(
                (px + 24) as i32,
                (py + 24) as i32,
                label,
                TYPE_SUBTITLE,
                0xFF_F2_F5_FB,
                FontFamily::Sans,
            );
            c.draw_text_aa(
                (px + 24) as i32,
                (py + 60) as i32,
                "the aurora reads through",
                TYPE_CAPTION,
                0xCC_D6_DA_E2,
                FontFamily::Sans,
            );
        }
        c.draw_text_aa(
            (x0) as i32,
            56,
            "Three glass tiers over the Aurora Mesh — opacity rises left to right",
            TYPE_TITLE,
            0xFF_ED_F1_F8,
            FontFamily::Sans,
        );
    }
    fb.save(
        dir,
        "glass-tiers-over-aurora.png",
        manifest,
        "raegfx::glass::draw_glass_surface — chrome/panel/popover tiers over the aurora (luma auto-adjust + iridescent edge); backdrop reads through, opacity rises left->right",
    );
}

/// Step 4 (§2.4): a 3× zoom of a panel corner so the iridescent rim subtlety is
/// visible — the cyan→violet→warm sweep + bright top edge over the aurora. This is
/// the "instantly recognizable" signature shot. We render a panel at 1× then
/// nearest-upscale a corner crop 3× into the output so the 2px rim is legible.
fn render_iridescent_edge_3x(dir: &PathBuf, manifest: &mut Vec<(String, usize, String)>) {
    use rae_tokens::GLASS_PANEL_DARK;
    // 1× source render.
    let (sw, sh) = (380usize, 380usize);
    let mut src = HostFb::new(sw, sh);
    let (pnx, pny, pnw, pnh) = (60usize, 60usize, 280usize, 280usize);
    {
        let mut c = src.canvas();
        raegfx::glass::render_aurora_dark(&mut c, 0, 0, sw, sh, 0);
        // a panel filling most of the frame so its BOTTOM-RIGHT corner sits inside.
        raegfx::glass::draw_glass_surface(&mut c, pnx, pny, pnw, pnh, 24, GLASS_PANEL_DARK);
        // OBSIDIAN: surfaces no longer carry the rim (IDENTITY-OBSIDIAN.md §2) —
        // this atom demos the still-shipped `draw_iridescent_rim` PRIMITIVE
        // (theming callers), so invoke it directly on the panel.
        raegfx::glass::draw_iridescent_rim(&mut c, pnx, pny, pnw, pnh, 24);
    }
    // 3× nearest crop of the BOTTOM-RIGHT corner — the TWO-HUE corner (Round-4
    // visual-QA note): the RIGHT edge renders violet (#B47CFF) and the BOTTOM edge
    // renders warm-amber (#FFC97C), so this one crop proves the violet→warm half of
    // the sweep in a single shot (the cyan top/left is proven separately by the full
    // wallpaper + tiers shots and the `rim_renders_chromatic_pixels` host KAT). The
    // old shot cropped the TOP-LEFT and showed cyan only — which is exactly why the
    // rim read as "a cyan line." A 120×120 crop → 360×360.
    let (cropx, cropy, cropw, croph) = (
        (pnx + pnw + 20).saturating_sub(120),
        (pny + pnh + 20).saturating_sub(120),
        120usize,
        120usize,
    );
    let scale = 3usize;
    let (ow, oh) = (cropw * scale, croph * scale + 40);
    let mut fb = HostFb::new(ow, oh);
    {
        let mut c = fb.canvas();
        c.fill_rect(0, 0, ow, oh, 0xFF_0A_0D_16);
        for dy in 0..croph * scale {
            for dx in 0..cropw * scale {
                let sx = cropx + dx / scale;
                let sy = cropy + dy / scale;
                if sx < sw && sy < sh {
                    let p = src.px[sy * sw + sx];
                    c.draw_pixel(dx, dy + 40, p);
                }
            }
        }
        c.draw_text_aa(
            16,
            12,
            "Iridescent rim @ 3x — BOTTOM-RIGHT corner: violet right (#B47CFF) -> warm-amber bottom (#FFC97C)",
            TYPE_LABEL,
            0xFF_C8_D0_E0,
            FontFamily::Sans,
        );
    }

    // FAIL-able (the iridescent-sweep acceptance): the cropped BOTTOM-RIGHT corner
    // must contain measurable VIOLET (right edge) AND WARM-amber (bottom edge)
    // pixels — proving the rim is a chromatic sweep, not a cyan line. The Round-3
    // fix made the rim render; the Round-4 fix (edge-centered hue stops + the fixed
    // perimeter walk) makes the right edge violet and the bottom edge warm. Revert
    // either and both counts here collapse toward zero.
    let band = rae_tokens::GLASS_EDGE_BAND_PX as usize;
    let chroma = |p: u32| -> (i64, i64, i64) {
        (
            ((p >> 16) & 0xFF) as i64,
            ((p >> 8) & 0xFF) as i64,
            (p & 0xFF) as i64,
        )
    };
    let (mut violet, mut warm) = (0u32, 0u32);
    // right edge violet: a strip just inside the right border (red & blue both above
    // green — the violet stop's R=0xB4,B=0xFF,G=0x7C signature).
    for sy in pny + 40..pny + pnh - 40 {
        for sx in pnx + pnw - band..pnx + pnw {
            let (r, g, b) = chroma(src.px[sy * sw + sx]);
            if r >= g + 12 && b >= g + 24 {
                violet += 1;
            }
        }
    }
    // bottom edge warm-amber: a strip just inside the bottom border (red & green
    // both above blue — the warm stop's R=0xFF,G=0xC9,B=0x7C signature).
    for sx in pnx + 40..pnx + pnw - 40 {
        for sy in pny + pnh - band..pny + pnh {
            let (r, g, b) = chroma(src.px[sy * sw + sx]);
            if r >= b + 24 && g >= b + 12 {
                warm += 1;
            }
        }
    }
    println!("  iridescent rim: violet(right)={violet} warm-amber(bottom)={warm} chromatic px  -> SWEEP RENDERS");
    assert!(
        violet > 30 && warm > 30,
        "iridescent rim two-hue corner missing chromatic pixels (violet={violet} warm={warm}) — \
         the sweep would read cyan-monochrome again"
    );

    fb.save(
        dir,
        "glass-iridescent-edge-3x.png",
        manifest,
        "raegfx::glass::draw_iridescent_rim — 3x zoom of a panel BOTTOM-RIGHT corner: the violet(right)->warm-amber(bottom) chromatic sweep over the aurora (the iridescent signature, two hues in one crop); FAIL-able chromatic-pixel proof",
    );
}
