//! QUARANTINED DEAD TWIN — DO NOT EXTEND (CLAUDE.md rule 7).
//!
//! This is a SECOND TrueType `FontEngine`, NOT declared as a `pub mod` in
//! `raegfx/src/lib.rs` (orphan — never compiled into the crate). The WIRED font
//! engine is the standalone `components/raefont` crate (more complete: hinting
//! interpreter, COLR/CPAL, shaper, the crisp filled rasterizer + `builtin`
//! embedded faces). The one text API is `raegfx::Canvas::draw_text_aa`
//! (`raegfx/src/text.rs`), which composites raefont coverage source-over.
//! See `docs/QUARANTINED_MODULES.md` and `docs/design/typography-rendering.md`
//! §3.2. New text work extends raefont, never this file.
//!
//! ---
//! (historical) Font rendering engine for RaeenOS. TrueType / OpenType parser
//! and rasterizer providing glyph-level metrics, bitmap rendering, text
//! measurement, layout with word-wrapping, and a glyph cache.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════
// Public types
// ═══════════════════════════════════════════════════════════════════════════

pub struct FontEngine {
    loaded_fonts: Vec<LoadedFont>,
    fallback_chain: Vec<usize>,
    cache: GlyphCache,
    default_size: f32,
    hinting: HintingMode,
    subpixel: SubpixelRendering,
    dpi: f32,
}

pub struct LoadedFont {
    name: String,
    family: String,
    style: FontStyle,
    weight: FontWeight,
    data: Vec<u8>,
    head: FontHead,
    cmap: CmapTable,
    glyf: Option<Vec<u8>>,
    hmtx: Vec<HorizontalMetric>,
    units_per_em: u16,
    ascender: i16,
    descender: i16,
    line_gap: i16,
    num_glyphs: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct FontHead {
    units_per_em: u16,
    x_min: i16,
    y_min: i16,
    x_max: i16,
    y_max: i16,
    mac_style: u16,
    lowest_rec_ppem: u16,
    index_to_loc_format: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontStyle { Normal, Italic, Oblique }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontWeight {
    Thin       = 100,
    ExtraLight = 200,
    Light      = 300,
    Regular    = 400,
    Medium     = 500,
    SemiBold   = 600,
    Bold       = 700,
    ExtraBold  = 800,
    Black      = 900,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintingMode { None, Light, Normal, Full }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubpixelRendering { None, Rgb, Bgr, VRgb, VBgr }

#[derive(Clone, Debug)]
pub struct GlyphMetrics {
    pub width: f32,
    pub height: f32,
    pub bearing_x: f32,
    pub bearing_y: f32,
    pub advance: f32,
}

pub struct RenderedGlyph {
    pub width: u32,
    pub height: u32,
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub advance: f32,
    pub bitmap: Vec<u8>,
    pub format: GlyphFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphFormat { Alpha8, Lcd, Mono }

pub struct GlyphCache {
    entries: BTreeMap<(u16, u32, u16), RenderedGlyph>,
    max_entries: usize,
    memory_used: usize,
    max_memory: usize,
}

#[derive(Clone, Debug)]
pub struct CmapTable {
    format: u16,
    entries: BTreeMap<u32, u16>,
}

#[derive(Clone, Copy, Debug)]
pub struct HorizontalMetric {
    advance_width: u16,
    left_side_bearing: i16,
}

#[derive(Clone, Debug)]
pub struct TextMetrics {
    pub width: f32,
    pub height: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
}

#[derive(Clone, Debug)]
pub struct GlyphPosition {
    pub codepoint: u32,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    pub glyph_index: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontError {
    InvalidData,
    UnsupportedFormat,
    GlyphNotFound,
    FontNotLoaded,
    CacheFull,
    InvalidFontId,
    TableMissing,
}

// ═══════════════════════════════════════════════════════════════════════════
// GlyphCache
// ═══════════════════════════════════════════════════════════════════════════

impl GlyphCache {
    pub fn new(max_entries: usize, max_memory: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            memory_used: 0,
            max_memory,
        }
    }

    pub fn get(&self, font_id: u16, codepoint: u32, size_x10: u16) -> Option<&RenderedGlyph> {
        self.entries.get(&(font_id, codepoint, size_x10))
    }

    pub fn insert(&mut self, font_id: u16, codepoint: u32, size_x10: u16, glyph: RenderedGlyph) {
        let mem = glyph.bitmap.len();

        while self.entries.len() >= self.max_entries || self.memory_used + mem > self.max_memory {
            if !self.evict_one() {
                break;
            }
        }

        self.memory_used += mem;
        self.entries.insert((font_id, codepoint, size_x10), glyph);
    }

    fn evict_one(&mut self) -> bool {
        let key = match self.entries.keys().next() {
            Some(k) => *k,
            None => return false,
        };
        if let Some(removed) = self.entries.remove(&key) {
            self.memory_used = self.memory_used.saturating_sub(removed.bitmap.len());
        }
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.memory_used = 0;
    }

    pub fn entry_count(&self) -> usize { self.entries.len() }
    pub fn memory_used(&self) -> usize { self.memory_used }
}

// ═══════════════════════════════════════════════════════════════════════════
// FontEngine
// ═══════════════════════════════════════════════════════════════════════════

impl FontEngine {
    pub fn new() -> Self {
        Self {
            loaded_fonts: Vec::new(),
            fallback_chain: Vec::new(),
            cache: GlyphCache::new(4096, 16 * 1024 * 1024),
            default_size: 16.0,
            hinting: HintingMode::Normal,
            subpixel: SubpixelRendering::None,
            dpi: 96.0,
        }
    }

    pub fn set_dpi(&mut self, dpi: f32) { self.dpi = dpi.max(72.0); }
    pub fn dpi(&self) -> f32 { self.dpi }

    pub fn set_hinting(&mut self, mode: HintingMode) { self.hinting = mode; }
    pub fn hinting(&self) -> HintingMode { self.hinting }

    pub fn set_subpixel(&mut self, mode: SubpixelRendering) { self.subpixel = mode; }
    pub fn subpixel(&self) -> SubpixelRendering { self.subpixel }

    pub fn set_default_size(&mut self, size: f32) { self.default_size = size.max(1.0); }
    pub fn default_size(&self) -> f32 { self.default_size }

    pub fn font_count(&self) -> usize { self.loaded_fonts.len() }

    pub fn load_font(&mut self, data: Vec<u8>) -> Result<usize, FontError> {
        if data.len() < 12 {
            return Err(FontError::InvalidData);
        }

        let head = Self::parse_ttf_header(&data)?;
        let cmap = Self::parse_cmap(&data)?;
        let num_glyphs = Self::read_u16(&data, 4).unwrap_or(0);
        let hmtx = Self::parse_hmtx(&data, num_glyphs);

        let (ascender, descender, line_gap) = Self::parse_hhea_metrics(&data);

        let name = Self::extract_name(&data).unwrap_or_else(|| String::from("Unknown"));
        let family = name.clone();

        let weight = if head.mac_style & 0x01 != 0 {
            FontWeight::Bold
        } else {
            FontWeight::Regular
        };
        let style = if head.mac_style & 0x02 != 0 {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };

        let font = LoadedFont {
            name,
            family,
            style,
            weight,
            data,
            head,
            cmap,
            glyf: None,
            hmtx,
            units_per_em: head.units_per_em,
            ascender,
            descender,
            line_gap,
            num_glyphs,
        };

        let id = self.loaded_fonts.len();
        self.loaded_fonts.push(font);
        self.fallback_chain.push(id);
        Ok(id)
    }

    pub fn set_size(&mut self, _font_id: usize, _size: f32) {
        // Size is passed per-call; this is a hint for default rendering.
    }

    pub fn glyph_index(&self, font_id: usize, codepoint: u32) -> Option<u16> {
        let font = self.loaded_fonts.get(font_id)?;
        font.cmap.entries.get(&codepoint).copied()
    }

    pub fn glyph_metrics(&self, font_id: usize, codepoint: u32, size: f32) -> Option<GlyphMetrics> {
        let font = self.loaded_fonts.get(font_id)?;
        let glyph_idx = font.cmap.entries.get(&codepoint).copied().unwrap_or(0);
        let scale = size / font.units_per_em as f32;

        let hm = font.hmtx.get(glyph_idx as usize).copied().unwrap_or(HorizontalMetric {
            advance_width: font.units_per_em / 2,
            left_side_bearing: 0,
        });

        Some(GlyphMetrics {
            width: hm.advance_width as f32 * scale,
            height: (font.ascender as f32 - font.descender as f32) * scale,
            bearing_x: hm.left_side_bearing as f32 * scale,
            bearing_y: font.ascender as f32 * scale,
            advance: hm.advance_width as f32 * scale,
        })
    }

    pub fn render_glyph(&mut self, font_id: usize, codepoint: u32, size: f32) -> Result<&RenderedGlyph, FontError> {
        if font_id >= self.loaded_fonts.len() {
            return Err(FontError::InvalidFontId);
        }

        let size_x10 = (size * 10.0) as u16;
        let cache_key = (font_id as u16, codepoint, size_x10);

        if self.cache.entries.contains_key(&cache_key) {
            return Ok(self.cache.entries.get(&cache_key).unwrap());
        }

        let font = &self.loaded_fonts[font_id];
        let scale = size / font.units_per_em as f32;
        let ppem = size * self.dpi / 72.0;

        let glyph_idx = font.cmap.entries.get(&codepoint).copied().unwrap_or(0);
        let hm = font.hmtx.get(glyph_idx as usize).copied().unwrap_or(HorizontalMetric {
            advance_width: font.units_per_em / 2,
            left_side_bearing: 0,
        });

        let w = (hm.advance_width as f32 * scale).ceil() as u32;
        let h = ((font.ascender as f32 - font.descender as f32) * scale).ceil() as u32;
        let w = w.max(1);
        let h = h.max(1);

        let bitmap = Self::rasterize_outline(&font.data, size, ppem, w, h);

        let glyph = RenderedGlyph {
            width: w,
            height: h,
            bearing_x: (hm.left_side_bearing as f32 * scale) as i32,
            bearing_y: (font.ascender as f32 * scale) as i32,
            advance: hm.advance_width as f32 * scale,
            bitmap,
            format: GlyphFormat::Alpha8,
        };

        self.cache.insert(font_id as u16, codepoint, size_x10, glyph);
        Ok(self.cache.entries.get(&cache_key).unwrap())
    }

    pub fn measure_text(&self, font_id: usize, text: &str, size: f32) -> TextMetrics {
        let font = match self.loaded_fonts.get(font_id) {
            Some(f) => f,
            None => return TextMetrics { width: 0.0, height: 0.0, ascent: 0.0, descent: 0.0, line_gap: 0.0 },
        };

        let scale = size / font.units_per_em as f32;
        let ascent = font.ascender as f32 * scale;
        let descent = font.descender as f32 * scale;
        let lg = font.line_gap as f32 * scale;

        let mut width = 0.0f32;
        let mut lines = 1u32;

        for c in text.chars() {
            if c == '\n' {
                lines += 1;
                continue;
            }
            let glyph_idx = font.cmap.entries.get(&(c as u32)).copied().unwrap_or(0);
            let hm = font.hmtx.get(glyph_idx as usize).copied().unwrap_or(HorizontalMetric {
                advance_width: font.units_per_em / 2,
                left_side_bearing: 0,
            });
            width += hm.advance_width as f32 * scale;
        }

        let line_height = ascent - descent + lg;
        TextMetrics {
            width,
            height: line_height * lines as f32,
            ascent,
            descent,
            line_gap: lg,
        }
    }

    pub fn layout_text(&self, font_id: usize, text: &str, size: f32, max_width: Option<f32>) -> Vec<GlyphPosition> {
        let font = match self.loaded_fonts.get(font_id) {
            Some(f) => f,
            None => return Vec::new(),
        };

        let scale = size / font.units_per_em as f32;
        let line_height = (font.ascender as f32 - font.descender as f32 + font.line_gap as f32) * scale;
        let ascent = font.ascender as f32 * scale;

        let mut positions = Vec::new();
        let mut x = 0.0f32;
        let mut y = ascent;
        let mut last_space_idx: Option<usize> = None;
        let mut last_space_x = 0.0f32;

        for c in text.chars() {
            if c == '\n' {
                x = 0.0;
                y += line_height;
                last_space_idx = None;
                continue;
            }

            let cp = c as u32;
            let glyph_idx = font.cmap.entries.get(&cp).copied().unwrap_or(0);
            let hm = font.hmtx.get(glyph_idx as usize).copied().unwrap_or(HorizontalMetric {
                advance_width: font.units_per_em / 2,
                left_side_bearing: 0,
            });
            let advance = hm.advance_width as f32 * scale;

            if c == ' ' {
                last_space_idx = Some(positions.len());
                last_space_x = x;
            }

            if let Some(mw) = max_width {
                if x + advance > mw && x > 0.0 {
                    if let Some(space_idx) = last_space_idx {
                        // Re-flow from the last space
                        let reflow_x = last_space_x;
                        for gp in &mut positions[space_idx..] {
                            gp.x -= reflow_x;
                            gp.y += line_height;
                        }
                        x -= reflow_x;
                        y += line_height;
                        last_space_idx = None;
                    } else {
                        x = 0.0;
                        y += line_height;
                    }
                }
            }

            positions.push(GlyphPosition {
                codepoint: cp,
                x,
                y,
                advance,
                glyph_index: glyph_idx,
            });
            x += advance;
        }

        positions
    }

    pub fn render_text_to_canvas(
        &mut self,
        canvas: &mut super::Canvas,
        font_id: usize,
        text: &str,
        x: i32,
        y: i32,
        size: f32,
        color: u32,
    ) {
        let positions = self.layout_text(font_id, text, size, None);

        let a_src = ((color >> 24) & 0xFF) as u32;
        let r_src = ((color >> 16) & 0xFF) as u32;
        let g_src = ((color >> 8) & 0xFF) as u32;
        let b_src = (color & 0xFF) as u32;

        for gp in &positions {
            let glyph = match self.render_glyph(font_id, gp.codepoint, size) {
                Ok(g) => g,
                Err(_) => continue,
            };

            let gx = x + gp.x as i32 + glyph.bearing_x;
            let gy = y + gp.y as i32 - glyph.bearing_y;

            for row in 0..glyph.height {
                for col in 0..glyph.width {
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

                    let alpha = glyph.bitmap[(row * glyph.width + col) as usize] as u32;
                    if alpha == 0 {
                        continue;
                    }

                    let a = (a_src * alpha) / 255;
                    let r = (r_src * alpha) / 255;
                    let g = (g_src * alpha) / 255;
                    let b = (b_src * alpha) / 255;
                    let pixel = (a << 24) | (r << 16) | (g << 8) | b;
                    canvas.draw_pixel(px, py, pixel);
                }
            }
        }
    }

    pub fn find_font_for_char(&self, c: char) -> Option<usize> {
        let cp = c as u32;
        for &idx in &self.fallback_chain {
            if let Some(font) = self.loaded_fonts.get(idx) {
                if font.cmap.entries.contains_key(&cp) {
                    return Some(idx);
                }
            }
        }
        None
    }

    pub fn set_fallback_chain(&mut self, chain: Vec<usize>) {
        self.fallback_chain = chain;
    }

    pub fn font_name(&self, font_id: usize) -> Option<&str> {
        self.loaded_fonts.get(font_id).map(|f| f.name.as_str())
    }

    pub fn font_family(&self, font_id: usize) -> Option<&str> {
        self.loaded_fonts.get(font_id).map(|f| f.family.as_str())
    }

    pub fn font_style(&self, font_id: usize) -> Option<FontStyle> {
        self.loaded_fonts.get(font_id).map(|f| f.style)
    }

    pub fn font_weight(&self, font_id: usize) -> Option<FontWeight> {
        self.loaded_fonts.get(font_id).map(|f| f.weight)
    }

    pub fn cache_stats(&self) -> (usize, usize) {
        (self.cache.entry_count(), self.cache.memory_used())
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    // ── Internal TrueType parsing ────────────────────────────────────────

    fn parse_ttf_header(data: &[u8]) -> Result<FontHead, FontError> {
        if data.len() < 54 {
            return Err(FontError::InvalidData);
        }

        // Scan the table directory for the 'head' table
        let num_tables = Self::read_u16(data, 4).ok_or(FontError::InvalidData)? as usize;
        let mut head_offset: Option<usize> = None;

        for i in 0..num_tables {
            let entry_off = 12 + i * 16;
            if entry_off + 16 > data.len() {
                break;
            }
            let tag = &data[entry_off..entry_off + 4];
            if tag == b"head" {
                head_offset = Self::read_u32(data, entry_off + 8).map(|v| v as usize);
                break;
            }
        }

        let off = head_offset.unwrap_or(0);
        if off + 54 > data.len() {
            return Ok(FontHead {
                units_per_em: 1000,
                x_min: 0, y_min: 0, x_max: 1000, y_max: 1000,
                mac_style: 0,
                lowest_rec_ppem: 8,
                index_to_loc_format: 0,
            });
        }

        let units_per_em = Self::read_u16(data, off + 18).unwrap_or(1000);
        let x_min = Self::read_i16(data, off + 36).unwrap_or(0);
        let y_min = Self::read_i16(data, off + 38).unwrap_or(0);
        let x_max = Self::read_i16(data, off + 40).unwrap_or(1000);
        let y_max = Self::read_i16(data, off + 42).unwrap_or(1000);
        let mac_style = Self::read_u16(data, off + 44).unwrap_or(0);
        let lowest_rec_ppem = Self::read_u16(data, off + 46).unwrap_or(8);
        let index_to_loc_format = Self::read_i16(data, off + 50).unwrap_or(0);

        Ok(FontHead {
            units_per_em: units_per_em.max(1),
            x_min, y_min, x_max, y_max,
            mac_style,
            lowest_rec_ppem,
            index_to_loc_format,
        })
    }

    fn parse_cmap(data: &[u8]) -> Result<CmapTable, FontError> {
        let num_tables = Self::read_u16(data, 4).unwrap_or(0) as usize;
        let mut cmap_offset: Option<usize> = None;

        for i in 0..num_tables {
            let entry_off = 12 + i * 16;
            if entry_off + 16 > data.len() { break; }
            if &data[entry_off..entry_off + 4] == b"cmap" {
                cmap_offset = Self::read_u32(data, entry_off + 8).map(|v| v as usize);
                break;
            }
        }

        let off = match cmap_offset {
            Some(o) if o + 4 <= data.len() => o,
            _ => {
                return Ok(CmapTable { format: 0, entries: BTreeMap::new() });
            }
        };

        let num_subtables = Self::read_u16(data, off + 2).unwrap_or(0) as usize;
        let mut best_subtable_offset: Option<usize> = None;

        for i in 0..num_subtables {
            let rec = off + 4 + i * 8;
            if rec + 8 > data.len() { break; }
            let platform_id = Self::read_u16(data, rec).unwrap_or(0);
            let _encoding_id = Self::read_u16(data, rec + 2).unwrap_or(0);
            let sub_off = Self::read_u32(data, rec + 4).unwrap_or(0) as usize;

            // Prefer Unicode (platform 0 or 3)
            if platform_id == 0 || platform_id == 3 {
                best_subtable_offset = Some(off + sub_off);
                break;
            }
            if best_subtable_offset.is_none() {
                best_subtable_offset = Some(off + sub_off);
            }
        }

        let sub_off = match best_subtable_offset {
            Some(o) if o + 2 <= data.len() => o,
            _ => return Ok(CmapTable { format: 0, entries: BTreeMap::new() }),
        };

        let format = Self::read_u16(data, sub_off).unwrap_or(0);
        let mut entries = BTreeMap::new();

        match format {
            0 => {
                // Format 0: byte encoding table
                if sub_off + 262 <= data.len() {
                    for i in 0u32..256 {
                        let glyph = data[sub_off + 6 + i as usize];
                        if glyph != 0 {
                            entries.insert(i, glyph as u16);
                        }
                    }
                }
            }
            4 => {
                // Format 4: segment mapping to delta values
                if sub_off + 14 <= data.len() {
                    let seg_count = (Self::read_u16(data, sub_off + 6).unwrap_or(0) / 2) as usize;
                    let end_base = sub_off + 14;
                    let start_base = end_base + seg_count * 2 + 2;
                    let delta_base = start_base + seg_count * 2;
                    let range_base = delta_base + seg_count * 2;

                    for i in 0..seg_count {
                        let end_code = Self::read_u16(data, end_base + i * 2).unwrap_or(0) as u32;
                        let start_code = Self::read_u16(data, start_base + i * 2).unwrap_or(0) as u32;
                        let delta = Self::read_i16(data, delta_base + i * 2).unwrap_or(0) as i32;
                        let range_off = Self::read_u16(data, range_base + i * 2).unwrap_or(0);

                        if start_code == 0xFFFF {
                            break;
                        }

                        for cp in start_code..=end_code {
                            let glyph_id = if range_off == 0 {
                                ((cp as i32 + delta) & 0xFFFF) as u16
                            } else {
                                let offset_idx = range_base + i * 2 + range_off as usize
                                    + (cp - start_code) as usize * 2;
                                if offset_idx + 2 <= data.len() {
                                    let gid = Self::read_u16(data, offset_idx).unwrap_or(0);
                                    if gid != 0 {
                                        ((gid as i32 + delta) & 0xFFFF) as u16
                                    } else {
                                        0
                                    }
                                } else {
                                    0
                                }
                            };
                            if glyph_id != 0 {
                                entries.insert(cp, glyph_id);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        Ok(CmapTable { format, entries })
    }

    fn parse_hmtx(data: &[u8], num_glyphs: u16) -> Vec<HorizontalMetric> {
        let num_tables = Self::read_u16(data, 4).unwrap_or(0) as usize;
        let mut hhea_offset: Option<usize> = None;
        let mut hmtx_offset: Option<usize> = None;

        for i in 0..num_tables {
            let entry_off = 12 + i * 16;
            if entry_off + 16 > data.len() { break; }
            let tag = &data[entry_off..entry_off + 4];
            let off = Self::read_u32(data, entry_off + 8).map(|v| v as usize);
            if tag == b"hhea" { hhea_offset = off; }
            if tag == b"hmtx" { hmtx_offset = off; }
        }

        let num_h_metrics = hhea_offset
            .and_then(|off| Self::read_u16(data, off + 34))
            .unwrap_or(num_glyphs) as usize;

        let hmtx_off = match hmtx_offset {
            Some(o) => o,
            None => return Vec::new(),
        };

        let mut metrics = Vec::with_capacity(num_glyphs as usize);
        for i in 0..num_h_metrics {
            let base = hmtx_off + i * 4;
            let aw = Self::read_u16(data, base).unwrap_or(0);
            let lsb = Self::read_i16(data, base + 2).unwrap_or(0);
            metrics.push(HorizontalMetric { advance_width: aw, left_side_bearing: lsb });
        }

        // Remaining glyphs share the last advance width
        let last_aw = metrics.last().map(|m| m.advance_width).unwrap_or(0);
        let lsb_base = hmtx_off + num_h_metrics * 4;
        for i in num_h_metrics..(num_glyphs as usize) {
            let off = lsb_base + (i - num_h_metrics) * 2;
            let lsb = Self::read_i16(data, off).unwrap_or(0);
            metrics.push(HorizontalMetric { advance_width: last_aw, left_side_bearing: lsb });
        }

        metrics
    }

    fn parse_hhea_metrics(data: &[u8]) -> (i16, i16, i16) {
        let num_tables = Self::read_u16(data, 4).unwrap_or(0) as usize;
        for i in 0..num_tables {
            let entry_off = 12 + i * 16;
            if entry_off + 16 > data.len() { break; }
            if &data[entry_off..entry_off + 4] == b"hhea" {
                let off = Self::read_u32(data, entry_off + 8).unwrap_or(0) as usize;
                if off + 10 <= data.len() {
                    let ascender = Self::read_i16(data, off + 4).unwrap_or(800);
                    let descender = Self::read_i16(data, off + 6).unwrap_or(-200);
                    let line_gap = Self::read_i16(data, off + 8).unwrap_or(0);
                    return (ascender, descender, line_gap);
                }
            }
        }
        (800, -200, 0)
    }

    fn extract_name(data: &[u8]) -> Option<String> {
        let num_tables = Self::read_u16(data, 4)? as usize;
        for i in 0..num_tables {
            let entry_off = 12 + i * 16;
            if entry_off + 16 > data.len() { break; }
            if &data[entry_off..entry_off + 4] == b"name" {
                let off = Self::read_u32(data, entry_off + 8)? as usize;
                if off + 6 > data.len() { return None; }
                let count = Self::read_u16(data, off + 2)? as usize;
                let string_offset = Self::read_u16(data, off + 4)? as usize;

                for j in 0..count {
                    let rec = off + 6 + j * 12;
                    if rec + 12 > data.len() { break; }
                    let name_id = Self::read_u16(data, rec + 6)?;
                    if name_id == 4 {
                        let length = Self::read_u16(data, rec + 8)? as usize;
                        let str_off = off + string_offset + Self::read_u16(data, rec + 10)? as usize;
                        if str_off + length <= data.len() {
                            let bytes = &data[str_off..str_off + length];
                            let name: String = bytes.iter()
                                .filter(|&&b| b >= 0x20 && b < 0x7F)
                                .map(|&b| b as char)
                                .collect();
                            if !name.is_empty() {
                                return Some(name);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn rasterize_outline(_glyph_data: &[u8], size: f32, _ppem: f32, w: u32, h: u32) -> Vec<u8> {
        // Simplified rasterizer: produces a rectangle approximation.
        // A full implementation would parse TrueType contour data and scan-convert.
        let mut bitmap = alloc::vec![0u8; (w * h) as usize];

        let margin_x = (w as f32 * 0.15) as u32;
        let margin_y = (h as f32 * 0.10) as u32;
        let intensity = (size * 8.0).min(255.0) as u8;

        for y in margin_y..h.saturating_sub(margin_y) {
            for x in margin_x..w.saturating_sub(margin_x) {
                let idx = (y * w + x) as usize;
                if idx < bitmap.len() {
                    bitmap[idx] = intensity;
                }
            }
        }
        bitmap
    }

    // ── Binary reading helpers ───────────────────────────────────────────

    #[inline]
    fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
        if offset + 2 > data.len() { return None; }
        Some(u16::from_be_bytes([data[offset], data[offset + 1]]))
    }

    #[inline]
    fn read_i16(data: &[u8], offset: usize) -> Option<i16> {
        Self::read_u16(data, offset).map(|v| v as i16)
    }

    #[inline]
    fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
        if offset + 4 > data.len() { return None; }
        Some(u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// LoadedFont accessors
// ═══════════════════════════════════════════════════════════════════════════

impl LoadedFont {
    pub fn name(&self) -> &str { &self.name }
    pub fn family(&self) -> &str { &self.family }
    pub fn style(&self) -> FontStyle { self.style }
    pub fn weight(&self) -> FontWeight { self.weight }
    pub fn units_per_em(&self) -> u16 { self.units_per_em }
    pub fn ascender(&self) -> i16 { self.ascender }
    pub fn descender(&self) -> i16 { self.descender }
    pub fn line_gap(&self) -> i16 { self.line_gap }
    pub fn num_glyphs(&self) -> u16 { self.num_glyphs }

    pub fn has_glyph(&self, codepoint: u32) -> bool {
        self.cmap.entries.contains_key(&codepoint)
    }

    pub fn supported_codepoints(&self) -> Vec<u32> {
        self.cmap.entries.keys().copied().collect()
    }

    pub fn is_bold(&self) -> bool { self.head.mac_style & 0x01 != 0 }
    pub fn is_italic(&self) -> bool { self.head.mac_style & 0x02 != 0 }
}
