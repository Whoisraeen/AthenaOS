//! AthUI i18n / Localization
//!
//! Provides string translation with key lookup, plural rules, number/date
//! formatting, RTL support, and a fallback chain (app locale → system → "en").

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── Layout Direction ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutDirection {
    LeftToRight,
    RightToLeft,
}

// ── Plural Category ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluralCategory {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

// ── Plural Rules ────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluralRules {
    English,
    French,
    Arabic,
    Polish,
    Japanese,
}

impl PluralRules {
    pub fn select(&self, count: u64) -> PluralCategory {
        match self {
            PluralRules::English => {
                if count == 1 {
                    PluralCategory::One
                } else {
                    PluralCategory::Other
                }
            }
            PluralRules::French => {
                if count == 0 || count == 1 {
                    PluralCategory::One
                } else {
                    PluralCategory::Other
                }
            }
            PluralRules::Arabic => {
                if count == 0 {
                    PluralCategory::Zero
                } else if count == 1 {
                    PluralCategory::One
                } else if count == 2 {
                    PluralCategory::Two
                } else if count % 100 >= 3 && count % 100 <= 10 {
                    PluralCategory::Few
                } else if count % 100 >= 11 && count % 100 <= 99 {
                    PluralCategory::Many
                } else {
                    PluralCategory::Other
                }
            }
            PluralRules::Polish => {
                let mod10 = count % 10;
                let mod100 = count % 100;
                if count == 1 {
                    PluralCategory::One
                } else if mod10 >= 2 && mod10 <= 4 && !(mod100 >= 12 && mod100 <= 14) {
                    PluralCategory::Few
                } else {
                    PluralCategory::Many
                }
            }
            PluralRules::Japanese => PluralCategory::Other,
        }
    }
}

// ── Number Format ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct NumberFormat {
    pub decimal_separator: char,
    pub thousands_separator: char,
    pub decimal_places: u8,
}

impl NumberFormat {
    pub fn english() -> Self {
        Self {
            decimal_separator: '.',
            thousands_separator: ',',
            decimal_places: 2,
        }
    }

    pub fn european() -> Self {
        Self {
            decimal_separator: ',',
            thousands_separator: '.',
            decimal_places: 2,
        }
    }

    pub fn format_integer(&self, value: i64) -> String {
        let negative = value < 0;
        let abs_val = if negative {
            (-(value as i128)) as u64
        } else {
            value as u64
        };
        let digits = format_u64(abs_val);

        let mut result = String::new();
        if negative {
            result.push('-');
        }

        let len = digits.len();
        for (i, ch) in digits.chars().enumerate() {
            result.push(ch);
            let remaining = len - 1 - i;
            if remaining > 0 && remaining % 3 == 0 {
                result.push(self.thousands_separator);
            }
        }
        result
    }

    pub fn format_float(&self, value: f32) -> String {
        let negative = value < 0.0;
        let abs_val = if negative { -value } else { value };

        let int_part = abs_val as u64;
        let multiplier = pow10(self.decimal_places as u32);
        let frac_part = ((abs_val - int_part as f32) * multiplier as f32) as u64;

        let mut result = String::new();
        if negative {
            result.push('-');
        }

        // Integer part with thousands separator
        let int_str = format_u64(int_part);
        let len = int_str.len();
        for (i, ch) in int_str.chars().enumerate() {
            result.push(ch);
            let remaining = len - 1 - i;
            if remaining > 0 && remaining % 3 == 0 {
                result.push(self.thousands_separator);
            }
        }

        if self.decimal_places > 0 {
            result.push(self.decimal_separator);
            let frac_str = format_u64(frac_part);
            // Pad with leading zeros if needed
            for _ in frac_str.len()..(self.decimal_places as usize) {
                result.push('0');
            }
            // Truncate to decimal_places
            for (i, ch) in frac_str.chars().enumerate() {
                if i >= self.decimal_places as usize {
                    break;
                }
                result.push(ch);
            }
        }
        result
    }
}

// ── Date Format ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFormat {
    YearMonthDay, // 2026-05-26
    DayMonthYear, // 26/05/2026
    MonthDayYear, // 05/26/2026
}

impl DateFormat {
    pub fn format(&self, year: u16, month: u8, day: u8) -> String {
        match self {
            DateFormat::YearMonthDay => {
                let mut s = String::new();
                push_u16(&mut s, year);
                s.push('-');
                push_u8_padded(&mut s, month);
                s.push('-');
                push_u8_padded(&mut s, day);
                s
            }
            DateFormat::DayMonthYear => {
                let mut s = String::new();
                push_u8_padded(&mut s, day);
                s.push('/');
                push_u8_padded(&mut s, month);
                s.push('/');
                push_u16(&mut s, year);
                s
            }
            DateFormat::MonthDayYear => {
                let mut s = String::new();
                push_u8_padded(&mut s, month);
                s.push('/');
                push_u8_padded(&mut s, day);
                s.push('/');
                push_u16(&mut s, year);
                s
            }
        }
    }
}

// ── Plural String Entry ─────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PluralEntry {
    pub zero: Option<String>,
    pub one: Option<String>,
    pub two: Option<String>,
    pub few: Option<String>,
    pub many: Option<String>,
    pub other: String,
}

impl PluralEntry {
    pub fn simple(one: &str, other: &str) -> Self {
        Self {
            zero: None,
            one: Some(String::from(one)),
            two: None,
            few: None,
            many: None,
            other: String::from(other),
        }
    }

    pub fn select(&self, category: PluralCategory) -> &str {
        match category {
            PluralCategory::Zero => self.zero.as_deref().unwrap_or(&self.other),
            PluralCategory::One => self.one.as_deref().unwrap_or(&self.other),
            PluralCategory::Two => self.two.as_deref().unwrap_or(&self.other),
            PluralCategory::Few => self.few.as_deref().unwrap_or(&self.other),
            PluralCategory::Many => self.many.as_deref().unwrap_or(&self.other),
            PluralCategory::Other => &self.other,
        }
    }
}

// ── I18n Context ────────────────────────────────────────────────────────

pub struct I18n {
    pub locale: String,
    pub strings: BTreeMap<String, String>,
    pub plurals: BTreeMap<String, PluralEntry>,
    pub fallback: Option<alloc::boxed::Box<I18n>>,
    pub plural_rules: PluralRules,
    pub number_format: NumberFormat,
    pub date_format: DateFormat,
    pub direction: LayoutDirection,
}

impl I18n {
    pub fn new(locale: &str) -> Self {
        let (plural_rules, number_format, date_format, direction) = locale_defaults(locale);
        Self {
            locale: String::from(locale),
            strings: BTreeMap::new(),
            plurals: BTreeMap::new(),
            fallback: None,
            plural_rules,
            number_format,
            date_format,
            direction,
        }
    }

    /// Create an English locale context as the ultimate fallback.
    pub fn english() -> Self {
        Self::new("en")
    }

    /// Set a fallback I18n context (used when a key is missing).
    pub fn with_fallback(mut self, fallback: I18n) -> Self {
        self.fallback = Some(alloc::boxed::Box::new(fallback));
        self
    }

    /// Add a translated string.
    pub fn add_string(&mut self, key: &str, value: &str) {
        self.strings.insert(String::from(key), String::from(value));
    }

    /// Add a plural entry.
    pub fn add_plural(&mut self, key: &str, entry: PluralEntry) {
        self.plurals.insert(String::from(key), entry);
    }

    /// Translate a key. Falls back through the chain if not found.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(val) = self.strings.get(key) {
            return val.as_str();
        }
        if let Some(ref fb) = self.fallback {
            return fb.t(key);
        }
        key
    }

    /// Translate with pluralization.
    pub fn t_plural(&self, key: &str, count: u64) -> String {
        let category = self.plural_rules.select(count);
        if let Some(entry) = self.plurals.get(key) {
            let template = entry.select(category);
            return replace_count(template, count);
        }
        if let Some(ref fb) = self.fallback {
            return fb.t_plural(key, count);
        }
        let mut s = String::from(key);
        s.push_str(": ");
        s.push_str(&format_u64(count));
        s
    }

    /// Format a number using the locale's format rules.
    pub fn format_number(&self, value: f32) -> String {
        self.number_format.format_float(value)
    }

    /// Format an integer using locale's thousands separator.
    pub fn format_integer(&self, value: i64) -> String {
        self.number_format.format_integer(value)
    }

    /// Format a date using the locale's date format.
    pub fn format_date(&self, year: u16, month: u8, day: u8) -> String {
        self.date_format.format(year, month, day)
    }

    /// Get the layout direction for this locale.
    pub fn layout_direction(&self) -> LayoutDirection {
        self.direction
    }

    /// Check if this locale uses RTL text direction.
    pub fn is_rtl(&self) -> bool {
        self.direction == LayoutDirection::RightToLeft
    }

    /// Load strings from a list of key-value pairs.
    pub fn load_strings(&mut self, pairs: &[(&str, &str)]) {
        for (key, value) in pairs {
            self.add_string(key, value);
        }
    }

    /// Get all available keys.
    pub fn keys(&self) -> Vec<&String> {
        self.strings.keys().collect()
    }

    /// Check if a key exists in this locale (not checking fallback).
    pub fn has_key(&self, key: &str) -> bool {
        self.strings.contains_key(key)
    }
}

// ── Locale Defaults ─────────────────────────────────────────────────────

fn locale_defaults(locale: &str) -> (PluralRules, NumberFormat, DateFormat, LayoutDirection) {
    let base = locale.split('-').next().unwrap_or(locale);
    match base {
        "en" => (
            PluralRules::English,
            NumberFormat::english(),
            DateFormat::MonthDayYear,
            LayoutDirection::LeftToRight,
        ),
        "fr" => (
            PluralRules::French,
            NumberFormat {
                decimal_separator: ',',
                thousands_separator: ' ',
                decimal_places: 2,
            },
            DateFormat::DayMonthYear,
            LayoutDirection::LeftToRight,
        ),
        "de" => (
            PluralRules::English,
            NumberFormat::european(),
            DateFormat::DayMonthYear,
            LayoutDirection::LeftToRight,
        ),
        "es" => (
            PluralRules::English,
            NumberFormat::european(),
            DateFormat::DayMonthYear,
            LayoutDirection::LeftToRight,
        ),
        "it" => (
            PluralRules::English,
            NumberFormat::european(),
            DateFormat::DayMonthYear,
            LayoutDirection::LeftToRight,
        ),
        "pt" => (
            PluralRules::English,
            NumberFormat::european(),
            DateFormat::DayMonthYear,
            LayoutDirection::LeftToRight,
        ),
        "ja" | "zh" | "ko" => (
            PluralRules::Japanese,
            NumberFormat::english(),
            DateFormat::YearMonthDay,
            LayoutDirection::LeftToRight,
        ),
        "ar" | "he" | "fa" => (
            PluralRules::Arabic,
            NumberFormat {
                decimal_separator: '.',
                thousands_separator: ',',
                decimal_places: 2,
            },
            DateFormat::DayMonthYear,
            LayoutDirection::RightToLeft,
        ),
        "pl" => (
            PluralRules::Polish,
            NumberFormat {
                decimal_separator: ',',
                thousands_separator: ' ',
                decimal_places: 2,
            },
            DateFormat::DayMonthYear,
            LayoutDirection::LeftToRight,
        ),
        "ru" => (
            PluralRules::Polish,
            NumberFormat {
                decimal_separator: ',',
                thousands_separator: ' ',
                decimal_places: 2,
            },
            DateFormat::DayMonthYear,
            LayoutDirection::LeftToRight,
        ),
        _ => (
            PluralRules::English,
            NumberFormat::english(),
            DateFormat::YearMonthDay,
            LayoutDirection::LeftToRight,
        ),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn format_u64(mut val: u64) -> String {
    if val == 0 {
        return String::from("0");
    }
    let mut digits = Vec::new();
    while val > 0 {
        digits.push((b'0' + (val % 10) as u8) as char);
        val /= 10;
    }
    digits.reverse();
    digits.into_iter().collect()
}

fn pow10(exp: u32) -> u64 {
    let mut result: u64 = 1;
    for _ in 0..exp {
        result *= 10;
    }
    result
}

fn push_u16(s: &mut String, val: u16) {
    let formatted = format_u64(val as u64);
    // Pad to 4 digits for year
    for _ in formatted.len()..4 {
        s.push('0');
    }
    s.push_str(&formatted);
}

fn push_u8_padded(s: &mut String, val: u8) {
    if val < 10 {
        s.push('0');
    }
    s.push_str(&format_u64(val as u64));
}

fn replace_count(template: &str, count: u64) -> String {
    let count_str = format_u64(count);
    let mut result = String::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'}' {
            result.push_str(&count_str);
            i += 2;
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

// ── Locale Chain Builder ────────────────────────────────────────────────

/// Build an I18n context with automatic fallback chain.
/// Example: `build_locale_chain("fr-FR", &[("fr-FR", &fr_strings), ("en", &en_strings)])`
pub fn build_locale_chain(locale: &str, translations: &[(&str, &[(&str, &str)])]) -> I18n {
    let mut chain: Option<I18n> = None;

    // Build from least specific (last) to most specific (first)
    for (loc, strings) in translations.iter().rev() {
        let mut ctx = I18n::new(loc);
        ctx.load_strings(strings);
        if let Some(prev) = chain {
            ctx.fallback = Some(alloc::boxed::Box::new(prev));
        }
        chain = Some(ctx);
    }

    chain.unwrap_or_else(|| I18n::new(locale))
}
