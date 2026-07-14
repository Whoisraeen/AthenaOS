//! AthUI Text Rendering
//!
//! Wires the `athfont` glyph rasterizer into AthUI. Provides text measurement,
//! line breaking, and glyph caching for efficient rendering into a Canvas.

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use athfont::{
    parse_glyph, FontHandle, Glyph, RasterConfig, Rasterizer, SimpleGlyph, SubpixelMode,
};
use athgfx::Canvas;

// ── Glyph Bitmap Cache ──────────────────────────────────────────────────

pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub advance: i32,
    pub pixels: Vec<u8>,
}

pub struct GlyphCacheMap {
    entries: BTreeMap<(u32, u16), GlyphBitmap>,
    capacity: usize,
}

impl GlyphCacheMap {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            capacity,
        }
    }

    pub fn get(&self, codepoint: u32, ppem: u16) -> Option<&GlyphBitmap> {
        self.entries.get(&(codepoint, ppem))
    }

    pub fn insert(&mut self, codepoint: u32, ppem: u16, bitmap: GlyphBitmap) {
        if self.entries.len() >= self.capacity {
            if let Some(&first_key) = self.entries.keys().next() {
                self.entries.remove(&first_key);
            }
        }
        self.entries.insert((codepoint, ppem), bitmap);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// ── Text Measurement ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct TextMetrics {
    pub width: f32,
    pub height: f32,
    pub line_count: usize,
}

pub fn measure_text(
    font: &FontHandle,
    text: &str,
    ppem: u16,
    max_width: Option<f32>,
) -> TextMetrics {
    let scale = ppem as f32 / font.head.units_per_em as f32;
    let line_height =
        ((font.hhea.ascender - font.hhea.descender + font.hhea.line_gap) as f32 * scale) as f32;

    if text.is_empty() {
        return TextMetrics {
            width: 0.0,
            height: line_height,
            line_count: 1,
        };
    }

    let max_w = max_width.unwrap_or(f32::MAX);
    let mut current_line_width: f32 = 0.0;
    let mut max_line_width: f32 = 0.0;
    let mut line_count: usize = 1;
    let mut word_width: f32 = 0.0;
    let mut in_word = false;

    for ch in text.chars() {
        if ch == '\n' {
            current_line_width += word_width;
            if current_line_width > max_line_width {
                max_line_width = current_line_width;
            }
            current_line_width = 0.0;
            word_width = 0.0;
            in_word = false;
            line_count += 1;
            continue;
        }

        let glyph_id = font.cmap.lookup(ch as u32).unwrap_or(0);
        let advance = font.hmtx.advance_width(glyph_id) as f32 * scale;

        if ch == ' ' {
            if in_word {
                current_line_width += word_width;
                word_width = 0.0;
                in_word = false;
            }
            current_line_width += advance;
            if current_line_width > max_w && current_line_width > advance {
                current_line_width = 0.0;
                line_count += 1;
            }
        } else {
            word_width += advance;
            in_word = true;

            if current_line_width + word_width > max_w && current_line_width > 0.0 {
                if current_line_width > max_line_width {
                    max_line_width = current_line_width;
                }
                current_line_width = 0.0;
                line_count += 1;
            }
        }
    }

    current_line_width += word_width;
    if current_line_width > max_line_width {
        max_line_width = current_line_width;
    }

    TextMetrics {
        width: max_line_width,
        height: line_height * line_count as f32,
        line_count,
    }
}

// ── Line Breaking ───────────────────────────────────────────────────────

pub struct TextLine {
    pub start: usize,
    pub end: usize,
    pub width: f32,
}

pub fn break_lines(font: &FontHandle, text: &str, ppem: u16, max_width: f32) -> Vec<TextLine> {
    let scale = ppem as f32 / font.head.units_per_em as f32;
    let mut lines: Vec<TextLine> = Vec::new();
    let mut line_start: usize = 0;
    let mut line_width: f32 = 0.0;
    let mut last_break: usize = 0;
    let mut width_at_break: f32 = 0.0;

    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            lines.push(TextLine {
                start: line_start,
                end: i,
                width: line_width,
            });
            line_start = i + ch.len_utf8();
            line_width = 0.0;
            last_break = line_start;
            width_at_break = 0.0;
            continue;
        }

        let glyph_id = font.cmap.lookup(ch as u32).unwrap_or(0);
        let advance = font.hmtx.advance_width(glyph_id) as f32 * scale;

        if ch == ' ' {
            last_break = i;
            width_at_break = line_width;
        }

        line_width += advance;

        if line_width > max_width && line_start < i {
            if last_break > line_start {
                lines.push(TextLine {
                    start: line_start,
                    end: last_break,
                    width: width_at_break,
                });
                line_start = last_break + 1;
                line_width -= width_at_break;
                // Skip the space
                let space_id = font.cmap.lookup(b' ' as u32).unwrap_or(0);
                let space_adv = font.hmtx.advance_width(space_id) as f32 * scale;
                line_width -= space_adv;
            } else {
                lines.push(TextLine {
                    start: line_start,
                    end: i,
                    width: line_width - advance,
                });
                line_start = i;
                line_width = advance;
            }
            last_break = line_start;
            width_at_break = 0.0;
        }
    }

    if line_start <= text.len() {
        lines.push(TextLine {
            start: line_start,
            end: text.len(),
            width: line_width,
        });
    }

    lines
}

// ── Text Renderer ───────────────────────────────────────────────────────

pub struct TextRenderer {
    pub cache: GlyphCacheMap,
    rasterizer: Rasterizer,
}

impl TextRenderer {
    pub fn new(cache_capacity: usize) -> Self {
        Self {
            cache: GlyphCacheMap::new(cache_capacity),
            rasterizer: Rasterizer::new(RasterConfig {
                ppem: 16,
                subpixel: SubpixelMode::None,
                gamma: 1.8,
                stem_darkening: false,
                auto_hint: false,
                fractional_positioning: false,
                oversample: 1,
            }),
        }
    }

    pub fn render_text(
        &mut self,
        canvas: &mut Canvas,
        font: &FontHandle,
        text: &str,
        x: usize,
        y: usize,
        ppem: u16,
        color: u32,
        max_width: Option<f32>,
    ) {
        let scale = ppem as f32 / font.head.units_per_em as f32;
        let ascender = (font.hhea.ascender as f32 * scale) as i32;
        let line_height = ((font.hhea.ascender - font.hhea.descender + font.hhea.line_gap) as f32
            * scale) as usize;

        let max_w = max_width.unwrap_or(canvas.width() as f32);
        let lines = break_lines(font, text, ppem, max_w);

        for (line_idx, line) in lines.iter().enumerate() {
            let line_y = y + line_idx * line_height;
            let line_text = &text[line.start..line.end];
            let mut pen_x = x as i32;

            for ch in line_text.chars() {
                let bitmap = self.rasterize_cached(font, ch, ppem);
                if let Some(bmp) = bitmap {
                    let gx = pen_x + bmp.bearing_x;
                    let gy = line_y as i32 + ascender - bmp.bearing_y;
                    self.blit_glyph(canvas, &bmp, gx, gy, color);
                    pen_x += bmp.advance;
                }
            }
        }
    }

    pub fn measure(
        &self,
        font: &FontHandle,
        text: &str,
        ppem: u16,
        max_width: Option<f32>,
    ) -> TextMetrics {
        measure_text(font, text, ppem, max_width)
    }

    fn rasterize_cached(&mut self, font: &FontHandle, ch: char, ppem: u16) -> Option<GlyphBitmap> {
        let cp = ch as u32;
        if let Some(cached) = self.cache.get(cp, ppem) {
            return Some(GlyphBitmap {
                width: cached.width,
                height: cached.height,
                bearing_x: cached.bearing_x,
                bearing_y: cached.bearing_y,
                advance: cached.advance,
                pixels: cached.pixels.clone(),
            });
        }

        let glyph_id = font.cmap.lookup(cp)?;
        let scale = ppem as f32 / font.head.units_per_em as f32;
        let advance = (font.hmtx.advance_width(glyph_id) as f32 * scale) as i32;

        let simple = self.get_simple_glyph(font, glyph_id)?;
        self.rasterizer = Rasterizer::new(RasterConfig {
            ppem,
            subpixel: SubpixelMode::None,
            gamma: 1.8,
            stem_darkening: false,
            auto_hint: false,
            fractional_positioning: false,
            oversample: 1,
        });
        let rast = self.rasterizer.rasterize(&simple, font.head.units_per_em);

        let bitmap = GlyphBitmap {
            width: rast.width,
            height: rast.height,
            bearing_x: rast.bearing_x,
            bearing_y: rast.bearing_y,
            advance,
            pixels: rast.pixels,
        };
        self.cache.insert(
            cp,
            ppem,
            GlyphBitmap {
                width: bitmap.width,
                height: bitmap.height,
                bearing_x: bitmap.bearing_x,
                bearing_y: bitmap.bearing_y,
                advance: bitmap.advance,
                pixels: bitmap.pixels.clone(),
            },
        );
        Some(bitmap)
    }

    fn get_simple_glyph(&self, font: &FontHandle, glyph_id: u16) -> Option<SimpleGlyph> {
        let (start, end) = font.loca.glyph_range(glyph_id)?;
        if start == end {
            return None;
        }
        let glyf_rec = font.offset_table.find_table(&athfont::TAG_GLYF)?;
        let abs_start = glyf_rec.offset as usize + start as usize;
        let abs_end = glyf_rec.offset as usize + end as usize;
        let glyph_data = font.data.get(abs_start..abs_end)?;
        match parse_glyph(glyph_data)? {
            Glyph::Simple(sg) => Some(sg),
            _ => None,
        }
    }

    fn blit_glyph(&self, canvas: &mut Canvas, bmp: &GlyphBitmap, gx: i32, gy: i32, color: u32) {
        let fg_r = ((color >> 16) & 0xFF) as u32;
        let fg_g = ((color >> 8) & 0xFF) as u32;
        let fg_b = (color & 0xFF) as u32;

        for row in 0..bmp.height {
            for col in 0..bmp.width {
                let alpha = bmp.pixels[(row * bmp.width + col) as usize] as u32;
                if alpha == 0 {
                    continue;
                }

                let px = gx + col as i32;
                let py = gy + row as i32;
                if px < 0 || py < 0 {
                    continue;
                }
                let px = px as usize;
                let py = py as usize;
                if px >= canvas.width() || py >= canvas.height() {
                    continue;
                }

                let blended = if alpha >= 255 {
                    0xFF_00_00_00 | (fg_r << 16) | (fg_g << 8) | fg_b
                } else {
                    let inv = 255 - alpha;
                    let r = (fg_r * alpha) / 255;
                    let g = (fg_g * alpha) / 255;
                    let b = (fg_b * alpha) / 255;
                    0xFF_00_00_00 | (r << 16) | (g << 8) | (b + inv * 0 / 255)
                };
                canvas.draw_pixel(px, py, blended);
            }
        }
    }
}
