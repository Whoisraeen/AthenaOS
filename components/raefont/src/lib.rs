// `#![no_std]` for the kernel/userspace build; `std` under `cargo test` so the
// rasterizer host KATs can link (the rae_tokens pattern — see CLAUDE.md §15 and
// docs/design/typography-rendering.md S1).
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// no_std float helpers
// ═══════════════════════════════════════════════════════════════════════════

fn f32_floor(x: f32) -> f32 {
    let i = x as i32;
    if (i as f32) > x {
        (i - 1) as f32
    } else {
        i as f32
    }
}

fn f32_ceil(x: f32) -> f32 {
    let i = x as i32;
    if (i as f32) < x {
        (i + 1) as f32
    } else {
        i as f32
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Binary reader helpers
// ═══════════════════════════════════════════════════════════════════════════

struct BinaryReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BinaryReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    fn read_u8(&mut self) -> Option<u8> {
        if self.pos < self.data.len() {
            let v = self.data[self.pos];
            self.pos += 1;
            Some(v)
        } else {
            None
        }
    }

    fn read_i8(&mut self) -> Option<i8> {
        self.read_u8().map(|v| v as i8)
    }

    fn read_u16(&mut self) -> Option<u16> {
        if self.pos + 2 <= self.data.len() {
            let v = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
            self.pos += 2;
            Some(v)
        } else {
            None
        }
    }

    fn read_i16(&mut self) -> Option<i16> {
        self.read_u16().map(|v| v as i16)
    }

    fn read_u32(&mut self) -> Option<u32> {
        if self.pos + 4 <= self.data.len() {
            let v = u32::from_be_bytes([
                self.data[self.pos],
                self.data[self.pos + 1],
                self.data[self.pos + 2],
                self.data[self.pos + 3],
            ]);
            self.pos += 4;
            Some(v)
        } else {
            None
        }
    }

    fn read_i32(&mut self) -> Option<i32> {
        self.read_u32().map(|v| v as i32)
    }

    fn read_u64(&mut self) -> Option<u64> {
        if self.pos + 8 <= self.data.len() {
            let v = u64::from_be_bytes([
                self.data[self.pos],
                self.data[self.pos + 1],
                self.data[self.pos + 2],
                self.data[self.pos + 3],
                self.data[self.pos + 4],
                self.data[self.pos + 5],
                self.data[self.pos + 6],
                self.data[self.pos + 7],
            ]);
            self.pos += 8;
            Some(v)
        } else {
            None
        }
    }

    fn read_tag(&mut self) -> Option<[u8; 4]> {
        if self.pos + 4 <= self.data.len() {
            let tag = [
                self.data[self.pos],
                self.data[self.pos + 1],
                self.data[self.pos + 2],
                self.data[self.pos + 3],
            ];
            self.pos += 4;
            Some(tag)
        } else {
            None
        }
    }

    fn read_bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.pos + n <= self.data.len() {
            let slice = &self.data[self.pos..self.pos + n];
            self.pos += n;
            Some(slice)
        } else {
            None
        }
    }

    fn read_fixed(&mut self) -> Option<Fixed> {
        self.read_i32().map(|v| Fixed(v))
    }

    fn read_f2dot14(&mut self) -> Option<f32> {
        self.read_i16().map(|v| v as f32 / 16384.0)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Fixed-point types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixed(pub i32);

impl Fixed {
    pub fn from_i32(v: i32) -> Self {
        Fixed(v << 16)
    }
    pub fn to_f32(self) -> f32 {
        self.0 as f32 / 65536.0
    }
    pub fn integer(self) -> i32 {
        self.0 >> 16
    }
    pub fn fraction(self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }
    pub fn mul(self, other: Fixed) -> Fixed {
        Fixed(((self.0 as i64 * other.0 as i64) >> 16) as i32)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Font table tags
// ═══════════════════════════════════════════════════════════════════════════

pub const TAG_CMAP: [u8; 4] = *b"cmap";
pub const TAG_GLYF: [u8; 4] = *b"glyf";
pub const TAG_HEAD: [u8; 4] = *b"head";
pub const TAG_HHEA: [u8; 4] = *b"hhea";
pub const TAG_HMTX: [u8; 4] = *b"hmtx";
pub const TAG_LOCA: [u8; 4] = *b"loca";
pub const TAG_MAXP: [u8; 4] = *b"maxp";
pub const TAG_NAME: [u8; 4] = *b"name";
pub const TAG_POST: [u8; 4] = *b"post";
pub const TAG_KERN: [u8; 4] = *b"kern";
pub const TAG_GPOS: [u8; 4] = *b"GPOS";
pub const TAG_GSUB: [u8; 4] = *b"GSUB";
pub const TAG_GDEF: [u8; 4] = *b"GDEF";
pub const TAG_OS2: [u8; 4] = *b"OS/2";
pub const TAG_CVT: [u8; 4] = *b"cvt ";
pub const TAG_FPGM: [u8; 4] = *b"fpgm";
pub const TAG_PREP: [u8; 4] = *b"prep";
pub const TAG_GASP: [u8; 4] = *b"gasp";
pub const TAG_DSIG: [u8; 4] = *b"DSIG";
pub const TAG_FVAR: [u8; 4] = *b"fvar";
pub const TAG_AVAR: [u8; 4] = *b"avar";
pub const TAG_GVAR: [u8; 4] = *b"gvar";
pub const TAG_HVAR: [u8; 4] = *b"HVAR";
pub const TAG_VVAR: [u8; 4] = *b"VVAR";
pub const TAG_MVAR: [u8; 4] = *b"MVAR";
pub const TAG_STAT: [u8; 4] = *b"STAT";
pub const TAG_COLR: [u8; 4] = *b"COLR";
pub const TAG_CPAL: [u8; 4] = *b"CPAL";
pub const TAG_CBDT: [u8; 4] = *b"CBDT";
pub const TAG_CBLC: [u8; 4] = *b"CBLC";
pub const TAG_SBIX: [u8; 4] = *b"sbix";
pub const TAG_SVG: [u8; 4] = *b"SVG ";
pub const TAG_EBDT: [u8; 4] = *b"EBDT";
pub const TAG_EBLC: [u8; 4] = *b"EBLC";
pub const TAG_EBSC: [u8; 4] = *b"EBSC";

// ═══════════════════════════════════════════════════════════════════════════
// sfnt container — offset table & table directory
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfntVersion {
    TrueType,
    Cff,
    TrueTypeCollection,
}

#[derive(Debug, Clone)]
pub struct TableRecord {
    pub tag: [u8; 4],
    pub checksum: u32,
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug, Clone)]
pub struct OffsetTable {
    pub sfnt_version: SfntVersion,
    pub num_tables: u16,
    pub search_range: u16,
    pub entry_selector: u16,
    pub range_shift: u16,
    pub records: Vec<TableRecord>,
}

impl OffsetTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let version_tag = r.read_u32()?;
        let sfnt_version = match version_tag {
            0x00010000 => SfntVersion::TrueType,
            0x4F54544F => SfntVersion::Cff,
            0x74746366 => SfntVersion::TrueTypeCollection,
            _ => return None,
        };
        let num_tables = r.read_u16()?;
        let search_range = r.read_u16()?;
        let entry_selector = r.read_u16()?;
        let range_shift = r.read_u16()?;
        let mut records = Vec::with_capacity(num_tables as usize);
        for _ in 0..num_tables {
            records.push(TableRecord {
                tag: r.read_tag()?,
                checksum: r.read_u32()?,
                offset: r.read_u32()?,
                length: r.read_u32()?,
            });
        }
        Some(OffsetTable {
            sfnt_version,
            num_tables,
            search_range,
            entry_selector,
            range_shift,
            records,
        })
    }

    pub fn find_table(&self, tag: &[u8; 4]) -> Option<&TableRecord> {
        self.records.iter().find(|r| &r.tag == tag)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// TrueType Collection (TTC)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct TtcHeader {
    pub major_version: u16,
    pub minor_version: u16,
    pub num_fonts: u32,
    pub offsets: Vec<u32>,
    pub dsig_tag: Option<u32>,
    pub dsig_length: Option<u32>,
    pub dsig_offset: Option<u32>,
}

impl TtcHeader {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let tag = r.read_tag()?;
        if &tag != b"ttcf" {
            return None;
        }
        let major_version = r.read_u16()?;
        let minor_version = r.read_u16()?;
        let num_fonts = r.read_u32()?;
        let mut offsets = Vec::with_capacity(num_fonts as usize);
        for _ in 0..num_fonts {
            offsets.push(r.read_u32()?);
        }
        let (dsig_tag, dsig_length, dsig_offset) = if major_version >= 2 {
            (
                Some(r.read_u32()?),
                Some(r.read_u32()?),
                Some(r.read_u32()?),
            )
        } else {
            (None, None, None)
        };
        Some(TtcHeader {
            major_version,
            minor_version,
            num_fonts,
            offsets,
            dsig_tag,
            dsig_length,
            dsig_offset,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// head table
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct HeadTable {
    pub major_version: u16,
    pub minor_version: u16,
    pub font_revision: Fixed,
    pub checksum_adjustment: u32,
    pub magic_number: u32,
    pub flags: u16,
    pub units_per_em: u16,
    pub created: i64,
    pub modified: i64,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    pub mac_style: u16,
    pub lowest_rec_ppem: u16,
    pub font_direction_hint: i16,
    pub index_to_loc_format: i16,
    pub glyph_data_format: i16,
}

impl HeadTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        Some(HeadTable {
            major_version: r.read_u16()?,
            minor_version: r.read_u16()?,
            font_revision: r.read_fixed()?,
            checksum_adjustment: r.read_u32()?,
            magic_number: r.read_u32()?,
            flags: r.read_u16()?,
            units_per_em: r.read_u16()?,
            created: r.read_u64()? as i64,
            modified: r.read_u64()? as i64,
            x_min: r.read_i16()?,
            y_min: r.read_i16()?,
            x_max: r.read_i16()?,
            y_max: r.read_i16()?,
            mac_style: r.read_u16()?,
            lowest_rec_ppem: r.read_u16()?,
            font_direction_hint: r.read_i16()?,
            index_to_loc_format: r.read_i16()?,
            glyph_data_format: r.read_i16()?,
        })
    }

    pub fn is_long_loca(&self) -> bool {
        self.index_to_loc_format == 1
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// hhea table — horizontal header
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct HheaTable {
    pub major_version: u16,
    pub minor_version: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub advance_width_max: u16,
    pub min_left_side_bearing: i16,
    pub min_right_side_bearing: i16,
    pub x_max_extent: i16,
    pub caret_slope_rise: i16,
    pub caret_slope_run: i16,
    pub caret_offset: i16,
    pub metric_data_format: i16,
    pub number_of_h_metrics: u16,
}

impl HheaTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let major_version = r.read_u16()?;
        let minor_version = r.read_u16()?;
        let ascender = r.read_i16()?;
        let descender = r.read_i16()?;
        let line_gap = r.read_i16()?;
        let advance_width_max = r.read_u16()?;
        let min_left_side_bearing = r.read_i16()?;
        let min_right_side_bearing = r.read_i16()?;
        let x_max_extent = r.read_i16()?;
        let caret_slope_rise = r.read_i16()?;
        let caret_slope_run = r.read_i16()?;
        let caret_offset = r.read_i16()?;
        r.skip(8); // reserved
        let metric_data_format = r.read_i16()?;
        let number_of_h_metrics = r.read_u16()?;
        Some(HheaTable {
            major_version,
            minor_version,
            ascender,
            descender,
            line_gap,
            advance_width_max,
            min_left_side_bearing,
            min_right_side_bearing,
            x_max_extent,
            caret_slope_rise,
            caret_slope_run,
            caret_offset,
            metric_data_format,
            number_of_h_metrics,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// hmtx table — horizontal metrics
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct HMetric {
    pub advance_width: u16,
    pub lsb: i16,
}

#[derive(Debug, Clone)]
pub struct HmtxTable {
    pub h_metrics: Vec<HMetric>,
    pub left_side_bearings: Vec<i16>,
}

impl HmtxTable {
    pub fn parse(data: &[u8], num_h_metrics: u16, num_glyphs: u16) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let mut h_metrics = Vec::with_capacity(num_h_metrics as usize);
        for _ in 0..num_h_metrics {
            h_metrics.push(HMetric {
                advance_width: r.read_u16()?,
                lsb: r.read_i16()?,
            });
        }
        let remaining = (num_glyphs as usize).saturating_sub(num_h_metrics as usize);
        let mut left_side_bearings = Vec::with_capacity(remaining);
        for _ in 0..remaining {
            left_side_bearings.push(r.read_i16()?);
        }
        Some(HmtxTable {
            h_metrics,
            left_side_bearings,
        })
    }

    pub fn advance_width(&self, glyph_id: u16) -> u16 {
        if (glyph_id as usize) < self.h_metrics.len() {
            self.h_metrics[glyph_id as usize].advance_width
        } else {
            self.h_metrics.last().map_or(0, |m| m.advance_width)
        }
    }

    pub fn left_side_bearing(&self, glyph_id: u16) -> i16 {
        if (glyph_id as usize) < self.h_metrics.len() {
            self.h_metrics[glyph_id as usize].lsb
        } else {
            let idx = (glyph_id as usize) - self.h_metrics.len();
            self.left_side_bearings.get(idx).copied().unwrap_or(0)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// maxp table
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct MaxpTable {
    pub version: u32,
    pub num_glyphs: u16,
    pub max_points: u16,
    pub max_contours: u16,
    pub max_composite_points: u16,
    pub max_composite_contours: u16,
    pub max_zones: u16,
    pub max_twilight_points: u16,
    pub max_storage: u16,
    pub max_function_defs: u16,
    pub max_instruction_defs: u16,
    pub max_stack_elements: u16,
    pub max_size_of_instructions: u16,
    pub max_component_elements: u16,
    pub max_component_depth: u16,
}

impl MaxpTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let version = r.read_u32()?;
        let num_glyphs = r.read_u16()?;
        if version == 0x00005000 {
            return Some(MaxpTable {
                version,
                num_glyphs,
                max_points: 0,
                max_contours: 0,
                max_composite_points: 0,
                max_composite_contours: 0,
                max_zones: 0,
                max_twilight_points: 0,
                max_storage: 0,
                max_function_defs: 0,
                max_instruction_defs: 0,
                max_stack_elements: 0,
                max_size_of_instructions: 0,
                max_component_elements: 0,
                max_component_depth: 0,
            });
        }
        Some(MaxpTable {
            version,
            num_glyphs,
            max_points: r.read_u16()?,
            max_contours: r.read_u16()?,
            max_composite_points: r.read_u16()?,
            max_composite_contours: r.read_u16()?,
            max_zones: r.read_u16()?,
            max_twilight_points: r.read_u16()?,
            max_storage: r.read_u16()?,
            max_function_defs: r.read_u16()?,
            max_instruction_defs: r.read_u16()?,
            max_stack_elements: r.read_u16()?,
            max_size_of_instructions: r.read_u16()?,
            max_component_elements: r.read_u16()?,
            max_component_depth: r.read_u16()?,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// loca table — glyph offsets index
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct LocaTable {
    pub offsets: Vec<u32>,
}

impl LocaTable {
    pub fn parse(data: &[u8], num_glyphs: u16, is_long: bool) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let count = num_glyphs as usize + 1;
        let mut offsets = Vec::with_capacity(count);
        if is_long {
            for _ in 0..count {
                offsets.push(r.read_u32()?);
            }
        } else {
            for _ in 0..count {
                offsets.push(r.read_u16()? as u32 * 2);
            }
        }
        Some(LocaTable { offsets })
    }

    pub fn glyph_range(&self, glyph_id: u16) -> Option<(u32, u32)> {
        let i = glyph_id as usize;
        if i + 1 < self.offsets.len() {
            let start = self.offsets[i];
            let end = self.offsets[i + 1];
            if start < end {
                Some((start, end))
            } else {
                None
            }
        } else {
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// cmap table — character-to-glyph mapping
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct CmapEncodingRecord {
    pub platform_id: u16,
    pub encoding_id: u16,
    pub subtable_offset: u32,
}

#[derive(Debug, Clone)]
pub enum CmapSubtable {
    Format0 {
        glyph_ids: [u8; 256],
    },
    Format4 {
        seg_count: u16,
        end_codes: Vec<u16>,
        start_codes: Vec<u16>,
        id_deltas: Vec<i16>,
        id_range_offsets: Vec<u16>,
        glyph_ids: Vec<u16>,
    },
    Format6 {
        first_code: u16,
        entry_count: u16,
        glyph_ids: Vec<u16>,
    },
    Format12 {
        groups: Vec<SequentialMapGroup>,
    },
    Format14 {
        var_selectors: Vec<VariationSelector>,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct SequentialMapGroup {
    pub start_char_code: u32,
    pub end_char_code: u32,
    pub start_glyph_id: u32,
}

#[derive(Debug, Clone)]
pub struct VariationSelector {
    pub var_selector: u32,
    pub default_uvs_offset: u32,
    pub non_default_uvs_offset: u32,
}

#[derive(Debug, Clone)]
pub struct CmapTable {
    pub version: u16,
    pub encoding_records: Vec<CmapEncodingRecord>,
    pub subtables: Vec<CmapSubtable>,
}

impl CmapTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let version = r.read_u16()?;
        let num_tables = r.read_u16()?;
        let mut encoding_records = Vec::with_capacity(num_tables as usize);
        for _ in 0..num_tables {
            encoding_records.push(CmapEncodingRecord {
                platform_id: r.read_u16()?,
                encoding_id: r.read_u16()?,
                subtable_offset: r.read_u32()?,
            });
        }
        let mut subtables = Vec::new();
        for rec in &encoding_records {
            if let Some(st) = Self::parse_subtable(data, rec.subtable_offset as usize) {
                subtables.push(st);
            }
        }
        Some(CmapTable {
            version,
            encoding_records,
            subtables,
        })
    }

    fn parse_subtable(data: &[u8], offset: usize) -> Option<CmapSubtable> {
        let mut r = BinaryReader::new(data);
        r.seek(offset);
        let format = r.read_u16()?;
        match format {
            0 => {
                let _length = r.read_u16()?;
                let _language = r.read_u16()?;
                let mut glyph_ids = [0u8; 256];
                let bytes = r.read_bytes(256)?;
                glyph_ids.copy_from_slice(bytes);
                Some(CmapSubtable::Format0 { glyph_ids })
            }
            4 => {
                let length = r.read_u16()?;
                let _language = r.read_u16()?;
                let seg_count_x2 = r.read_u16()?;
                let seg_count = seg_count_x2 / 2;
                let _search_range = r.read_u16()?;
                let _entry_selector = r.read_u16()?;
                let _range_shift = r.read_u16()?;
                let mut end_codes = Vec::with_capacity(seg_count as usize);
                for _ in 0..seg_count {
                    end_codes.push(r.read_u16()?);
                }
                let _reserved_pad = r.read_u16()?;
                let mut start_codes = Vec::with_capacity(seg_count as usize);
                for _ in 0..seg_count {
                    start_codes.push(r.read_u16()?);
                }
                let mut id_deltas = Vec::with_capacity(seg_count as usize);
                for _ in 0..seg_count {
                    id_deltas.push(r.read_i16()?);
                }
                let range_offset_start = r.pos;
                let mut id_range_offsets = Vec::with_capacity(seg_count as usize);
                for _ in 0..seg_count {
                    id_range_offsets.push(r.read_u16()?);
                }
                let remaining_bytes = (length as usize + offset).saturating_sub(r.pos);
                let glyph_count = remaining_bytes / 2;
                let mut glyph_ids = Vec::with_capacity(glyph_count);
                for _ in 0..glyph_count {
                    glyph_ids.push(r.read_u16().unwrap_or(0));
                }
                Some(CmapSubtable::Format4 {
                    seg_count,
                    end_codes,
                    start_codes,
                    id_deltas,
                    id_range_offsets,
                    glyph_ids,
                })
            }
            6 => {
                let _length = r.read_u16()?;
                let _language = r.read_u16()?;
                let first_code = r.read_u16()?;
                let entry_count = r.read_u16()?;
                let mut glyph_ids = Vec::with_capacity(entry_count as usize);
                for _ in 0..entry_count {
                    glyph_ids.push(r.read_u16()?);
                }
                Some(CmapSubtable::Format6 {
                    first_code,
                    entry_count,
                    glyph_ids,
                })
            }
            12 => {
                let _reserved = r.read_u16()?;
                let _length = r.read_u32()?;
                let _language = r.read_u32()?;
                let num_groups = r.read_u32()?;
                let mut groups = Vec::with_capacity(num_groups as usize);
                for _ in 0..num_groups {
                    groups.push(SequentialMapGroup {
                        start_char_code: r.read_u32()?,
                        end_char_code: r.read_u32()?,
                        start_glyph_id: r.read_u32()?,
                    });
                }
                Some(CmapSubtable::Format12 { groups })
            }
            14 => {
                let _length = r.read_u32()?;
                let num_var_selector_records = r.read_u32()?;
                let mut var_selectors = Vec::with_capacity(num_var_selector_records as usize);
                for _ in 0..num_var_selector_records {
                    let b0 = r.read_u8()? as u32;
                    let b1 = r.read_u8()? as u32;
                    let b2 = r.read_u8()? as u32;
                    let var_selector = (b0 << 16) | (b1 << 8) | b2;
                    var_selectors.push(VariationSelector {
                        var_selector,
                        default_uvs_offset: r.read_u32()?,
                        non_default_uvs_offset: r.read_u32()?,
                    });
                }
                Some(CmapSubtable::Format14 { var_selectors })
            }
            _ => None,
        }
    }

    pub fn lookup(&self, codepoint: u32) -> Option<u16> {
        for st in &self.subtables {
            if let Some(gid) = Self::lookup_subtable(st, codepoint) {
                if gid != 0 {
                    return Some(gid);
                }
            }
        }
        None
    }

    fn lookup_subtable(st: &CmapSubtable, cp: u32) -> Option<u16> {
        match st {
            CmapSubtable::Format0 { glyph_ids } => {
                if cp < 256 {
                    Some(glyph_ids[cp as usize] as u16)
                } else {
                    None
                }
            }
            CmapSubtable::Format4 {
                seg_count,
                end_codes,
                start_codes,
                id_deltas,
                id_range_offsets,
                glyph_ids,
            } => {
                if cp > 0xFFFF {
                    return None;
                }
                let cp16 = cp as u16;
                for i in 0..*seg_count as usize {
                    if cp16 <= end_codes[i] && cp16 >= start_codes[i] {
                        if id_range_offsets[i] == 0 {
                            return Some(((cp16 as i32 + id_deltas[i] as i32) & 0xFFFF) as u16);
                        } else {
                            let idx = (id_range_offsets[i] as usize / 2)
                                + (cp16 as usize - start_codes[i] as usize)
                                - (*seg_count as usize - i);
                            if let Some(&gid) = glyph_ids.get(idx) {
                                if gid != 0 {
                                    return Some(
                                        ((gid as i32 + id_deltas[i] as i32) & 0xFFFF) as u16,
                                    );
                                }
                            }
                            return Some(0);
                        }
                    }
                }
                None
            }
            CmapSubtable::Format6 {
                first_code,
                entry_count,
                glyph_ids,
            } => {
                if cp >= *first_code as u32 && cp < (*first_code as u32 + *entry_count as u32) {
                    Some(glyph_ids[(cp - *first_code as u32) as usize])
                } else {
                    None
                }
            }
            CmapSubtable::Format12 { groups } => {
                for g in groups {
                    if cp >= g.start_char_code && cp <= g.end_char_code {
                        return Some((g.start_glyph_id + (cp - g.start_char_code)) as u16);
                    }
                }
                None
            }
            CmapSubtable::Format14 { .. } => None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// glyf table — glyph outlines
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct GlyphPoint {
    pub x: i16,
    pub y: i16,
    pub on_curve: bool,
}

#[derive(Debug, Clone)]
pub struct SimpleGlyph {
    pub contour_ends: Vec<u16>,
    pub instructions: Vec<u8>,
    pub points: Vec<GlyphPoint>,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
}

const GLYF_ON_CURVE_POINT: u8 = 0x01;
const GLYF_X_SHORT_VECTOR: u8 = 0x02;
const GLYF_Y_SHORT_VECTOR: u8 = 0x04;
const GLYF_REPEAT_FLAG: u8 = 0x08;
const GLYF_X_IS_SAME: u8 = 0x10;
const GLYF_Y_IS_SAME: u8 = 0x20;

impl SimpleGlyph {
    pub fn parse(data: &[u8], num_contours: i16) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let x_min = r.read_i16()?;
        let y_min = r.read_i16()?;
        let x_max = r.read_i16()?;
        let y_max = r.read_i16()?;
        let nc = num_contours as usize;
        let mut contour_ends = Vec::with_capacity(nc);
        for _ in 0..nc {
            contour_ends.push(r.read_u16()?);
        }
        let num_points = contour_ends.last().map_or(0, |&e| e as usize + 1);
        let inst_len = r.read_u16()? as usize;
        let instructions = r.read_bytes(inst_len)?.to_vec();
        let mut flags = Vec::with_capacity(num_points);
        while flags.len() < num_points {
            let flag = r.read_u8()?;
            flags.push(flag);
            if flag & GLYF_REPEAT_FLAG != 0 {
                let repeat = r.read_u8()? as usize;
                for _ in 0..repeat {
                    if flags.len() < num_points {
                        flags.push(flag);
                    }
                }
            }
        }
        let mut x_coords = Vec::with_capacity(num_points);
        let mut prev_x: i16 = 0;
        for &f in &flags {
            let dx: i16 = if f & GLYF_X_SHORT_VECTOR != 0 {
                let val = r.read_u8()? as i16;
                if f & GLYF_X_IS_SAME != 0 {
                    val
                } else {
                    -val
                }
            } else if f & GLYF_X_IS_SAME != 0 {
                0
            } else {
                r.read_i16()?
            };
            prev_x = prev_x.wrapping_add(dx);
            x_coords.push(prev_x);
        }
        let mut y_coords = Vec::with_capacity(num_points);
        let mut prev_y: i16 = 0;
        for &f in &flags {
            let dy: i16 = if f & GLYF_Y_SHORT_VECTOR != 0 {
                let val = r.read_u8()? as i16;
                if f & GLYF_Y_IS_SAME != 0 {
                    val
                } else {
                    -val
                }
            } else if f & GLYF_Y_IS_SAME != 0 {
                0
            } else {
                r.read_i16()?
            };
            prev_y = prev_y.wrapping_add(dy);
            y_coords.push(prev_y);
        }
        let mut points = Vec::with_capacity(num_points);
        for i in 0..num_points {
            points.push(GlyphPoint {
                x: x_coords[i],
                y: y_coords[i],
                on_curve: flags[i] & GLYF_ON_CURVE_POINT != 0,
            });
        }
        Some(SimpleGlyph {
            contour_ends,
            instructions,
            points,
            x_min,
            y_min,
            x_max,
            y_max,
        })
    }
}

// Composite glyph flags
const COMP_ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const COMP_ARGS_ARE_XY_VALUES: u16 = 0x0002;
const COMP_ROUND_XY_TO_GRID: u16 = 0x0004;
const COMP_WE_HAVE_A_SCALE: u16 = 0x0008;
const COMP_MORE_COMPONENTS: u16 = 0x0020;
const COMP_WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const COMP_WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const COMP_WE_HAVE_INSTRUCTIONS: u16 = 0x0100;
const COMP_USE_MY_METRICS: u16 = 0x0200;
const COMP_OVERLAP_COMPOUND: u16 = 0x0400;

#[derive(Debug, Clone, Copy)]
pub struct CompositeComponent {
    pub flags: u16,
    pub glyph_index: u16,
    pub arg1: i32,
    pub arg2: i32,
    pub scale_xx: f32,
    pub scale_xy: f32,
    pub scale_yx: f32,
    pub scale_yy: f32,
}

#[derive(Debug, Clone)]
pub struct CompositeGlyph {
    pub components: Vec<CompositeComponent>,
    pub instructions: Vec<u8>,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
}

impl CompositeGlyph {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let x_min = r.read_i16()?;
        let y_min = r.read_i16()?;
        let x_max = r.read_i16()?;
        let y_max = r.read_i16()?;
        let mut components = Vec::new();
        let mut has_more = true;
        let mut has_instructions = false;
        while has_more {
            let flags = r.read_u16()?;
            let glyph_index = r.read_u16()?;
            let (arg1, arg2) = if flags & COMP_ARG_1_AND_2_ARE_WORDS != 0 {
                if flags & COMP_ARGS_ARE_XY_VALUES != 0 {
                    (r.read_i16()? as i32, r.read_i16()? as i32)
                } else {
                    (r.read_u16()? as i32, r.read_u16()? as i32)
                }
            } else if flags & COMP_ARGS_ARE_XY_VALUES != 0 {
                (r.read_i8()? as i32, r.read_i8()? as i32)
            } else {
                (r.read_u8()? as i32, r.read_u8()? as i32)
            };
            let (mut sxx, mut sxy, mut syx, mut syy) = (1.0f32, 0.0f32, 0.0f32, 1.0f32);
            if flags & COMP_WE_HAVE_A_SCALE != 0 {
                let s = r.read_f2dot14()?;
                sxx = s;
                syy = s;
            } else if flags & COMP_WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                sxx = r.read_f2dot14()?;
                syy = r.read_f2dot14()?;
            } else if flags & COMP_WE_HAVE_A_TWO_BY_TWO != 0 {
                sxx = r.read_f2dot14()?;
                sxy = r.read_f2dot14()?;
                syx = r.read_f2dot14()?;
                syy = r.read_f2dot14()?;
            }
            components.push(CompositeComponent {
                flags,
                glyph_index,
                arg1,
                arg2,
                scale_xx: sxx,
                scale_xy: sxy,
                scale_yx: syx,
                scale_yy: syy,
            });
            if flags & COMP_WE_HAVE_INSTRUCTIONS != 0 {
                has_instructions = true;
            }
            has_more = flags & COMP_MORE_COMPONENTS != 0;
        }
        let instructions = if has_instructions {
            let len = r.read_u16()? as usize;
            r.read_bytes(len)?.to_vec()
        } else {
            Vec::new()
        };
        Some(CompositeGlyph {
            components,
            instructions,
            x_min,
            y_min,
            x_max,
            y_max,
        })
    }
}

#[derive(Debug, Clone)]
pub enum Glyph {
    Empty,
    Simple(SimpleGlyph),
    Composite(CompositeGlyph),
}

pub fn parse_glyph(data: &[u8]) -> Option<Glyph> {
    if data.is_empty() {
        return Some(Glyph::Empty);
    }
    let mut r = BinaryReader::new(data);
    let num_contours = r.read_i16()?;
    if num_contours >= 0 {
        SimpleGlyph::parse(&data[2..], num_contours).map(Glyph::Simple)
    } else {
        CompositeGlyph::parse(&data[2..]).map(Glyph::Composite)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// name table
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct NameRecord {
    pub platform_id: u16,
    pub encoding_id: u16,
    pub language_id: u16,
    pub name_id: u16,
    pub length: u16,
    pub offset: u16,
}

pub const NAME_COPYRIGHT: u16 = 0;
pub const NAME_FAMILY: u16 = 1;
pub const NAME_SUBFAMILY: u16 = 2;
pub const NAME_UNIQUE_ID: u16 = 3;
pub const NAME_FULL_NAME: u16 = 4;
pub const NAME_VERSION: u16 = 5;
pub const NAME_POSTSCRIPT: u16 = 6;
pub const NAME_TRADEMARK: u16 = 7;
pub const NAME_MANUFACTURER: u16 = 8;
pub const NAME_DESIGNER: u16 = 9;
pub const NAME_DESCRIPTION: u16 = 10;
pub const NAME_TYPO_FAMILY: u16 = 16;
pub const NAME_TYPO_SUBFAMILY: u16 = 17;

#[derive(Debug, Clone)]
pub struct NameTable {
    pub format: u16,
    pub records: Vec<NameRecord>,
    pub storage: Vec<u8>,
}

impl NameTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let format = r.read_u16()?;
        let count = r.read_u16()?;
        let storage_offset = r.read_u16()? as usize;
        let mut records = Vec::with_capacity(count as usize);
        for _ in 0..count {
            records.push(NameRecord {
                platform_id: r.read_u16()?,
                encoding_id: r.read_u16()?,
                language_id: r.read_u16()?,
                name_id: r.read_u16()?,
                length: r.read_u16()?,
                offset: r.read_u16()?,
            });
        }
        let storage = if storage_offset < data.len() {
            data[storage_offset..].to_vec()
        } else {
            Vec::new()
        };
        Some(NameTable {
            format,
            records,
            storage,
        })
    }

    pub fn get_name(&self, name_id: u16) -> Option<String> {
        for rec in &self.records {
            if rec.name_id == name_id {
                let start = rec.offset as usize;
                let end = start + rec.length as usize;
                if end <= self.storage.len() {
                    let raw = &self.storage[start..end];
                    if rec.platform_id == 3 || rec.platform_id == 0 {
                        let mut s = String::new();
                        let mut i = 0;
                        while i + 1 < raw.len() {
                            let ch = u16::from_be_bytes([raw[i], raw[i + 1]]);
                            if let Some(c) = char::from_u32(ch as u32) {
                                s.push(c);
                            }
                            i += 2;
                        }
                        return Some(s);
                    } else if rec.platform_id == 1 {
                        let s: String = raw
                            .iter()
                            .filter_map(|&b| char::from_u32(b as u32))
                            .collect();
                        return Some(s);
                    }
                }
            }
        }
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// post table
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct PostTable {
    pub format: Fixed,
    pub italic_angle: Fixed,
    pub underline_position: i16,
    pub underline_thickness: i16,
    pub is_fixed_pitch: u32,
    pub min_mem_type42: u32,
    pub max_mem_type42: u32,
    pub min_mem_type1: u32,
    pub max_mem_type1: u32,
}

impl PostTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        Some(PostTable {
            format: r.read_fixed()?,
            italic_angle: r.read_fixed()?,
            underline_position: r.read_i16()?,
            underline_thickness: r.read_i16()?,
            is_fixed_pitch: r.read_u32()?,
            min_mem_type42: r.read_u32()?,
            max_mem_type42: r.read_u32()?,
            min_mem_type1: r.read_u32()?,
            max_mem_type1: r.read_u32()?,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// kern table
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct KernPair {
    pub left: u16,
    pub right: u16,
    pub value: i16,
}

#[derive(Debug, Clone)]
pub struct KernTable {
    pub pairs: Vec<KernPair>,
}

impl KernTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let version = r.read_u16()?;
        let n_tables = r.read_u16()?;
        let mut pairs = Vec::new();
        for _ in 0..n_tables {
            let _sub_version = r.read_u16()?;
            let sub_length = r.read_u16()?;
            let coverage = r.read_u16()?;
            let format = coverage >> 8;
            if format == 0 {
                let n_pairs = r.read_u16()?;
                let _search_range = r.read_u16()?;
                let _entry_selector = r.read_u16()?;
                let _range_shift = r.read_u16()?;
                for _ in 0..n_pairs {
                    pairs.push(KernPair {
                        left: r.read_u16()?,
                        right: r.read_u16()?,
                        value: r.read_i16()?,
                    });
                }
            } else {
                r.skip(sub_length as usize - 6);
            }
        }
        Some(KernTable { pairs })
    }

    pub fn get_kerning(&self, left: u16, right: u16) -> i16 {
        for p in &self.pairs {
            if p.left == left && p.right == right {
                return p.value;
            }
        }
        0
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// OS/2 table
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct Os2Table {
    pub version: u16,
    pub x_avg_char_width: i16,
    pub us_weight_class: u16,
    pub us_width_class: u16,
    pub fs_type: u16,
    pub y_subscript_x_size: i16,
    pub y_subscript_y_size: i16,
    pub y_subscript_x_offset: i16,
    pub y_subscript_y_offset: i16,
    pub y_superscript_x_size: i16,
    pub y_superscript_y_size: i16,
    pub y_superscript_x_offset: i16,
    pub y_superscript_y_offset: i16,
    pub y_strikeout_size: i16,
    pub y_strikeout_position: i16,
    pub s_family_class: i16,
    pub panose: [u8; 10],
    pub ul_unicode_range: [u32; 4],
    pub ach_vend_id: [u8; 4],
    pub fs_selection: u16,
    pub us_first_char_index: u16,
    pub us_last_char_index: u16,
    pub s_typo_ascender: i16,
    pub s_typo_descender: i16,
    pub s_typo_line_gap: i16,
    pub us_win_ascent: u16,
    pub us_win_descent: u16,
    pub s_x_height: i16,
    pub s_cap_height: i16,
}

impl Os2Table {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let version = r.read_u16()?;
        let x_avg = r.read_i16()?;
        let weight = r.read_u16()?;
        let width = r.read_u16()?;
        let fs_type = r.read_u16()?;
        let ysubx = r.read_i16()?;
        let ysuby = r.read_i16()?;
        let ysuboxoff = r.read_i16()?;
        let ysuboyoff = r.read_i16()?;
        let ysupx = r.read_i16()?;
        let ysupy = r.read_i16()?;
        let ysupxoff = r.read_i16()?;
        let ysupyoff = r.read_i16()?;
        let strikeout_size = r.read_i16()?;
        let strikeout_pos = r.read_i16()?;
        let family_class = r.read_i16()?;
        let mut panose = [0u8; 10];
        for p in &mut panose {
            *p = r.read_u8()?;
        }
        let mut ul_unicode_range = [0u32; 4];
        for u in &mut ul_unicode_range {
            *u = r.read_u32()?;
        }
        let mut vend_id = [0u8; 4];
        for v in &mut vend_id {
            *v = r.read_u8()?;
        }
        let fs_selection = r.read_u16()?;
        let first_char = r.read_u16()?;
        let last_char = r.read_u16()?;
        let typo_asc = r.read_i16()?;
        let typo_desc = r.read_i16()?;
        let typo_gap = r.read_i16()?;
        let win_asc = r.read_u16()?;
        let win_desc = r.read_u16()?;
        let (x_height, cap_height) = if version >= 2 {
            r.skip(8); // ulCodePageRange
            (r.read_i16().unwrap_or(0), r.read_i16().unwrap_or(0))
        } else {
            (0, 0)
        };
        Some(Os2Table {
            version,
            x_avg_char_width: x_avg,
            us_weight_class: weight,
            us_width_class: width,
            fs_type,
            y_subscript_x_size: ysubx,
            y_subscript_y_size: ysuby,
            y_subscript_x_offset: ysuboxoff,
            y_subscript_y_offset: ysuboyoff,
            y_superscript_x_size: ysupx,
            y_superscript_y_size: ysupy,
            y_superscript_x_offset: ysupxoff,
            y_superscript_y_offset: ysupyoff,
            y_strikeout_size: strikeout_size,
            y_strikeout_position: strikeout_pos,
            s_family_class: family_class,
            panose,
            ul_unicode_range: ul_unicode_range,
            ach_vend_id: vend_id,
            fs_selection,
            us_first_char_index: first_char,
            us_last_char_index: last_char,
            s_typo_ascender: typo_asc,
            s_typo_descender: typo_desc,
            s_typo_line_gap: typo_gap,
            us_win_ascent: win_asc,
            us_win_descent: win_desc,
            s_x_height: x_height,
            s_cap_height: cap_height,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// gasp table
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct GaspRange {
    pub max_ppem: u16,
    pub behavior: u16,
}

pub const GASP_GRIDFIT: u16 = 0x0001;
pub const GASP_DOGRAY: u16 = 0x0002;
pub const GASP_SYMMETRIC_GRIDFIT: u16 = 0x0004;
pub const GASP_SYMMETRIC_SMOOTHING: u16 = 0x0008;

#[derive(Debug, Clone)]
pub struct GaspTable {
    pub version: u16,
    pub ranges: Vec<GaspRange>,
}

impl GaspTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let version = r.read_u16()?;
        let num_ranges = r.read_u16()?;
        let mut ranges = Vec::with_capacity(num_ranges as usize);
        for _ in 0..num_ranges {
            ranges.push(GaspRange {
                max_ppem: r.read_u16()?,
                behavior: r.read_u16()?,
            });
        }
        Some(GaspTable { version, ranges })
    }

    pub fn behavior_for_ppem(&self, ppem: u16) -> u16 {
        for range in &self.ranges {
            if ppem <= range.max_ppem {
                return range.behavior;
            }
        }
        GASP_DOGRAY | GASP_GRIDFIT
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// TrueType hinting — instruction interpreter
// ═══════════════════════════════════════════════════════════════════════════

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtInstruction {
    PushB0 = 0xB0,
    PushB1 = 0xB1,
    PushB2 = 0xB2,
    PushB3 = 0xB3,
    PushB4 = 0xB4,
    PushB5 = 0xB5,
    PushB6 = 0xB6,
    PushB7 = 0xB7,
    PushW0 = 0xB8,
    PushW1 = 0xB9,
    PushW2 = 0xBA,
    PushW3 = 0xBB,
    PushW4 = 0xBC,
    PushW5 = 0xBD,
    PushW6 = 0xBE,
    PushW7 = 0xBF,
    NPushB = 0x40,
    NPushW = 0x41,
    Srp0 = 0x10,
    Srp1 = 0x11,
    Srp2 = 0x12,
    Szp0 = 0x13,
    Szp1 = 0x14,
    Szp2 = 0x15,
    Sloop = 0x17,
    Smd = 0x1A,
    Sround = 0x76,
    S45Round = 0x77,
    InstCtrl = 0x8E,
    ScanCtrl = 0x85,
    ScanType = 0x8D,
    Scvtci = 0x1D,
    Sswci = 0x1E,
    Ssw = 0x1F,
    FlipOn = 0x4E,
    FlipOff = 0x4F,
    Sangw = 0x7E,
    Aa = 0x7F,
    DeltaP1 = 0x5D,
    DeltaP2 = 0x71,
    DeltaP3 = 0x72,
    DeltaC1 = 0x73,
    DeltaC2 = 0x74,
    DeltaC3 = 0x75,
    Round00 = 0x68,
    Round01 = 0x69,
    Round10 = 0x6A,
    Round11 = 0x6B,
    NRound00 = 0x6C,
    NRound01 = 0x6D,
    NRound10 = 0x6E,
    NRound11 = 0x6F,
    Mdap0 = 0x2E,
    Mdap1 = 0x2F,
    Miap0 = 0x3E,
    Miap1 = 0x3F,
    Iup0 = 0x30,
    Iup1 = 0x31,
    Shp0 = 0x32,
    Shp1 = 0x33,
    Shc0 = 0x34,
    Shc1 = 0x35,
    Shz0 = 0x36,
    Shz1 = 0x37,
    Shpix = 0x38,
    Msirp0 = 0x3A,
    Msirp1 = 0x3B,
    Mdrp00 = 0xC0,
    Mirp00 = 0xE0,
    AlignRp = 0x3C,
    Ip = 0x39,
    Utp = 0x29,
    Isect = 0x0F,
    AlignPts = 0x27,
    FlipPt = 0x80,
    FlipRgOn = 0x81,
    FlipRgOff = 0x82,
    If = 0x58,
    Else = 0x1B,
    Eif = 0x59,
    Jrot = 0x78,
    Jrof = 0x79,
    Call = 0x2B,
    Fdef = 0x2C,
    Endf = 0x2D,
    Loopcall = 0x2A,
    Mul = 0x63,
    Div = 0x62,
    Add = 0x60,
    Sub = 0x61,
    Neg = 0x65,
    Abs = 0x64,
    Floor = 0x66,
    Ceiling = 0x67,
    Max = 0x8B,
    Min = 0x8C,
    And = 0x5A,
    Or = 0x5B,
    Not = 0x5C,
    Eq = 0x54,
    Neq = 0x55,
    Lt = 0x50,
    LtEq = 0x51,
    Gt = 0x52,
    GtEq = 0x53,
    Odd = 0x56,
    Even = 0x57,
    GetInfo = 0x88,
    Idef = 0x89,
}

#[derive(Debug, Clone)]
pub struct GraphicsState {
    pub rp0: usize,
    pub rp1: usize,
    pub rp2: usize,
    pub zp0: usize,
    pub zp1: usize,
    pub zp2: usize,
    pub freedom_vector: (i32, i32),
    pub projection_vector: (i32, i32),
    pub dual_projection_vector: (i32, i32),
    pub round_state: u8,
    pub loop_count: u32,
    pub minimum_distance: i32,
    pub control_value_cut_in: i32,
    pub single_width_cut_in: i32,
    pub single_width_value: i32,
    pub auto_flip: bool,
    pub delta_base: u16,
    pub delta_shift: u16,
    pub instruct_control: u8,
    pub scan_control: u16,
    pub scan_type: u16,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            rp0: 0,
            rp1: 0,
            rp2: 0,
            zp0: 1,
            zp1: 1,
            zp2: 1,
            freedom_vector: (0x4000, 0),
            projection_vector: (0x4000, 0),
            dual_projection_vector: (0x4000, 0),
            round_state: 1,
            loop_count: 1,
            minimum_distance: 64,
            control_value_cut_in: 68,
            single_width_cut_in: 0,
            single_width_value: 0,
            auto_flip: true,
            delta_base: 9,
            delta_shift: 3,
            instruct_control: 0,
            scan_control: 0,
            scan_type: 0,
        }
    }
}

pub struct HintingEngine {
    pub gs: GraphicsState,
    pub stack: Vec<i32>,
    pub cvt: Vec<i32>,
    pub storage: Vec<i32>,
    pub functions: BTreeMap<u32, Vec<u8>>,
    pub twilight_zone: Vec<GlyphPoint>,
    pub glyph_zone: Vec<GlyphPoint>,
}

impl HintingEngine {
    pub fn new(max_stack: usize, cvt_len: usize, storage_len: usize, max_twilight: usize) -> Self {
        Self {
            gs: GraphicsState::default(),
            stack: Vec::with_capacity(max_stack),
            cvt: vec![0; cvt_len],
            storage: vec![0; storage_len],
            functions: BTreeMap::new(),
            twilight_zone: vec![
                GlyphPoint {
                    x: 0,
                    y: 0,
                    on_curve: true
                };
                max_twilight
            ],
            glyph_zone: Vec::new(),
        }
    }

    pub fn load_cvt(&mut self, data: &[u8]) {
        let mut r = BinaryReader::new(data);
        let mut i = 0;
        while i < self.cvt.len() {
            if let Some(v) = r.read_i16() {
                self.cvt[i] = v as i32 * 64; // F26Dot6
                i += 1;
            } else {
                break;
            }
        }
    }

    pub fn execute(&mut self, bytecode: &[u8]) {
        let mut pc = 0usize;
        let len = bytecode.len();
        while pc < len {
            let op = bytecode[pc];
            pc += 1;
            match op {
                0xB0..=0xB7 => {
                    // PUSHB[n]
                    let count = (op - 0xB0 + 1) as usize;
                    for _ in 0..count {
                        if pc < len {
                            self.stack.push(bytecode[pc] as i32);
                            pc += 1;
                        }
                    }
                }
                0xB8..=0xBF => {
                    // PUSHW[n]
                    let count = (op - 0xB8 + 1) as usize;
                    for _ in 0..count {
                        if pc + 1 < len {
                            let v = i16::from_be_bytes([bytecode[pc], bytecode[pc + 1]]);
                            self.stack.push(v as i32);
                            pc += 2;
                        }
                    }
                }
                0x40 => {
                    // NPUSHB
                    if pc < len {
                        let n = bytecode[pc] as usize;
                        pc += 1;
                        for _ in 0..n {
                            if pc < len {
                                self.stack.push(bytecode[pc] as i32);
                                pc += 1;
                            }
                        }
                    }
                }
                0x41 => {
                    // NPUSHW
                    if pc < len {
                        let n = bytecode[pc] as usize;
                        pc += 1;
                        for _ in 0..n {
                            if pc + 1 < len {
                                let v = i16::from_be_bytes([bytecode[pc], bytecode[pc + 1]]);
                                self.stack.push(v as i32);
                                pc += 2;
                            }
                        }
                    }
                }
                0x10 => {
                    let v = self.pop();
                    self.gs.rp0 = v as usize;
                } // SRP0
                0x11 => {
                    let v = self.pop();
                    self.gs.rp1 = v as usize;
                } // SRP1
                0x12 => {
                    let v = self.pop();
                    self.gs.rp2 = v as usize;
                } // SRP2
                0x13 => {
                    let v = self.pop();
                    self.gs.zp0 = v as usize;
                } // SZP0
                0x14 => {
                    let v = self.pop();
                    self.gs.zp1 = v as usize;
                } // SZP1
                0x15 => {
                    let v = self.pop();
                    self.gs.zp2 = v as usize;
                } // SZP2
                0x17 => {
                    let v = self.pop();
                    self.gs.loop_count = v as u32;
                } // SLOOP
                0x1A => {
                    let v = self.pop();
                    self.gs.minimum_distance = v;
                } // SMD
                0x1D => {
                    let v = self.pop();
                    self.gs.control_value_cut_in = v;
                } // SCVTCI
                0x1E => {
                    let v = self.pop();
                    self.gs.single_width_cut_in = v;
                } // SSWCI
                0x1F => {
                    let v = self.pop();
                    self.gs.single_width_value = v;
                } // SSW
                0x4E => {
                    self.gs.auto_flip = true;
                } // FLIPON
                0x4F => {
                    self.gs.auto_flip = false;
                } // FLIPOFF
                0x60 => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(a.wrapping_add(b));
                } // ADD
                0x61 => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(a.wrapping_sub(b));
                } // SUB
                0x63 => {
                    // MUL (26.6 × 26.6)
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(((a as i64 * b as i64) >> 6) as i32);
                }
                0x62 => {
                    // DIV
                    let b = self.pop();
                    let a = self.pop();
                    if b != 0 {
                        self.stack.push(((a as i64) << 6) as i32 / b);
                    } else {
                        self.stack.push(0);
                    }
                }
                0x65 => {
                    let a = self.pop();
                    self.stack.push(-a);
                } // NEG
                0x64 => {
                    let a = self.pop();
                    self.stack.push(a.abs());
                } // ABS
                0x66 => {
                    let a = self.pop();
                    self.stack.push(a & !63);
                } // FLOOR
                0x67 => {
                    let a = self.pop();
                    self.stack.push((a + 63) & !63);
                } // CEILING
                0x8B => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(a.max(b));
                } // MAX
                0x8C => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(a.min(b));
                } // MIN
                0x5A => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(if a != 0 && b != 0 { 1 } else { 0 });
                } // AND
                0x5B => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(if a != 0 || b != 0 { 1 } else { 0 });
                } // OR
                0x5C => {
                    let a = self.pop();
                    self.stack.push(if a == 0 { 1 } else { 0 });
                } // NOT
                0x54 => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(if a == b { 1 } else { 0 });
                } // EQ
                0x55 => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(if a != b { 1 } else { 0 });
                } // NEQ
                0x50 => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(if a < b { 1 } else { 0 });
                } // LT
                0x51 => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(if a <= b { 1 } else { 0 });
                } // LTEQ
                0x52 => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(if a > b { 1 } else { 0 });
                } // GT
                0x53 => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(if a >= b { 1 } else { 0 });
                } // GTEQ
                0x56 => {
                    let a = self.pop();
                    self.stack.push(if (self.round_value(a) / 64) % 2 != 0 {
                        1
                    } else {
                        0
                    });
                } // ODD
                0x57 => {
                    let a = self.pop();
                    self.stack.push(if (self.round_value(a) / 64) % 2 == 0 {
                        1
                    } else {
                        0
                    });
                } // EVEN
                0x58 => {
                    // IF
                    let cond = self.pop();
                    if cond == 0 {
                        pc = Self::skip_to_else_or_eif(bytecode, pc);
                    }
                }
                0x1B => {
                    // ELSE
                    pc = Self::skip_to_eif(bytecode, pc);
                }
                0x59 => {} // EIF
                0x78 => {
                    // JROT
                    let cond = self.pop();
                    let offset = self.pop();
                    if cond != 0 {
                        pc = (pc as i32 + offset - 2).max(0) as usize;
                    }
                }
                0x79 => {
                    // JROF
                    let cond = self.pop();
                    let offset = self.pop();
                    if cond == 0 {
                        pc = (pc as i32 + offset - 2).max(0) as usize;
                    }
                }
                0x2C => {
                    // FDEF
                    let func_id = self.pop() as u32;
                    let start = pc;
                    while pc < len && bytecode[pc] != 0x2D {
                        pc += 1;
                    }
                    self.functions.insert(func_id, bytecode[start..pc].to_vec());
                    if pc < len {
                        pc += 1;
                    } // skip ENDF
                }
                0x2D => {} // ENDF
                0x2B => {
                    // CALL
                    let func_id = self.pop() as u32;
                    if let Some(body) = self.functions.get(&func_id).cloned() {
                        self.execute(&body);
                    }
                }
                0x2A => {
                    // LOOPCALL
                    let func_id = self.pop() as u32;
                    let count = self.pop();
                    if let Some(body) = self.functions.get(&func_id).cloned() {
                        for _ in 0..count {
                            self.execute(&body);
                        }
                    }
                }
                0x20 => {
                    let a = self.pop();
                    self.stack.push(a);
                    self.stack.push(a);
                } // DUP
                0x21 => {
                    self.pop();
                } // POP
                0x22 => {
                    self.stack.clear();
                } // CLEAR
                0x23 => {
                    // SWAP
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(b);
                    self.stack.push(a);
                }
                0x24 => {
                    let n = self.stack.len();
                    if n > 0 {
                        self.stack.push(self.stack[n - 1]);
                    }
                } // DEPTH → push depth
                0x25 => {
                    // CINDEX
                    let k = self.pop() as usize;
                    let n = self.stack.len();
                    if k > 0 && k <= n {
                        self.stack.push(self.stack[n - k]);
                    }
                }
                0x26 => {
                    // MINDEX
                    let k = self.pop() as usize;
                    let n = self.stack.len();
                    if k > 0 && k <= n {
                        let val = self.stack.remove(n - k);
                        self.stack.push(val);
                    }
                }
                0x42 => {
                    // WS (write storage)
                    let val = self.pop();
                    let idx = self.pop() as usize;
                    if idx < self.storage.len() {
                        self.storage[idx] = val;
                    }
                }
                0x43 => {
                    // RS (read storage)
                    let idx = self.pop() as usize;
                    let val = if idx < self.storage.len() {
                        self.storage[idx]
                    } else {
                        0
                    };
                    self.stack.push(val);
                }
                0x44 => {
                    // WCVTP
                    let val = self.pop();
                    let idx = self.pop() as usize;
                    if idx < self.cvt.len() {
                        self.cvt[idx] = val;
                    }
                }
                0x45 => {
                    // RCVT
                    let idx = self.pop() as usize;
                    let val = if idx < self.cvt.len() {
                        self.cvt[idx]
                    } else {
                        0
                    };
                    self.stack.push(val);
                }
                0x88 => {
                    // GETINFO
                    let selector = self.pop();
                    let mut result = 0i32;
                    if selector & 1 != 0 {
                        result |= 42;
                    } // version (arbitrary engine version)
                    if selector & 2 != 0 {
                        result |= 0;
                    } // glyph rotation: not rotated
                    if selector & 4 != 0 {
                        result |= 0;
                    } // glyph stretched: not stretched
                    self.stack.push(result);
                }
                0x85 => {
                    let v = self.pop();
                    self.gs.scan_control = v as u16;
                } // SCANCTRL
                0x8D => {
                    let v = self.pop();
                    self.gs.scan_type = v as u16;
                } // SCANTYPE
                0x8E => {
                    // INSTCTRL
                    let s = self.pop();
                    let v = self.pop();
                    if s == 1 {
                        self.gs.instruct_control = (self.gs.instruct_control & !1) | (v as u8 & 1);
                    }
                    if s == 2 {
                        self.gs.instruct_control =
                            (self.gs.instruct_control & !2) | ((v as u8 & 1) << 1);
                    }
                }
                _ => {} // unhandled instruction — skip
            }
        }
    }

    fn pop(&mut self) -> i32 {
        self.stack.pop().unwrap_or(0)
    }

    fn round_value(&self, val: i32) -> i32 {
        match self.gs.round_state {
            0 => val,              // round to half-grid
            1 => (val + 32) & !63, // round to grid
            2 => {
                // round to double grid
                let v = (val + 16) & !31;
                if v & 32 != 0 {
                    v
                } else {
                    v
                }
            }
            3 => val,              // round down to grid
            4 => (val + 63) & !63, // round up to grid
            _ => (val + 32) & !63,
        }
    }

    fn skip_to_else_or_eif(bytecode: &[u8], mut pc: usize) -> usize {
        let mut depth = 1u32;
        while pc < bytecode.len() && depth > 0 {
            match bytecode[pc] {
                0x58 => {
                    depth += 1;
                    pc += 1;
                }
                0x1B if depth == 1 => {
                    pc += 1;
                    return pc;
                }
                0x59 => {
                    depth -= 1;
                    pc += 1;
                }
                0xB0..=0xB7 => {
                    pc += 1 + (bytecode[pc] - 0xB0 + 1) as usize;
                }
                0xB8..=0xBF => {
                    pc += 1 + (bytecode[pc] - 0xB8 + 1) as usize * 2;
                }
                0x40 => {
                    if pc + 1 < bytecode.len() {
                        pc += 2 + bytecode[pc + 1] as usize;
                    } else {
                        pc += 1;
                    }
                }
                0x41 => {
                    if pc + 1 < bytecode.len() {
                        pc += 2 + bytecode[pc + 1] as usize * 2;
                    } else {
                        pc += 1;
                    }
                }
                _ => {
                    pc += 1;
                }
            }
        }
        pc
    }

    fn skip_to_eif(bytecode: &[u8], mut pc: usize) -> usize {
        let mut depth = 1u32;
        while pc < bytecode.len() && depth > 0 {
            match bytecode[pc] {
                0x58 => {
                    depth += 1;
                    pc += 1;
                }
                0x59 => {
                    depth -= 1;
                    pc += 1;
                }
                0xB0..=0xB7 => {
                    pc += 1 + (bytecode[pc] - 0xB0 + 1) as usize;
                }
                0xB8..=0xBF => {
                    pc += 1 + (bytecode[pc] - 0xB8 + 1) as usize * 2;
                }
                0x40 => {
                    if pc + 1 < bytecode.len() {
                        pc += 2 + bytecode[pc + 1] as usize;
                    } else {
                        pc += 1;
                    }
                }
                0x41 => {
                    if pc + 1 < bytecode.len() {
                        pc += 2 + bytecode[pc + 1] as usize * 2;
                    } else {
                        pc += 1;
                    }
                }
                _ => {
                    pc += 1;
                }
            }
        }
        pc
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// OpenType layout — GSUB / GPOS / GDEF
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CoverageTable {
    pub glyphs: Vec<u16>,
    pub ranges: Vec<CoverageRange>,
}

#[derive(Debug, Clone, Copy)]
pub struct CoverageRange {
    pub start_glyph: u16,
    pub end_glyph: u16,
    pub start_coverage_index: u16,
}

impl CoverageTable {
    pub fn parse(data: &[u8], offset: usize) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        r.seek(offset);
        let format = r.read_u16()?;
        match format {
            1 => {
                let count = r.read_u16()?;
                let mut glyphs = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    glyphs.push(r.read_u16()?);
                }
                Some(CoverageTable {
                    glyphs,
                    ranges: Vec::new(),
                })
            }
            2 => {
                let count = r.read_u16()?;
                let mut ranges = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    ranges.push(CoverageRange {
                        start_glyph: r.read_u16()?,
                        end_glyph: r.read_u16()?,
                        start_coverage_index: r.read_u16()?,
                    });
                }
                Some(CoverageTable {
                    glyphs: Vec::new(),
                    ranges,
                })
            }
            _ => None,
        }
    }

    pub fn coverage_index(&self, glyph_id: u16) -> Option<u16> {
        if !self.glyphs.is_empty() {
            self.glyphs
                .iter()
                .position(|&g| g == glyph_id)
                .map(|i| i as u16)
        } else {
            for rng in &self.ranges {
                if glyph_id >= rng.start_glyph && glyph_id <= rng.end_glyph {
                    return Some(rng.start_coverage_index + (glyph_id - rng.start_glyph));
                }
            }
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClassDefTable {
    pub class_map: BTreeMap<u16, u16>,
}

impl ClassDefTable {
    pub fn parse(data: &[u8], offset: usize) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        r.seek(offset);
        let format = r.read_u16()?;
        let mut class_map = BTreeMap::new();
        match format {
            1 => {
                let start_glyph = r.read_u16()?;
                let count = r.read_u16()?;
                for i in 0..count {
                    let cls = r.read_u16()?;
                    class_map.insert(start_glyph + i, cls);
                }
            }
            2 => {
                let count = r.read_u16()?;
                for _ in 0..count {
                    let start = r.read_u16()?;
                    let end = r.read_u16()?;
                    let cls = r.read_u16()?;
                    for g in start..=end {
                        class_map.insert(g, cls);
                    }
                }
            }
            _ => return None,
        }
        Some(ClassDefTable { class_map })
    }

    pub fn get_class(&self, glyph_id: u16) -> u16 {
        self.class_map.get(&glyph_id).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsubLookupType {
    Single = 1,
    Multiple = 2,
    Alternate = 3,
    Ligature = 4,
    Context = 5,
    ChainingContext = 6,
    Extension = 7,
    ReverseChaining = 8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GposLookupType {
    SingleAdjustment = 1,
    PairAdjustment = 2,
    CursiveAttachment = 3,
    MarkToBase = 4,
    MarkToLigature = 5,
    MarkToMark = 6,
    Context = 7,
    ChainingContext = 8,
    Extension = 9,
}

#[derive(Debug, Clone, Copy)]
pub struct ValueRecord {
    pub x_placement: i16,
    pub y_placement: i16,
    pub x_advance: i16,
    pub y_advance: i16,
}

impl ValueRecord {
    pub fn parse(r: &mut BinaryReader, format: u16) -> Option<Self> {
        let x_placement = if format & 0x0001 != 0 {
            r.read_i16()?
        } else {
            0
        };
        let y_placement = if format & 0x0002 != 0 {
            r.read_i16()?
        } else {
            0
        };
        let x_advance = if format & 0x0004 != 0 {
            r.read_i16()?
        } else {
            0
        };
        let y_advance = if format & 0x0008 != 0 {
            r.read_i16()?
        } else {
            0
        };
        if format & 0x0010 != 0 {
            r.read_i16()?;
        } // xPlaDevice
        if format & 0x0020 != 0 {
            r.read_i16()?;
        } // yPlaDevice
        if format & 0x0040 != 0 {
            r.read_i16()?;
        } // xAdvDevice
        if format & 0x0080 != 0 {
            r.read_i16()?;
        } // yAdvDevice
        Some(ValueRecord {
            x_placement,
            y_placement,
            x_advance,
            y_advance,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScriptRecord {
    pub tag: [u8; 4],
    pub offset: u16,
}

#[derive(Debug, Clone)]
pub struct LangSysRecord {
    pub tag: [u8; 4],
    pub offset: u16,
}

#[derive(Debug, Clone)]
pub struct LangSys {
    pub lookup_order: u16,
    pub required_feature_index: u16,
    pub feature_indices: Vec<u16>,
}

#[derive(Debug, Clone)]
pub struct FeatureRecord {
    pub tag: [u8; 4],
    pub offset: u16,
}

#[derive(Debug, Clone)]
pub struct Feature {
    pub feature_params: u16,
    pub lookup_indices: Vec<u16>,
}

#[derive(Debug, Clone)]
pub struct OtLayoutHeader {
    pub major_version: u16,
    pub minor_version: u16,
    pub script_list_offset: u16,
    pub feature_list_offset: u16,
    pub lookup_list_offset: u16,
}

impl OtLayoutHeader {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        Some(OtLayoutHeader {
            major_version: r.read_u16()?,
            minor_version: r.read_u16()?,
            script_list_offset: r.read_u16()?,
            feature_list_offset: r.read_u16()?,
            lookup_list_offset: r.read_u16()?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum GdefGlyphClass {
    Base = 1,
    Ligature = 2,
    Mark = 3,
    Component = 4,
}

#[derive(Debug, Clone)]
pub struct GdefTable {
    pub major_version: u16,
    pub minor_version: u16,
    pub glyph_class_def_offset: u16,
    pub attach_list_offset: u16,
    pub lig_caret_list_offset: u16,
    pub mark_attach_class_def_offset: u16,
    pub mark_glyph_sets_def_offset: Option<u16>,
    pub glyph_class_def: Option<ClassDefTable>,
}

impl GdefTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let major = r.read_u16()?;
        let minor = r.read_u16()?;
        let glyph_class_off = r.read_u16()?;
        let attach_off = r.read_u16()?;
        let lig_caret_off = r.read_u16()?;
        let mark_attach_off = r.read_u16()?;
        let mark_sets_off = if minor >= 2 {
            Some(r.read_u16()?)
        } else {
            None
        };
        let glyph_class_def = if glyph_class_off != 0 {
            ClassDefTable::parse(data, glyph_class_off as usize)
        } else {
            None
        };
        Some(GdefTable {
            major_version: major,
            minor_version: minor,
            glyph_class_def_offset: glyph_class_off,
            attach_list_offset: attach_off,
            lig_caret_list_offset: lig_caret_off,
            mark_attach_class_def_offset: mark_attach_off,
            mark_glyph_sets_def_offset: mark_sets_off,
            glyph_class_def,
        })
    }

    pub fn glyph_class(&self, glyph_id: u16) -> u16 {
        self.glyph_class_def
            .as_ref()
            .map_or(0, |cd| cd.get_class(glyph_id))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Variable fonts — fvar, avar, gvar, HVAR, MVAR, STAT
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VariationAxis {
    pub tag: [u8; 4],
    pub min_value: Fixed,
    pub default_value: Fixed,
    pub max_value: Fixed,
    pub flags: u16,
    pub name_id: u16,
}

#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub subfamily_name_id: u16,
    pub flags: u16,
    pub coordinates: Vec<Fixed>,
    pub postscript_name_id: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct FvarTable {
    pub major_version: u16,
    pub minor_version: u16,
    pub axes: Vec<VariationAxis>,
    pub instances: Vec<InstanceRecord>,
}

impl FvarTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let major = r.read_u16()?;
        let minor = r.read_u16()?;
        let axes_array_offset = r.read_u16()?;
        let _reserved = r.read_u16()?;
        let axis_count = r.read_u16()?;
        let axis_size = r.read_u16()?;
        let instance_count = r.read_u16()?;
        let instance_size = r.read_u16()?;
        let mut axes = Vec::with_capacity(axis_count as usize);
        r.seek(axes_array_offset as usize);
        for _ in 0..axis_count {
            let start = r.pos;
            axes.push(VariationAxis {
                tag: r.read_tag()?,
                min_value: r.read_fixed()?,
                default_value: r.read_fixed()?,
                max_value: r.read_fixed()?,
                flags: r.read_u16()?,
                name_id: r.read_u16()?,
            });
            r.seek(start + axis_size as usize);
        }
        let mut instances = Vec::with_capacity(instance_count as usize);
        for _ in 0..instance_count {
            let start = r.pos;
            let subfamily_name_id = r.read_u16()?;
            let flags = r.read_u16()?;
            let mut coordinates = Vec::with_capacity(axis_count as usize);
            for _ in 0..axis_count {
                coordinates.push(r.read_fixed()?);
            }
            let ps_name_id = if instance_size as usize > 4 + axis_count as usize * 4 {
                Some(r.read_u16()?)
            } else {
                None
            };
            instances.push(InstanceRecord {
                subfamily_name_id,
                flags,
                coordinates,
                postscript_name_id: ps_name_id,
            });
            r.seek(start + instance_size as usize);
        }
        Some(FvarTable {
            major_version: major,
            minor_version: minor,
            axes,
            instances,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AvarSegmentMap {
    pub axis_index: u16,
    pub pairs: Vec<(i16, i16)>,
}

#[derive(Debug, Clone)]
pub struct AvarTable {
    pub major_version: u16,
    pub minor_version: u16,
    pub segment_maps: Vec<AvarSegmentMap>,
}

impl AvarTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let major = r.read_u16()?;
        let minor = r.read_u16()?;
        let _reserved = r.read_u16()?;
        let axis_count = r.read_u16()?;
        let mut segment_maps = Vec::with_capacity(axis_count as usize);
        for i in 0..axis_count {
            let pair_count = r.read_u16()?;
            let mut pairs = Vec::with_capacity(pair_count as usize);
            for _ in 0..pair_count {
                pairs.push((r.read_i16()?, r.read_i16()?));
            }
            segment_maps.push(AvarSegmentMap {
                axis_index: i,
                pairs,
            });
        }
        Some(AvarTable {
            major_version: major,
            minor_version: minor,
            segment_maps,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Emoji / Color — COLR, CPAL
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct ColorRecord {
    pub blue: u8,
    pub green: u8,
    pub red: u8,
    pub alpha: u8,
}

#[derive(Debug, Clone)]
pub struct CpalTable {
    pub version: u16,
    pub num_palette_entries: u16,
    pub num_palettes: u16,
    pub color_records: Vec<ColorRecord>,
    pub palette_offsets: Vec<u16>,
}

impl CpalTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let version = r.read_u16()?;
        let num_palette_entries = r.read_u16()?;
        let num_palettes = r.read_u16()?;
        let num_color_records = r.read_u16()?;
        let color_records_offset = r.read_u32()? as usize;
        let mut palette_offsets = Vec::with_capacity(num_palettes as usize);
        for _ in 0..num_palettes {
            palette_offsets.push(r.read_u16()?);
        }
        let mut color_records = Vec::with_capacity(num_color_records as usize);
        r.seek(color_records_offset);
        for _ in 0..num_color_records {
            color_records.push(ColorRecord {
                blue: r.read_u8()?,
                green: r.read_u8()?,
                red: r.read_u8()?,
                alpha: r.read_u8()?,
            });
        }
        Some(CpalTable {
            version,
            num_palette_entries,
            num_palettes,
            color_records,
            palette_offsets,
        })
    }

    pub fn get_color(&self, palette_index: u16, entry_index: u16) -> Option<ColorRecord> {
        let offset =
            *self.palette_offsets.get(palette_index as usize)? as usize + entry_index as usize;
        self.color_records.get(offset).copied()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ColrLayerRecord {
    pub glyph_id: u16,
    pub palette_entry_index: u16,
}

#[derive(Debug, Clone)]
pub struct ColrBaseGlyph {
    pub glyph_id: u16,
    pub first_layer_index: u16,
    pub num_layers: u16,
}

#[derive(Debug, Clone)]
pub struct ColrTable {
    pub version: u16,
    pub base_glyphs: Vec<ColrBaseGlyph>,
    pub layers: Vec<ColrLayerRecord>,
}

impl ColrTable {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = BinaryReader::new(data);
        let version = r.read_u16()?;
        let num_base_glyphs = r.read_u16()?;
        let base_glyph_offset = r.read_u32()? as usize;
        let layer_offset = r.read_u32()? as usize;
        let num_layers = r.read_u16()?;
        let mut base_glyphs = Vec::with_capacity(num_base_glyphs as usize);
        r.seek(base_glyph_offset);
        for _ in 0..num_base_glyphs {
            base_glyphs.push(ColrBaseGlyph {
                glyph_id: r.read_u16()?,
                first_layer_index: r.read_u16()?,
                num_layers: r.read_u16()?,
            });
        }
        let mut layers = Vec::with_capacity(num_layers as usize);
        r.seek(layer_offset);
        for _ in 0..num_layers {
            layers.push(ColrLayerRecord {
                glyph_id: r.read_u16()?,
                palette_entry_index: r.read_u16()?,
            });
        }
        Some(ColrTable {
            version,
            base_glyphs,
            layers,
        })
    }

    pub fn get_layers(&self, glyph_id: u16) -> Option<&[ColrLayerRecord]> {
        let bg = self.base_glyphs.iter().find(|bg| bg.glyph_id == glyph_id)?;
        let start = bg.first_layer_index as usize;
        let end = start + bg.num_layers as usize;
        self.layers.get(start..end)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Bitmap fonts — BDF/PCF parsing
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BdfGlyph {
    pub encoding: u32,
    pub width: u16,
    pub height: u16,
    pub x_offset: i16,
    pub y_offset: i16,
    pub device_width: u16,
    pub bitmap: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BdfFont {
    pub name: String,
    pub point_size: u16,
    pub x_dpi: u16,
    pub y_dpi: u16,
    pub ascent: i16,
    pub descent: i16,
    pub glyphs: BTreeMap<u32, BdfGlyph>,
}

impl BdfFont {
    pub fn parse(data: &[u8]) -> Option<Self> {
        let text = core::str::from_utf8(data).ok()?;
        let mut name = String::new();
        let mut point_size = 0u16;
        let mut x_dpi = 0u16;
        let mut y_dpi = 0u16;
        let mut ascent = 0i16;
        let mut descent = 0i16;
        let mut glyphs = BTreeMap::new();
        let mut in_bitmap = false;
        let mut current_encoding = 0u32;
        let mut current_width = 0u16;
        let mut current_height = 0u16;
        let mut current_xoff = 0i16;
        let mut current_yoff = 0i16;
        let mut current_dwidth = 0u16;
        let mut current_bitmap_data = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if in_bitmap {
                if line == "ENDCHAR" {
                    in_bitmap = false;
                    glyphs.insert(
                        current_encoding,
                        BdfGlyph {
                            encoding: current_encoding,
                            width: current_width,
                            height: current_height,
                            x_offset: current_xoff,
                            y_offset: current_yoff,
                            device_width: current_dwidth,
                            bitmap: core::mem::take(&mut current_bitmap_data),
                        },
                    );
                } else {
                    let mut i = 0;
                    while i + 1 < line.len() {
                        if let Ok(byte) = u8::from_str_radix(&line[i..i + 2], 16) {
                            current_bitmap_data.push(byte);
                        }
                        i += 2;
                    }
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("FONT ") {
                name = String::from(rest);
            } else if let Some(rest) = line.strip_prefix("SIZE ") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() >= 3 {
                    point_size = parse_u16(parts[0]);
                    x_dpi = parse_u16(parts[1]);
                    y_dpi = parse_u16(parts[2]);
                }
            } else if let Some(rest) = line.strip_prefix("FONT_ASCENT ") {
                ascent = parse_i16(rest.trim());
            } else if let Some(rest) = line.strip_prefix("FONT_DESCENT ") {
                descent = parse_i16(rest.trim());
            } else if let Some(rest) = line.strip_prefix("ENCODING ") {
                current_encoding = parse_u32(rest.trim());
            } else if let Some(rest) = line.strip_prefix("BBX ") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() >= 4 {
                    current_width = parse_u16(parts[0]);
                    current_height = parse_u16(parts[1]);
                    current_xoff = parse_i16(parts[2]);
                    current_yoff = parse_i16(parts[3]);
                }
            } else if let Some(rest) = line.strip_prefix("DWIDTH ") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if !parts.is_empty() {
                    current_dwidth = parse_u16(parts[0]);
                }
            } else if line == "BITMAP" {
                in_bitmap = true;
                current_bitmap_data.clear();
            }
        }
        Some(BdfFont {
            name,
            point_size,
            x_dpi,
            y_dpi,
            ascent,
            descent,
            glyphs,
        })
    }
}

fn parse_u16(s: &str) -> u16 {
    s.parse().unwrap_or(0)
}
fn parse_i16(s: &str) -> i16 {
    s.parse().unwrap_or(0)
}
fn parse_u32(s: &str) -> u32 {
    s.parse().unwrap_or(0)
}

// ═══════════════════════════════════════════════════════════════════════════
// Rasterizer — outline to bitmap
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubpixelMode {
    None,
    Rgb,
    Bgr,
    Vrgb,
    Vbgr,
}

#[derive(Debug, Clone)]
pub struct RasterConfig {
    pub ppem: u16,
    pub subpixel: SubpixelMode,
    pub gamma: f32,
    pub stem_darkening: bool,
    pub auto_hint: bool,
    pub fractional_positioning: bool,
    pub oversample: u8,
}

impl Default for RasterConfig {
    fn default() -> Self {
        Self {
            ppem: 16,
            subpixel: SubpixelMode::None,
            gamma: 1.8,
            stem_darkening: false,
            auto_hint: false,
            fractional_positioning: false,
            oversample: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RasterizedGlyph {
    pub width: u32,
    pub height: u32,
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub advance: i32,
    pub pixels: Vec<u8>,
}

pub struct Rasterizer {
    config: RasterConfig,
    scanline_buf: Vec<f32>,
}

impl Rasterizer {
    pub fn new(config: RasterConfig) -> Self {
        Self {
            config,
            scanline_buf: Vec::new(),
        }
    }

    pub fn rasterize(&mut self, glyph: &SimpleGlyph, units_per_em: u16) -> RasterizedGlyph {
        let scale = self.config.ppem as f32 / units_per_em as f32;
        let oversample = self.config.oversample.max(1) as f32;
        let total_scale = scale * oversample;

        let x_min = f32_floor(glyph.x_min as f32 * total_scale) as i32;
        let y_min = f32_floor(glyph.y_min as f32 * total_scale) as i32;
        let x_max = f32_ceil(glyph.x_max as f32 * total_scale) as i32;
        let y_max = f32_ceil(glyph.y_max as f32 * total_scale) as i32;
        let w = ((x_max - x_min) as u32).max(1);
        let h = ((y_max - y_min) as u32).max(1);

        let final_w = f32_ceil(w as f32 / oversample) as u32;
        let final_h = f32_ceil(h as f32 / oversample) as u32;

        let pixel_count = match self.config.subpixel {
            SubpixelMode::None => (final_w * final_h) as usize,
            SubpixelMode::Rgb | SubpixelMode::Bgr => (final_w * final_h * 3) as usize,
            SubpixelMode::Vrgb | SubpixelMode::Vbgr => (final_w * final_h * 3) as usize,
        };
        let mut pixels = vec![0u8; pixel_count];

        self.scanline_buf.resize(w as usize, 0.0);

        let mut contour_start = 0usize;
        for &end_idx in &glyph.contour_ends {
            let end = end_idx as usize + 1;
            let contour = &glyph.points[contour_start..end.min(glyph.points.len())];
            if contour.len() >= 2 {
                self.rasterize_contour(
                    contour,
                    total_scale,
                    x_min,
                    y_min,
                    w,
                    h,
                    &mut pixels,
                    final_w,
                    oversample,
                );
            }
            contour_start = end;
        }

        if self.config.gamma != 1.0 {
            let inv_gamma = 1.0 / self.config.gamma;
            for p in &mut pixels {
                let normalized = *p as f32 / 255.0;
                *p = (pow_approx(normalized, inv_gamma) * 255.0) as u8;
            }
        }

        RasterizedGlyph {
            width: final_w,
            height: final_h,
            bearing_x: (x_min as f32 / oversample) as i32,
            bearing_y: (y_max as f32 / oversample) as i32,
            advance: (glyph.x_max as f32 * scale) as i32,
            pixels,
        }
    }

    fn rasterize_contour(
        &self,
        contour: &[GlyphPoint],
        scale: f32,
        x_off: i32,
        y_off: i32,
        w: u32,
        h: u32,
        pixels: &mut [u8],
        final_w: u32,
        oversample: f32,
    ) {
        let len = contour.len();
        for i in 0..len {
            let p0 = &contour[i];
            let p1 = &contour[(i + 1) % len];
            let x0 = p0.x as f32 * scale - x_off as f32;
            let y0 = p0.y as f32 * scale - y_off as f32;
            let x1 = p1.x as f32 * scale - x_off as f32;
            let y1 = p1.y as f32 * scale - y_off as f32;
            if p0.on_curve && p1.on_curve {
                self.draw_line(x0, y0, x1, y1, w, h, pixels, final_w, oversample);
            } else if !p0.on_curve && p1.on_curve {
                let prev = &contour[(i + len - 1) % len];
                let cx = p0.x as f32 * scale - x_off as f32;
                let cy = p0.y as f32 * scale - y_off as f32;
                let sx = prev.x as f32 * scale - x_off as f32;
                let sy = prev.y as f32 * scale - y_off as f32;
                self.draw_quadratic(sx, sy, cx, cy, x1, y1, w, h, pixels, final_w, oversample);
            }
        }
    }

    fn draw_line(
        &self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        w: u32,
        h: u32,
        pixels: &mut [u8],
        final_w: u32,
        oversample: f32,
    ) {
        let steps = ((x1 - x0).abs().max((y1 - y0).abs()) as u32).max(1);
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let px = x0 + (x1 - x0) * t;
            let py = y0 + (y1 - y0) * t;
            let fx = (px / oversample) as i32;
            let fy = (py / oversample) as i32;
            let fw = f32_ceil(w as f32 / oversample) as u32;
            let fh = f32_ceil(h as f32 / oversample) as u32;
            if fx >= 0 && fy >= 0 && (fx as u32) < fw && (fy as u32) < fh {
                let idx = (fy as u32 * final_w + fx as u32) as usize;
                if idx < pixels.len() {
                    pixels[idx] = pixels[idx].saturating_add(128);
                }
            }
        }
    }

    fn draw_quadratic(
        &self,
        x0: f32,
        y0: f32,
        cx: f32,
        cy: f32,
        x1: f32,
        y1: f32,
        w: u32,
        h: u32,
        pixels: &mut [u8],
        final_w: u32,
        oversample: f32,
    ) {
        let steps = 16u32;
        let mut prev_x = x0;
        let mut prev_y = y0;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let inv = 1.0 - t;
            let nx = inv * inv * x0 + 2.0 * inv * t * cx + t * t * x1;
            let ny = inv * inv * y0 + 2.0 * inv * t * cy + t * t * y1;
            self.draw_line(prev_x, prev_y, nx, ny, w, h, pixels, final_w, oversample);
            prev_x = nx;
            prev_y = ny;
        }
    }
}

fn pow_approx(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    if base >= 1.0 {
        return 1.0;
    }
    let ln_base = ln_approx(base);
    exp_approx(ln_base * exp)
}

fn ln_approx(x: f32) -> f32 {
    let mut y = x - 1.0;
    let y2 = y * y;
    y - y2 * 0.5 + y2 * y * 0.333333
}

fn exp_approx(x: f32) -> f32 {
    let x2 = x * x;
    1.0 + x + x2 * 0.5 + x2 * x * 0.166667 + x2 * x2 * 0.041667
}

// ═══════════════════════════════════════════════════════════════════════════
// Text shaping — complex scripts
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    Latin,
    Arabic,
    Hebrew,
    Devanagari,
    Bengali,
    Thai,
    Hangul,
    Tibetan,
    Myanmar,
    Khmer,
    Syriac,
    CJK,
    Common,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BidiClass {
    L,
    R,
    AL,
    EN,
    ES,
    ET,
    AN,
    CS,
    NSM,
    BN,
    B,
    S,
    WS,
    ON,
    LRE,
    LRO,
    RLE,
    RLO,
    PDF,
    LRI,
    RLI,
    FSI,
    PDI,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArabicJoiningType {
    Right,
    Left,
    Dual,
    Causing,
    NonJoining,
    Transparent,
}

#[derive(Debug, Clone, Copy)]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    pub cluster: u32,
    pub x_advance: i32,
    pub y_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

pub struct TextShaper {
    direction: TextDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection {
    Ltr,
    Rtl,
    Ttb,
    Btt,
}

impl TextShaper {
    pub fn new(direction: TextDirection) -> Self {
        Self { direction }
    }

    pub fn detect_script(codepoint: u32) -> ScriptKind {
        match codepoint {
            0x0000..=0x024F => ScriptKind::Latin,
            0x0590..=0x05FF => ScriptKind::Hebrew,
            0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => {
                ScriptKind::Arabic
            }
            0x0900..=0x097F => ScriptKind::Devanagari,
            0x0980..=0x09FF => ScriptKind::Bengali,
            0x0E00..=0x0E7F => ScriptKind::Thai,
            0x0F00..=0x0FFF => ScriptKind::Tibetan,
            0x1000..=0x109F => ScriptKind::Myanmar,
            0x1780..=0x17FF => ScriptKind::Khmer,
            0x0700..=0x074F => ScriptKind::Syriac,
            0xAC00..=0xD7AF | 0x1100..=0x11FF => ScriptKind::Hangul,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x2E80..=0x2EFF => ScriptKind::CJK,
            _ => ScriptKind::Common,
        }
    }

    pub fn bidi_class(cp: u32) -> BidiClass {
        match cp {
            0x0041..=0x005A | 0x0061..=0x007A | 0x00C0..=0x024F => BidiClass::L,
            0x05D0..=0x05EA | 0xFB1D..=0xFB4F => BidiClass::R,
            0x0600..=0x06FF | 0x0750..=0x077F => BidiClass::AL,
            0x0030..=0x0039 => BidiClass::EN,
            0x002B | 0x002D => BidiClass::ES,
            0x0023..=0x0025 => BidiClass::ET,
            0x0660..=0x0669 | 0x06F0..=0x06F9 => BidiClass::AN,
            0x002C | 0x002E | 0x003A => BidiClass::CS,
            0x0300..=0x036F | 0x0591..=0x05BD => BidiClass::NSM,
            0x000A | 0x000D | 0x001C..=0x001E => BidiClass::B,
            0x0009 | 0x000B | 0x001F => BidiClass::S,
            0x0020 | 0x1680 | 0x2000..=0x200A | 0x3000 => BidiClass::WS,
            0x202A => BidiClass::LRE,
            0x202B => BidiClass::RLE,
            0x202C => BidiClass::PDF,
            0x202D => BidiClass::LRO,
            0x202E => BidiClass::RLO,
            0x2066 => BidiClass::LRI,
            0x2067 => BidiClass::RLI,
            0x2068 => BidiClass::FSI,
            0x2069 => BidiClass::PDI,
            _ => BidiClass::ON,
        }
    }

    pub fn arabic_joining(cp: u32) -> ArabicJoiningType {
        match cp {
            0x0627
            | 0x0622
            | 0x0623
            | 0x0625
            | 0x0672..=0x0673
            | 0x0621
            | 0x062F..=0x0632
            | 0x0648
            | 0x0698 => ArabicJoiningType::Right,
            0x0628
            | 0x062A..=0x062E
            | 0x0633..=0x063A
            | 0x0641..=0x0647
            | 0x0649..=0x064A
            | 0x066E..=0x066F => ArabicJoiningType::Dual,
            0x0640 => ArabicJoiningType::Causing,
            0x064B..=0x065F | 0x0670 => ArabicJoiningType::Transparent,
            _ => ArabicJoiningType::NonJoining,
        }
    }

    pub fn shape_run(&self, codepoints: &[u32], font: &FontHandle) -> Vec<ShapedGlyph> {
        let mut result = Vec::with_capacity(codepoints.len());
        for (i, &cp) in codepoints.iter().enumerate() {
            let glyph_id = font.glyph_index(cp).unwrap_or(0);
            let advance = font.advance_width(glyph_id) as i32;
            result.push(ShapedGlyph {
                glyph_id,
                cluster: i as u32,
                x_advance: advance,
                y_advance: 0,
                x_offset: 0,
                y_offset: 0,
            });
        }
        if self.direction == TextDirection::Rtl {
            result.reverse();
        }
        for i in 1..result.len() {
            let left_gid = result[i - 1].glyph_id;
            let right_gid = result[i].glyph_id;
            let kern = font.kerning(left_gid, right_gid);
            result[i].x_offset += kern as i32;
        }
        result
    }

    pub fn hangul_decompose(syllable: u32) -> Option<(u32, u32, Option<u32>)> {
        const S_BASE: u32 = 0xAC00;
        const L_BASE: u32 = 0x1100;
        const V_BASE: u32 = 0x1161;
        const T_BASE: u32 = 0x11A7;
        const V_COUNT: u32 = 21;
        const T_COUNT: u32 = 28;
        const N_COUNT: u32 = V_COUNT * T_COUNT;
        if syllable < S_BASE || syllable > 0xD7A3 {
            return None;
        }
        let s_index = syllable - S_BASE;
        let l_index = s_index / N_COUNT;
        let v_index = (s_index % N_COUNT) / T_COUNT;
        let t_index = s_index % T_COUNT;
        let lead = L_BASE + l_index;
        let vowel = V_BASE + v_index;
        let trail = if t_index > 0 {
            Some(T_BASE + t_index)
        } else {
            None
        };
        Some((lead, vowel, trail))
    }

    pub fn is_grapheme_break(a: u32, b: u32) -> bool {
        let cat_a = Self::grapheme_category(a);
        let cat_b = Self::grapheme_category(b);
        match (cat_a, cat_b) {
            (GraphemeCat::CR, GraphemeCat::LF) => false,
            (GraphemeCat::Control | GraphemeCat::CR | GraphemeCat::LF, _) => true,
            (_, GraphemeCat::Control | GraphemeCat::CR | GraphemeCat::LF) => true,
            (_, GraphemeCat::Extend | GraphemeCat::ZWJ) => false,
            (_, GraphemeCat::SpacingMark) => false,
            (GraphemeCat::Prepend, _) => false,
            _ => true,
        }
    }

    fn grapheme_category(cp: u32) -> GraphemeCat {
        match cp {
            0x000D => GraphemeCat::CR,
            0x000A => GraphemeCat::LF,
            0x0000..=0x001F | 0x007F..=0x009F | 0x200B => GraphemeCat::Control,
            0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F => GraphemeCat::Extend,
            0x200D => GraphemeCat::ZWJ,
            0x0903 | 0x093B | 0x093E..=0x0940 | 0x0949..=0x094C => GraphemeCat::SpacingMark,
            0x0600..=0x0605 | 0x06DD | 0x070F | 0x0890..=0x0891 => GraphemeCat::Prepend,
            _ => GraphemeCat::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphemeCat {
    CR,
    LF,
    Control,
    Extend,
    ZWJ,
    SpacingMark,
    Prepend,
    Other,
}

// ═══════════════════════════════════════════════════════════════════════════
// Unicode normalization (NFC, NFD, NFKC, NFKD)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizationForm {
    Nfc,
    Nfd,
    Nfkc,
    Nfkd,
}

pub fn canonical_combining_class(cp: u32) -> u8 {
    match cp {
        0x0300 => 230,
        0x0301 => 230,
        0x0302 => 230,
        0x0303 => 230,
        0x0304 => 230,
        0x0305 => 230,
        0x0306 => 230,
        0x0307 => 230,
        0x0308 => 230,
        0x0309 => 230,
        0x030A => 230,
        0x030B => 230,
        0x030C => 230,
        0x030D => 230,
        0x030E => 230,
        0x030F => 230,
        0x0327 => 202,
        0x0328 => 202,
        0x0316..=0x0319 => 220,
        0x031C..=0x0320 => 220,
        0x0323..=0x0326 => 220,
        _ => 0,
    }
}

pub fn normalize(input: &[u32], form: NormalizationForm) -> Vec<u32> {
    let mut decomposed = Vec::with_capacity(input.len());
    for &cp in input {
        decompose_char(cp, form, &mut decomposed);
    }
    canonical_reorder(&mut decomposed);
    if matches!(form, NormalizationForm::Nfc | NormalizationForm::Nfkc) {
        compose(&mut decomposed);
    }
    decomposed
}

fn decompose_char(cp: u32, _form: NormalizationForm, out: &mut Vec<u32>) {
    match cp {
        0x00C0 => {
            out.push(0x0041);
            out.push(0x0300);
        }
        0x00C1 => {
            out.push(0x0041);
            out.push(0x0301);
        }
        0x00C2 => {
            out.push(0x0041);
            out.push(0x0302);
        }
        0x00C3 => {
            out.push(0x0041);
            out.push(0x0303);
        }
        0x00C4 => {
            out.push(0x0041);
            out.push(0x0308);
        }
        0x00C7 => {
            out.push(0x0043);
            out.push(0x0327);
        }
        0x00C8 => {
            out.push(0x0045);
            out.push(0x0300);
        }
        0x00C9 => {
            out.push(0x0045);
            out.push(0x0301);
        }
        0x00CA => {
            out.push(0x0045);
            out.push(0x0302);
        }
        0x00CB => {
            out.push(0x0045);
            out.push(0x0308);
        }
        0x00D1 => {
            out.push(0x004E);
            out.push(0x0303);
        }
        0x00D6 => {
            out.push(0x004F);
            out.push(0x0308);
        }
        0x00DC => {
            out.push(0x0055);
            out.push(0x0308);
        }
        0x00E0 => {
            out.push(0x0061);
            out.push(0x0300);
        }
        0x00E1 => {
            out.push(0x0061);
            out.push(0x0301);
        }
        0x00E2 => {
            out.push(0x0061);
            out.push(0x0302);
        }
        0x00E3 => {
            out.push(0x0061);
            out.push(0x0303);
        }
        0x00E4 => {
            out.push(0x0061);
            out.push(0x0308);
        }
        0x00E7 => {
            out.push(0x0063);
            out.push(0x0327);
        }
        0x00E8 => {
            out.push(0x0065);
            out.push(0x0300);
        }
        0x00E9 => {
            out.push(0x0065);
            out.push(0x0301);
        }
        0x00EA => {
            out.push(0x0065);
            out.push(0x0302);
        }
        0x00EB => {
            out.push(0x0065);
            out.push(0x0308);
        }
        0x00F1 => {
            out.push(0x006E);
            out.push(0x0303);
        }
        0x00F6 => {
            out.push(0x006F);
            out.push(0x0308);
        }
        0x00FC => {
            out.push(0x0075);
            out.push(0x0308);
        }
        _ => {
            out.push(cp);
        }
    }
}

fn canonical_reorder(buf: &mut [u32]) {
    let len = buf.len();
    if len < 2 {
        return;
    }
    let mut i = 1;
    while i < len {
        let ccc = canonical_combining_class(buf[i]);
        if ccc != 0 {
            let mut j = i;
            while j > 0 && canonical_combining_class(buf[j - 1]) > ccc {
                buf.swap(j, j - 1);
                j -= 1;
            }
        }
        i += 1;
    }
}

fn compose(buf: &mut Vec<u32>) {
    if buf.len() < 2 {
        return;
    }
    let mut i = 0;
    while i + 1 < buf.len() {
        if let Some(composed) = try_compose(buf[i], buf[i + 1]) {
            buf[i] = composed;
            buf.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

fn try_compose(a: u32, b: u32) -> Option<u32> {
    match (a, b) {
        (0x0041, 0x0300) => Some(0x00C0),
        (0x0041, 0x0301) => Some(0x00C1),
        (0x0041, 0x0302) => Some(0x00C2),
        (0x0041, 0x0303) => Some(0x00C3),
        (0x0041, 0x0308) => Some(0x00C4),
        (0x0043, 0x0327) => Some(0x00C7),
        (0x0045, 0x0300) => Some(0x00C8),
        (0x0045, 0x0301) => Some(0x00C9),
        (0x0045, 0x0302) => Some(0x00CA),
        (0x0045, 0x0308) => Some(0x00CB),
        (0x004E, 0x0303) => Some(0x00D1),
        (0x004F, 0x0308) => Some(0x00D6),
        (0x0055, 0x0308) => Some(0x00DC),
        (0x0061, 0x0300) => Some(0x00E0),
        (0x0061, 0x0301) => Some(0x00E1),
        (0x0061, 0x0302) => Some(0x00E2),
        (0x0061, 0x0303) => Some(0x00E3),
        (0x0061, 0x0308) => Some(0x00E4),
        (0x0063, 0x0327) => Some(0x00E7),
        (0x0065, 0x0300) => Some(0x00E8),
        (0x0065, 0x0301) => Some(0x00E9),
        (0x0065, 0x0302) => Some(0x00EA),
        (0x0065, 0x0308) => Some(0x00EB),
        (0x006E, 0x0303) => Some(0x00F1),
        (0x006F, 0x0308) => Some(0x00F6),
        (0x0075, 0x0308) => Some(0x00FC),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Font metrics
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct FontMetrics {
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub x_height: i16,
    pub cap_height: i16,
    pub underline_position: i16,
    pub underline_thickness: i16,
    pub strikeout_position: i16,
    pub strikeout_thickness: i16,
}

#[derive(Debug, Clone, Copy)]
pub struct GlyphMetrics {
    pub advance_width: u16,
    pub left_side_bearing: i16,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
}

// ═══════════════════════════════════════════════════════════════════════════
// Font matching & fallback
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FontWeight {
    Thin = 100,
    ExtraLight = 200,
    Light = 300,
    Regular = 400,
    Medium = 500,
    SemiBold = 600,
    Bold = 700,
    ExtraBold = 800,
    Black = 900,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontWidth {
    UltraCondensed = 1,
    ExtraCondensed = 2,
    Condensed = 3,
    SemiCondensed = 4,
    Normal = 5,
    SemiExpanded = 6,
    Expanded = 7,
    ExtraExpanded = 8,
    UltraExpanded = 9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSlant {
    Normal,
    Italic,
    Oblique,
}

#[derive(Debug, Clone)]
pub struct FontPattern {
    pub family: String,
    pub weight: FontWeight,
    pub width: FontWidth,
    pub slant: FontSlant,
}

impl Default for FontPattern {
    fn default() -> Self {
        Self {
            family: String::new(),
            weight: FontWeight::Regular,
            width: FontWidth::Normal,
            slant: FontSlant::Normal,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Glyph cache (LRU)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct GlyphCacheKey {
    glyph_id: u16,
    ppem: u16,
    subpixel: u8,
}

pub struct GlyphCache {
    capacity: usize,
    entries: Vec<(GlyphCacheKey, RasterizedGlyph)>,
    access_order: Vec<usize>,
}

impl GlyphCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Vec::with_capacity(capacity),
            access_order: Vec::with_capacity(capacity),
        }
    }

    pub fn get(
        &mut self,
        glyph_id: u16,
        ppem: u16,
        subpixel: SubpixelMode,
    ) -> Option<&RasterizedGlyph> {
        let key = GlyphCacheKey {
            glyph_id,
            ppem,
            subpixel: subpixel as u8,
        };
        if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
            self.access_order.retain(|&i| i != pos);
            self.access_order.push(pos);
            Some(&self.entries[pos].1)
        } else {
            None
        }
    }

    pub fn insert(
        &mut self,
        glyph_id: u16,
        ppem: u16,
        subpixel: SubpixelMode,
        glyph: RasterizedGlyph,
    ) {
        let key = GlyphCacheKey {
            glyph_id,
            ppem,
            subpixel: subpixel as u8,
        };
        if self.entries.len() >= self.capacity && !self.access_order.is_empty() {
            let evict = self.access_order.remove(0);
            self.entries.remove(evict);
            for idx in &mut self.access_order {
                if *idx > evict {
                    *idx -= 1;
                }
            }
        }
        let pos = self.entries.len();
        self.entries.push((key, glyph));
        self.access_order.push(pos);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.access_order.clear();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Font handle — high-level per-font API
// ═══════════════════════════════════════════════════════════════════════════

pub struct FontHandle {
    pub data: Vec<u8>,
    pub offset_table: OffsetTable,
    pub head: HeadTable,
    pub hhea: HheaTable,
    pub hmtx: HmtxTable,
    pub maxp: MaxpTable,
    pub loca: LocaTable,
    pub cmap: CmapTable,
    pub name: NameTable,
    pub post: PostTable,
    pub kern: Option<KernTable>,
    pub os2: Option<Os2Table>,
    pub gdef: Option<GdefTable>,
    pub gasp: Option<GaspTable>,
    pub fvar: Option<FvarTable>,
    pub colr: Option<ColrTable>,
    pub cpal: Option<CpalTable>,
}

impl FontHandle {
    pub fn from_bytes(data: Vec<u8>) -> Option<Self> {
        let ot = OffsetTable::parse(&data)?;
        let table_data = |tag: &[u8; 4]| -> Option<&[u8]> {
            let rec = ot.find_table(tag)?;
            data.get(rec.offset as usize..(rec.offset + rec.length) as usize)
        };
        let head = HeadTable::parse(table_data(&TAG_HEAD)?)?;
        let hhea = HheaTable::parse(table_data(&TAG_HHEA)?)?;
        let maxp = MaxpTable::parse(table_data(&TAG_MAXP)?)?;
        let loca = LocaTable::parse(table_data(&TAG_LOCA)?, maxp.num_glyphs, head.is_long_loca())?;
        let hmtx = HmtxTable::parse(
            table_data(&TAG_HMTX)?,
            hhea.number_of_h_metrics,
            maxp.num_glyphs,
        )?;
        let cmap = CmapTable::parse(table_data(&TAG_CMAP)?)?;
        let name = NameTable::parse(table_data(&TAG_NAME)?)?;
        let post = PostTable::parse(table_data(&TAG_POST)?)?;
        let kern = table_data(&TAG_KERN).and_then(KernTable::parse);
        let os2 = table_data(&TAG_OS2).and_then(Os2Table::parse);
        let gdef = table_data(&TAG_GDEF).and_then(GdefTable::parse);
        let gasp = table_data(&TAG_GASP).and_then(GaspTable::parse);
        let fvar = table_data(&TAG_FVAR).and_then(FvarTable::parse);
        let colr = table_data(&TAG_COLR).and_then(ColrTable::parse);
        let cpal = table_data(&TAG_CPAL).and_then(CpalTable::parse);
        Some(FontHandle {
            data,
            offset_table: ot,
            head,
            hhea,
            hmtx,
            maxp,
            loca,
            cmap,
            name,
            post,
            kern,
            os2,
            gdef,
            gasp,
            fvar,
            colr,
            cpal,
        })
    }

    pub fn family_name(&self) -> Option<String> {
        self.name.get_name(NAME_FAMILY)
    }
    pub fn full_name(&self) -> Option<String> {
        self.name.get_name(NAME_FULL_NAME)
    }
    pub fn postscript_name(&self) -> Option<String> {
        self.name.get_name(NAME_POSTSCRIPT)
    }
    pub fn num_glyphs(&self) -> u16 {
        self.maxp.num_glyphs
    }

    pub fn glyph_index(&self, codepoint: u32) -> Option<u16> {
        self.cmap.lookup(codepoint)
    }

    pub fn advance_width(&self, glyph_id: u16) -> u16 {
        self.hmtx.advance_width(glyph_id)
    }
    pub fn left_side_bearing(&self, glyph_id: u16) -> i16 {
        self.hmtx.left_side_bearing(glyph_id)
    }

    pub fn kerning(&self, left: u16, right: u16) -> i16 {
        self.kern.as_ref().map_or(0, |k| k.get_kerning(left, right))
    }

    pub fn metrics(&self) -> FontMetrics {
        FontMetrics {
            units_per_em: self.head.units_per_em,
            ascender: self.hhea.ascender,
            descender: self.hhea.descender,
            line_gap: self.hhea.line_gap,
            x_height: self.os2.as_ref().map_or(0, |o| o.s_x_height),
            cap_height: self.os2.as_ref().map_or(0, |o| o.s_cap_height),
            underline_position: self.post.underline_position,
            underline_thickness: self.post.underline_thickness,
            strikeout_position: self.os2.as_ref().map_or(0, |o| o.y_strikeout_position),
            strikeout_thickness: self.os2.as_ref().map_or(0, |o| o.y_strikeout_size),
        }
    }

    pub fn glyph_metrics(&self, glyph_id: u16) -> Option<GlyphMetrics> {
        let (start, end) = self.loca.glyph_range(glyph_id)?;
        let glyf_rec = self.offset_table.find_table(&TAG_GLYF)?;
        let abs_start = glyf_rec.offset as usize + start as usize;
        let abs_end = glyf_rec.offset as usize + end as usize;
        let glyph_data = self.data.get(abs_start..abs_end)?;
        let glyph = parse_glyph(glyph_data)?;
        let (x_min, y_min, x_max, y_max) = match &glyph {
            Glyph::Empty => (0, 0, 0, 0),
            Glyph::Simple(sg) => (sg.x_min, sg.y_min, sg.x_max, sg.y_max),
            Glyph::Composite(cg) => (cg.x_min, cg.y_min, cg.x_max, cg.y_max),
        };
        Some(GlyphMetrics {
            advance_width: self.hmtx.advance_width(glyph_id),
            left_side_bearing: self.hmtx.left_side_bearing(glyph_id),
            x_min,
            y_min,
            x_max,
            y_max,
        })
    }

    pub fn is_variable(&self) -> bool {
        self.fvar.is_some()
    }

    pub fn variation_axes(&self) -> &[VariationAxis] {
        self.fvar.as_ref().map_or(&[], |f| &f.axes)
    }

    pub fn is_color(&self) -> bool {
        self.colr.is_some() || self.cpal.is_some()
    }

    pub fn weight_class(&self) -> u16 {
        self.os2.as_ref().map_or(400, |o| o.us_weight_class)
    }

    pub fn width_class(&self) -> u16 {
        self.os2.as_ref().map_or(5, |o| o.us_width_class)
    }

    pub fn is_italic(&self) -> bool {
        self.os2
            .as_ref()
            .map_or(false, |o| o.fs_selection & 0x01 != 0)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// FontDatabase — enumeration, matching, fallback
// ═══════════════════════════════════════════════════════════════════════════

pub struct FontDatabase {
    fonts: Vec<FontEntry>,
    fallback_chain: Vec<usize>,
}

struct FontEntry {
    handle: FontHandle,
    family: String,
    weight: u16,
    width: u16,
    italic: bool,
}

impl FontDatabase {
    pub fn new() -> Self {
        Self {
            fonts: Vec::new(),
            fallback_chain: Vec::new(),
        }
    }

    pub fn add_font(&mut self, data: Vec<u8>) -> Option<usize> {
        let handle = FontHandle::from_bytes(data)?;
        let family = handle.family_name().unwrap_or_default();
        let weight = handle.weight_class();
        let width = handle.width_class();
        let italic = handle.is_italic();
        let idx = self.fonts.len();
        self.fonts.push(FontEntry {
            handle,
            family,
            weight,
            width,
            italic,
        });
        Some(idx)
    }

    pub fn add_ttc(&mut self, data: Vec<u8>) -> Vec<usize> {
        let mut indices = Vec::new();
        if let Some(ttc) = TtcHeader::parse(&data) {
            for &offset in &ttc.offsets {
                if let Some(ot) = OffsetTable::parse(&data[offset as usize..]) {
                    let font_data = data.clone();
                    if let Some(idx) = self.add_font(font_data) {
                        indices.push(idx);
                    }
                }
            }
        }
        indices
    }

    pub fn font_count(&self) -> usize {
        self.fonts.len()
    }

    pub fn font(&self, index: usize) -> Option<&FontHandle> {
        self.fonts.get(index).map(|e| &e.handle)
    }

    pub fn match_font(&self, pattern: &FontPattern) -> Option<usize> {
        let mut best_idx = None;
        let mut best_score = u32::MAX;
        for (i, entry) in self.fonts.iter().enumerate() {
            let mut score = 0u32;
            if !pattern.family.is_empty() && entry.family != pattern.family {
                score += 10000;
            }
            let target_weight = pattern.weight as u16;
            score += (entry.weight as i32 - target_weight as i32).unsigned_abs();
            let target_width = pattern.width as u16;
            score += ((entry.width as i32 - target_width as i32).unsigned_abs()) * 10;
            if entry.italic != (pattern.slant != FontSlant::Normal) {
                score += 1000;
            }
            if score < best_score {
                best_score = score;
                best_idx = Some(i);
            }
        }
        best_idx
    }

    pub fn set_fallback_chain(&mut self, chain: Vec<usize>) {
        self.fallback_chain = chain;
    }

    pub fn resolve_glyph(&self, codepoint: u32, preferred: usize) -> Option<(usize, u16)> {
        if let Some(entry) = self.fonts.get(preferred) {
            if let Some(gid) = entry.handle.glyph_index(codepoint) {
                return Some((preferred, gid));
            }
        }
        for &idx in &self.fallback_chain {
            if let Some(entry) = self.fonts.get(idx) {
                if let Some(gid) = entry.handle.glyph_index(codepoint) {
                    return Some((idx, gid));
                }
            }
        }
        None
    }

    pub fn enumerate_families(&self) -> Vec<String> {
        let mut families: Vec<String> = self.fonts.iter().map(|e| e.family.clone()).collect();
        families.sort();
        families.dedup();
        families
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Global FONT_ENGINE
// ═══════════════════════════════════════════════════════════════════════════

static FONT_ENGINE_INIT: AtomicBool = AtomicBool::new(false);

pub struct FontEngine {
    pub database: FontDatabase,
    pub shaper: TextShaper,
    pub rasterizer: Rasterizer,
    pub glyph_cache: GlyphCache,
}

static mut FONT_ENGINE: Option<FontEngine> = None;

impl FontEngine {
    pub fn init() {
        if FONT_ENGINE_INIT
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            unsafe {
                FONT_ENGINE = Some(FontEngine {
                    database: FontDatabase::new(),
                    shaper: TextShaper::new(TextDirection::Ltr),
                    rasterizer: Rasterizer::new(RasterConfig::default()),
                    glyph_cache: GlyphCache::new(4096),
                });
            }
        }
    }

    pub fn get() -> Option<&'static mut FontEngine> {
        if FONT_ENGINE_INIT.load(Ordering::SeqCst) {
            unsafe { FONT_ENGINE.as_mut() }
        } else {
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Crisp filled-outline rasterizer (the wired text path)
//
// The original `Rasterizer::rasterize` walks the contour and stamps the OUTLINE
// (`draw_line` saturating-adds 128 along edges) — it produces a hollow wireframe,
// not a filled glyph, so it cannot render crisp UI text. The path below is a
// proper scanline polygon fill with grayscale area-coverage AA: flatten the
// quadratic contours into line segments, then for each pixel row sample
// `SUBSAMPLES` sub-scanlines, count even-odd edge crossings to find spans inside
// the glyph, and accumulate per-pixel horizontal coverage. This is the
// FreeType-class pipeline `docs/design/typography-rendering.md` §"AA correctness"
// specifies. Output is an 8-bit coverage mask = source alpha for source-over.
// ═══════════════════════════════════════════════════════════════════════════

/// Vertical sub-scanlines per pixel row (grayscale AA quality). 4 = good
/// quality / cheap; matches the spec's grayscale-only decision.
const FILL_SUBSAMPLES: u32 = 4;

/// A scaled, device-space line segment (one flattened contour edge).
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rasterizer {
    /// Rasterize a simple glyph as a FILLED, grayscale-anti-aliased coverage
    /// mask. This is the crisp path used by `Canvas::draw_text_aa`. Returns a
    /// `RasterizedGlyph` whose `pixels` are 8-bit coverage (0 = no ink,
    /// 255 = solid), `width`/`height` the tight bitmap, and `bearing_*` the
    /// pen offsets (bearing_y = pixels from baseline UP to the bitmap top row).
    ///
    /// `units_per_em` comes from `head`; `self.config.ppem` is the pixel size.
    pub fn rasterize_filled(&mut self, glyph: &SimpleGlyph, units_per_em: u16) -> RasterizedGlyph {
        let upem = units_per_em.max(1) as f32;
        let scale = self.config.ppem as f32 / upem;

        // Device-space bbox (y is flipped to top-down screen space below).
        let x_min = f32_floor(glyph.x_min as f32 * scale);
        let x_max = f32_ceil(glyph.x_max as f32 * scale);
        let y_min = f32_floor(glyph.y_min as f32 * scale);
        let y_max = f32_ceil(glyph.y_max as f32 * scale);

        let w = ((x_max - x_min) as i32).max(1) as u32;
        let h = ((y_max - y_min) as i32).max(1) as u32;
        // Cap pathological sizes (corrupt glyph / absurd ppem) so we never
        // allocate gigabytes; UI text never approaches this.
        if w > 4096 || h > 4096 {
            return RasterizedGlyph {
                width: 1,
                height: 1,
                bearing_x: 0,
                bearing_y: 0,
                advance: (glyph.x_max as f32 * scale) as i32,
                pixels: vec![0u8],
            };
        }

        // Flatten all contours to device-space edges. Screen space is top-down,
        // so y' = y_max - (font_y * scale): the glyph's top maps to row 0.
        let mut edges: Vec<Edge> = Vec::new();
        let map = |px: f32, py: f32| -> (f32, f32) { (px * scale - x_min, y_max - py * scale) };
        let mut start = 0usize;
        for &end_idx in &glyph.contour_ends {
            let end = (end_idx as usize + 1).min(glyph.points.len());
            let pts = &glyph.points[start..end];
            start = end;
            if pts.len() < 2 {
                continue;
            }
            flatten_contour(pts, &map, &mut edges);
        }
        if edges.is_empty() {
            return RasterizedGlyph {
                width: w,
                height: h,
                bearing_x: x_min as i32,
                bearing_y: y_max as i32,
                advance: (glyph.x_max as f32 * scale) as i32,
                pixels: vec![0u8; (w * h) as usize],
            };
        }

        let mut pixels = vec![0u8; (w * h) as usize];
        // Per-row coverage accumulator (one f32 per pixel column).
        let mut cov = vec![0.0f32; w as usize];
        let sub = FILL_SUBSAMPLES;
        let sub_weight = 1.0 / sub as f32;
        let mut xs: Vec<f32> = Vec::new();

        for row in 0..h {
            for c in cov.iter_mut() {
                *c = 0.0;
            }
            for s in 0..sub {
                // Sample y at the center of each sub-scanline.
                let sy = row as f32 + (s as f32 + 0.5) / sub as f32;
                xs.clear();
                for e in &edges {
                    let (ey0, ey1) = (e.y0, e.y1);
                    // Half-open [min,max) crossing test (avoids double-count at
                    // shared vertices).
                    let (lo, hi, x_lo, x_hi) = if ey0 < ey1 {
                        (ey0, ey1, e.x0, e.x1)
                    } else {
                        (ey1, ey0, e.x1, e.x0)
                    };
                    if sy >= lo && sy < hi {
                        let t = (sy - lo) / (hi - lo);
                        xs.push(x_lo + (x_hi - x_lo) * t);
                    }
                }
                if xs.len() < 2 {
                    continue;
                }
                xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
                // Even-odd fill: span between consecutive crossing pairs is inside.
                let mut i = 0;
                while i + 1 < xs.len() {
                    let xa = xs[i].max(0.0);
                    let xb = xs[i + 1].min(w as f32);
                    if xb > xa {
                        accumulate_span(&mut cov, xa, xb, sub_weight);
                    }
                    i += 2;
                }
            }
            let base = (row * w) as usize;
            for (col, &c) in cov.iter().enumerate() {
                let mut a = c;
                if a > 1.0 {
                    a = 1.0;
                }
                pixels[base + col] = (a * 255.0 + 0.5) as u8;
            }
        }

        RasterizedGlyph {
            width: w,
            height: h,
            bearing_x: x_min as i32,
            bearing_y: y_max as i32,
            advance: (glyph.x_max as f32 * scale) as i32,
            pixels,
        }
    }
}

/// Flatten one closed contour (on/off-curve TrueType points) to device-space
/// edges, subdividing each quadratic Bézier into line segments.
fn flatten_contour<F: Fn(f32, f32) -> (f32, f32)>(
    pts: &[GlyphPoint],
    map: &F,
    out: &mut Vec<Edge>,
) {
    let n = pts.len();
    // Find a starting on-curve point; if none, synthesize one (midpoint of the
    // first two off-curve points — standard TrueType implied-point rule).
    let mut start_pt: Option<(f32, f32)> = None;
    let mut start_idx = 0usize;
    for (i, p) in pts.iter().enumerate() {
        if p.on_curve {
            start_pt = Some(map(p.x as f32, p.y as f32));
            start_idx = i;
            break;
        }
    }
    let (mut cur, start_off) = match start_pt {
        Some(p) => (p, 0usize),
        None => {
            // All off-curve: implied start = midpoint of pts[0] and pts[1].
            let a = map(pts[0].x as f32, pts[0].y as f32);
            let b = map(pts[1].x as f32, pts[1].y as f32);
            (((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5), 0usize)
        }
    };
    let first = cur;
    let _ = start_off;

    let mut pending_ctrl: Option<(f32, f32)> = None;
    for k in 1..=n {
        let idx = (start_idx + k) % n;
        let p = &pts[idx];
        let dp = map(p.x as f32, p.y as f32);
        if p.on_curve {
            match pending_ctrl.take() {
                Some(ctrl) => {
                    push_quad(cur, ctrl, dp, out);
                }
                None => {
                    out.push(Edge {
                        x0: cur.0,
                        y0: cur.1,
                        x1: dp.0,
                        y1: dp.1,
                    });
                }
            }
            cur = dp;
        } else {
            match pending_ctrl.take() {
                Some(ctrl) => {
                    // Two consecutive off-curve: implied on-curve midpoint.
                    let mid = ((ctrl.0 + dp.0) * 0.5, (ctrl.1 + dp.1) * 0.5);
                    push_quad(cur, ctrl, mid, out);
                    cur = mid;
                    pending_ctrl = Some(dp);
                }
                None => {
                    pending_ctrl = Some(dp);
                }
            }
        }
    }
    // Close the contour back to the first point.
    match pending_ctrl.take() {
        Some(ctrl) => push_quad(cur, ctrl, first, out),
        None => out.push(Edge {
            x0: cur.0,
            y0: cur.1,
            x1: first.0,
            y1: first.1,
        }),
    }
}

/// Subdivide a quadratic Bézier (start, control, end) into line edges.
fn push_quad(p0: (f32, f32), c: (f32, f32), p1: (f32, f32), out: &mut Vec<Edge>) {
    const STEPS: u32 = 8;
    let mut prev = p0;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        let inv = 1.0 - t;
        let x = inv * inv * p0.0 + 2.0 * inv * t * c.0 + t * t * p1.0;
        let y = inv * inv * p0.1 + 2.0 * inv * t * c.1 + t * t * p1.1;
        out.push(Edge {
            x0: prev.0,
            y0: prev.1,
            x1: x,
            y1: y,
        });
        prev = (x, y);
    }
}

/// Add `weight` coverage to columns covered by the span `[xa, xb)`, with
/// fractional coverage on the two partial edge pixels (analytic horizontal AA).
fn accumulate_span(cov: &mut [f32], xa: f32, xb: f32, weight: f32) {
    let xi0 = f32_floor(xa) as i32;
    let xi1 = f32_ceil(xb) as i32;
    if xi0 == xi1 {
        return;
    }
    for col in xi0..xi1 {
        if col < 0 || col as usize >= cov.len() {
            continue;
        }
        let cell_l = col as f32;
        let cell_r = cell_l + 1.0;
        let covered = xb.min(cell_r) - xa.max(cell_l);
        if covered > 0.0 {
            cov[col as usize] += covered * weight;
        }
    }
}

impl FontHandle {
    /// Extract the parsed outline for a glyph id, resolving composite glyphs by
    /// flattening their components into a single `SimpleGlyph` (device-unit
    /// space). Returns `None` for empty/space glyphs.
    pub fn glyph_outline(&self, glyph_id: u16) -> Option<SimpleGlyph> {
        self.glyph_outline_depth(glyph_id, 0)
    }

    fn glyph_outline_depth(&self, glyph_id: u16, depth: u8) -> Option<SimpleGlyph> {
        if depth > 5 {
            return None; // composite recursion guard
        }
        let (start, end) = self.loca.glyph_range(glyph_id)?;
        if end <= start {
            return None; // empty glyph (e.g. space)
        }
        let glyf_rec = self.offset_table.find_table(&TAG_GLYF)?;
        let abs_start = glyf_rec.offset as usize + start as usize;
        let abs_end = glyf_rec.offset as usize + end as usize;
        let glyph_data = self.data.get(abs_start..abs_end)?;
        match parse_glyph(glyph_data)? {
            Glyph::Simple(sg) => Some(sg),
            Glyph::Empty => None,
            Glyph::Composite(cg) => {
                // Merge components into one SimpleGlyph in font units.
                let mut points: Vec<GlyphPoint> = Vec::new();
                let mut contour_ends: Vec<u16> = Vec::new();
                let (mut gx_min, mut gy_min, mut gx_max, mut gy_max) =
                    (i16::MAX, i16::MAX, i16::MIN, i16::MIN);
                for comp in &cg.components {
                    let sub = self.glyph_outline_depth(comp.glyph_index, depth + 1)?;
                    // Only XY-offset placement supported (the overwhelmingly
                    // common case for Latin accented glyphs); scale applied too.
                    let (dx, dy) = if comp.flags & COMP_ARGS_ARE_XY_VALUES != 0 {
                        (comp.arg1 as f32, comp.arg2 as f32)
                    } else {
                        (0.0, 0.0)
                    };
                    let base = points.len();
                    for p in &sub.points {
                        let fx = p.x as f32;
                        let fy = p.y as f32;
                        let nx = comp.scale_xx * fx + comp.scale_yx * fy + dx;
                        let ny = comp.scale_xy * fx + comp.scale_yy * fy + dy;
                        let xi = nx as i16;
                        let yi = ny as i16;
                        gx_min = gx_min.min(xi);
                        gy_min = gy_min.min(yi);
                        gx_max = gx_max.max(xi);
                        gy_max = gy_max.max(yi);
                        points.push(GlyphPoint {
                            x: xi,
                            y: yi,
                            on_curve: p.on_curve,
                        });
                    }
                    for ce in &sub.contour_ends {
                        contour_ends.push(*ce + base as u16);
                    }
                }
                if points.is_empty() {
                    return None;
                }
                Some(SimpleGlyph {
                    contour_ends,
                    instructions: Vec::new(),
                    points,
                    x_min: gx_min,
                    y_min: gy_min,
                    x_max: gx_max,
                    y_max: gy_max,
                })
            }
        }
    }

    /// Convenience: look up `codepoint`, extract its outline, and rasterize a
    /// FILLED grayscale-AA coverage mask at `ppem`. Returns `None` for missing
    /// or empty (space) glyphs — the caller should advance by the metric width
    /// in that case. This is the single entry point `raegfx` uses per glyph.
    pub fn rasterize_codepoint(&self, codepoint: u32, ppem: u16) -> Option<RasterizedGlyph> {
        // A cmap hit to gid 0 (notdef) is NOT real coverage — treat it as a miss
        // so callers fall through to the supplement / notdef box instead of
        // stamping the font's own tofu box silently.
        let gid = self.cmap.lookup(codepoint).filter(|&g| g != 0)?;
        let outline = self.glyph_outline(gid)?;
        let mut cfg = RasterConfig::default();
        cfg.ppem = ppem.max(1);
        cfg.gamma = 1.0; // gamma handled by the blend, not the coverage mask
        cfg.subpixel = SubpixelMode::None; // grayscale only (spec decision)
        let mut rast = Rasterizer::new(cfg);
        let mut g = rast.rasterize_filled(&outline, self.head.units_per_em);
        // Use the proper hmtx advance (rasterize_filled defaults to x_max, which
        // is wrong for pen positioning — it ignores side bearings/spacing).
        g.advance = self.advance_px(gid, ppem).max(1);
        Some(g)
    }

    /// True iff this face has a real (non-notdef) glyph for `cp`.
    pub fn has_glyph(&self, cp: u32) -> bool {
        matches!(self.cmap.lookup(cp), Some(g) if g != 0)
    }

    /// The full coverage path the UI should use: the embedded face, then the
    /// procedural symbol supplement for UI glyphs the OFL faces lack, then an
    /// explicit visible notdef box at the correct advance. NEVER returns `None`
    /// for a printable codepoint — an uncovered glyph is a deliberate tofu box,
    /// not an invisible gap. Space / zero-width chars still rasterize empty (the
    /// caller advances by the metric), preserving the existing `None` contract
    /// only for those.
    pub fn rasterize_codepoint_or_fallback(
        &self,
        codepoint: u32,
        ppem: u16,
    ) -> Option<RasterizedGlyph> {
        // 1. Real font glyph.
        if let Some(g) = self.rasterize_codepoint(codepoint, ppem) {
            return Some(g);
        }
        // 2. Whitespace / format chars: no ink, advance by metric — keep None so
        //    the existing space behavior (and word layout) is unchanged.
        if is_blank_codepoint(codepoint) {
            return None;
        }
        // 3. Procedural UI-symbol supplement (✕ ⇄ ☰ ⚙ …).
        if let Some(outline) = synth_symbol_glyph(codepoint) {
            let mut cfg = RasterConfig::default();
            cfg.ppem = ppem.max(1);
            cfg.gamma = 1.0;
            cfg.subpixel = SubpixelMode::None;
            let mut rast = Rasterizer::new(cfg);
            let mut g = rast.rasterize_filled(&outline, SYMBOL_UPEM);
            let adv_units = synth_symbol_advance(codepoint).unwrap_or(SYMBOL_UPEM);
            g.advance = ((adv_units as f32 * ppem as f32 / SYMBOL_UPEM as f32) + 0.5) as i32;
            g.advance = g.advance.max(1);
            return Some(g);
        }
        // 4. Visible, intentional missing-glyph box.
        let adv = (ppem as i32 * 3 / 5).max(2);
        Some(notdef_box(ppem, adv))
    }

    /// Scale factor from font units to pixels at `ppem`.
    pub fn px_scale(&self, ppem: u16) -> f32 {
        ppem as f32 / self.head.units_per_em.max(1) as f32
    }

    /// Pixel advance for a glyph id at `ppem` (rounded).
    pub fn advance_px(&self, glyph_id: u16, ppem: u16) -> i32 {
        let adv = self.hmtx.advance_width(glyph_id) as f32 * self.px_scale(ppem);
        (adv + 0.5) as i32
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Symbol supplement — synthesized outlines for UI symbols the embedded OFL faces
// do NOT cover.
//
// The bundled faces (Inter / JetBrains Mono) carry excellent Latin-1 + General
// Punctuation + arrows + math coverage, so the em-dash, ellipsis, curly quotes,
// bullet, arrows (left/right/up/down/lr), multiply, divide, check, degree, math
// (>=, <=, !=, +/-, approx), middot, section, guillemets, geometric shapes etc.
// all render real ink from the font itself. A small set of UI-used symbols is NOT
// in those faces and would otherwise be a tofu box (or an invisible zero-glyph)
// on a user-facing surface — the same severity bug as the bitmap font (CLAUDE.md
// i18n quality bar). The OFL Reserved-Font-Name rule forbids editing the .ttf
// binaries in-tree, so we synthesize these few glyphs procedurally instead, in
// font-unit (em=2048) space, then rasterize them through the SAME proven
// `rasterize_filled` grayscale-AA path the real glyphs use — so they scale and
// AA identically and share the type-ramp metrics.
//
// Covered here (verified missing from both embedded faces unless noted):
//   U+2715  MULTIPLICATION X      (missing from Inter; used as a close/cancel
//                                   glyph + PlayStation cross hint, gameos.rs)
//   U+21C4  L-R ARROW PAIR        (swap / sync affordance)
//   U+2630  TRIGRAM / "hamburger" (menu affordance)
//   U+2699  GEAR                  (settings affordance)
//
// Anything still uncovered after the font + this supplement (CJK, emoji, rare
// symbols) renders the explicit visible notdef box via `notdef_box` at the
// correct advance — an intentional tofu, never an invisible drop.
// ═══════════════════════════════════════════════════════════════════════════

/// The em these synthesized outlines are authored in (matches Inter/JBMono so
/// `rasterize_filled(glyph, SYMBOL_UPEM)` yields type-ramp-consistent metrics).
pub const SYMBOL_UPEM: u16 = 2048;

/// Codepoints that legitimately produce no ink (whitespace / format / control).
/// These keep the `None` (advance-only) contract in
/// `rasterize_codepoint_or_fallback` so they never draw a tofu box.
pub fn is_blank_codepoint(cp: u32) -> bool {
    matches!(cp,
        0x0009 | 0x000A | 0x000B | 0x000C | 0x000D | 0x0020
        | 0x00A0                            // NBSP
        | 0x1680                            // OGHAM SPACE MARK
        | 0x2000..=0x200F                   // EN/EM/thin spaces, ZWSP, ZWNJ, ZWJ
        | 0x2028 | 0x2029                   // line / para sep
        | 0x202A..=0x202E | 0x2060..=0x2064 // bidi formatting, word joiner
        | 0x2066..=0x206F                   // bidi isolates
        | 0x3000 | 0xFEFF                   // ideographic space, BOM
        | 0x0000..=0x0008 | 0x000E..=0x001F | 0x007F..=0x009F // controls
    )
}

/// Build an axis-aligned filled rectangle contour (CCW) into `pts`/`ends`.
fn sym_rect(pts: &mut Vec<GlyphPoint>, ends: &mut Vec<u16>, x0: i16, y0: i16, x1: i16, y1: i16) {
    let base = pts.len();
    for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
        pts.push(GlyphPoint {
            x,
            y,
            on_curve: true,
        });
    }
    ends.push((base + 3) as u16);
}

/// Push an arbitrary closed polygon (CCW) of on-curve points.
fn sym_poly(pts: &mut Vec<GlyphPoint>, ends: &mut Vec<u16>, poly: &[(i16, i16)]) {
    let base = pts.len();
    for &(x, y) in poly {
        pts.push(GlyphPoint {
            x,
            y,
            on_curve: true,
        });
    }
    ends.push((base + poly.len() - 1) as u16);
}

/// A thick line segment from (ax,ay) to (bx,by) of half-width `hw`, as a quad.
fn sym_thick_line(
    pts: &mut Vec<GlyphPoint>,
    ends: &mut Vec<u16>,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    hw: f32,
) {
    let dx = bx - ax;
    let dy = by - ay;
    let len = sym_sqrt(dx * dx + dy * dy).max(1.0);
    let nx = -dy / len * hw;
    let ny = dx / len * hw;
    sym_poly(
        pts,
        ends,
        &[
            ((ax + nx) as i16, (ay + ny) as i16),
            ((bx + nx) as i16, (by + ny) as i16),
            ((bx - nx) as i16, (by - ny) as i16),
            ((ax - nx) as i16, (ay - ny) as i16),
        ],
    );
}

/// no_std f32 sqrt (Newton iteration) — only used building static outlines.
fn sym_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = x;
    for _ in 0..20 {
        g = 0.5 * (g + x / g);
    }
    g
}

/// no_std sin/cos via a short Taylor series, sufficient for static gear geometry.
fn sym_sin(mut x: f32) -> f32 {
    let tau = core::f32::consts::PI * 2.0;
    while x > core::f32::consts::PI {
        x -= tau;
    }
    while x < -core::f32::consts::PI {
        x += tau;
    }
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 + x2 * x2 / 120.0 - x2 * x2 * x2 / 5040.0)
}
fn sym_cos(x: f32) -> f32 {
    sym_sin(x + core::f32::consts::PI / 2.0)
}

/// Synthesize a `SimpleGlyph` (em=SYMBOL_UPEM) for a UI symbol missing from the
/// embedded faces, or `None` if this supplement does not provide `codepoint`.
/// Geometry follows Inter's symbol box: glyphs sit roughly in y in [0,1462] with
/// the centre axis ~775 and advance ~1550 units.
pub fn synth_symbol_glyph(codepoint: u32) -> Option<SimpleGlyph> {
    const LO: i16 = 250;
    const HI: i16 = 1300;
    const STROKE: f32 = 150.0;
    let mut pts: Vec<GlyphPoint> = Vec::new();
    let mut ends: Vec<u16> = Vec::new();

    match codepoint {
        // U+2715  MULTIPLICATION X — two diagonal bars corner-to-corner.
        0x2715 => {
            sym_thick_line(
                &mut pts, &mut ends, LO as f32, LO as f32, HI as f32, HI as f32, STROKE,
            );
            sym_thick_line(
                &mut pts, &mut ends, LO as f32, HI as f32, HI as f32, LO as f32, STROKE,
            );
        }
        // U+2630  TRIGRAM FOR HEAVEN ("hamburger") — three horizontal bars.
        0x2630 => {
            let bar = (STROKE * 1.2) as i16;
            let mid = (LO + HI) / 2;
            for cy in [HI - bar, mid - bar / 2, LO] {
                sym_rect(&mut pts, &mut ends, LO, cy, HI, cy + bar);
            }
        }
        // U+21C4  RIGHTWARDS ARROW OVER LEFTWARDS ARROW — two opposed arrows.
        0x21C4 => {
            let upper = 900.0f32; // top arrow points right
            let lower = 500.0f32; // bottom arrow points left
            let head = 230.0f32;
            sym_thick_line(
                &mut pts,
                &mut ends,
                LO as f32,
                upper,
                HI as f32,
                upper,
                STROKE * 0.6,
            );
            sym_poly(
                &mut pts,
                &mut ends,
                &[
                    (HI, upper as i16),
                    ((HI as f32 - head) as i16, (upper + head) as i16),
                    ((HI as f32 - head) as i16, (upper - head) as i16),
                ],
            );
            sym_thick_line(
                &mut pts,
                &mut ends,
                LO as f32,
                lower,
                HI as f32,
                lower,
                STROKE * 0.6,
            );
            sym_poly(
                &mut pts,
                &mut ends,
                &[
                    (LO, lower as i16),
                    ((LO as f32 + head) as i16, (lower + head) as i16),
                    ((LO as f32 + head) as i16, (lower - head) as i16),
                ],
            );
        }
        // U+2699  GEAR — octagonal hub (with hole) + 8 radial teeth. Not a circle,
        // but unambiguous as a settings glyph at UI sizes, and never tofu.
        0x2699 => {
            let cx = (LO + HI) as f32 / 2.0;
            let cy = (LO + HI) as f32 / 2.0;
            let r_out = (HI - LO) as f32 / 2.0;
            let mut a = 0.0f32;
            for _ in 0..8 {
                let (s, c) = (sym_sin(a), sym_cos(a));
                sym_thick_line(
                    &mut pts,
                    &mut ends,
                    cx + c * r_out * 0.45,
                    cy + s * r_out * 0.45,
                    cx + c * r_out,
                    cy + s * r_out,
                    STROKE * 0.9,
                );
                a += core::f32::consts::PI / 4.0;
            }
            let mut outer: Vec<(i16, i16)> = Vec::new();
            let mut inner: Vec<(i16, i16)> = Vec::new();
            let mut a2 = core::f32::consts::PI / 8.0;
            for _ in 0..8 {
                let (s, c) = (sym_sin(a2), sym_cos(a2));
                outer.push(((cx + c * r_out * 0.6) as i16, (cy + s * r_out * 0.6) as i16));
                inner.push((
                    (cx + c * r_out * 0.28) as i16,
                    (cy + s * r_out * 0.28) as i16,
                ));
                a2 += core::f32::consts::PI / 4.0;
            }
            sym_poly(&mut pts, &mut ends, &outer);
            inner.reverse(); // opposite winding -> even-odd centre hole
            sym_poly(&mut pts, &mut ends, &inner);
        }
        _ => return None,
    }

    if pts.is_empty() {
        return None;
    }
    let mut x_min = i16::MAX;
    let mut y_min = i16::MAX;
    let mut x_max = i16::MIN;
    let mut y_max = i16::MIN;
    for p in &pts {
        x_min = x_min.min(p.x);
        y_min = y_min.min(p.y);
        x_max = x_max.max(p.x);
        y_max = y_max.max(p.y);
    }
    Some(SimpleGlyph {
        contour_ends: ends,
        instructions: Vec::new(),
        points: pts,
        x_min,
        y_min,
        x_max,
        y_max,
    })
}

/// Advance (em units) for a supplemented symbol — a uniform square cell.
pub fn synth_symbol_advance(codepoint: u32) -> Option<u16> {
    if synth_symbol_glyph(codepoint).is_some() {
        Some(1550)
    } else {
        None
    }
}

/// Render a visible, intentional notdef/tofu box: a hollow rectangle at the glyph
/// advance, so an uncovered codepoint shows a clear missing-glyph marker (never an
/// invisible drop). `advance_px` is the pen advance; the box is inset from it.
pub fn notdef_box(ppem: u16, advance_px: i32) -> RasterizedGlyph {
    let ppem = ppem.max(4);
    let h = ppem as i32; // full cap height
    let w = (advance_px.max(ppem as i32 / 2) - (ppem as i32 / 8)).max(2);
    let wu = w as u32;
    let hu = h as u32;
    let mut pixels = vec![0u8; (wu * hu) as usize];
    let t = (ppem / 12).max(1) as i32; // border thickness
    for y in 0..h {
        for x in 0..w {
            let edge = x < t || x >= w - t || y < t || y >= h - t;
            if edge {
                pixels[(y as u32 * wu + x as u32) as usize] = 255;
            }
        }
    }
    RasterizedGlyph {
        width: wu,
        height: hu,
        bearing_x: (ppem as i32) / 16,
        bearing_y: h, // box top at cap height above baseline
        advance: advance_px.max(1),
        pixels,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// builtin — the embedded system faces (no filesystem dependency, available at
// first boot before RaeFS mounts). OFL-licensed; the upstream OFL.txt files ship
// alongside in components/raefont/assets/ (see assets/README.md).
//
// RaeSans  = Inter (variable .ttf — the glyf table carries the default-instance
//            Regular outlines, which raefont rasterizes directly; gvar deltas /
//            other weight instances are a follow-up).
// RaeMono  = JetBrains Mono Regular (static).
// ═══════════════════════════════════════════════════════════════════════════
pub mod builtin {
    /// Inter Variable — the RaeSans UI face. ~857 KiB.
    pub const INTER_VARIABLE: &[u8] = include_bytes!("../assets/Inter-Variable.ttf");
    /// JetBrains Mono Regular — the RaeMono terminal/code face. ~267 KiB.
    pub const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.ttf");

    /// RaeSans (Inter) bytes.
    pub fn rae_sans() -> &'static [u8] {
        INTER_VARIABLE
    }
    /// RaeMono (JetBrains Mono) bytes.
    pub fn rae_mono() -> &'static [u8] {
        JETBRAINS_MONO
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Host KATs — the rasterizer's first tests (docs/design/typography-rendering.md
// S1). Run with `cargo test -p raefont` (per-crate; never `--workspace` —
// memory `no-std-workspace-host-test`). These are FAIL-able: missing font bytes
// or a parser/rasterizer regression drives coverage to zero and reds the suite.
// ═══════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    fn load_sans() -> FontHandle {
        FontHandle::from_bytes(builtin::rae_sans().to_vec())
            .expect("builtin RaeSans (Inter) must parse")
    }

    #[test]
    fn rasterize_inter_A_at_title() {
        let font = load_sans();
        let g = font
            .rasterize_codepoint('A' as u32, 22)
            .expect("'A' must have an outline");
        assert!(
            g.width > 0 && g.height > 0,
            "glyph bitmap must be non-empty"
        );
        let cov_sum: u64 = g.pixels.iter().map(|&p| p as u64).sum();
        // THE fail line: real filled ink. Outline-only / no-bytes => ~0.
        assert!(cov_sum > 0, "rasterized 'A' must have non-zero coverage");
        // Filled (not hollow): a center column near the bbox mid-height should be
        // inked for 'A' (the crossbar / leg interior), proving FILL not stroke.
        let mid_y = g.height / 2;
        let row = &g.pixels[(mid_y * g.width) as usize..((mid_y + 1) * g.width) as usize];
        let row_ink: u32 = row.iter().filter(|&&p| p > 0).count() as u32;
        assert!(row_ink > 0, "mid row of 'A' must contain ink (filled)");
        // No overflow ink: the four corners outside the glyph must be blank.
        assert_eq!(g.pixels[0], 0, "top-left corner must be empty");
        assert_eq!(
            g.pixels[(g.width - 1) as usize],
            0,
            "top-right corner must be empty"
        );
        println!(
            "[raefont-kat] 'A'@22 W={} H={} cov_sum={} mid_row_ink={} (>0)",
            g.width, g.height, cov_sum, row_ink
        );
    }

    #[test]
    fn coverage_is_grayscale_not_binary() {
        // A filled, anti-aliased glyph must contain *intermediate* coverage
        // values on its edges — pure 0/255 would mean no AA. Use a round glyph.
        let font = load_sans();
        let g = font.rasterize_codepoint('o' as u32, 24).unwrap();
        let has_partial = g.pixels.iter().any(|&p| p > 0 && p < 255);
        assert!(
            has_partial,
            "AA edges must produce partial coverage (0<p<255)"
        );
        println!("[raefont-kat] 'o'@24 has partial-coverage AA edges");
    }

    #[test]
    fn jetbrains_mono_parses_and_renders() {
        let font = FontHandle::from_bytes(builtin::rae_mono().to_vec())
            .expect("builtin RaeMono (JetBrains Mono) must parse");
        let g = font
            .rasterize_codepoint('M' as u32, 16)
            .expect("'M' outline");
        let cov: u64 = g.pixels.iter().map(|&p| p as u64).sum();
        assert!(cov > 0, "RaeMono 'M'@16 must have ink");
        println!("[raefont-kat] RaeMono 'M'@16 cov_sum={}", cov);
    }

    #[test]
    fn space_has_no_outline_but_advances() {
        let font = load_sans();
        // Space has no glyf outline -> None, but a positive advance.
        assert!(font.rasterize_codepoint(' ' as u32, 16).is_none());
        let gid = font.cmap.lookup(' ' as u32).expect("space in cmap");
        assert!(font.advance_px(gid, 16) > 0, "space must advance");
        println!("[raefont-kat] space: no outline, advance>0 OK");
    }

    fn ink(g: &RasterizedGlyph) -> u64 {
        g.pixels.iter().map(|&p| p as u64).sum()
    }

    /// The common Latin punctuation + symbols the RaeenOS UI actually renders MUST
    /// produce real ink in the default UI face (RaeSans/Inter). The em-dash
    /// especially is everywhere. This is the anti-tofu guard for the font itself.
    #[test]
    fn ui_punctuation_renders_real_ink_in_raesans() {
        let sans = load_sans();
        // (codepoint, name) — drawn from a grep of raeshell/apps/rae_tokens.
        let used: &[(u32, &str)] = &[
            (0x2014, "em-dash"),
            (0x2013, "en-dash"),
            (0x2018, "left-single-quote"),
            (0x2019, "right-single-quote"),
            (0x201C, "left-double-quote"),
            (0x201D, "right-double-quote"),
            (0x2026, "ellipsis"),
            (0x2022, "bullet"),
            (0x2190, "left-arrow"),
            (0x2192, "right-arrow"),
            (0x2191, "up-arrow"),
            (0x2193, "down-arrow"),
            (0x2194, "lr-arrow"),
            (0x21D2, "rightwards-double-arrow"),
            (0x00D7, "multiply"),
            (0x00F7, "divide"),
            (0x2713, "check"),
            (0x00B0, "degree"),
            (0x2265, "greater-equal"),
            (0x2264, "less-equal"),
            (0x2260, "not-equal"),
            (0x00B1, "plus-minus"),
            (0x2248, "almost-equal"),
            (0x00B7, "middle-dot"),
            (0x00A7, "section"),
            (0x2212, "minus"),
            (0x203A, "single-right-angle-quote"),
            (0x2039, "single-left-angle-quote"),
            (0x00BB, "right-guillemet"),
            (0x00AB, "left-guillemet"),
            (0x25EF, "large-circle"),
            (0x25B3, "white-up-triangle"),
            (0x25A1, "white-square"),
            (0x232B, "erase-to-left"),
            (0x00B2, "superscript-2"),
            (0x00B3, "superscript-3"),
            (0x03C0, "pi"),
            (0x00B5, "micro"),
        ];
        for &(cp, name) in used {
            assert!(
                sans.has_glyph(cp),
                "RaeSans must cover U+{:04X} {} (UI-used)",
                cp,
                name
            );
            let g = sans
                .rasterize_codepoint(cp, 18)
                .unwrap_or_else(|| panic!("U+{:04X} {} produced no glyph", cp, name));
            assert!(
                ink(&g) > 0,
                "U+{:04X} {} must render real ink, not tofu",
                cp,
                name
            );
            assert!(g.advance > 0, "U+{:04X} {} must advance", cp, name);
        }
        println!(
            "[raefont-kat] RaeSans renders {} UI punctuation/symbol codepoints with real ink",
            used.len()
        );
    }

    /// The handful of UI symbols the embedded OFL faces do NOT carry are filled by
    /// the procedural supplement — they must now render real ink (no tofu) at the
    /// correct advance, through the SAME fill path. Previously each was a miss.
    #[test]
    fn supplemented_symbols_render_real_ink() {
        let sans = load_sans();
        // Each of these is verified absent from BOTH embedded faces (✕ absent from
        // Inter); the supplement provides them.
        let supp: &[(u32, &str)] = &[
            (0x2715, "multiplication-x (close / PS-cross)"),
            (0x21C4, "left-right arrow pair"),
            (0x2630, "trigram (hamburger menu)"),
            (0x2699, "gear (settings)"),
        ];
        for &(cp, name) in supp {
            // The raw font must NOT have it (else the supplement is dead code).
            assert!(
                !sans.has_glyph(cp),
                "U+{:04X} {} is supposed to be supplemented, but the font now \
                 covers it — drop it from the supplement",
                cp,
                name
            );
            // synth must provide an outline...
            assert!(
                synth_symbol_glyph(cp).is_some(),
                "supplement must provide U+{:04X} {}",
                cp,
                name
            );
            // ...and the full fallback path must render real ink for it.
            let g = sans
                .rasterize_codepoint_or_fallback(cp, 18)
                .unwrap_or_else(|| panic!("U+{:04X} {} produced no glyph", cp, name));
            let cov = ink(&g);
            assert!(
                cov > 0,
                "U+{:04X} {} supplement must render real ink (got {})",
                cp,
                name,
                cov
            );
            assert!(g.advance > 0, "U+{:04X} {} must advance", cp, name);
            assert!(
                g.width > 1 && g.height > 1,
                "U+{:04X} {} must be a real bitmap, not a 1px stub",
                cp,
                name
            );
            println!(
                "[raefont-kat] supplement U+{:04X} {:<32} W={} H={} ink={} adv={}",
                cp, name, g.width, g.height, cov, g.advance
            );
        }
    }

    /// FAIL-ability proof: a codepoint we did NOT add (CJK ideograph) is absent
    /// from the font AND the supplement, so the raw paths miss — but the fallback
    /// path must still return a VISIBLE notdef box (intentional tofu), never an
    /// invisible drop. Demonstrates the test can distinguish "added" from "tofu".
    #[test]
    fn uncovered_codepoint_is_visible_notdef_box() {
        let sans = load_sans();
        let cjk = 0x6587u32; // 文 — not in Inter, not supplemented
        assert!(
            !sans.has_glyph(cjk),
            "control: CJK must be uncovered by font"
        );
        assert!(
            sans.rasterize_codepoint(cjk, 18).is_none(),
            "control: raw font path must MISS the uncovered cp"
        );
        assert!(
            synth_symbol_glyph(cjk).is_none(),
            "control: supplement must NOT cover the uncovered cp"
        );
        // But the fallback path yields a visible box (ink>0) at a real advance.
        let g = sans
            .rasterize_codepoint_or_fallback(cjk, 18)
            .expect("fallback must produce a visible notdef box");
        assert!(ink(&g) > 0, "notdef box must have visible ink");
        assert!(g.advance > 0, "notdef box must advance");
        println!(
            "[raefont-kat] uncovered U+{:04X} -> visible notdef box W={} H={} ink={} (FAIL-able demo)",
            cjk,
            g.width,
            g.height,
            ink(&g)
        );
    }

    /// Whitespace keeps the advance-only (None) contract through the fallback —
    /// it must NEVER draw a tofu box.
    #[test]
    fn blank_codepoints_do_not_draw_tofu() {
        let sans = load_sans();
        for cp in [0x20u32, 0x00A0, 0x2009, 0x200B, 0x3000, 0xFEFF] {
            assert!(
                sans.rasterize_codepoint_or_fallback(cp, 18).is_none(),
                "blank U+{:04X} must not draw a glyph/box",
                cp
            );
        }
        println!("[raefont-kat] whitespace/format codepoints stay ink-free (no false tofu)");
    }

    /// RaeMono (JetBrains Mono) coverage of the same UI punctuation, for the
    /// terminal/code surface — must also render real ink.
    #[test]
    fn ui_punctuation_renders_in_raemono() {
        let mono = FontHandle::from_bytes(builtin::rae_mono().to_vec()).unwrap();
        for &cp in &[0x2014u32, 0x2026, 0x2022, 0x2192, 0x00D7, 0x2713, 0x00B0] {
            let g = mono
                .rasterize_codepoint_or_fallback(cp, 16)
                .unwrap_or_else(|| panic!("RaeMono U+{:04X} produced no glyph", cp));
            assert!(ink(&g) > 0, "RaeMono U+{:04X} must render real ink", cp);
        }
        // ✕ IS present in JetBrains Mono (only Inter lacks it) — confirm it comes
        // from the real face there, not the supplement.
        assert!(
            mono.has_glyph(0x2715),
            "RaeMono should cover U+2715 natively"
        );
        println!("[raefont-kat] RaeMono renders UI punctuation incl. native U+2715");
    }
}
