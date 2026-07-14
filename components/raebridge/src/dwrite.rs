//! DirectWrite text rendering API emulation for RaeBridge.
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::{HResult, WinHandle};

// ---------------------------------------------------------------------------
// HRESULT constants
// ---------------------------------------------------------------------------

pub const S_OK: i32 = 0;
pub const E_FAIL: i32 = -2147467259;
pub const E_INVALIDARG: i32 = -2147024809;
pub const E_NOTIMPL: i32 = -2147467263;

// ---------------------------------------------------------------------------
// Font weight
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum DWriteFontWeight {
    Thin = 100,
    ExtraLight = 200,
    Light = 300,
    SemiLight = 350,
    Normal = 400,
    Medium = 500,
    SemiBold = 600,
    Bold = 700,
    ExtraBold = 800,
    Black = 900,
    UltraBlack = 950,
}

impl DWriteFontWeight {
    pub fn from_u32(value: u32) -> Self {
        match value {
            0..=149 => Self::Thin,
            150..=249 => Self::ExtraLight,
            250..=324 => Self::Light,
            325..=374 => Self::SemiLight,
            375..=449 => Self::Normal,
            450..=549 => Self::Medium,
            550..=649 => Self::SemiBold,
            650..=749 => Self::Bold,
            750..=849 => Self::ExtraBold,
            850..=924 => Self::Black,
            _ => Self::UltraBlack,
        }
    }

    pub fn to_u32(self) -> u32 {
        self as u32
    }
}

// ---------------------------------------------------------------------------
// Font style
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DWriteFontStyle {
    Normal = 0,
    Oblique = 1,
    Italic = 2,
}

// ---------------------------------------------------------------------------
// Font stretch
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum DWriteFontStretch {
    Undefined = 0,
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

// ---------------------------------------------------------------------------
// Text alignment & paragraph alignment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteTextAlignment {
    Leading,
    Trailing,
    Center,
    Justified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteParagraphAlignment {
    Near,
    Far,
    Center,
}

// ---------------------------------------------------------------------------
// Word wrapping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteWordWrapping {
    Wrap,
    NoWrap,
    EmergencyBreak,
    WholeWord,
    Character,
}

// ---------------------------------------------------------------------------
// Reading direction & flow direction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteReadingDirection {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteFlowDirection {
    TopToBottom,
    BottomToTop,
    LeftToRight,
    RightToLeft,
}

// ---------------------------------------------------------------------------
// Line spacing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteLineSpacingMethod {
    Default,
    Uniform,
    Proportional,
}

#[derive(Debug, Clone, Copy)]
pub struct DWriteLineSpacing {
    pub method: DWriteLineSpacingMethod,
    pub spacing: f32,
    pub baseline: f32,
}

// ---------------------------------------------------------------------------
// Trimming
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteTrimmingGranularity {
    None,
    Character,
    Word,
}

#[derive(Debug, Clone)]
pub struct DWriteTrimming {
    pub granularity: DWriteTrimmingGranularity,
    pub delimiter: u32,
    pub delimiter_count: u32,
}

// ---------------------------------------------------------------------------
// Text metrics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct DWriteTextMetrics {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub width_including_trailing_whitespace: f32,
    pub height: f32,
    pub layout_width: f32,
    pub layout_height: f32,
    pub max_bidi_reordering_depth: u32,
    pub line_count: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DWriteLineMetrics {
    pub length: u32,
    pub trailing_whitespace_length: u32,
    pub newline_length: u32,
    pub height: f32,
    pub baseline: f32,
    pub is_trimmed: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DWriteOverhangMetrics {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DWriteHitTestMetrics {
    pub text_position: u32,
    pub length: u32,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub bidi_level: u32,
    pub is_text: bool,
    pub is_trimmed: bool,
}

// ---------------------------------------------------------------------------
// Script analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteScriptId {
    Latin,
    Arabic,
    Hebrew,
    CJKUnified,
    Hangul,
    Hiragana,
    Katakana,
    Thai,
    Devanagari,
    Cyrillic,
    Greek,
    Georgian,
    Armenian,
    Bengali,
    Tamil,
    Telugu,
    Common,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct DWriteScriptAnalysis {
    pub script: u16,
    pub script_id: DWriteScriptId,
    pub shapes: DWriteScriptShapes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteScriptShapes {
    Default,
    NoVisual,
}

// ---------------------------------------------------------------------------
// Bidi & line breaking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct DWriteLineBreakpoint {
    pub break_condition_before: u8,
    pub break_condition_after: u8,
    pub is_whitespace: bool,
    pub is_soft_hyphen: bool,
    pub padding: u8,
}

// ---------------------------------------------------------------------------
// OpenType feature tags
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DWriteFontFeatureTag(pub u32);

impl DWriteFontFeatureTag {
    pub const LIGA: Self = Self(u32::from_le_bytes(*b"liga"));
    pub const CLIG: Self = Self(u32::from_le_bytes(*b"clig"));
    pub const DLIG: Self = Self(u32::from_le_bytes(*b"dlig"));
    pub const HLIG: Self = Self(u32::from_le_bytes(*b"hlig"));
    pub const SALT: Self = Self(u32::from_le_bytes(*b"salt"));
    pub const SMCP: Self = Self(u32::from_le_bytes(*b"smcp"));
    pub const C2SC: Self = Self(u32::from_le_bytes(*b"c2sc"));
    pub const ONUM: Self = Self(u32::from_le_bytes(*b"onum"));
    pub const LNUM: Self = Self(u32::from_le_bytes(*b"lnum"));
    pub const TNUM: Self = Self(u32::from_le_bytes(*b"tnum"));
    pub const PNUM: Self = Self(u32::from_le_bytes(*b"pnum"));
    pub const FRAC: Self = Self(u32::from_le_bytes(*b"frac"));
    pub const ZERO: Self = Self(u32::from_le_bytes(*b"zero"));
    pub const KERN: Self = Self(u32::from_le_bytes(*b"kern"));
    pub const MARK: Self = Self(u32::from_le_bytes(*b"mark"));
    pub const MKMK: Self = Self(u32::from_le_bytes(*b"mkmk"));
    pub const CALT: Self = Self(u32::from_le_bytes(*b"calt"));
    pub const CCMP: Self = Self(u32::from_le_bytes(*b"ccmp"));
    pub const LOCL: Self = Self(u32::from_le_bytes(*b"locl"));
    pub const RLIG: Self = Self(u32::from_le_bytes(*b"rlig"));
    pub const SS01: Self = Self(u32::from_le_bytes(*b"ss01"));
    pub const SS02: Self = Self(u32::from_le_bytes(*b"ss02"));
    pub const SS03: Self = Self(u32::from_le_bytes(*b"ss03"));
    pub const SS04: Self = Self(u32::from_le_bytes(*b"ss04"));
    pub const SS05: Self = Self(u32::from_le_bytes(*b"ss05"));
    pub const SS06: Self = Self(u32::from_le_bytes(*b"ss06"));
    pub const SS07: Self = Self(u32::from_le_bytes(*b"ss07"));
    pub const SS08: Self = Self(u32::from_le_bytes(*b"ss08"));
    pub const SS09: Self = Self(u32::from_le_bytes(*b"ss09"));
    pub const SS10: Self = Self(u32::from_le_bytes(*b"ss10"));
    pub const SS11: Self = Self(u32::from_le_bytes(*b"ss11"));
    pub const SS12: Self = Self(u32::from_le_bytes(*b"ss12"));
    pub const SS13: Self = Self(u32::from_le_bytes(*b"ss13"));
    pub const SS14: Self = Self(u32::from_le_bytes(*b"ss14"));
    pub const SS15: Self = Self(u32::from_le_bytes(*b"ss15"));
    pub const SS16: Self = Self(u32::from_le_bytes(*b"ss16"));
    pub const SS17: Self = Self(u32::from_le_bytes(*b"ss17"));
    pub const SS18: Self = Self(u32::from_le_bytes(*b"ss18"));
    pub const SS19: Self = Self(u32::from_le_bytes(*b"ss19"));
    pub const SS20: Self = Self(u32::from_le_bytes(*b"ss20"));
}

#[derive(Debug, Clone, Copy)]
pub struct DWriteFontFeature {
    pub tag: DWriteFontFeatureTag,
    pub parameter: u32,
}

#[derive(Debug, Clone)]
pub struct DWriteTypography {
    pub features: Vec<DWriteFontFeature>,
}

impl DWriteTypography {
    pub fn new() -> Self {
        Self {
            features: Vec::new(),
        }
    }

    pub fn add_font_feature(&mut self, feature: DWriteFontFeature) {
        self.features.push(feature);
    }

    pub fn get_font_feature_count(&self) -> u32 {
        self.features.len() as u32
    }

    pub fn get_font_feature(&self, index: u32) -> Option<&DWriteFontFeature> {
        self.features.get(index as usize)
    }
}

// ---------------------------------------------------------------------------
// Rendering mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteRenderingMode {
    Default,
    Aliased,
    GdiClassic,
    GdiNatural,
    Natural,
    NaturalSymmetric,
    Outline,
    NaturalSymmetricDownsampled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWritePixelGeometry {
    Flat,
    Rgb,
    Bgr,
}

#[derive(Debug, Clone, Copy)]
pub struct DWriteRenderingParams {
    pub gamma: f32,
    pub enhanced_contrast: f32,
    pub cleartype_level: f32,
    pub pixel_geometry: DWritePixelGeometry,
    pub rendering_mode: DWriteRenderingMode,
}

// ---------------------------------------------------------------------------
// Number substitution
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteNumberSubstitutionMethod {
    FromCulture,
    Contextual,
    None,
    National,
    Traditional,
}

#[derive(Debug, Clone)]
pub struct DWriteNumberSubstitution {
    pub method: DWriteNumberSubstitutionMethod,
    pub locale_name: String,
    pub ignore_user_override: bool,
}

// ---------------------------------------------------------------------------
// Font collection & font face
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DWriteFontFamily {
    pub name: String,
    pub fonts: Vec<DWriteFontFaceInfo>,
}

#[derive(Debug, Clone)]
pub struct DWriteFontFaceInfo {
    pub family_name: String,
    pub weight: DWriteFontWeight,
    pub style: DWriteFontStyle,
    pub stretch: DWriteFontStretch,
    pub is_monospace: bool,
    pub has_character: bool,
}

#[derive(Debug, Clone)]
pub struct DWriteFontCollection {
    pub handle: WinHandle,
    pub families: Vec<DWriteFontFamily>,
    pub is_system: bool,
}

impl DWriteFontCollection {
    pub fn get_font_family_count(&self) -> u32 {
        self.families.len() as u32
    }

    pub fn find_family_name(&self, name: &str) -> Option<u32> {
        self.families
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }

    pub fn get_font_family(&self, index: u32) -> Option<&DWriteFontFamily> {
        self.families.get(index as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteFontFaceType {
    Cff,
    TrueType,
    OpenTypeCollection,
    Type1,
    Vector,
    Bitmap,
    Unknown,
    RawCff,
}

#[derive(Debug, Clone)]
pub struct DWriteFontFace {
    pub handle: WinHandle,
    pub face_type: DWriteFontFaceType,
    pub glyph_count: u16,
    pub index: u32,
    pub simulations: u32,
    pub is_symbol: bool,
    pub metrics_em: u16,
    pub ascent: i16,
    pub descent: i16,
    pub line_gap: i16,
    pub cap_height: i16,
    pub x_height: i16,
    pub underline_position: i16,
    pub underline_thickness: i16,
    pub strikethrough_position: i16,
    pub strikethrough_thickness: i16,
}

// ---------------------------------------------------------------------------
// Font fallback
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DWriteFontFallbackMapping {
    pub unicode_ranges: Vec<(u32, u32)>,
    pub target_family_names: Vec<String>,
    pub scale: f32,
}

#[derive(Debug, Clone)]
pub struct DWriteFontFallback {
    pub mappings: Vec<DWriteFontFallbackMapping>,
    pub default_family: String,
}

impl DWriteFontFallback {
    pub fn map_characters(
        &self,
        text: &str,
        base_family: &str,
        _weight: DWriteFontWeight,
        _style: DWriteFontStyle,
        _stretch: DWriteFontStretch,
    ) -> Vec<DWriteFontFallbackResult> {
        let mut results = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return results;
        }

        let mut i = 0;
        while i < chars.len() {
            let cp = chars[i] as u32;
            let mut matched_family = None;

            for mapping in &self.mappings {
                for &(start, end) in &mapping.unicode_ranges {
                    if cp >= start && cp <= end {
                        if let Some(family) = mapping.target_family_names.first() {
                            matched_family = Some(family.clone());
                        }
                        break;
                    }
                }
                if matched_family.is_some() {
                    break;
                }
            }

            let family = matched_family.unwrap_or_else(|| String::from(base_family));
            let start_pos = i;
            i += 1;
            while i < chars.len() {
                let next_cp = chars[i] as u32;
                let next_family = self.resolve_family(next_cp, base_family);
                if next_family != family {
                    break;
                }
                i += 1;
            }
            results.push(DWriteFontFallbackResult {
                family_name: family,
                text_position: start_pos as u32,
                text_length: (i - start_pos) as u32,
                scale: 1.0,
            });
        }
        results
    }

    fn resolve_family(&self, codepoint: u32, base_family: &str) -> String {
        for mapping in &self.mappings {
            for &(start, end) in &mapping.unicode_ranges {
                if codepoint >= start && codepoint <= end {
                    if let Some(family) = mapping.target_family_names.first() {
                        return family.clone();
                    }
                }
            }
        }
        String::from(base_family)
    }
}

#[derive(Debug, Clone)]
pub struct DWriteFontFallbackResult {
    pub family_name: String,
    pub text_position: u32,
    pub text_length: u32,
    pub scale: f32,
}

// ---------------------------------------------------------------------------
// Inline object
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct DWriteInlineObjectMetrics {
    pub width: f32,
    pub height: f32,
    pub baseline: f32,
    pub supports_sideways: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteBreakCondition {
    Neutral,
    CanBreak,
    MayNotBreak,
    MustBreak,
}

#[derive(Debug, Clone)]
pub struct DWriteInlineObject {
    pub handle: WinHandle,
    pub metrics: DWriteInlineObjectMetrics,
    pub break_condition_before: DWriteBreakCondition,
    pub break_condition_after: DWriteBreakCondition,
}

// ---------------------------------------------------------------------------
// Text range
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct DWriteTextRange {
    pub start_position: u32,
    pub length: u32,
}

// ---------------------------------------------------------------------------
// IDWriteTextFormat
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DWriteTextFormat {
    pub handle: WinHandle,
    pub font_family_name: String,
    pub font_weight: DWriteFontWeight,
    pub font_style: DWriteFontStyle,
    pub font_stretch: DWriteFontStretch,
    pub font_size: f32,
    pub locale_name: String,
    pub text_alignment: DWriteTextAlignment,
    pub paragraph_alignment: DWriteParagraphAlignment,
    pub word_wrapping: DWriteWordWrapping,
    pub reading_direction: DWriteReadingDirection,
    pub flow_direction: DWriteFlowDirection,
    pub incremental_tab_stop: f32,
    pub trimming: DWriteTrimming,
    pub line_spacing: DWriteLineSpacing,
    pub font_collection: Option<WinHandle>,
}

impl DWriteTextFormat {
    pub fn set_text_alignment(&mut self, alignment: DWriteTextAlignment) -> HResult {
        self.text_alignment = alignment;
        HResult(S_OK)
    }

    pub fn set_paragraph_alignment(&mut self, alignment: DWriteParagraphAlignment) -> HResult {
        self.paragraph_alignment = alignment;
        HResult(S_OK)
    }

    pub fn set_word_wrapping(&mut self, wrapping: DWriteWordWrapping) -> HResult {
        self.word_wrapping = wrapping;
        HResult(S_OK)
    }

    pub fn set_reading_direction(&mut self, direction: DWriteReadingDirection) -> HResult {
        self.reading_direction = direction;
        HResult(S_OK)
    }

    pub fn set_flow_direction(&mut self, direction: DWriteFlowDirection) -> HResult {
        self.flow_direction = direction;
        HResult(S_OK)
    }

    pub fn set_incremental_tab_stop(&mut self, tab_stop: f32) -> HResult {
        self.incremental_tab_stop = tab_stop;
        HResult(S_OK)
    }

    pub fn set_trimming(&mut self, trimming: DWriteTrimming) -> HResult {
        self.trimming = trimming;
        HResult(S_OK)
    }

    pub fn set_line_spacing(
        &mut self,
        method: DWriteLineSpacingMethod,
        spacing: f32,
        baseline: f32,
    ) -> HResult {
        self.line_spacing = DWriteLineSpacing {
            method,
            spacing,
            baseline,
        };
        HResult(S_OK)
    }

    pub fn get_font_collection(&self) -> Option<WinHandle> {
        self.font_collection
    }

    pub fn get_font_family_name_length(&self) -> u32 {
        self.font_family_name.len() as u32
    }

    pub fn get_font_family_name(&self) -> &str {
        &self.font_family_name
    }

    pub fn get_font_weight(&self) -> DWriteFontWeight {
        self.font_weight
    }

    pub fn get_font_style(&self) -> DWriteFontStyle {
        self.font_style
    }

    pub fn get_font_stretch(&self) -> DWriteFontStretch {
        self.font_stretch
    }

    pub fn get_font_size(&self) -> f32 {
        self.font_size
    }

    pub fn get_locale_name_length(&self) -> u32 {
        self.locale_name.len() as u32
    }

    pub fn get_locale_name(&self) -> &str {
        &self.locale_name
    }
}

// ---------------------------------------------------------------------------
// IDWriteTextLayout
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DWriteTextLayout {
    pub handle: WinHandle,
    pub text: String,
    pub format: DWriteTextFormat,
    pub max_width: f32,
    pub max_height: f32,
    pub range_font_sizes: Vec<(DWriteTextRange, f32)>,
    pub range_font_weights: Vec<(DWriteTextRange, DWriteFontWeight)>,
    pub range_font_styles: Vec<(DWriteTextRange, DWriteFontStyle)>,
    pub range_underlines: Vec<(DWriteTextRange, bool)>,
    pub range_strikethroughs: Vec<(DWriteTextRange, bool)>,
    pub range_font_families: Vec<(DWriteTextRange, String)>,
    pub range_typography: Vec<(DWriteTextRange, DWriteTypography)>,
    pub range_inline_objects: Vec<(DWriteTextRange, WinHandle)>,
    pub range_font_collections: Vec<(DWriteTextRange, WinHandle)>,
    pub cached_metrics: Option<DWriteTextMetrics>,
}

impl DWriteTextLayout {
    pub fn set_max_width(&mut self, max_width: f32) -> HResult {
        self.max_width = max_width;
        self.cached_metrics = None;
        HResult(S_OK)
    }

    pub fn set_max_height(&mut self, max_height: f32) -> HResult {
        self.max_height = max_height;
        self.cached_metrics = None;
        HResult(S_OK)
    }

    pub fn get_metrics(&mut self) -> DWriteTextMetrics {
        if let Some(m) = self.cached_metrics {
            return m;
        }
        let metrics = self.compute_metrics();
        self.cached_metrics = Some(metrics);
        metrics
    }

    pub fn get_overhang_metrics(&self) -> DWriteOverhangMetrics {
        DWriteOverhangMetrics::default()
    }

    pub fn get_line_metrics(&self) -> Vec<DWriteLineMetrics> {
        let line_count = self.text.matches('\n').count() + 1;
        let line_height = self.format.font_size * 1.2;
        let mut lines = Vec::with_capacity(line_count);
        for segment in self.text.split('\n') {
            lines.push(DWriteLineMetrics {
                length: segment.len() as u32,
                trailing_whitespace_length: 0,
                newline_length: 1,
                height: line_height,
                baseline: self.format.font_size,
                is_trimmed: false,
            });
        }
        lines
    }

    pub fn set_font_size(&mut self, font_size: f32, range: DWriteTextRange) -> HResult {
        self.range_font_sizes.push((range, font_size));
        self.cached_metrics = None;
        HResult(S_OK)
    }

    pub fn set_font_weight(&mut self, weight: DWriteFontWeight, range: DWriteTextRange) -> HResult {
        self.range_font_weights.push((range, weight));
        self.cached_metrics = None;
        HResult(S_OK)
    }

    pub fn set_font_style(&mut self, style: DWriteFontStyle, range: DWriteTextRange) -> HResult {
        self.range_font_styles.push((range, style));
        self.cached_metrics = None;
        HResult(S_OK)
    }

    pub fn set_underline(&mut self, has_underline: bool, range: DWriteTextRange) -> HResult {
        self.range_underlines.push((range, has_underline));
        HResult(S_OK)
    }

    pub fn set_strikethrough(
        &mut self,
        has_strikethrough: bool,
        range: DWriteTextRange,
    ) -> HResult {
        self.range_strikethroughs.push((range, has_strikethrough));
        HResult(S_OK)
    }

    pub fn set_font_collection(
        &mut self,
        collection: WinHandle,
        range: DWriteTextRange,
    ) -> HResult {
        self.range_font_collections.push((range, collection));
        self.cached_metrics = None;
        HResult(S_OK)
    }

    pub fn set_font_family_name(&mut self, name: String, range: DWriteTextRange) -> HResult {
        self.range_font_families.push((range, name));
        self.cached_metrics = None;
        HResult(S_OK)
    }

    pub fn set_typography(
        &mut self,
        typography: DWriteTypography,
        range: DWriteTextRange,
    ) -> HResult {
        self.range_typography.push((range, typography));
        HResult(S_OK)
    }

    pub fn set_inline_object(&mut self, obj: WinHandle, range: DWriteTextRange) -> HResult {
        self.range_inline_objects.push((range, obj));
        self.cached_metrics = None;
        HResult(S_OK)
    }

    pub fn hit_test_point(&self, point_x: f32, point_y: f32) -> DWriteHitTestMetrics {
        let line_height = self.format.font_size * 1.2;
        let avg_char_width = self.format.font_size * 0.5;
        let line_index = (point_y / line_height).max(0.0) as u32;
        let char_index = (point_x / avg_char_width).max(0.0) as u32;
        let text_position = (line_index * 80 + char_index).min(self.text.len() as u32);
        DWriteHitTestMetrics {
            text_position,
            length: 1,
            left: char_index as f32 * avg_char_width,
            top: line_index as f32 * line_height,
            width: avg_char_width,
            height: line_height,
            bidi_level: 0,
            is_text: text_position < self.text.len() as u32,
            is_trimmed: false,
        }
    }

    pub fn hit_test_text_position(
        &self,
        text_position: u32,
        is_trailing_hit: bool,
    ) -> (f32, f32, DWriteHitTestMetrics) {
        let avg_char_width = self.format.font_size * 0.5;
        let line_height = self.format.font_size * 1.2;
        let pos = text_position.min(self.text.len() as u32);
        let x = pos as f32 * avg_char_width + if is_trailing_hit { avg_char_width } else { 0.0 };
        let y = 0.0f32;
        let metrics = DWriteHitTestMetrics {
            text_position: pos,
            length: 1,
            left: pos as f32 * avg_char_width,
            top: 0.0,
            width: avg_char_width,
            height: line_height,
            bidi_level: 0,
            is_text: pos < self.text.len() as u32,
            is_trimmed: false,
        };
        (x, y, metrics)
    }

    pub fn hit_test_text_range(
        &self,
        text_position: u32,
        text_length: u32,
        _origin_x: f32,
        _origin_y: f32,
    ) -> Vec<DWriteHitTestMetrics> {
        let avg_char_width = self.format.font_size * 0.5;
        let line_height = self.format.font_size * 1.2;
        let mut results = Vec::new();
        results.push(DWriteHitTestMetrics {
            text_position,
            length: text_length,
            left: text_position as f32 * avg_char_width,
            top: 0.0,
            width: text_length as f32 * avg_char_width,
            height: line_height,
            bidi_level: 0,
            is_text: true,
            is_trimmed: false,
        });
        results
    }

    pub fn determine_min_width(&self) -> f32 {
        let avg_char_width = self.format.font_size * 0.5;
        let max_word_len = self
            .text
            .split_whitespace()
            .map(|w| w.len())
            .max()
            .unwrap_or(0);
        max_word_len as f32 * avg_char_width
    }

    pub fn draw(&self) -> HResult {
        HResult(S_OK)
    }

    fn compute_metrics(&self) -> DWriteTextMetrics {
        let avg_char_width = self.format.font_size * 0.5;
        let line_height = self.format.font_size * 1.2;
        let line_count = self.text.matches('\n').count() as u32 + 1;
        let max_line_width = self
            .text
            .lines()
            .map(|l| l.len() as f32 * avg_char_width)
            .fold(0.0f32, f32::max);

        DWriteTextMetrics {
            left: 0.0,
            top: 0.0,
            width: max_line_width.min(self.max_width),
            width_including_trailing_whitespace: max_line_width,
            height: line_count as f32 * line_height,
            layout_width: self.max_width,
            layout_height: self.max_height,
            max_bidi_reordering_depth: 1,
            line_count,
        }
    }
}

// ---------------------------------------------------------------------------
// Glyph run analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DWriteGlyphRunAnalysis {
    pub handle: WinHandle,
    pub rendering_mode: DWriteRenderingMode,
    pub measuring_mode: DWriteMeasuringMode,
    pub glyph_count: u32,
    pub bounds_left: i32,
    pub bounds_top: i32,
    pub bounds_right: i32,
    pub bounds_bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DWriteMeasuringMode {
    Natural,
    GdiClassic,
    GdiNatural,
}

// ---------------------------------------------------------------------------
// Text analyzer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DWriteTextAnalyzer {
    pub handle: WinHandle,
}

impl DWriteTextAnalyzer {
    pub fn analyze_script(&self, text: &str) -> Vec<(u32, u32, DWriteScriptAnalysis)> {
        let mut results = Vec::new();
        let len = text.len() as u32;
        if len > 0 {
            results.push((
                0,
                len,
                DWriteScriptAnalysis {
                    script: 0,
                    script_id: DWriteScriptId::Latin,
                    shapes: DWriteScriptShapes::Default,
                },
            ));
        }
        results
    }

    pub fn analyze_bidi(&self, text: &str) -> Vec<(u32, u32, u8)> {
        let mut results = Vec::new();
        let len = text.len() as u32;
        if len > 0 {
            results.push((0, len, 0u8));
        }
        results
    }

    pub fn analyze_line_breakpoints(&self, text: &str) -> Vec<DWriteLineBreakpoint> {
        text.chars()
            .map(|c| DWriteLineBreakpoint {
                break_condition_before: 0,
                break_condition_after: if c == ' ' || c == '-' { 1 } else { 0 },
                is_whitespace: c == ' ' || c == '\t',
                is_soft_hyphen: c == '\u{00AD}',
                padding: 0,
            })
            .collect()
    }

    pub fn get_glyph_placements(
        &self,
        _text: &str,
        _glyph_indices: &[u16],
        glyph_count: u32,
        font_size: f32,
    ) -> Vec<(f32, f32)> {
        let advance = font_size * 0.5;
        (0..glyph_count).map(|_| (advance, 0.0)).collect()
    }
}

// ---------------------------------------------------------------------------
// IDWriteFactory
// ---------------------------------------------------------------------------

pub struct DWriteFactory {
    pub handle: WinHandle,
    pub next_handle: u64,
    pub system_font_collection: DWriteFontCollection,
    pub text_formats: BTreeMap<u64, DWriteTextFormat>,
    pub text_layouts: BTreeMap<u64, DWriteTextLayout>,
    pub font_faces: BTreeMap<u64, DWriteFontFace>,
    pub inline_objects: BTreeMap<u64, DWriteInlineObject>,
    pub rendering_params: BTreeMap<u64, DWriteRenderingParams>,
    pub font_fallback: DWriteFontFallback,
    pub custom_collections: BTreeMap<u64, DWriteFontCollection>,
    pub text_analyzers: BTreeMap<u64, DWriteTextAnalyzer>,
    pub number_substitutions: BTreeMap<u64, DWriteNumberSubstitution>,
    pub glyph_run_analyses: BTreeMap<u64, DWriteGlyphRunAnalysis>,
}

impl DWriteFactory {
    fn alloc_handle(&mut self) -> WinHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        WinHandle(h)
    }

    pub fn create_text_format(
        &mut self,
        font_family_name: &str,
        font_collection: Option<WinHandle>,
        weight: DWriteFontWeight,
        style: DWriteFontStyle,
        stretch: DWriteFontStretch,
        size: f32,
        locale_name: &str,
    ) -> HResult {
        let handle = self.alloc_handle();
        self.text_formats.insert(
            handle.0,
            DWriteTextFormat {
                handle,
                font_family_name: String::from(font_family_name),
                font_weight: weight,
                font_style: style,
                font_stretch: stretch,
                font_size: size,
                locale_name: String::from(locale_name),
                text_alignment: DWriteTextAlignment::Leading,
                paragraph_alignment: DWriteParagraphAlignment::Near,
                word_wrapping: DWriteWordWrapping::Wrap,
                reading_direction: DWriteReadingDirection::LeftToRight,
                flow_direction: DWriteFlowDirection::TopToBottom,
                incremental_tab_stop: 48.0,
                trimming: DWriteTrimming {
                    granularity: DWriteTrimmingGranularity::None,
                    delimiter: 0,
                    delimiter_count: 0,
                },
                line_spacing: DWriteLineSpacing {
                    method: DWriteLineSpacingMethod::Default,
                    spacing: 0.0,
                    baseline: 0.0,
                },
                font_collection,
            },
        );
        HResult(S_OK)
    }

    pub fn create_text_layout(
        &mut self,
        text: &str,
        text_format_handle: WinHandle,
        max_width: f32,
        max_height: f32,
    ) -> HResult {
        let format = match self.text_formats.get(&text_format_handle.0) {
            Some(f) => f.clone(),
            None => return HResult(E_INVALIDARG),
        };
        let handle = self.alloc_handle();
        self.text_layouts.insert(
            handle.0,
            DWriteTextLayout {
                handle,
                text: String::from(text),
                format,
                max_width,
                max_height,
                range_font_sizes: Vec::new(),
                range_font_weights: Vec::new(),
                range_font_styles: Vec::new(),
                range_underlines: Vec::new(),
                range_strikethroughs: Vec::new(),
                range_font_families: Vec::new(),
                range_typography: Vec::new(),
                range_inline_objects: Vec::new(),
                range_font_collections: Vec::new(),
                cached_metrics: None,
            },
        );
        HResult(S_OK)
    }

    pub fn get_system_font_collection(&self) -> &DWriteFontCollection {
        &self.system_font_collection
    }

    pub fn create_custom_font_collection(&mut self) -> HResult {
        let handle = self.alloc_handle();
        self.custom_collections.insert(
            handle.0,
            DWriteFontCollection {
                handle,
                families: Vec::new(),
                is_system: false,
            },
        );
        HResult(S_OK)
    }

    pub fn register_font_file_loader(&mut self, _loader_id: u64) -> HResult {
        HResult(S_OK)
    }

    pub fn unregister_font_file_loader(&mut self, _loader_id: u64) -> HResult {
        HResult(S_OK)
    }

    pub fn create_font_file_reference(&mut self, _file_path: &str) -> HResult {
        HResult(S_OK)
    }

    pub fn create_font_face(
        &mut self,
        face_type: DWriteFontFaceType,
        _font_file_handle: WinHandle,
        face_index: u32,
    ) -> HResult {
        let handle = self.alloc_handle();
        self.font_faces.insert(
            handle.0,
            DWriteFontFace {
                handle,
                face_type,
                glyph_count: 65535,
                index: face_index,
                simulations: 0,
                is_symbol: false,
                metrics_em: 2048,
                ascent: 1854,
                descent: 434,
                line_gap: 0,
                cap_height: 1456,
                x_height: 1062,
                underline_position: -292,
                underline_thickness: 100,
                strikethrough_position: 530,
                strikethrough_thickness: 100,
            },
        );
        HResult(S_OK)
    }

    pub fn create_rendering_params(&mut self) -> HResult {
        let handle = self.alloc_handle();
        self.rendering_params.insert(
            handle.0,
            DWriteRenderingParams {
                gamma: 1.8,
                enhanced_contrast: 0.5,
                cleartype_level: 1.0,
                pixel_geometry: DWritePixelGeometry::Rgb,
                rendering_mode: DWriteRenderingMode::NaturalSymmetric,
            },
        );
        HResult(S_OK)
    }

    pub fn create_custom_rendering_params(
        &mut self,
        gamma: f32,
        enhanced_contrast: f32,
        cleartype_level: f32,
        pixel_geometry: DWritePixelGeometry,
        rendering_mode: DWriteRenderingMode,
    ) -> HResult {
        let handle = self.alloc_handle();
        self.rendering_params.insert(
            handle.0,
            DWriteRenderingParams {
                gamma,
                enhanced_contrast,
                cleartype_level,
                pixel_geometry,
                rendering_mode,
            },
        );
        HResult(S_OK)
    }

    pub fn create_text_analyzer(&mut self) -> HResult {
        let handle = self.alloc_handle();
        self.text_analyzers
            .insert(handle.0, DWriteTextAnalyzer { handle });
        HResult(S_OK)
    }

    pub fn create_number_substitution(
        &mut self,
        method: DWriteNumberSubstitutionMethod,
        locale_name: &str,
        ignore_user_override: bool,
    ) -> HResult {
        let handle = self.alloc_handle();
        self.number_substitutions.insert(
            handle.0,
            DWriteNumberSubstitution {
                method,
                locale_name: String::from(locale_name),
                ignore_user_override,
            },
        );
        HResult(S_OK)
    }

    pub fn create_glyph_run_analysis(
        &mut self,
        glyph_count: u32,
        rendering_mode: DWriteRenderingMode,
        measuring_mode: DWriteMeasuringMode,
    ) -> HResult {
        let handle = self.alloc_handle();
        self.glyph_run_analyses.insert(
            handle.0,
            DWriteGlyphRunAnalysis {
                handle,
                rendering_mode,
                measuring_mode,
                glyph_count,
                bounds_left: 0,
                bounds_top: 0,
                bounds_right: glyph_count as i32 * 10,
                bounds_bottom: 16,
            },
        );
        HResult(S_OK)
    }

    pub fn create_ellipsis_trimming_sign(&mut self, _format: WinHandle) -> HResult {
        let handle = self.alloc_handle();
        self.inline_objects.insert(
            handle.0,
            DWriteInlineObject {
                handle,
                metrics: DWriteInlineObjectMetrics {
                    width: 12.0,
                    height: 16.0,
                    baseline: 12.0,
                    supports_sideways: false,
                },
                break_condition_before: DWriteBreakCondition::Neutral,
                break_condition_after: DWriteBreakCondition::Neutral,
            },
        );
        HResult(S_OK)
    }
}

// ---------------------------------------------------------------------------
// Global DirectWrite runtime
// ---------------------------------------------------------------------------

pub struct DWriteRuntime {
    pub initialized: bool,
    pub factory: Option<DWriteFactory>,
    pub dpi_x: f32,
    pub dpi_y: f32,
}

impl DWriteRuntime {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            factory: None,
            dpi_x: 96.0,
            dpi_y: 96.0,
        }
    }

    pub fn init(&mut self) {
        if self.initialized {
            return;
        }

        let system_collection = DWriteFontCollection {
            handle: WinHandle(0xDFFF0001),
            families: default_system_fonts(),
            is_system: true,
        };

        let font_fallback = DWriteFontFallback {
            mappings: default_fallback_mappings(),
            default_family: String::from("Segoe UI"),
        };

        self.factory = Some(DWriteFactory {
            handle: WinHandle(0xDFFF0000),
            next_handle: 0xDFFF0100,
            system_font_collection: system_collection,
            text_formats: BTreeMap::new(),
            text_layouts: BTreeMap::new(),
            font_faces: BTreeMap::new(),
            inline_objects: BTreeMap::new(),
            rendering_params: BTreeMap::new(),
            font_fallback,
            custom_collections: BTreeMap::new(),
            text_analyzers: BTreeMap::new(),
            number_substitutions: BTreeMap::new(),
            glyph_run_analyses: BTreeMap::new(),
        });
        self.initialized = true;
    }

    pub fn factory(&self) -> Option<&DWriteFactory> {
        self.factory.as_ref()
    }

    pub fn factory_mut(&mut self) -> Option<&mut DWriteFactory> {
        self.factory.as_mut()
    }
}

fn default_system_fonts() -> Vec<DWriteFontFamily> {
    let families = [
        "Segoe UI",
        "Arial",
        "Times New Roman",
        "Courier New",
        "Consolas",
        "Calibri",
        "Cambria",
        "Tahoma",
        "Verdana",
        "Georgia",
        "Trebuchet MS",
        "Comic Sans MS",
        "Impact",
        "Lucida Console",
        "Palatino Linotype",
        "Segoe UI Symbol",
        "Segoe UI Emoji",
        "Yu Gothic",
        "Malgun Gothic",
        "Microsoft YaHei",
    ];
    families
        .iter()
        .map(|&name| DWriteFontFamily {
            name: String::from(name),
            fonts: Vec::new(),
        })
        .collect()
}

fn default_fallback_mappings() -> Vec<DWriteFontFallbackMapping> {
    let mut mappings = Vec::new();
    mappings.push(DWriteFontFallbackMapping {
        unicode_ranges: vec![(0x4E00, 0x9FFF), (0x3400, 0x4DBF)],
        target_family_names: vec![String::from("Microsoft YaHei")],
        scale: 1.0,
    });
    mappings.push(DWriteFontFallbackMapping {
        unicode_ranges: vec![(0xAC00, 0xD7AF)],
        target_family_names: vec![String::from("Malgun Gothic")],
        scale: 1.0,
    });
    mappings.push(DWriteFontFallbackMapping {
        unicode_ranges: vec![(0x3040, 0x309F), (0x30A0, 0x30FF)],
        target_family_names: vec![String::from("Yu Gothic")],
        scale: 1.0,
    });
    mappings.push(DWriteFontFallbackMapping {
        unicode_ranges: vec![(0x0600, 0x06FF)],
        target_family_names: vec![String::from("Segoe UI")],
        scale: 1.0,
    });
    mappings.push(DWriteFontFallbackMapping {
        unicode_ranges: vec![(0x0590, 0x05FF)],
        target_family_names: vec![String::from("Segoe UI")],
        scale: 1.0,
    });
    mappings
}

static mut DWRITE: DWriteRuntime = DWriteRuntime::new();

pub fn init() {
    unsafe {
        DWRITE.init();
    }
}

pub fn runtime() -> &'static DWriteRuntime {
    unsafe { &DWRITE }
}

pub fn runtime_mut() -> &'static mut DWriteRuntime {
    unsafe { &mut DWRITE }
}
