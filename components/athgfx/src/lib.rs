// no_std for real builds; std under `cargo test` so the tessellate host KAT can
// link (the gpu_userspace path is std anyway).
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod cache;
pub mod device;
pub mod glass;
pub mod hdr;
pub mod icon;
pub mod memory;
pub mod shader;
pub mod shared_queue;
pub mod surface;
#[cfg(feature = "tessellate")]
pub mod tessellate;
pub mod text;
pub mod vulkan;
#[cfg(feature = "gpu_userspace")]
pub mod wgpu_backend;
#[cfg(feature = "wgsl")]
pub mod wgsl;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════
// Canvas — framebuffer software rasterizer (existing)
// ═══════════════════════════════════════════════════════════════════════════

pub struct Canvas {
    buffer_ptr: *mut u8,
    width: usize,
    height: usize,
    bytes_per_pixel: usize,
}

impl Canvas {
    /// Create a new canvas from a raw framebuffer pointer.
    ///
    /// # Safety
    /// The pointer must be valid for `width * height * bytes_per_pixel` bytes
    /// and must be mapped with write permissions.
    pub unsafe fn new(ptr: *mut u8, width: usize, height: usize, bpp: usize) -> Self {
        Canvas {
            buffer_ptr: ptr,
            width,
            height,
            bytes_per_pixel: bpp,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }

    #[inline(always)]
    pub fn draw_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            let offset = (y * self.width + x) * self.bytes_per_pixel;
            unsafe {
                let p = self.buffer_ptr.add(offset);
                if self.bytes_per_pixel == 4 {
                    *(p as *mut u32) = color;
                } else if self.bytes_per_pixel == 3 {
                    // Assume BGR (typical for 24-bit VESA)
                    *p.add(0) = (color & 0xFF) as u8;
                    *p.add(1) = ((color >> 8) & 0xFF) as u8;
                    *p.add(2) = ((color >> 16) & 0xFF) as u8;
                }
            }
        }
    }

    pub fn clear(&mut self, color: u32) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.draw_pixel(x, y, color);
            }
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let x_end = core::cmp::min(x + w, self.width);
        let y_end = core::cmp::min(y + h, self.height);
        if x >= x_end || y >= y_end {
            return;
        }

        // Fast path: bpp=4 (ARGB) allows contiguous u32 writes per row.
        // ~10× faster than the per-pixel draw_pixel loop, which is what the
        // compositor surface format uses.
        if self.bytes_per_pixel == 4 {
            let span = x_end - x;
            for cy in y..y_end {
                let row_offset = (cy * self.width + x) * 4;
                unsafe {
                    let row = self.buffer_ptr.add(row_offset) as *mut u32;
                    for i in 0..span {
                        row.add(i).write(color);
                    }
                }
            }
        } else {
            // bpp=3 BGR fallback
            for curr_y in y..y_end {
                for curr_x in x..x_end {
                    self.draw_pixel(curr_x, curr_y, color);
                }
            }
        }
    }

    /// Draw a 1-pixel-wide axis-aligned rectangle outline.
    pub fn draw_rect_outline(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let x1 = x + w - 1;
        let y1 = y + h - 1;
        for xx in x..=x1.min(self.width.saturating_sub(1)) {
            self.draw_pixel(xx, y, color);
            self.draw_pixel(xx, y1, color);
        }
        for yy in y..=y1.min(self.height.saturating_sub(1)) {
            self.draw_pixel(x, yy, color);
            self.draw_pixel(x1, yy, color);
        }
    }

    /// Draw a single 8x8 glyph at (x, y) in the given color.
    /// `bg` colors the OFF pixels (`None` leaves them untouched).
    pub fn draw_glyph(&mut self, x: usize, y: usize, c: char, fg: u32, bg: Option<u32>) {
        use font8x8::UnicodeFonts;
        let glyph = font8x8::BASIC_FONTS
            .get(c)
            .or_else(|| font8x8::BASIC_FONTS.get('?'))
            .unwrap_or([0; 8]);
        for (row, &byte) in glyph.iter().enumerate() {
            for col in 0..8 {
                if (byte >> col) & 1 == 1 {
                    self.draw_pixel(x + col, y + row, fg);
                } else if let Some(bg_color) = bg {
                    self.draw_pixel(x + col, y + row, bg_color);
                }
            }
        }
    }

    /// Draw a string of 8x8 glyphs left-to-right at (x, y).
    /// Returns the x coordinate after the last glyph.
    pub fn draw_text(&mut self, x: usize, y: usize, s: &str, fg: u32, bg: Option<u32>) -> usize {
        let mut cur = x;
        for ch in s.chars() {
            self.draw_glyph(cur, y, ch, fg, bg);
            cur += 8;
        }
        cur
    }

    /// Draw a glyph upscaled by `scale`× with bilinear anti-aliasing, so the
    /// 8×8 bitmap font yields smooth large text (titles/headings) instead of
    /// blocky pixels. Integer fixed-point (×256); no floats → no_std-safe.
    pub fn draw_glyph_scaled(&mut self, x: usize, y: usize, c: char, fg: u32, scale: usize) {
        if scale <= 1 {
            self.draw_glyph(x, y, c, fg, None);
            return;
        }
        use font8x8::UnicodeFonts;
        let glyph = font8x8::BASIC_FONTS
            .get(c)
            .or_else(|| font8x8::BASIC_FONTS.get('?'))
            .unwrap_or([0; 8]);
        let mut cov = [[0u32; 8]; 8];
        for (row, &byte) in glyph.iter().enumerate() {
            for col in 0..8 {
                if (byte >> col) & 1 == 1 {
                    cov[row][col] = 255;
                }
            }
        }
        let rgb = fg & 0x00FF_FFFF;
        let dst = 8 * scale;
        let sample = |cov: &[[u32; 8]; 8], yy: i64, xx: i64| -> u32 {
            if yy < 0 || yy > 7 || xx < 0 || xx > 7 {
                0
            } else {
                cov[yy as usize][xx as usize]
            }
        };
        for dy in 0..dst {
            let sfy = ((dy * 2 + 1) * 256 / (2 * scale)) as i64 - 128;
            let iy = sfy >> 8;
            let fy = (sfy & 255) as u32;
            for dx in 0..dst {
                let sfx = ((dx * 2 + 1) * 256 / (2 * scale)) as i64 - 128;
                let ix = sfx >> 8;
                let fx = (sfx & 255) as u32;
                let a = sample(&cov, iy, ix);
                let b = sample(&cov, iy, ix + 1);
                let cc = sample(&cov, iy + 1, ix);
                let d = sample(&cov, iy + 1, ix + 1);
                let top = a * (256 - fx) + b * fx;
                let bot = cc * (256 - fx) + d * fx;
                let coverage = (top * (256 - fy) + bot * fy) >> 16; // 0..=255
                if coverage > 0 {
                    self.blend_pixel(x + dx, y + dy, (coverage << 24) | rgb);
                }
            }
        }
    }

    /// Draw a string with [`draw_glyph_scaled`]; advances `8 * scale` px per
    /// char. Returns the x after the last glyph.
    pub fn draw_text_scaled(
        &mut self,
        x: usize,
        y: usize,
        s: &str,
        fg: u32,
        scale: usize,
    ) -> usize {
        let mut cur = x;
        let adv = 8 * scale.max(1);
        for ch in s.chars() {
            self.draw_glyph_scaled(cur, y, ch, fg, scale);
            cur += adv;
        }
        cur
    }

    // ── Modern UI primitives (rounded cards, gradients, glass, circles) ──────
    // Pure pixel math, no `sqrt` (no_std-safe), host-KAT'd in `tests` below.
    // These give every Canvas-based surface the soft, layered, glassmorphic look
    // the reference desktops have — instead of flat, hard-edged rectangles.

    /// Read back an ARGB pixel (returns 0 outside bounds). Used by alpha
    /// blending and the host tests.
    #[inline]
    pub fn get_pixel(&self, x: usize, y: usize) -> u32 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        let offset = (y * self.width + x) * self.bytes_per_pixel;
        unsafe {
            let p = self.buffer_ptr.add(offset);
            if self.bytes_per_pixel == 4 {
                *(p as *const u32)
            } else if self.bytes_per_pixel == 3 {
                let b = *p.add(0) as u32;
                let g = *p.add(1) as u32;
                let r = *p.add(2) as u32;
                (r << 16) | (g << 8) | b
            } else {
                0
            }
        }
    }

    /// Alpha-blend `src` (ARGB) onto (x, y) using src-over. Standard alpha:
    /// `a == 0` is a no-op, `a == 0xFF` is an opaque write, in between blends
    /// against the existing pixel. The result is written fully opaque (the
    /// framebuffer has no destination alpha).
    #[inline]
    pub fn blend_pixel(&mut self, x: usize, y: usize, src: u32) {
        let a = (src >> 24) & 0xFF;
        if a == 0 {
            return;
        }
        if a == 0xFF {
            self.draw_pixel(x, y, src);
            return;
        }
        if x >= self.width || y >= self.height {
            return;
        }
        let dst = self.get_pixel(x, y);
        let inv = 255 - a;
        let sr = (src >> 16) & 0xFF;
        let sg = (src >> 8) & 0xFF;
        let sb = src & 0xFF;
        let dr = (dst >> 16) & 0xFF;
        let dg = (dst >> 8) & 0xFF;
        let db = dst & 0xFF;
        let r = (sr * a + dr * inv) / 255;
        let g = (sg * a + dg * inv) / 255;
        let b = (sb * a + db * inv) / 255;
        self.draw_pixel(x, y, 0xFF00_0000 | (r << 16) | (g << 8) | b);
    }

    /// Filled rounded rectangle with anti-aliased corners (2×2 supersampled).
    /// `color` may be translucent (e.g. `0xC8_FFFFFF`) for a glass card — both
    /// the corners and the translucency blend against whatever is underneath.
    /// A zero alpha byte (`0x00RRGGBB`) is treated as opaque (legacy colors).
    pub fn fill_rounded_rect(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        radius: usize,
        color: u32,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let r = radius.min(w / 2).min(h / 2);
        let base_a = {
            let a = (color >> 24) & 0xFF;
            if a == 0 {
                255
            } else {
                a
            }
        };
        let rgb = color & 0x00FF_FFFF;
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for cy in y..y_end {
            for cx in x..x_end {
                let cov = rr_coverage(cx, cy, x, y, w, h, r);
                if cov == 0 {
                    continue;
                }
                if cov == 4 && base_a == 255 {
                    self.draw_pixel(cx, cy, 0xFF00_0000 | rgb);
                } else {
                    let eff = base_a * cov as u32 / 4;
                    self.blend_pixel(cx, cy, (eff << 24) | rgb);
                }
            }
        }
    }

    /// Soft drop shadow for a rounded rectangle — a macOS-grade *ambient* shadow.
    ///
    /// Replaces the "single offset silhouette" look (one translucent rect nudged
    /// down-right), which reads as a hard 90s bevel ledge. This computes a smooth
    /// feathered falloff in ONE pass via a rounded-rect signed-distance field:
    /// each pixel's opacity ramps quadratically from `peak` at the card edge to 0
    /// at `blur` px out. Integer-only (octagonal distance approx — no sqrt/float,
    /// so it's `no_std`-safe and cheap).
    ///
    /// Draw this BEFORE the card, passing the CARD's own `(x,y,w,h,radius)`.
    /// `rgb` must be a NEUTRAL dark (a saturated hue reads as a *colored* shadow —
    /// the wrong look); `dy` shifts the cast downward for a light-from-above feel.
    /// The card's solid interior is skipped (it's overdrawn by the opaque card),
    /// so only the visible fringe + corners are painted.
    pub fn fill_rounded_rect_shadow(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        radius: usize,
        rgb: u32,
        blur: usize,
        dy: i32,
    ) {
        if w == 0 || h == 0 || blur == 0 {
            return;
        }
        let rgb = rgb & 0x00FF_FFFF;
        // Peak opacity (/255) at the card edge; the quadratic falloff makes most
        // of the spread far lighter, so this reads as a soft ~25% ambient shadow.
        let peak: i32 = 64;
        let r = radius.min(w / 2).min(h / 2) as i32;
        let blur = blur as i32;
        let half_x = (w / 2) as i32;
        let half_y = (h / 2) as i32;
        let cxc = x as i32 + half_x;
        let cyc = y as i32 + half_y + dy; // vertical center, cast down by `dy`
        let bx0 = (cxc - half_x - blur).max(0);
        let by0 = (cyc - half_y - blur).max(0);
        let bx1 = (cxc + half_x + blur).min(self.width as i32);
        let by1 = (cyc + half_y + blur).min(self.height as i32);
        // Solid card interior (always overdrawn by the opaque card) — skip for speed.
        let inx0 = x as i32 + r;
        let inx1 = (x + w) as i32 - r;
        let iny0 = y as i32 + dy + r;
        let iny1 = (y + h) as i32 + dy - r;
        let inner = (half_x - r, half_y - r);
        for cy in by0..by1 {
            for cx in bx0..bx1 {
                if cx >= inx0 && cx < inx1 && cy >= iny0 && cy < iny1 {
                    continue;
                }
                // Distance from this pixel to the card's rounded edge (octagonal
                // approximation of Euclidean: max + min/2, minus the corner radius).
                let ax = ((cx - cxc).abs() - inner.0).max(0);
                let ay = ((cy - cyc).abs() - inner.1).max(0);
                let (mx, mn) = if ax >= ay { (ax, ay) } else { (ay, ax) };
                let dist = mx + mn / 2 - r;
                if dist >= blur {
                    continue;
                }
                let t = blur - dist.max(0); // blur (at edge) .. down toward 0
                let a = peak * t * t / (blur * blur); // quadratic feather
                if a <= 0 {
                    continue;
                }
                self.blend_pixel(cx as usize, cy as usize, ((a as u32) << 24) | rgb);
            }
        }
    }

    /// Anti-aliased 1px rounded-rectangle outline — the hairline border that
    /// gives glass cards their crisp edge.
    pub fn draw_rounded_rect_outline(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        radius: usize,
        color: u32,
    ) {
        if w < 2 || h < 2 {
            return;
        }
        let r = radius.min(w / 2).min(h / 2);
        let base_a = {
            let a = (color >> 24) & 0xFF;
            if a == 0 {
                255
            } else {
                a
            }
        };
        let rgb = color & 0x00FF_FFFF;
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for cy in y..y_end {
            for cx in x..x_end {
                let outer = rr_coverage(cx, cy, x, y, w, h, r);
                if outer == 0 {
                    continue;
                }
                let inner = rr_coverage(
                    cx,
                    cy,
                    x + 1,
                    y + 1,
                    w.saturating_sub(2),
                    h.saturating_sub(2),
                    r.saturating_sub(1),
                );
                let edge_cov = outer.saturating_sub(inner);
                if edge_cov == 0 {
                    continue;
                }
                let eff = base_a * edge_cov as u32 / 4;
                self.blend_pixel(cx, cy, (eff << 24) | rgb);
            }
        }
    }

    /// Vertical (top→bottom) linear gradient fill. The reference desktops use
    /// these for window/panel backgrounds and primary buttons.
    pub fn fill_rect_gradient(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        top: u32,
        bottom: u32,
    ) {
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        if x >= x_end || y >= y_end {
            return;
        }
        let tr = ((top >> 16) & 0xFF) as i64;
        let tg = ((top >> 8) & 0xFF) as i64;
        let tb = (top & 0xFF) as i64;
        let br = ((bottom >> 16) & 0xFF) as i64;
        let bg = ((bottom >> 8) & 0xFF) as i64;
        let bb = (bottom & 0xFF) as i64;
        let denom = if h > 1 { (h - 1) as i64 } else { 1 };
        for cy in y..y_end {
            let t = ((cy - y) as i64).min(denom);
            let r = (tr + (br - tr) * t / denom) as u32;
            let g = (tg + (bg - tg) * t / denom) as u32;
            let b = (tb + (bb - tb) * t / denom) as u32;
            let col = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            for cx in x..x_end {
                self.draw_pixel(cx, cy, col);
            }
        }
    }

    /// Filled circle with an anti-aliased edge (icons, avatars, status dots).
    pub fn fill_circle(&mut self, cx: usize, cy: usize, r: usize, color: u32) {
        if r == 0 {
            return;
        }
        let base_a = {
            let a = (color >> 24) & 0xFF;
            if a == 0 {
                255
            } else {
                a
            }
        };
        let rgb = color & 0x00FF_FFFF;
        let x0 = cx.saturating_sub(r);
        let y0 = cy.saturating_sub(r);
        let x1 = (cx + r + 1).min(self.width);
        let y1 = (cy + r + 1).min(self.height);
        for py in y0..y1 {
            for px in x0..x1 {
                let cov = circle_coverage(px, py, cx, cy, r);
                if cov == 0 {
                    continue;
                }
                if cov == 4 && base_a == 255 {
                    self.draw_pixel(px, py, 0xFF00_0000 | rgb);
                } else {
                    let eff = base_a * cov as u32 / 4;
                    self.blend_pixel(px, py, (eff << 24) | rgb);
                }
            }
        }
    }

    /// Vertex for the software 3D rasterizer.
    ///
    /// Screen-space 2D + per-vertex ARGB color. A real 3D vertex would also
    /// carry a Z value for depth and a `w` for perspective division, but the
    /// canonical "hello triangle" demo only needs interpolated color.
    pub fn _vertex_phantom() {}

    /// Rasterize a colored triangle into the canvas using barycentric
    /// interpolation. Pure integer math (i64 edge functions); no floats.
    ///
    /// `v0`, `v1`, `v2` give screen-space (x, y) + ARGB color. The triangle's
    /// interior is filled with linearly-interpolated color across the three
    /// vertices — the classic "Vulkan triangle" output, every graphics-API
    /// hello-world ever shipped.
    pub fn draw_triangle(
        &mut self,
        (x0, y0, c0): (i32, i32, u32),
        (x1, y1, c1): (i32, i32, u32),
        (x2, y2, c2): (i32, i32, u32),
    ) {
        // Bounding box, clipped to canvas.
        let min_x = x0.min(x1).min(x2).max(0);
        let min_y = y0.min(y1).min(y2).max(0);
        let max_x = x0.max(x1).max(x2).min(self.width as i32 - 1);
        let max_y = y0.max(y1).max(y2).min(self.height as i32 - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }

        // Twice the signed area of triangle (v0, v1, v2). Sign tells us the
        // winding order; we use it to decide which way "inside" points.
        let area = edge(x0, y0, x1, y1, x2, y2);
        if area == 0 {
            return;
        } // degenerate

        // Unpack vertex colors once.
        let (a0, r0, g0, b0) = unpack_argb(c0);
        let (a1, r1, g1, b1) = unpack_argb(c1);
        let (a2, r2, g2, b2) = unpack_argb(c2);

        // Walk the bounding box; for each pixel compute the three edge
        // functions. If all three have the same sign as `area`, the pixel
        // is inside. Then interpolate color by the barycentric weights.
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let w0 = edge(x1, y1, x2, y2, px, py);
                let w1 = edge(x2, y2, x0, y0, px, py);
                let w2 = edge(x0, y0, x1, y1, px, py);
                let inside = if area > 0 {
                    w0 >= 0 && w1 >= 0 && w2 >= 0
                } else {
                    w0 <= 0 && w1 <= 0 && w2 <= 0
                };
                if !inside {
                    continue;
                }

                // u + v + w = 1 in floating point; we keep the un-normalised
                // weights and divide once at the end (per channel) to avoid
                // a per-pixel float divide.
                let a = ((w0 * a0 as i64 + w1 * a1 as i64 + w2 * a2 as i64) / area) as u8;
                let r = ((w0 * r0 as i64 + w1 * r1 as i64 + w2 * r2 as i64) / area) as u8;
                let g = ((w0 * g0 as i64 + w1 * g1 as i64 + w2 * g2 as i64) / area) as u8;
                let b = ((w0 * b0 as i64 + w1 * b1 as i64 + w2 * b2 as i64) / area) as u8;
                let pixel =
                    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                self.draw_pixel(px as usize, py as usize, pixel);
            }
        }
    }

    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;

        let mut x = x0;
        let mut y = y0;

        loop {
            if x >= 0 && x < self.width as i32 && y >= 0 && y < self.height as i32 {
                self.draw_pixel(x as usize, y as usize, color);
            }

            if x == x1 && y == y1 {
                break;
            }

            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draw a crisp anti-aliased line icon (`crate::icon::Icon`) at pixel
    /// `(x, y)`, occupying a `size × size` box, tinted `color` (ARGB; alpha =
    /// overall opacity). This is the real glyph the shell tiles use instead of
    /// the letter placeholders flagged by visual-QA — vector line art, NOT a
    /// bitmap and NOT an 8x8 letter. Scales crisply to any tile/hero size.
    #[inline]
    pub fn draw_icon(&mut self, icon: crate::icon::Icon, x: i32, y: i32, size: i32, color: u32) {
        crate::icon::draw_icon(self, icon, x, y, size, color);
    }
}

// ── Helpers for the triangle rasterizer ──────────────────────────────────

/// Edge function: 2× signed area of triangle (a, b, c). Sign of the result
/// tells us which side of edge (a, b) the point c lies on.
#[inline]
fn edge(ax: i32, ay: i32, bx: i32, by: i32, cx: i32, cy: i32) -> i64 {
    (bx as i64 - ax as i64) * (cy as i64 - ay as i64)
        - (by as i64 - ay as i64) * (cx as i64 - ax as i64)
}

#[inline]
fn unpack_argb(c: u32) -> (u8, u8, u8, u8) {
    (
        ((c >> 24) & 0xff) as u8,
        ((c >> 16) & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        (c & 0xff) as u8,
    )
}

// ── Coverage helpers for the modern UI primitives ────────────────────────
// Rounded-rect signed-distance + circle tests, 2×2 supersampled (integers
// scaled ×4), so corners/edges are anti-aliased WITHOUT a `sqrt` (no_std-safe).
// Return the number of sub-samples inside (0..=4) -> the pixel's coverage.

/// Rounded-rectangle coverage of pixel (cx, cy) for the rect [x, x+w) ×
/// [y, y+h) with corner radius `r`. Uses the canonical rounded-box SDF
/// `length(max(|p - center| - (half_extent - r), 0)) <= r`.
#[inline]
fn rr_coverage(cx: usize, cy: usize, x: usize, y: usize, w: usize, h: usize, r: usize) -> u8 {
    if r == 0 {
        return 4;
    }
    let center4_x = 4 * x as i64 + 2 * w as i64;
    let center4_y = 4 * y as i64 + 2 * h as i64;
    let ehw = 2 * w as i64 - 4 * r as i64; // (w/2 - r) × 4
    let ehh = 2 * h as i64 - 4 * r as i64;
    let rr = (4 * r as i64) * (4 * r as i64);
    let mut cnt = 0u8;
    for &sy in &[1i64, 3] {
        for &sx in &[1i64, 3] {
            let px = 4 * cx as i64 + sx;
            let py = 4 * cy as i64 + sy;
            let dx = (px - center4_x).abs() - ehw;
            let dy = (py - center4_y).abs() - ehh;
            let ddx = if dx > 0 { dx } else { 0 };
            let ddy = if dy > 0 { dy } else { 0 };
            if ddx * ddx + ddy * ddy <= rr {
                cnt += 1;
            }
        }
    }
    cnt
}

/// Circle coverage of pixel (px, py) for the circle centered on pixel
/// (cx, cy) with radius `r`. 2×2 supersampled like [`rr_coverage`].
#[inline]
fn circle_coverage(px: usize, py: usize, cx: usize, cy: usize, r: usize) -> u8 {
    let center4_x = 4 * cx as i64 + 2;
    let center4_y = 4 * cy as i64 + 2;
    let rr = (4 * r as i64) * (4 * r as i64);
    let mut cnt = 0u8;
    for &sy in &[1i64, 3] {
        for &sx in &[1i64, 3] {
            let x4 = 4 * px as i64 + sx;
            let y4 = 4 * py as i64 + sy;
            let dx = x4 - center4_x;
            let dy = y4 - center4_y;
            if dx * dx + dy * dy <= rr {
                cnt += 1;
            }
        }
    }
    cnt
}

// ═══════════════════════════════════════════════════════════════════════════
// R10 Artifacts
// ═══════════════════════════════════════════════════════════════════════════

static mut RESOURCE_MANAGER: Option<ResourceManager> = None;

/// Initialize the global graphics resource manager.
pub fn init() {
    unsafe {
        RESOURCE_MANAGER = Some(ResourceManager::new());
    }
}

pub fn resource_manager() -> &'static mut ResourceManager {
    unsafe {
        #[allow(static_mut_refs)]
        RESOURCE_MANAGER.as_mut().expect("AthGFX not initialized")
    }
}

/// Prove behavioral correctness of the software rasterizer and resource manager.
pub fn run_boot_smoketest() -> bool {
    // 1. Test Canvas (Software Rasterizer)
    let mut pixels = [0u32; 100 * 100];
    let mut canvas = unsafe { Canvas::new(pixels.as_mut_ptr() as *mut u8, 100, 100, 4) };
    canvas.clear(0xFF_FF_00_00); // Red
    canvas.fill_rect(10, 10, 20, 20, 0xFF_00_FF_00); // Green rect

    let clear_ok = pixels[0] == 0xFF_FF_00_00;
    let rect_ok = pixels[15 * 100 + 15] == 0xFF_00_FF_00;

    // 2. Test Resource Manager
    let mut rm = ResourceManager::new();
    let buf_res = rm.create_buffer(&BufferDescriptor {
        size: 1024,
        usage: BufferUsage::Vertex,
        label: None,
    });

    let rm_ok = buf_res.is_ok();

    clear_ok && rect_ok && rm_ok
}

// ═══════════════════════════════════════════════════════════════════════════
// AthGFX Pipeline API — Vulkan-level capabilities, friendlier surface
// ═══════════════════════════════════════════════════════════════════════════

// ── Handles ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextureHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShaderHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PipelineHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RenderPassHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FenceHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemaphoreHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FramebufferHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DescriptorSetHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DescriptorSetLayoutHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PipelineLayoutHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SamplerHandle(pub u64);

// ── Enums ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8Unorm,
    Bgra8Unorm,
    Rgba8Srgb,
    Bgra8Srgb,
    Rgba16Float,
    Rgba32Float,
    Rg11B10Float,
    Depth24Stencil8,
    Depth32Float,
    R8Unorm,
    Rg8Unorm,
    Bc1Unorm,
    Bc3Unorm,
    Bc7Unorm,
}

impl PixelFormat {
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            Self::R8Unorm => 1,
            Self::Rg8Unorm => 2,
            Self::Rgba8Unorm
            | Self::Bgra8Unorm
            | Self::Rgba8Srgb
            | Self::Bgra8Srgb
            | Self::Depth24Stencil8
            | Self::Depth32Float
            | Self::Rg11B10Float => 4,
            Self::Rgba16Float => 8,
            Self::Rgba32Float => 16,
            Self::Bc1Unorm => 1, // compressed
            Self::Bc3Unorm | Self::Bc7Unorm => 1,
        }
    }

    pub fn is_depth(&self) -> bool {
        matches!(self, Self::Depth24Stencil8 | Self::Depth32Float)
    }

    pub fn is_srgb(&self) -> bool {
        matches!(self, Self::Rgba8Srgb | Self::Bgra8Srgb)
    }

    pub fn is_hdr_capable(&self) -> bool {
        matches!(
            self,
            Self::Rgba16Float | Self::Rgba32Float | Self::Rg11B10Float
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
    Geometry,
    TessControl,
    TessEvaluation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTopology {
    PointList,
    LineList,
    LineStrip,
    TriangleList,
    TriangleStrip,
    TriangleFan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullMode {
    None,
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontFace {
    Clockwise,
    CounterClockwise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolygonMode {
    Fill,
    Line,
    Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendOp {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Never,
    Less,
    Equal,
    LessOrEqual,
    Greater,
    NotEqual,
    GreaterOrEqual,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Nearest,
    Linear,
    Anisotropic(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressMode {
    Repeat,
    MirroredRepeat,
    ClampToEdge,
    ClampToBorder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferUsage {
    Vertex,
    Index,
    Uniform,
    Storage,
    Indirect,
    Transfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureUsage {
    Sampled,
    Storage,
    RenderTarget,
    DepthStencil,
    TransferSrc,
    TransferDst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOp {
    Load,
    Clear,
    DontCare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    Store,
    DontCare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    U16,
    U32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    Float,
    Float2,
    Float3,
    Float4,
    Int,
    Int2,
    Int3,
    Int4,
    UByte4Norm,
}

impl VertexFormat {
    pub fn size(&self) -> usize {
        match self {
            Self::Float | Self::Int => 4,
            Self::Float2 | Self::Int2 => 8,
            Self::Float3 | Self::Int3 => 12,
            Self::Float4 | Self::Int4 | Self::UByte4Norm => 16,
        }
    }
}

// ── Descriptors ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VertexAttribute {
    pub location: u32,
    pub format: VertexFormat,
    pub offset: u32,
}

#[derive(Debug, Clone)]
pub struct VertexBufferLayout {
    pub stride: u32,
    pub step_rate: u32,
    pub attributes: Vec<VertexAttribute>,
}

#[derive(Debug, Clone, Copy)]
pub struct BlendState {
    pub enabled: bool,
    pub src_factor: BlendFactor,
    pub dst_factor: BlendFactor,
    pub op: BlendOp,
    pub src_alpha_factor: BlendFactor,
    pub dst_alpha_factor: BlendFactor,
    pub alpha_op: BlendOp,
}

impl BlendState {
    pub const OPAQUE: Self = Self {
        enabled: false,
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::Zero,
        op: BlendOp::Add,
        src_alpha_factor: BlendFactor::One,
        dst_alpha_factor: BlendFactor::Zero,
        alpha_op: BlendOp::Add,
    };

    pub const ALPHA_BLEND: Self = Self {
        enabled: true,
        src_factor: BlendFactor::SrcAlpha,
        dst_factor: BlendFactor::OneMinusSrcAlpha,
        op: BlendOp::Add,
        src_alpha_factor: BlendFactor::One,
        dst_alpha_factor: BlendFactor::OneMinusSrcAlpha,
        alpha_op: BlendOp::Add,
    };

    pub const ADDITIVE: Self = Self {
        enabled: true,
        src_factor: BlendFactor::SrcAlpha,
        dst_factor: BlendFactor::One,
        op: BlendOp::Add,
        src_alpha_factor: BlendFactor::One,
        dst_alpha_factor: BlendFactor::One,
        alpha_op: BlendOp::Add,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct DepthStencilState {
    pub depth_test: bool,
    pub depth_write: bool,
    pub depth_compare: CompareOp,
    pub stencil_enabled: bool,
}

impl DepthStencilState {
    pub const DISABLED: Self = Self {
        depth_test: false,
        depth_write: false,
        depth_compare: CompareOp::Always,
        stencil_enabled: false,
    };

    pub const DEPTH_READ_WRITE: Self = Self {
        depth_test: true,
        depth_write: true,
        depth_compare: CompareOp::Less,
        stencil_enabled: false,
    };

    pub const DEPTH_READ_ONLY: Self = Self {
        depth_test: true,
        depth_write: false,
        depth_compare: CompareOp::LessOrEqual,
        stencil_enabled: false,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct RasterState {
    pub cull_mode: CullMode,
    pub front_face: FrontFace,
    pub polygon_mode: PolygonMode,
    pub depth_bias: f32,
    pub depth_bias_slope: f32,
    pub line_width: f32,
}

impl Default for RasterState {
    fn default() -> Self {
        Self {
            cull_mode: CullMode::Back,
            front_face: FrontFace::CounterClockwise,
            polygon_mode: PolygonMode::Fill,
            depth_bias: 0.0,
            depth_bias_slope: 0.0,
            line_width: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SamplerDescriptor {
    pub min_filter: FilterMode,
    pub mag_filter: FilterMode,
    pub mipmap_filter: FilterMode,
    pub address_u: AddressMode,
    pub address_v: AddressMode,
    pub address_w: AddressMode,
    pub max_anisotropy: u8,
    pub compare: Option<CompareOp>,
    pub lod_min: f32,
    pub lod_max: f32,
}

impl Default for SamplerDescriptor {
    fn default() -> Self {
        Self {
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Linear,
            address_u: AddressMode::Repeat,
            address_v: AddressMode::Repeat,
            address_w: AddressMode::Repeat,
            max_anisotropy: 1,
            compare: None,
            lod_min: 0.0,
            lod_max: 1000.0,
        }
    }
}

// ── Resource creation descriptors ────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BufferDescriptor {
    pub size: u64,
    pub usage: BufferUsage,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TextureDescriptor {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mip_levels: u32,
    pub format: PixelFormat,
    pub usage: TextureUsage,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ShaderDescriptor {
    pub stage: ShaderStage,
    pub entry_point: String,
    pub spirv: Vec<u8>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ColorAttachment {
    pub texture: TextureHandle,
    pub load_op: LoadOp,
    pub store_op: StoreOp,
    pub clear_color: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct DepthAttachment {
    pub texture: TextureHandle,
    pub load_op: LoadOp,
    pub store_op: StoreOp,
    pub clear_depth: f32,
    pub clear_stencil: u8,
}

#[derive(Debug, Clone)]
pub struct RenderPassDescriptor {
    pub color_attachments: Vec<ColorAttachment>,
    pub depth_attachment: Option<DepthAttachment>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GraphicsPipelineDescriptor {
    pub vertex_shader: ShaderHandle,
    pub fragment_shader: ShaderHandle,
    pub vertex_layouts: Vec<VertexBufferLayout>,
    pub topology: PrimitiveTopology,
    pub raster: RasterState,
    pub blend: BlendState,
    pub depth_stencil: DepthStencilState,
    pub color_formats: Vec<PixelFormat>,
    pub depth_format: Option<PixelFormat>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ComputePipelineDescriptor {
    pub shader: ShaderHandle,
    pub label: Option<String>,
}

// ── Image layout & synchronization primitives ────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageLayout {
    Undefined,
    General,
    ColorAttachment,
    DepthStencilAttachment,
    ShaderReadOnly,
    TransferSrc,
    TransferDst,
    PresentSrc,
}

#[derive(Debug, Clone, Copy)]
pub enum ClearValue {
    Color([f32; 4]),
    DepthStencil { depth: f32, stencil: u8 },
}

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Scissor {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage {
    TopOfPipe,
    VertexInput,
    VertexShader,
    FragmentShader,
    EarlyFragmentTests,
    LateFragmentTests,
    ColorAttachmentOutput,
    ComputeShader,
    Transfer,
    BottomOfPipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessFlags {
    None,
    VertexBufferRead,
    IndexBufferRead,
    UniformBufferRead,
    ShaderRead,
    ShaderWrite,
    ColorAttachmentRead,
    ColorAttachmentWrite,
    DepthStencilRead,
    DepthStencilWrite,
    TransferRead,
    TransferWrite,
}

#[derive(Debug, Clone)]
pub struct ImageBarrier {
    pub image: TextureHandle,
    pub old_layout: ImageLayout,
    pub new_layout: ImageLayout,
    pub src_access: AccessFlags,
    pub dst_access: AccessFlags,
}

#[derive(Debug, Clone)]
pub struct BarrierInfo {
    pub src_stage: PipelineStage,
    pub dst_stage: PipelineStage,
    pub image_barriers: Vec<ImageBarrier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandBufferState {
    Initial,
    Recording,
    Executable,
}

// ── Draw commands ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DrawCommand {
    Draw {
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    },
    DrawIndexed {
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    },
    Dispatch {
        x: u32,
        y: u32,
        z: u32,
    },
    SetViewport {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    },
    SetScissor {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    BindVertexBuffer {
        slot: u32,
        buffer: BufferHandle,
        offset: u64,
    },
    BindIndexBuffer {
        buffer: BufferHandle,
        offset: u64,
        index_type: IndexType,
    },
    BindPipeline(PipelineHandle),
    PushConstants {
        offset: u32,
        data: Vec<u8>,
    },
    CopyBufferToTexture {
        src: BufferHandle,
        dst: TextureHandle,
        width: u32,
        height: u32,
    },
    BeginRenderPass {
        render_pass: RenderPassHandle,
        framebuffer: FramebufferHandle,
        clear_values: Vec<ClearValue>,
    },
    EndRenderPass,
    PipelineBarrier(BarrierInfo),
    CopyBuffer {
        src: BufferHandle,
        dst: BufferHandle,
        src_offset: u64,
        dst_offset: u64,
        size: u64,
    },
    CopyImageToBuffer {
        src: TextureHandle,
        dst: BufferHandle,
    },
    BindDescriptorSet {
        set_index: u32,
        descriptor_set: DescriptorSetHandle,
    },
}

// ── Command buffer ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CommandBuffer {
    pub commands: Vec<DrawCommand>,
    pub label: Option<String>,
    pub state: CommandBufferState,
}

impl CommandBuffer {
    pub fn new(label: Option<&str>) -> Self {
        Self {
            commands: Vec::new(),
            label: label.map(String::from),
            state: CommandBufferState::Initial,
        }
    }

    pub fn begin(&mut self) -> Result<(), GfxError> {
        match self.state {
            CommandBufferState::Initial | CommandBufferState::Executable => {
                self.commands.clear();
                self.state = CommandBufferState::Recording;
                Ok(())
            }
            CommandBufferState::Recording => Err(GfxError::NotSupported),
        }
    }

    pub fn end(&mut self) -> Result<(), GfxError> {
        if self.state != CommandBufferState::Recording {
            return Err(GfxError::NotSupported);
        }
        self.state = CommandBufferState::Executable;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.commands.clear();
        self.state = CommandBufferState::Initial;
    }

    pub fn is_recording(&self) -> bool {
        self.state == CommandBufferState::Recording
    }

    pub fn bind_pipeline(&mut self, pipeline: PipelineHandle) {
        self.commands.push(DrawCommand::BindPipeline(pipeline));
    }

    pub fn bind_vertex_buffer(&mut self, slot: u32, buffer: BufferHandle, offset: u64) {
        self.commands.push(DrawCommand::BindVertexBuffer {
            slot,
            buffer,
            offset,
        });
    }

    pub fn bind_index_buffer(&mut self, buffer: BufferHandle, offset: u64, index_type: IndexType) {
        self.commands.push(DrawCommand::BindIndexBuffer {
            buffer,
            offset,
            index_type,
        });
    }

    pub fn set_viewport(&mut self, x: f32, y: f32, w: f32, h: f32, min_d: f32, max_d: f32) {
        self.commands.push(DrawCommand::SetViewport {
            x,
            y,
            width: w,
            height: h,
            min_depth: min_d,
            max_depth: max_d,
        });
    }

    pub fn set_scissor(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.commands.push(DrawCommand::SetScissor {
            x,
            y,
            width: w,
            height: h,
        });
    }

    pub fn draw(&mut self, vertex_count: u32, instance_count: u32) {
        self.commands.push(DrawCommand::Draw {
            vertex_count,
            instance_count,
            first_vertex: 0,
            first_instance: 0,
        });
    }

    pub fn draw_indexed(&mut self, index_count: u32, instance_count: u32) {
        self.commands.push(DrawCommand::DrawIndexed {
            index_count,
            instance_count,
            first_index: 0,
            vertex_offset: 0,
            first_instance: 0,
        });
    }

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        self.commands.push(DrawCommand::Dispatch { x, y, z });
    }

    pub fn push_constants(&mut self, offset: u32, data: &[u8]) {
        self.commands.push(DrawCommand::PushConstants {
            offset,
            data: Vec::from(data),
        });
    }

    pub fn command_count(&self) -> usize {
        self.commands.len()
    }

    pub fn begin_render_pass(
        &mut self,
        render_pass: RenderPassHandle,
        framebuffer: FramebufferHandle,
        clear_values: Vec<ClearValue>,
    ) {
        self.commands.push(DrawCommand::BeginRenderPass {
            render_pass,
            framebuffer,
            clear_values,
        });
    }

    pub fn end_render_pass(&mut self) {
        self.commands.push(DrawCommand::EndRenderPass);
    }

    pub fn pipeline_barrier(&mut self, barrier: BarrierInfo) {
        self.commands.push(DrawCommand::PipelineBarrier(barrier));
    }

    pub fn copy_buffer(
        &mut self,
        src: BufferHandle,
        dst: BufferHandle,
        src_offset: u64,
        dst_offset: u64,
        size: u64,
    ) {
        self.commands.push(DrawCommand::CopyBuffer {
            src,
            dst,
            src_offset,
            dst_offset,
            size,
        });
    }

    pub fn copy_buffer_to_texture(
        &mut self,
        src: BufferHandle,
        dst: TextureHandle,
        width: u32,
        height: u32,
    ) {
        self.commands.push(DrawCommand::CopyBufferToTexture {
            src,
            dst,
            width,
            height,
        });
    }

    pub fn bind_descriptor_set(&mut self, set_index: u32, descriptor_set: DescriptorSetHandle) {
        self.commands.push(DrawCommand::BindDescriptorSet {
            set_index,
            descriptor_set,
        });
    }
}

// ── HDR / VRR / Display ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrMode {
    Sdr,
    Hdr10,
    HdrScRgb,
    DolbyVision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrrMode {
    Off,
    AdaptiveSync,
    GSync,
    FreeSync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentMode {
    Immediate,
    Fifo,
    FifoRelaxed,
    Mailbox,
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayConfig {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub format: PixelFormat,
    pub hdr: HdrMode,
    pub vrr: VrrMode,
    pub present_mode: PresentMode,
    pub exclusive_fullscreen: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            refresh_hz: 60,
            format: PixelFormat::Bgra8Srgb,
            hdr: HdrMode::Sdr,
            vrr: VrrMode::Off,
            present_mode: PresentMode::Fifo,
            exclusive_fullscreen: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwapchainInfo {
    pub config: DisplayConfig,
    pub image_count: u32,
    pub current_image: u32,
}

// ── Shader cache ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CachedShader {
    pub hash: [u8; 32],
    pub spirv: Vec<u8>,
    pub native_binary: Vec<u8>,
    pub stage: ShaderStage,
    pub entry_point: String,
    pub hits: u64,
    pub last_used: u64,
}

pub struct ShaderCache {
    entries: BTreeMap<u64, CachedShader>,
    max_entries: usize,
    max_bytes: u64,
    total_bytes: u64,
    hits: u64,
    misses: u64,
}

impl ShaderCache {
    pub fn new(max_entries: usize, max_bytes: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            max_bytes,
            total_bytes: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn lookup(&mut self, hash_key: u64) -> Option<&CachedShader> {
        if let Some(entry) = self.entries.get_mut(&hash_key) {
            entry.hits += 1;
            self.hits += 1;
            Some(entry)
        } else {
            self.misses += 1;
            None
        }
    }

    pub fn insert(&mut self, hash_key: u64, shader: CachedShader) {
        let size = shader.spirv.len() as u64 + shader.native_binary.len() as u64;
        while self.entries.len() >= self.max_entries || self.total_bytes + size > self.max_bytes {
            if !self.evict_lru() {
                break;
            }
        }
        self.total_bytes += size;
        self.entries.insert(hash_key, shader);
    }

    fn evict_lru(&mut self) -> bool {
        let lru_key = self
            .entries
            .iter()
            .min_by_key(|(_, v)| v.last_used)
            .map(|(k, _)| *k);
        if let Some(key) = lru_key {
            if let Some(removed) = self.entries.remove(&key) {
                self.total_bytes = self.total_bytes.saturating_sub(
                    removed.spirv.len() as u64 + removed.native_binary.len() as u64,
                );
            }
            true
        } else {
            false
        }
    }

    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f32 / total as f32
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn total_size(&self) -> u64 {
        self.total_bytes
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }
}

// ── GPU device trait ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GfxError {
    OutOfMemory,
    DeviceLost,
    InvalidHandle,
    UnsupportedFormat,
    ShaderCompilationFailed,
    PipelineCreationFailed,
    SwapchainOutOfDate,
    Timeout,
    NotSupported,
}

pub trait GfxDevice {
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferHandle, GfxError>;
    fn destroy_buffer(&mut self, handle: BufferHandle);
    fn write_buffer(
        &mut self,
        handle: BufferHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<(), GfxError>;

    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureHandle, GfxError>;
    fn destroy_texture(&mut self, handle: TextureHandle);

    fn create_shader(&mut self, desc: &ShaderDescriptor) -> Result<ShaderHandle, GfxError>;
    fn destroy_shader(&mut self, handle: ShaderHandle);

    fn create_graphics_pipeline(
        &mut self,
        desc: &GraphicsPipelineDescriptor,
    ) -> Result<PipelineHandle, GfxError>;
    fn create_compute_pipeline(
        &mut self,
        desc: &ComputePipelineDescriptor,
    ) -> Result<PipelineHandle, GfxError>;
    fn destroy_pipeline(&mut self, handle: PipelineHandle);

    fn create_fence(&mut self) -> Result<FenceHandle, GfxError>;
    fn wait_fence(&self, fence: FenceHandle, timeout_ns: u64) -> Result<(), GfxError>;
    fn reset_fence(&mut self, fence: FenceHandle);
    fn destroy_fence(&mut self, fence: FenceHandle);

    fn submit(
        &mut self,
        commands: &CommandBuffer,
        signal_fence: Option<FenceHandle>,
    ) -> Result<(), GfxError>;

    fn configure_display(&mut self, config: &DisplayConfig) -> Result<SwapchainInfo, GfxError>;
    fn acquire_next_image(&mut self) -> Result<(u32, TextureHandle), GfxError>;
    fn present(&mut self) -> Result<(), GfxError>;
}

// ── Per-game GPU profile ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GpuProfile {
    pub game_id: u64,
    pub display: DisplayConfig,
    pub power_limit_percent: u8,
    pub fan_curve_override: bool,
    pub shader_cache_enabled: bool,
    pub anisotropic_filtering: u8,
    pub vsync: bool,
    pub frame_limiter: Option<u32>,
}

impl GpuProfile {
    pub fn default_for_game(game_id: u64) -> Self {
        Self {
            game_id,
            display: DisplayConfig::default(),
            power_limit_percent: 100,
            fan_curve_override: false,
            shader_cache_enabled: true,
            anisotropic_filtering: 16,
            vsync: true,
            frame_limiter: None,
        }
    }

    pub fn competitive(game_id: u64) -> Self {
        Self {
            game_id,
            display: DisplayConfig {
                present_mode: PresentMode::Immediate,
                vrr: VrrMode::Off,
                exclusive_fullscreen: true,
                ..DisplayConfig::default()
            },
            power_limit_percent: 115,
            fan_curve_override: true,
            shader_cache_enabled: true,
            anisotropic_filtering: 4,
            vsync: false,
            frame_limiter: None,
        }
    }
}

// ── Frame timing / perf counters ─────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct FrameTiming {
    pub frame_number: u64,
    pub cpu_time_us: u64,
    pub gpu_time_us: u64,
    pub present_time_us: u64,
    pub total_time_us: u64,
    pub draw_calls: u32,
    pub triangles: u64,
    pub buffer_uploads_bytes: u64,
}

impl FrameTiming {
    pub fn fps(&self) -> f32 {
        if self.total_time_us == 0 {
            return 0.0;
        }
        1_000_000.0 / self.total_time_us as f32
    }

    pub fn frametime_ms(&self) -> f32 {
        self.total_time_us as f32 / 1000.0
    }

    pub fn gpu_bound(&self) -> bool {
        self.gpu_time_us > self.cpu_time_us
    }
}

pub struct FrameTimingHistory {
    pub timings: Vec<FrameTiming>,
    pub max_entries: usize,
}

impl FrameTimingHistory {
    pub fn new(max_entries: usize) -> Self {
        Self {
            timings: Vec::new(),
            max_entries,
        }
    }

    pub fn record(&mut self, timing: FrameTiming) {
        if self.timings.len() >= self.max_entries {
            self.timings.remove(0);
        }
        self.timings.push(timing);
    }

    pub fn avg_fps(&self) -> f32 {
        if self.timings.is_empty() {
            return 0.0;
        }
        let total_time: u64 = self.timings.iter().map(|t| t.total_time_us).sum();
        let count = self.timings.len() as f32;
        count / (total_time as f32 / 1_000_000.0)
    }

    pub fn percentile_frametime_ms(&self, pct: f32) -> f32 {
        if self.timings.is_empty() {
            return 0.0;
        }
        let mut times: Vec<u64> = self.timings.iter().map(|t| t.total_time_us).collect();
        times.sort();
        let idx = ((pct / 100.0) * times.len() as f32) as usize;
        let idx = idx.min(times.len() - 1);
        times[idx] as f32 / 1000.0
    }

    pub fn one_percent_low_fps(&self) -> f32 {
        let ft = self.percentile_frametime_ms(99.0);
        if ft == 0.0 {
            return 0.0;
        }
        1000.0 / ft
    }

    pub fn latest(&self) -> Option<&FrameTiming> {
        self.timings.last()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AthGFX Command Submission API — Vulkan-equivalent resource management,
// pipeline state, render passes, and queue submission
// ═══════════════════════════════════════════════════════════════════════════

// ── Descriptor set types ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorType {
    UniformBuffer,
    StorageBuffer,
    SampledImage,
    StorageImage,
    Sampler,
    CombinedImageSampler,
}

#[derive(Debug, Clone)]
pub struct DescriptorSetLayoutBinding {
    pub binding: u32,
    pub descriptor_type: DescriptorType,
    pub count: u32,
    pub stage: ShaderStage,
}

#[derive(Debug, Clone)]
pub struct DescriptorSetLayout {
    pub handle: DescriptorSetLayoutHandle,
    pub bindings: Vec<DescriptorSetLayoutBinding>,
}

#[derive(Debug, Clone)]
pub enum DescriptorResource {
    Buffer {
        handle: BufferHandle,
        offset: u64,
        size: u64,
    },
    Image {
        handle: TextureHandle,
        layout: ImageLayout,
    },
    SamplerBinding {
        handle: SamplerHandle,
    },
    CombinedImageSampler {
        image: TextureHandle,
        sampler: SamplerHandle,
        layout: ImageLayout,
    },
}

#[derive(Debug, Clone)]
pub struct DescriptorWrite {
    pub binding: u32,
    pub resource: DescriptorResource,
}

#[derive(Debug, Clone)]
pub struct DescriptorSet {
    pub handle: DescriptorSetHandle,
    pub layout: DescriptorSetLayoutHandle,
    pub writes: Vec<DescriptorWrite>,
}

// ── Push constant range ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct PushConstantRange {
    pub stage: ShaderStage,
    pub offset: u32,
    pub size: u32,
}

// ── Pipeline layout ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PipelineLayoutDescriptor {
    pub set_layouts: Vec<DescriptorSetLayoutHandle>,
    pub push_constant_ranges: Vec<PushConstantRange>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PipelineLayout {
    pub handle: PipelineLayoutHandle,
    pub set_layouts: Vec<DescriptorSetLayoutHandle>,
    pub push_constant_ranges: Vec<PushConstantRange>,
}

// ── Pipeline objects ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GraphicsPipeline {
    pub handle: PipelineHandle,
    pub layout: PipelineLayoutHandle,
    pub vertex_shader: ShaderHandle,
    pub fragment_shader: ShaderHandle,
    pub vertex_layouts: Vec<VertexBufferLayout>,
    pub topology: PrimitiveTopology,
    pub raster: RasterState,
    pub blend: BlendState,
    pub depth_stencil: DepthStencilState,
    pub color_formats: Vec<PixelFormat>,
    pub depth_format: Option<PixelFormat>,
}

#[derive(Debug, Clone)]
pub struct ComputePipeline {
    pub handle: PipelineHandle,
    pub layout: PipelineLayoutHandle,
    pub shader: ShaderHandle,
}

// ── Render pass ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentType {
    Color,
    DepthStencil,
}

#[derive(Debug, Clone, Copy)]
pub struct AttachmentDescription {
    pub format: PixelFormat,
    pub load_op: LoadOp,
    pub store_op: StoreOp,
    pub initial_layout: ImageLayout,
    pub final_layout: ImageLayout,
    pub attachment_type: AttachmentType,
}

#[derive(Debug, Clone)]
pub struct SubpassDescription {
    pub color_attachments: Vec<u32>,
    pub depth_attachment: Option<u32>,
    pub input_attachments: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct SubpassDependency {
    pub src_subpass: u32,
    pub dst_subpass: u32,
    pub src_stage: PipelineStage,
    pub dst_stage: PipelineStage,
    pub src_access: AccessFlags,
    pub dst_access: AccessFlags,
}

#[derive(Debug, Clone)]
pub struct RenderPassCreateInfo {
    pub attachments: Vec<AttachmentDescription>,
    pub subpasses: Vec<SubpassDescription>,
    pub dependencies: Vec<SubpassDependency>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RenderPass {
    pub handle: RenderPassHandle,
    pub attachments: Vec<AttachmentDescription>,
    pub subpasses: Vec<SubpassDescription>,
    pub dependencies: Vec<SubpassDependency>,
}

impl RenderPass {
    pub fn color_attachment_count(&self) -> usize {
        self.attachments
            .iter()
            .filter(|a| a.attachment_type == AttachmentType::Color)
            .count()
    }

    pub fn has_depth(&self) -> bool {
        self.attachments
            .iter()
            .any(|a| a.attachment_type == AttachmentType::DepthStencil)
    }
}

// ── Framebuffer ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FramebufferCreateInfo {
    pub render_pass: RenderPassHandle,
    pub attachments: Vec<TextureHandle>,
    pub width: u32,
    pub height: u32,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Framebuffer {
    pub handle: FramebufferHandle,
    pub render_pass: RenderPassHandle,
    pub attachments: Vec<TextureHandle>,
    pub width: u32,
    pub height: u32,
}

// ── Resource objects (software-backed) ───────────────────────────────────

#[derive(Debug, Clone)]
pub struct Buffer {
    pub handle: BufferHandle,
    pub size: u64,
    pub usage: BufferUsage,
    pub data: Vec<u8>,
}

impl Buffer {
    pub fn read(&self, offset: u64, len: u64) -> Option<&[u8]> {
        let start = offset as usize;
        let end = start + len as usize;
        if end <= self.data.len() {
            Some(&self.data[start..end])
        } else {
            None
        }
    }

    pub fn write(&mut self, offset: u64, src: &[u8]) -> bool {
        let start = offset as usize;
        let end = start + src.len();
        if end <= self.data.len() {
            self.data[start..end].copy_from_slice(src);
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone)]
pub struct Image {
    pub handle: TextureHandle,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mip_levels: u32,
    pub format: PixelFormat,
    pub usage: TextureUsage,
    pub layout: ImageLayout,
    pub data: Vec<u8>,
}

impl Image {
    pub fn byte_size(&self) -> usize {
        self.width as usize
            * self.height as usize
            * self.depth as usize
            * self.format.bytes_per_pixel()
    }

    pub fn row_pitch(&self) -> usize {
        self.width as usize * self.format.bytes_per_pixel()
    }
}

#[derive(Debug, Clone)]
pub struct Sampler {
    pub handle: SamplerHandle,
    pub descriptor: SamplerDescriptor,
}

#[derive(Debug, Clone)]
pub struct Shader {
    pub handle: ShaderHandle,
    pub stage: ShaderStage,
    pub entry_point: String,
    pub spirv: Vec<u8>,
}

// ── Synchronization primitives ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceState {
    Unsignaled,
    Signaled,
}

#[derive(Debug)]
pub struct Fence {
    pub handle: FenceHandle,
    pub state: FenceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemaphoreState {
    Unsignaled,
    Signaled,
}

#[derive(Debug)]
pub struct Semaphore {
    pub handle: SemaphoreHandle,
    pub state: SemaphoreState,
}

// ── Resource manager ─────────────────────────────────────────────────────

pub struct ResourceManager {
    next_id: u64,
    buffers: BTreeMap<u64, Buffer>,
    images: BTreeMap<u64, Image>,
    samplers: BTreeMap<u64, Sampler>,
    shaders: BTreeMap<u64, Shader>,
    descriptor_set_layouts: BTreeMap<u64, DescriptorSetLayout>,
    descriptor_sets: BTreeMap<u64, DescriptorSet>,
    pipeline_layouts: BTreeMap<u64, PipelineLayout>,
    graphics_pipelines: BTreeMap<u64, GraphicsPipeline>,
    compute_pipelines: BTreeMap<u64, ComputePipeline>,
    render_passes: BTreeMap<u64, RenderPass>,
    framebuffers: BTreeMap<u64, Framebuffer>,
    fences: BTreeMap<u64, Fence>,
    semaphores: BTreeMap<u64, Semaphore>,
}

impl ResourceManager {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            buffers: BTreeMap::new(),
            images: BTreeMap::new(),
            samplers: BTreeMap::new(),
            shaders: BTreeMap::new(),
            descriptor_set_layouts: BTreeMap::new(),
            descriptor_sets: BTreeMap::new(),
            pipeline_layouts: BTreeMap::new(),
            graphics_pipelines: BTreeMap::new(),
            compute_pipelines: BTreeMap::new(),
            render_passes: BTreeMap::new(),
            framebuffers: BTreeMap::new(),
            fences: BTreeMap::new(),
            semaphores: BTreeMap::new(),
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ── Buffer ───────────────────────────────────────────────────────────

    pub fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferHandle, GfxError> {
        if desc.size == 0 {
            return Err(GfxError::NotSupported);
        }
        let id = self.alloc_id();
        let handle = BufferHandle(id);
        let buf = Buffer {
            handle,
            size: desc.size,
            usage: desc.usage,
            data: alloc::vec![0u8; desc.size as usize],
        };
        self.buffers.insert(id, buf);
        Ok(handle)
    }

    pub fn destroy_buffer(&mut self, handle: BufferHandle) {
        self.buffers.remove(&handle.0);
    }

    pub fn get_buffer(&self, handle: BufferHandle) -> Option<&Buffer> {
        self.buffers.get(&handle.0)
    }

    pub fn get_buffer_mut(&mut self, handle: BufferHandle) -> Option<&mut Buffer> {
        self.buffers.get_mut(&handle.0)
    }

    pub fn write_buffer(
        &mut self,
        handle: BufferHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<(), GfxError> {
        let buf = self
            .buffers
            .get_mut(&handle.0)
            .ok_or(GfxError::InvalidHandle)?;
        if !buf.write(offset, data) {
            return Err(GfxError::OutOfMemory);
        }
        Ok(())
    }

    // ── Image ────────────────────────────────────────────────────────────

    pub fn create_image(&mut self, desc: &TextureDescriptor) -> Result<TextureHandle, GfxError> {
        if desc.width == 0 || desc.height == 0 {
            return Err(GfxError::NotSupported);
        }
        let id = self.alloc_id();
        let handle = TextureHandle(id);
        let byte_size = desc.width as usize
            * desc.height as usize
            * desc.depth.max(1) as usize
            * desc.format.bytes_per_pixel();
        let img = Image {
            handle,
            width: desc.width,
            height: desc.height,
            depth: desc.depth.max(1),
            mip_levels: desc.mip_levels.max(1),
            format: desc.format,
            usage: desc.usage,
            layout: ImageLayout::Undefined,
            data: alloc::vec![0u8; byte_size],
        };
        self.images.insert(id, img);
        Ok(handle)
    }

    pub fn destroy_image(&mut self, handle: TextureHandle) {
        self.images.remove(&handle.0);
    }

    pub fn get_image(&self, handle: TextureHandle) -> Option<&Image> {
        self.images.get(&handle.0)
    }

    pub fn get_image_mut(&mut self, handle: TextureHandle) -> Option<&mut Image> {
        self.images.get_mut(&handle.0)
    }

    // ── Sampler ──────────────────────────────────────────────────────────

    pub fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerHandle, GfxError> {
        let id = self.alloc_id();
        let handle = SamplerHandle(id);
        self.samplers.insert(
            id,
            Sampler {
                handle,
                descriptor: desc.clone(),
            },
        );
        Ok(handle)
    }

    pub fn destroy_sampler(&mut self, handle: SamplerHandle) {
        self.samplers.remove(&handle.0);
    }

    pub fn get_sampler(&self, handle: SamplerHandle) -> Option<&Sampler> {
        self.samplers.get(&handle.0)
    }

    // ── Shader ───────────────────────────────────────────────────────────

    pub fn create_shader(&mut self, desc: &ShaderDescriptor) -> Result<ShaderHandle, GfxError> {
        if desc.spirv.is_empty() {
            return Err(GfxError::ShaderCompilationFailed);
        }
        let id = self.alloc_id();
        let handle = ShaderHandle(id);
        self.shaders.insert(
            id,
            Shader {
                handle,
                stage: desc.stage,
                entry_point: desc.entry_point.clone(),
                spirv: desc.spirv.clone(),
            },
        );
        Ok(handle)
    }

    pub fn destroy_shader(&mut self, handle: ShaderHandle) {
        self.shaders.remove(&handle.0);
    }

    pub fn get_shader(&self, handle: ShaderHandle) -> Option<&Shader> {
        self.shaders.get(&handle.0)
    }

    // ── Descriptor set layout ────────────────────────────────────────────

    pub fn create_descriptor_set_layout(
        &mut self,
        bindings: Vec<DescriptorSetLayoutBinding>,
    ) -> Result<DescriptorSetLayoutHandle, GfxError> {
        let id = self.alloc_id();
        let handle = DescriptorSetLayoutHandle(id);
        self.descriptor_set_layouts
            .insert(id, DescriptorSetLayout { handle, bindings });
        Ok(handle)
    }

    pub fn destroy_descriptor_set_layout(&mut self, handle: DescriptorSetLayoutHandle) {
        self.descriptor_set_layouts.remove(&handle.0);
    }

    pub fn get_descriptor_set_layout(
        &self,
        handle: DescriptorSetLayoutHandle,
    ) -> Option<&DescriptorSetLayout> {
        self.descriptor_set_layouts.get(&handle.0)
    }

    // ── Descriptor set ───────────────────────────────────────────────────

    pub fn allocate_descriptor_set(
        &mut self,
        layout: DescriptorSetLayoutHandle,
    ) -> Result<DescriptorSetHandle, GfxError> {
        if !self.descriptor_set_layouts.contains_key(&layout.0) {
            return Err(GfxError::InvalidHandle);
        }
        let id = self.alloc_id();
        let handle = DescriptorSetHandle(id);
        self.descriptor_sets.insert(
            id,
            DescriptorSet {
                handle,
                layout,
                writes: Vec::new(),
            },
        );
        Ok(handle)
    }

    pub fn update_descriptor_set(
        &mut self,
        handle: DescriptorSetHandle,
        writes: Vec<DescriptorWrite>,
    ) -> Result<(), GfxError> {
        let set = self
            .descriptor_sets
            .get_mut(&handle.0)
            .ok_or(GfxError::InvalidHandle)?;
        set.writes = writes;
        Ok(())
    }

    pub fn free_descriptor_set(&mut self, handle: DescriptorSetHandle) {
        self.descriptor_sets.remove(&handle.0);
    }

    pub fn get_descriptor_set(&self, handle: DescriptorSetHandle) -> Option<&DescriptorSet> {
        self.descriptor_sets.get(&handle.0)
    }

    // ── Pipeline layout ──────────────────────────────────────────────────

    pub fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutHandle, GfxError> {
        for sl in &desc.set_layouts {
            if !self.descriptor_set_layouts.contains_key(&sl.0) {
                return Err(GfxError::InvalidHandle);
            }
        }
        let id = self.alloc_id();
        let handle = PipelineLayoutHandle(id);
        self.pipeline_layouts.insert(
            id,
            PipelineLayout {
                handle,
                set_layouts: desc.set_layouts.clone(),
                push_constant_ranges: desc.push_constant_ranges.clone(),
            },
        );
        Ok(handle)
    }

    pub fn destroy_pipeline_layout(&mut self, handle: PipelineLayoutHandle) {
        self.pipeline_layouts.remove(&handle.0);
    }

    pub fn get_pipeline_layout(&self, handle: PipelineLayoutHandle) -> Option<&PipelineLayout> {
        self.pipeline_layouts.get(&handle.0)
    }

    // ── Graphics pipeline ────────────────────────────────────────────────

    pub fn create_graphics_pipeline(
        &mut self,
        desc: &GraphicsPipelineDescriptor,
        layout: PipelineLayoutHandle,
    ) -> Result<PipelineHandle, GfxError> {
        if !self.pipeline_layouts.contains_key(&layout.0) {
            return Err(GfxError::InvalidHandle);
        }
        if !self.shaders.contains_key(&desc.vertex_shader.0) {
            return Err(GfxError::InvalidHandle);
        }
        if !self.shaders.contains_key(&desc.fragment_shader.0) {
            return Err(GfxError::InvalidHandle);
        }
        let id = self.alloc_id();
        let handle = PipelineHandle(id);
        self.graphics_pipelines.insert(
            id,
            GraphicsPipeline {
                handle,
                layout,
                vertex_shader: desc.vertex_shader,
                fragment_shader: desc.fragment_shader,
                vertex_layouts: desc.vertex_layouts.clone(),
                topology: desc.topology,
                raster: desc.raster,
                blend: desc.blend,
                depth_stencil: desc.depth_stencil,
                color_formats: desc.color_formats.clone(),
                depth_format: desc.depth_format,
            },
        );
        Ok(handle)
    }

    pub fn destroy_graphics_pipeline(&mut self, handle: PipelineHandle) {
        self.graphics_pipelines.remove(&handle.0);
    }

    pub fn get_graphics_pipeline(&self, handle: PipelineHandle) -> Option<&GraphicsPipeline> {
        self.graphics_pipelines.get(&handle.0)
    }

    // ── Compute pipeline ─────────────────────────────────────────────────

    pub fn create_compute_pipeline(
        &mut self,
        desc: &ComputePipelineDescriptor,
        layout: PipelineLayoutHandle,
    ) -> Result<PipelineHandle, GfxError> {
        if !self.pipeline_layouts.contains_key(&layout.0) {
            return Err(GfxError::InvalidHandle);
        }
        if !self.shaders.contains_key(&desc.shader.0) {
            return Err(GfxError::InvalidHandle);
        }
        let id = self.alloc_id();
        let handle = PipelineHandle(id);
        self.compute_pipelines.insert(
            id,
            ComputePipeline {
                handle,
                layout,
                shader: desc.shader,
            },
        );
        Ok(handle)
    }

    pub fn destroy_compute_pipeline(&mut self, handle: PipelineHandle) {
        self.compute_pipelines.remove(&handle.0);
    }

    pub fn get_compute_pipeline(&self, handle: PipelineHandle) -> Option<&ComputePipeline> {
        self.compute_pipelines.get(&handle.0)
    }

    // ── Render pass ──────────────────────────────────────────────────────

    pub fn create_render_pass(
        &mut self,
        info: &RenderPassCreateInfo,
    ) -> Result<RenderPassHandle, GfxError> {
        if info.subpasses.is_empty() {
            return Err(GfxError::NotSupported);
        }
        let id = self.alloc_id();
        let handle = RenderPassHandle(id);
        self.render_passes.insert(
            id,
            RenderPass {
                handle,
                attachments: info.attachments.clone(),
                subpasses: info.subpasses.clone(),
                dependencies: info.dependencies.clone(),
            },
        );
        Ok(handle)
    }

    pub fn destroy_render_pass(&mut self, handle: RenderPassHandle) {
        self.render_passes.remove(&handle.0);
    }

    pub fn get_render_pass(&self, handle: RenderPassHandle) -> Option<&RenderPass> {
        self.render_passes.get(&handle.0)
    }

    // ── Framebuffer ──────────────────────────────────────────────────────

    pub fn create_framebuffer(
        &mut self,
        info: &FramebufferCreateInfo,
    ) -> Result<FramebufferHandle, GfxError> {
        if !self.render_passes.contains_key(&info.render_pass.0) {
            return Err(GfxError::InvalidHandle);
        }
        for att in &info.attachments {
            if !self.images.contains_key(&att.0) {
                return Err(GfxError::InvalidHandle);
            }
        }
        let id = self.alloc_id();
        let handle = FramebufferHandle(id);
        self.framebuffers.insert(
            id,
            Framebuffer {
                handle,
                render_pass: info.render_pass,
                attachments: info.attachments.clone(),
                width: info.width,
                height: info.height,
            },
        );
        Ok(handle)
    }

    pub fn destroy_framebuffer(&mut self, handle: FramebufferHandle) {
        self.framebuffers.remove(&handle.0);
    }

    pub fn get_framebuffer(&self, handle: FramebufferHandle) -> Option<&Framebuffer> {
        self.framebuffers.get(&handle.0)
    }

    // ── Fence ────────────────────────────────────────────────────────────

    pub fn create_fence(&mut self, signaled: bool) -> Result<FenceHandle, GfxError> {
        let id = self.alloc_id();
        let handle = FenceHandle(id);
        self.fences.insert(
            id,
            Fence {
                handle,
                state: if signaled {
                    FenceState::Signaled
                } else {
                    FenceState::Unsignaled
                },
            },
        );
        Ok(handle)
    }

    pub fn destroy_fence(&mut self, handle: FenceHandle) {
        self.fences.remove(&handle.0);
    }

    pub fn get_fence(&self, handle: FenceHandle) -> Option<&Fence> {
        self.fences.get(&handle.0)
    }

    pub fn reset_fence(&mut self, handle: FenceHandle) -> Result<(), GfxError> {
        let fence = self
            .fences
            .get_mut(&handle.0)
            .ok_or(GfxError::InvalidHandle)?;
        fence.state = FenceState::Unsignaled;
        Ok(())
    }

    pub fn signal_fence(&mut self, handle: FenceHandle) -> Result<(), GfxError> {
        let fence = self
            .fences
            .get_mut(&handle.0)
            .ok_or(GfxError::InvalidHandle)?;
        fence.state = FenceState::Signaled;
        Ok(())
    }

    pub fn wait_fence(&self, handle: FenceHandle) -> Result<bool, GfxError> {
        let fence = self.fences.get(&handle.0).ok_or(GfxError::InvalidHandle)?;
        Ok(fence.state == FenceState::Signaled)
    }

    // ── Semaphore ────────────────────────────────────────────────────────

    pub fn create_semaphore(&mut self) -> Result<SemaphoreHandle, GfxError> {
        let id = self.alloc_id();
        let handle = SemaphoreHandle(id);
        self.semaphores.insert(
            id,
            Semaphore {
                handle,
                state: SemaphoreState::Unsignaled,
            },
        );
        Ok(handle)
    }

    pub fn destroy_semaphore(&mut self, handle: SemaphoreHandle) {
        self.semaphores.remove(&handle.0);
    }

    pub fn get_semaphore(&self, handle: SemaphoreHandle) -> Option<&Semaphore> {
        self.semaphores.get(&handle.0)
    }

    // ── Command pool ─────────────────────────────────────────────────────

    pub fn allocate_command_buffer(&self, label: Option<&str>) -> CommandBuffer {
        CommandBuffer::new(label)
    }
}

// ── Queue submission ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SubmitInfo {
    pub command_buffers: Vec<CommandBuffer>,
    pub wait_semaphores: Vec<SemaphoreHandle>,
    pub signal_semaphores: Vec<SemaphoreHandle>,
}

pub struct Queue {
    pending: Vec<SubmitInfo>,
    submissions_completed: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            submissions_completed: 0,
        }
    }

    pub fn submit(
        &mut self,
        info: SubmitInfo,
        signal_fence: Option<FenceHandle>,
        resources: &mut ResourceManager,
    ) -> Result<(), GfxError> {
        for cb in &info.command_buffers {
            if cb.state != CommandBufferState::Executable {
                return Err(GfxError::NotSupported);
            }
        }
        for &sem in &info.wait_semaphores {
            if resources.get_semaphore(sem).is_none() {
                return Err(GfxError::InvalidHandle);
            }
        }
        for &sem in &info.signal_semaphores {
            if resources.get_semaphore(sem).is_none() {
                return Err(GfxError::InvalidHandle);
            }
        }
        if let Some(fence) = signal_fence {
            if resources.get_fence(fence).is_none() {
                return Err(GfxError::InvalidHandle);
            }
        }

        self.pending.push(info);

        if let Some(fence) = signal_fence {
            let _ = resources.signal_fence(fence);
        }

        Ok(())
    }

    pub fn flush(
        &mut self,
        resources: &mut ResourceManager,
        canvas: &mut Canvas,
    ) -> Result<u64, GfxError> {
        let mut executed = 0u64;
        let submissions = core::mem::take(&mut self.pending);

        for info in submissions {
            for &sem in &info.wait_semaphores {
                if let Some(s) = resources.semaphores.get_mut(&sem.0) {
                    s.state = SemaphoreState::Unsignaled;
                }
            }

            for cb in &info.command_buffers {
                Self::execute_command_buffer(cb, resources, canvas)?;
                executed += 1;
            }

            for &sem in &info.signal_semaphores {
                if let Some(s) = resources.semaphores.get_mut(&sem.0) {
                    s.state = SemaphoreState::Signaled;
                }
            }
        }

        self.submissions_completed += executed;
        Ok(executed)
    }

    pub fn submissions_completed(&self) -> u64 {
        self.submissions_completed
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn execute_command_buffer(
        cb: &CommandBuffer,
        resources: &mut ResourceManager,
        canvas: &mut Canvas,
    ) -> Result<(), GfxError> {
        let mut current_pipeline: Option<PipelineHandle> = None;
        let mut bound_vertex_buffers: BTreeMap<u32, (BufferHandle, u64)> = BTreeMap::new();
        let mut bound_index_buffer: Option<(BufferHandle, u64, IndexType)> = None;
        let mut viewport = Viewport {
            x: 0.0,
            y: 0.0,
            width: canvas.width() as f32,
            height: canvas.height() as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let mut scissor = Scissor {
            x: 0,
            y: 0,
            width: canvas.width() as u32,
            height: canvas.height() as u32,
        };
        let mut _in_render_pass = false;

        for cmd in &cb.commands {
            match cmd {
                DrawCommand::BindPipeline(handle) => {
                    current_pipeline = Some(*handle);
                }
                DrawCommand::BindVertexBuffer {
                    slot,
                    buffer,
                    offset,
                } => {
                    bound_vertex_buffers.insert(*slot, (*buffer, *offset));
                }
                DrawCommand::BindIndexBuffer {
                    buffer,
                    offset,
                    index_type,
                } => {
                    bound_index_buffer = Some((*buffer, *offset, *index_type));
                }
                DrawCommand::BindDescriptorSet { .. } => {
                    // Descriptor set bindings are recorded but not consumed
                    // by the software rasterizer — they'll feed real GPU
                    // shader uniform data when a hardware backend exists.
                }
                DrawCommand::SetViewport {
                    x,
                    y,
                    width,
                    height,
                    min_depth,
                    max_depth,
                } => {
                    viewport = Viewport {
                        x: *x,
                        y: *y,
                        width: *width,
                        height: *height,
                        min_depth: *min_depth,
                        max_depth: *max_depth,
                    };
                }
                DrawCommand::SetScissor {
                    x,
                    y,
                    width,
                    height,
                } => {
                    scissor = Scissor {
                        x: *x,
                        y: *y,
                        width: *width,
                        height: *height,
                    };
                }
                DrawCommand::BeginRenderPass {
                    framebuffer,
                    clear_values,
                    ..
                } => {
                    _in_render_pass = true;
                    if let Some(fb) = resources.get_framebuffer(*framebuffer) {
                        let fb_w = fb.width;
                        let fb_h = fb.height;
                        for (i, cv) in clear_values.iter().enumerate() {
                            if let ClearValue::Color(rgba) = cv {
                                let r = (rgba[0].clamp(0.0, 1.0) * 255.0) as u32;
                                let g = (rgba[1].clamp(0.0, 1.0) * 255.0) as u32;
                                let b = (rgba[2].clamp(0.0, 1.0) * 255.0) as u32;
                                let a = (rgba[3].clamp(0.0, 1.0) * 255.0) as u32;
                                let color = (a << 24) | (r << 16) | (g << 8) | b;
                                if i == 0 {
                                    canvas.fill_rect(0, 0, fb_w as usize, fb_h as usize, color);
                                }
                            }
                        }
                    }
                }
                DrawCommand::EndRenderPass => {
                    _in_render_pass = false;
                }
                DrawCommand::Draw {
                    vertex_count,
                    instance_count,
                    first_vertex,
                    ..
                } => {
                    if current_pipeline.is_none() {
                        continue;
                    }
                    let _vp = viewport;
                    let _sc = scissor;

                    if let Some((&_slot, &(vb_handle, vb_offset))) =
                        bound_vertex_buffers.iter().next()
                    {
                        if let Some(buf) = resources.get_buffer(vb_handle) {
                            let stride = 20usize; // position (x,y: 2×i32=8) + color (u32=4) + padding to 20
                            for inst in 0..*instance_count {
                                let _ = inst;
                                Self::sw_draw_triangles(
                                    canvas,
                                    &buf.data,
                                    vb_offset as usize,
                                    stride,
                                    *first_vertex,
                                    *vertex_count,
                                );
                            }
                        }
                    }
                }
                DrawCommand::DrawIndexed {
                    index_count,
                    instance_count,
                    first_index,
                    vertex_offset,
                    ..
                } => {
                    if current_pipeline.is_none() {
                        continue;
                    }

                    let ib = match bound_index_buffer {
                        Some(ib) => ib,
                        None => continue,
                    };
                    let (ib_handle, ib_offset, idx_type) = ib;
                    let vb = match bound_vertex_buffers.iter().next() {
                        Some((_, &v)) => v,
                        None => continue,
                    };

                    let ib_data = match resources.get_buffer(ib_handle) {
                        Some(b) => &b.data,
                        None => continue,
                    };
                    let vb_data = match resources.get_buffer(vb.0) {
                        Some(b) => &b.data,
                        None => continue,
                    };

                    let stride = 20usize;
                    let idx_size = match idx_type {
                        IndexType::U16 => 2usize,
                        IndexType::U32 => 4usize,
                    };

                    for _inst in 0..*instance_count {
                        let tri_count = *index_count / 3;
                        for tri in 0..tri_count {
                            let base = ib_offset as usize
                                + (*first_index as usize + tri as usize * 3) * idx_size;
                            let mut indices = [0u32; 3];
                            for v in 0..3 {
                                let off = base + v * idx_size;
                                indices[v] = match idx_type {
                                    IndexType::U16 => {
                                        if off + 2 <= ib_data.len() {
                                            u16::from_le_bytes([ib_data[off], ib_data[off + 1]])
                                                as u32
                                        } else {
                                            0
                                        }
                                    }
                                    IndexType::U32 => {
                                        if off + 4 <= ib_data.len() {
                                            u32::from_le_bytes([
                                                ib_data[off],
                                                ib_data[off + 1],
                                                ib_data[off + 2],
                                                ib_data[off + 3],
                                            ])
                                        } else {
                                            0
                                        }
                                    }
                                };
                                indices[v] = (indices[v] as i32 + vertex_offset) as u32;
                            }

                            let verts: [(i32, i32, u32); 3] = core::array::from_fn(|i| {
                                let vi = indices[i] as usize;
                                let off = vb.1 as usize + vi * stride;
                                if off + 12 <= vb_data.len() {
                                    let x = i32::from_le_bytes([
                                        vb_data[off],
                                        vb_data[off + 1],
                                        vb_data[off + 2],
                                        vb_data[off + 3],
                                    ]);
                                    let y = i32::from_le_bytes([
                                        vb_data[off + 4],
                                        vb_data[off + 5],
                                        vb_data[off + 6],
                                        vb_data[off + 7],
                                    ]);
                                    let c = u32::from_le_bytes([
                                        vb_data[off + 8],
                                        vb_data[off + 9],
                                        vb_data[off + 10],
                                        vb_data[off + 11],
                                    ]);
                                    (x, y, c)
                                } else {
                                    (0, 0, 0)
                                }
                            });

                            canvas.draw_triangle(verts[0], verts[1], verts[2]);
                        }
                    }
                }
                DrawCommand::Dispatch { .. } => {
                    // Compute dispatch — no software path yet; requires shader
                    // interpreter. Silently skip for now.
                }
                DrawCommand::PushConstants { .. } => {
                    // Push constant data stored but not consumed by SW path.
                }
                DrawCommand::CopyBuffer {
                    src,
                    dst,
                    src_offset,
                    dst_offset,
                    size,
                } => {
                    let src_data = match resources.get_buffer(*src) {
                        Some(b) => {
                            let s = *src_offset as usize;
                            let e = s + *size as usize;
                            if e <= b.data.len() {
                                b.data[s..e].to_vec()
                            } else {
                                continue;
                            }
                        }
                        None => continue,
                    };
                    if let Some(dst_buf) = resources.get_buffer_mut(*dst) {
                        let d = *dst_offset as usize;
                        let e = d + *size as usize;
                        if e <= dst_buf.data.len() {
                            dst_buf.data[d..e].copy_from_slice(&src_data);
                        }
                    }
                }
                DrawCommand::CopyBufferToTexture {
                    src,
                    dst,
                    width,
                    height,
                } => {
                    let src_data = match resources.get_buffer(*src) {
                        Some(b) => b.data.clone(),
                        None => continue,
                    };
                    if let Some(img) = resources.get_image_mut(*dst) {
                        let bpp = img.format.bytes_per_pixel();
                        let row_bytes = *width as usize * bpp;
                        let total = row_bytes * *height as usize;
                        if total <= src_data.len() && total <= img.data.len() {
                            img.data[..total].copy_from_slice(&src_data[..total]);
                        }
                    }
                }
                DrawCommand::CopyImageToBuffer { src, dst } => {
                    let src_data = match resources.get_image(*src) {
                        Some(img) => img.data.clone(),
                        None => continue,
                    };
                    if let Some(buf) = resources.get_buffer_mut(*dst) {
                        let len = src_data.len().min(buf.data.len());
                        buf.data[..len].copy_from_slice(&src_data[..len]);
                    }
                }
                DrawCommand::PipelineBarrier(barrier) => {
                    for ib in &barrier.image_barriers {
                        if let Some(img) = resources.get_image_mut(ib.image) {
                            img.layout = ib.new_layout;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Software rasterizer: interpret a vertex buffer as a packed stream of
    /// triangles (3 vertices each) and draw them via `Canvas::draw_triangle`.
    ///
    /// Vertex layout assumed: x: i32, y: i32, color: u32 (12 bytes minimum),
    /// padded to `stride` bytes per vertex.
    fn sw_draw_triangles(
        canvas: &mut Canvas,
        vb_data: &[u8],
        vb_offset: usize,
        stride: usize,
        first_vertex: u32,
        vertex_count: u32,
    ) {
        let tri_count = vertex_count / 3;
        for tri in 0..tri_count {
            let mut verts = [(0i32, 0i32, 0u32); 3];
            for v in 0..3u32 {
                let vi = first_vertex + tri * 3 + v;
                let off = vb_offset + vi as usize * stride;
                if off + 12 > vb_data.len() {
                    return;
                }
                let x = i32::from_le_bytes([
                    vb_data[off],
                    vb_data[off + 1],
                    vb_data[off + 2],
                    vb_data[off + 3],
                ]);
                let y = i32::from_le_bytes([
                    vb_data[off + 4],
                    vb_data[off + 5],
                    vb_data[off + 6],
                    vb_data[off + 7],
                ]);
                let c = u32::from_le_bytes([
                    vb_data[off + 8],
                    vb_data[off + 9],
                    vb_data[off + 10],
                    vb_data[off + 11],
                ]);
                verts[v as usize] = (x, y, c);
            }
            canvas.draw_triangle(verts[0], verts[1], verts[2]);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Host KATs for the modern UI primitives (cargo test -p athgfx)
// FAIL-able pixel-math proofs: rounded corners are actually cut, gradients
// actually interpolate, circles are bounded, alpha blends correctly. These run
// on the dev box (no QEMU/iron) — the cheapest real proof for pure pixel logic.
// ═══════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod ui_primitive_tests {
    use super::*;

    /// Build a black (zeroed) ARGB canvas over a caller-owned buffer.
    fn make<'a>(buf: &'a mut Vec<u8>, w: usize, h: usize) -> Canvas {
        *buf = alloc::vec![0u8; w * h * 4];
        unsafe { Canvas::new(buf.as_mut_ptr(), w, h, 4) }
    }

    #[test]
    fn pixel_roundtrip_and_blend() {
        let mut buf = Vec::new();
        let mut c = make(&mut buf, 4, 4);

        c.draw_pixel(1, 1, 0xFF_FF_FF_FF);
        assert_eq!(c.get_pixel(1, 1) & 0xFF_FF_FF, 0xFF_FF_FF);

        // Opaque source replaces the destination.
        c.blend_pixel(1, 1, 0xFF_FF_00_00);
        assert_eq!(c.get_pixel(1, 1) & 0xFF_FF_FF, 0xFF_00_00);

        // 50% black over white ≈ mid-grey.
        c.draw_pixel(2, 2, 0xFF_FF_FF_FF);
        c.blend_pixel(2, 2, 0x80_00_00_00);
        let r = (c.get_pixel(2, 2) >> 16) & 0xFF;
        assert!((118..=137).contains(&r), "expected mid-grey, got {}", r);

        // Fully transparent source is a no-op.
        c.draw_pixel(0, 0, 0xFF_12_34_56);
        c.blend_pixel(0, 0, 0x00_FF_FF_FF);
        assert_eq!(c.get_pixel(0, 0) & 0xFF_FF_FF, 0x12_34_56);
    }

    #[test]
    fn rounded_rect_cuts_corners_keeps_body() {
        let mut buf = Vec::new();
        let mut c = make(&mut buf, 20, 20);
        c.fill_rounded_rect(0, 0, 20, 20, 8, 0xFF_FF_00_00);

        // Body + straight edges are filled.
        assert_eq!(c.get_pixel(10, 10) & 0xFF_FF_FF, 0xFF_00_00, "center");
        assert_eq!(c.get_pixel(10, 0) & 0xFF_FF_FF, 0xFF_00_00, "top edge");
        assert_eq!(c.get_pixel(0, 10) & 0xFF_FF_FF, 0xFF_00_00, "left edge");

        // The extreme corner pixel is rounded away (left untouched → 0).
        assert_eq!(c.get_pixel(0, 0), 0, "top-left corner must be cut");
        assert_eq!(c.get_pixel(19, 0), 0, "top-right corner must be cut");
    }

    #[test]
    fn vertical_gradient_interpolates() {
        let mut buf = Vec::new();
        let mut c = make(&mut buf, 4, 4);
        c.fill_rect_gradient(0, 0, 4, 4, 0xFF_00_00_00, 0xFF_FF_FF_FF);

        assert_eq!(c.get_pixel(0, 0) & 0xFF_FF_FF, 0x00_00_00, "top = start");
        assert_eq!(c.get_pixel(0, 3) & 0xFF_FF_FF, 0xFF_FF_FF, "bottom = end");
        let mid = c.get_pixel(0, 1) & 0xFF;
        assert!(mid > 0 && mid < 255, "middle row interpolated, got {}", mid);
    }

    #[test]
    fn circle_is_bounded() {
        let mut buf = Vec::new();
        let mut c = make(&mut buf, 11, 11);
        c.fill_circle(5, 5, 5, 0xFF_00_FF_00);

        assert_eq!(c.get_pixel(5, 5) & 0xFF_FF_FF, 0x00_FF_00, "center filled");
        assert_eq!(c.get_pixel(0, 0), 0, "corner outside radius untouched");
    }

    #[test]
    fn scaled_text_is_antialiased() {
        let mut buf = Vec::new();
        let mut c = make(&mut buf, 64, 64);
        // 'A' has diagonal edges; at 4× bilinear must yield intermediate grays.
        c.draw_glyph_scaled(8, 8, 'A', 0xFF_FF_FF_FF, 4);
        let mut levels = alloc::collections::BTreeSet::new();
        for y in 8..48 {
            for x in 8..48 {
                levels.insert(c.get_pixel(x, y) & 0xFF);
            }
        }
        let partials = levels.iter().filter(|&&v| v > 0 && v < 255).count();
        assert!(partials > 0, "expected AA edge grays, got {:?}", levels);
    }
}
