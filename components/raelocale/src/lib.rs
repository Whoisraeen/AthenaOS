#![no_std]

extern crate alloc;

/// Alternate keyboard-layout support (parity gap #5): scancode→char tables for
/// non-US layouts (AZERTY/QWERTZ/Dvorak) + a pure, never-panic lookup API.
pub mod keyboard;

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// Locale identifier — BCP 47 / IETF language tags
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locale {
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
    pub variant: Option<String>,
    pub extensions: Vec<LocaleExtension>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleExtension {
    pub singleton: char,
    pub value: String,
}

impl Locale {
    pub fn new(language: &str) -> Self {
        Self {
            language: String::from(language),
            script: None,
            region: None,
            variant: None,
            extensions: Vec::new(),
        }
    }

    pub fn parse(tag: &str) -> Option<Self> {
        let mut parts = tag.split('-');
        let language = String::from(parts.next()?);
        if language.is_empty() {
            return None;
        }

        let mut locale = Locale {
            language,
            script: None,
            region: None,
            variant: None,
            extensions: Vec::new(),
        };

        let mut state = ParseState::AfterLanguage;
        for part in parts {
            match state {
                ParseState::AfterLanguage => {
                    if part.len() == 4 && part.as_bytes()[0].is_ascii_uppercase() {
                        locale.script = Some(String::from(part));
                        state = ParseState::AfterScript;
                    } else if part.len() == 2 && part.as_bytes()[0].is_ascii_uppercase() {
                        locale.region = Some(String::from(part));
                        state = ParseState::AfterRegion;
                    } else if part.len() == 3 && part.as_bytes()[0].is_ascii_digit() {
                        locale.region = Some(String::from(part));
                        state = ParseState::AfterRegion;
                    } else if part.len() >= 5 {
                        locale.variant = Some(String::from(part));
                        state = ParseState::AfterVariant;
                    } else if part.len() == 1 {
                        state = ParseState::InExtension(part.as_bytes()[0] as char);
                    }
                }
                ParseState::AfterScript => {
                    if part.len() == 2 && part.as_bytes()[0].is_ascii_uppercase() {
                        locale.region = Some(String::from(part));
                        state = ParseState::AfterRegion;
                    } else if part.len() == 3 && part.as_bytes()[0].is_ascii_digit() {
                        locale.region = Some(String::from(part));
                        state = ParseState::AfterRegion;
                    } else if part.len() == 1 {
                        state = ParseState::InExtension(part.as_bytes()[0] as char);
                    }
                }
                ParseState::AfterRegion => {
                    if part.len() >= 5 {
                        locale.variant = Some(String::from(part));
                        state = ParseState::AfterVariant;
                    } else if part.len() == 1 {
                        state = ParseState::InExtension(part.as_bytes()[0] as char);
                    }
                }
                ParseState::AfterVariant => {
                    if part.len() == 1 {
                        state = ParseState::InExtension(part.as_bytes()[0] as char);
                    }
                }
                ParseState::InExtension(singleton) => {
                    locale.extensions.push(LocaleExtension {
                        singleton,
                        value: String::from(part),
                    });
                    state = ParseState::AfterVariant;
                }
            }
        }

        Some(locale)
    }

    pub fn to_string(&self) -> String {
        let mut s = self.language.clone();
        if let Some(ref sc) = self.script {
            s.push('-');
            s.push_str(sc);
        }
        if let Some(ref rg) = self.region {
            s.push('-');
            s.push_str(rg);
        }
        if let Some(ref v) = self.variant {
            s.push('-');
            s.push_str(v);
        }
        for ext in &self.extensions {
            s.push('-');
            s.push(ext.singleton);
            s.push('-');
            s.push_str(&ext.value);
        }
        s
    }

    pub fn inheritance_chain(&self) -> Vec<String> {
        let mut chain = Vec::new();
        chain.push(self.to_string());
        if self.variant.is_some() {
            let mut loc = self.clone();
            loc.variant = None;
            chain.push(loc.to_string());
        }
        if self.region.is_some() {
            let mut loc = self.clone();
            loc.region = None;
            loc.variant = None;
            chain.push(loc.to_string());
        }
        if self.script.is_some() {
            let mut loc = self.clone();
            loc.script = None;
            loc.region = None;
            loc.variant = None;
            chain.push(loc.to_string());
        }
        chain.push(String::from("root"));
        chain
    }

    pub fn likely_subtag_expand(&mut self) {
        if self.script.is_none() {
            self.script = Some(String::from(match self.language.as_str() {
                "zh" => "Hans",
                "ja" => "Jpan",
                "ko" => "Kore",
                "ar" => "Arab",
                "he" => "Hebr",
                "hi" => "Deva",
                "bn" => "Beng",
                "ta" => "Taml",
                "te" => "Telu",
                "th" => "Thai",
                "ka" => "Geor",
                "hy" => "Armn",
                "el" => "Grek",
                "ru" | "uk" | "bg" | "sr" => "Cyrl",
                _ => "Latn",
            }));
        }
        if self.region.is_none() {
            self.region = Some(String::from(match self.language.as_str() {
                "en" => "US",
                "fr" => "FR",
                "de" => "DE",
                "es" => "ES",
                "pt" => "BR",
                "zh" => "CN",
                "ja" => "JP",
                "ko" => "KR",
                "ar" => "SA",
                "he" => "IL",
                "hi" => "IN",
                "ru" => "RU",
                "it" => "IT",
                "nl" => "NL",
                "pl" => "PL",
                "tr" => "TR",
                "sv" => "SE",
                "da" => "DK",
                "fi" => "FI",
                "nb" | "nn" => "NO",
                "th" => "TH",
                "vi" => "VN",
                "id" => "ID",
                "ms" => "MY",
                _ => "001",
            }));
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ParseState {
    AfterLanguage,
    AfterScript,
    AfterRegion,
    AfterVariant,
    InExtension(char),
}

pub fn locale_match(requested: &[Locale], available: &[Locale]) -> Option<usize> {
    for req in requested {
        for (i, avail) in available.iter().enumerate() {
            if req.language == avail.language {
                if req.region == avail.region || req.region.is_none() || avail.region.is_none() {
                    return Some(i);
                }
            }
        }
    }
    for req in requested {
        for (i, avail) in available.iter().enumerate() {
            if req.language == avail.language {
                return Some(i);
            }
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════
// Number formatting
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingMode {
    HalfUp,
    HalfDown,
    HalfEven,
    Ceiling,
    Floor,
    Truncate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberSystem {
    Latn,
    Arab,
    Deva,
    Beng,
    Guru,
    Gujr,
    Orya,
    Taml,
    Telu,
    Knda,
    Mlym,
    Thai,
    Laoo,
    Tibt,
    Mymr,
}

impl NumberSystem {
    pub fn zero_digit(self) -> u32 {
        match self {
            Self::Latn => 0x0030,
            Self::Arab => 0x0660,
            Self::Deva => 0x0966,
            Self::Beng => 0x09E6,
            Self::Guru => 0x0A66,
            Self::Gujr => 0x0AE6,
            Self::Orya => 0x0B66,
            Self::Taml => 0x0BE6,
            Self::Telu => 0x0C66,
            Self::Knda => 0x0CE6,
            Self::Mlym => 0x0D66,
            Self::Thai => 0x0E50,
            Self::Laoo => 0x0ED0,
            Self::Tibt => 0x0F20,
            Self::Mymr => 0x1040,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NumberFormatSymbols {
    pub decimal_separator: char,
    pub grouping_separator: char,
    pub percent_sign: char,
    pub minus_sign: char,
    pub plus_sign: char,
    pub exponent_separator: String,
    pub infinity: String,
    pub nan: String,
    pub currency_symbol: String,
    pub currency_code: String,
    pub number_system: NumberSystem,
}

impl Default for NumberFormatSymbols {
    fn default() -> Self {
        Self {
            decimal_separator: '.',
            grouping_separator: ',',
            percent_sign: '%',
            minus_sign: '-',
            plus_sign: '+',
            exponent_separator: String::from("E"),
            infinity: String::from("\u{221E}"),
            nan: String::from("NaN"),
            currency_symbol: String::from("$"),
            currency_code: String::from("USD"),
            number_system: NumberSystem::Latn,
        }
    }
}

pub fn symbols_for_locale(locale: &Locale) -> NumberFormatSymbols {
    let mut sym = NumberFormatSymbols::default();
    match locale.language.as_str() {
        "de" => {
            sym.decimal_separator = ',';
            sym.grouping_separator = '.';
            sym.currency_symbol = String::from("\u{20AC}");
            sym.currency_code = String::from("EUR");
        }
        "fr" => {
            sym.decimal_separator = ',';
            sym.grouping_separator = '\u{202F}';
            sym.currency_symbol = String::from("\u{20AC}");
            sym.currency_code = String::from("EUR");
        }
        "ar" => {
            sym.decimal_separator = '\u{066B}';
            sym.grouping_separator = '\u{066C}';
            sym.percent_sign = '\u{066A}';
            sym.number_system = NumberSystem::Arab;
        }
        "hi" => {
            sym.number_system = NumberSystem::Deva;
            sym.currency_symbol = String::from("\u{20B9}");
            sym.currency_code = String::from("INR");
        }
        "ja" => {
            sym.currency_symbol = String::from("\u{00A5}");
            sym.currency_code = String::from("JPY");
        }
        "ko" => {
            sym.currency_symbol = String::from("\u{20A9}");
            sym.currency_code = String::from("KRW");
        }
        "zh" => {
            sym.currency_symbol = String::from("\u{00A5}");
            sym.currency_code = String::from("CNY");
        }
        "ru" => {
            sym.decimal_separator = ',';
            sym.grouping_separator = '\u{00A0}';
            sym.currency_symbol = String::from("\u{20BD}");
            sym.currency_code = String::from("RUB");
        }
        "pt" => {
            sym.decimal_separator = ',';
            sym.grouping_separator = '.';
            sym.currency_symbol = String::from("R$");
            sym.currency_code = String::from("BRL");
        }
        "tr" => {
            sym.decimal_separator = ',';
            sym.grouping_separator = '.';
            sym.currency_symbol = String::from("\u{20BA}");
            sym.currency_code = String::from("TRY");
        }
        _ => {}
    }
    sym
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactStyle {
    Short,
    Long,
}

pub struct NumberFormatter {
    pub symbols: NumberFormatSymbols,
    pub min_integer_digits: u8,
    pub min_fraction_digits: u8,
    pub max_fraction_digits: u8,
    pub grouping_size: u8,
    pub secondary_grouping_size: u8,
    pub rounding_mode: RoundingMode,
    pub use_grouping: bool,
}

impl NumberFormatter {
    pub fn new(symbols: NumberFormatSymbols) -> Self {
        Self {
            symbols,
            min_integer_digits: 1,
            min_fraction_digits: 0,
            max_fraction_digits: 3,
            grouping_size: 3,
            secondary_grouping_size: 0,
            rounding_mode: RoundingMode::HalfEven,
            use_grouping: true,
        }
    }

    pub fn format_i64(&self, value: i64) -> String {
        let negative = value < 0;
        let abs_val = if value == i64::MIN {
            value as u64
        } else {
            value.unsigned_abs()
        };
        let mut digits = Vec::new();
        let mut v = abs_val;
        if v == 0 {
            digits.push(0u8);
        }
        while v > 0 {
            digits.push((v % 10) as u8);
            v /= 10;
        }
        digits.reverse();
        while digits.len() < self.min_integer_digits as usize {
            digits.insert(0, 0);
        }
        let zero_base = self.symbols.number_system.zero_digit();
        let mut result = String::new();
        if negative {
            result.push(self.symbols.minus_sign);
        }
        let len = digits.len();
        for (i, &d) in digits.iter().enumerate() {
            if let Some(ch) = char::from_u32(zero_base + d as u32) {
                result.push(ch);
            }
            if self.use_grouping && i < len - 1 {
                let remaining = len - 1 - i;
                let primary = self.grouping_size as usize;
                let secondary = if self.secondary_grouping_size > 0 {
                    self.secondary_grouping_size as usize
                } else {
                    primary
                };
                if primary > 0 && remaining == primary {
                    result.push(self.symbols.grouping_separator);
                } else if remaining > primary && (remaining - primary) % secondary == 0 {
                    result.push(self.symbols.grouping_separator);
                }
            }
        }
        result
    }

    pub fn format_f64(&self, value: f64) -> String {
        if value.is_nan() {
            return self.symbols.nan.clone();
        }
        if value.is_infinite() {
            let mut s = String::new();
            if value < 0.0 {
                s.push(self.symbols.minus_sign);
            }
            s.push_str(&self.symbols.infinity);
            return s;
        }
        let negative = value < 0.0;
        let abs_val = if negative { -value } else { value };
        let rounded = self.round(abs_val, self.max_fraction_digits);
        let int_part = rounded as u64;
        let frac = rounded - int_part as f64;
        let mut result = self.format_i64(if negative {
            -(int_part as i64)
        } else {
            int_part as i64
        });
        let frac_digits = self.max_fraction_digits.max(self.min_fraction_digits);
        if frac_digits > 0 {
            result.push(self.symbols.decimal_separator);
            let mut frac_val = frac;
            let zero_base = self.symbols.number_system.zero_digit();
            for _ in 0..frac_digits {
                frac_val *= 10.0;
                let digit = frac_val as u8;
                frac_val -= digit as f64;
                if let Some(ch) = char::from_u32(zero_base + digit as u32) {
                    result.push(ch);
                }
            }
        }
        result
    }

    pub fn format_percent(&self, value: f64) -> String {
        let mut s = self.format_f64(value * 100.0);
        s.push(self.symbols.percent_sign);
        s
    }

    pub fn format_currency(&self, value: f64) -> String {
        let mut s = self.symbols.currency_symbol.clone();
        s.push_str(&self.format_f64(value));
        s
    }

    pub fn format_compact(&self, value: f64, style: CompactStyle) -> String {
        let (divisor, suffix) = if value.abs() >= 1_000_000_000_000.0 {
            (
                1_000_000_000_000.0,
                match style {
                    CompactStyle::Short => "T",
                    CompactStyle::Long => " trillion",
                },
            )
        } else if value.abs() >= 1_000_000_000.0 {
            (
                1_000_000_000.0,
                match style {
                    CompactStyle::Short => "B",
                    CompactStyle::Long => " billion",
                },
            )
        } else if value.abs() >= 1_000_000.0 {
            (
                1_000_000.0,
                match style {
                    CompactStyle::Short => "M",
                    CompactStyle::Long => " million",
                },
            )
        } else if value.abs() >= 1_000.0 {
            (
                1_000.0,
                match style {
                    CompactStyle::Short => "K",
                    CompactStyle::Long => " thousand",
                },
            )
        } else {
            (1.0, "")
        };
        let mut s = self.format_f64(value / divisor);
        s.push_str(suffix);
        s
    }

    pub fn format_scientific(&self, value: f64) -> String {
        if value == 0.0 {
            return String::from("0E0");
        }
        let negative = value < 0.0;
        let abs_val = if negative { -value } else { value };
        let exp = log10_approx(abs_val) as i32;
        let mantissa = abs_val / pow10(exp);
        let mut s = String::new();
        if negative {
            s.push(self.symbols.minus_sign);
        }
        let rounded = self.round(mantissa, self.max_fraction_digits);
        s.push_str(&format_simple_f64(
            rounded,
            self.max_fraction_digits,
            self.symbols.decimal_separator,
        ));
        s.push_str(&self.symbols.exponent_separator);
        if exp < 0 {
            s.push(self.symbols.minus_sign);
        }
        s.push_str(&format_simple_u64(exp.unsigned_abs() as u64));
        s
    }

    fn round(&self, value: f64, places: u8) -> f64 {
        let factor = pow10(places as i32);
        let scaled = value * factor;
        let rounded = match self.rounding_mode {
            RoundingMode::HalfUp => (scaled + 0.5) as u64 as f64,
            RoundingMode::HalfDown => {
                let frac = scaled - (scaled as u64 as f64);
                if frac > 0.5 {
                    (scaled as u64 + 1) as f64
                } else {
                    scaled as u64 as f64
                }
            }
            RoundingMode::HalfEven => {
                let floor = scaled as u64;
                let frac = scaled - floor as f64;
                if frac > 0.5 {
                    (floor + 1) as f64
                } else if frac < 0.5 {
                    floor as f64
                } else if floor % 2 == 0 {
                    floor as f64
                } else {
                    (floor + 1) as f64
                }
            }
            RoundingMode::Ceiling => {
                let floor = scaled as u64;
                if scaled > floor as f64 {
                    (floor + 1) as f64
                } else {
                    floor as f64
                }
            }
            RoundingMode::Floor => scaled as u64 as f64,
            RoundingMode::Truncate => scaled as u64 as f64,
        };
        rounded / factor
    }
}

fn log10_approx(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut exp = 0i32;
    let mut val = x;
    while val >= 10.0 {
        val /= 10.0;
        exp += 1;
    }
    while val < 1.0 {
        val *= 10.0;
        exp -= 1;
    }
    exp as f64
}

fn pow10(exp: i32) -> f64 {
    let mut result = 1.0;
    let abs_exp = exp.unsigned_abs();
    for _ in 0..abs_exp {
        result *= 10.0;
    }
    if exp < 0 {
        1.0 / result
    } else {
        result
    }
}

fn format_simple_f64(value: f64, frac_digits: u8, sep: char) -> String {
    let int_part = value as u64;
    let frac = value - int_part as f64;
    let mut s = format_simple_u64(int_part);
    if frac_digits > 0 {
        s.push(sep);
        let mut fv = frac;
        for _ in 0..frac_digits {
            fv *= 10.0;
            let d = fv as u8;
            fv -= d as f64;
            s.push((b'0' + d) as char);
        }
    }
    s
}

fn format_simple_u64(value: u64) -> String {
    if value == 0 {
        return String::from("0");
    }
    let mut digits = Vec::new();
    let mut v = value;
    while v > 0 {
        digits.push((b'0' + (v % 10) as u8) as char);
        v /= 10;
    }
    digits.reverse();
    digits.into_iter().collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Date/Time formatting
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateStyle {
    Full,
    Long,
    Medium,
    Short,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarSystem {
    Gregorian,
    Islamic,
    Hebrew,
    Chinese,
    Japanese,
    Buddhist,
    Persian,
    Coptic,
    Ethiopian,
    Indian,
}

#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanosecond: u32,
    pub utc_offset_minutes: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayPeriod {
    Am,
    Pm,
    Noon,
    Midnight,
}

impl DateTime {
    pub fn day_of_week(&self) -> u8 {
        let y = if self.month <= 2 {
            self.year - 1
        } else {
            self.year
        };
        let m = if self.month <= 2 {
            self.month as i32 + 12
        } else {
            self.month as i32
        };
        let q = self.day as i32;
        let k = y % 100;
        let j = y / 100;
        let h = (q + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 - 2 * j) % 7;
        (((h + 6) % 7) as u8)
    }

    pub fn day_period(&self) -> DayPeriod {
        if self.hour == 0 && self.minute == 0 {
            DayPeriod::Midnight
        } else if self.hour == 12 && self.minute == 0 {
            DayPeriod::Noon
        } else if self.hour < 12 {
            DayPeriod::Am
        } else {
            DayPeriod::Pm
        }
    }

    pub fn era(&self) -> &'static str {
        if self.year > 0 {
            "AD"
        } else {
            "BC"
        }
    }

    pub fn quarter(&self) -> u8 {
        (self.month - 1) / 3 + 1
    }

    pub fn is_leap_year(&self) -> bool {
        (self.year % 4 == 0 && self.year % 100 != 0) || self.year % 400 == 0
    }

    pub fn days_in_month(&self) -> u8 {
        match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if self.is_leap_year() {
                    29
                } else {
                    28
                }
            }
            _ => 30,
        }
    }

    pub fn day_of_year(&self) -> u16 {
        let month_days = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let mut day = month_days[self.month as usize - 1] + self.day as u16;
        if self.month > 2 && self.is_leap_year() {
            day += 1;
        }
        day
    }
}

#[derive(Debug, Clone)]
pub struct WeekData {
    pub first_day: u8,
    pub weekend_start: u8,
    pub weekend_end: u8,
    pub min_days_in_first_week: u8,
}

impl WeekData {
    pub fn for_region(region: &str) -> Self {
        match region {
            "US" | "CA" | "JP" | "TW" | "KR" | "TH" | "PH" | "IL" => WeekData {
                first_day: 0,
                weekend_start: 6,
                weekend_end: 0,
                min_days_in_first_week: 1,
            },
            "SA" | "AE" | "BH" | "DJ" | "DZ" | "EG" | "IQ" | "JO" | "KW" | "LY" | "OM" | "QA"
            | "SD" | "SY" | "YE" => WeekData {
                first_day: 6,
                weekend_start: 5,
                weekend_end: 6,
                min_days_in_first_week: 1,
            },
            "AF" | "IR" => WeekData {
                first_day: 6,
                weekend_start: 4,
                weekend_end: 5,
                min_days_in_first_week: 1,
            },
            _ => WeekData {
                first_day: 1,
                weekend_start: 6,
                weekend_end: 0,
                min_days_in_first_week: 4,
            },
        }
    }
}

pub struct DateTimeFormatter {
    pub locale: Locale,
    pub calendar: CalendarSystem,
}

impl DateTimeFormatter {
    pub fn new(locale: Locale) -> Self {
        Self {
            locale,
            calendar: CalendarSystem::Gregorian,
        }
    }

    pub fn format_date(&self, dt: &DateTime, style: DateStyle) -> String {
        match style {
            DateStyle::Full => self.format_pattern(dt, "EEEE, MMMM d, y"),
            DateStyle::Long => self.format_pattern(dt, "MMMM d, y"),
            DateStyle::Medium => self.format_pattern(dt, "MMM d, y"),
            DateStyle::Short => self.format_pattern(dt, "M/d/yy"),
        }
    }

    pub fn format_time(&self, dt: &DateTime, style: DateStyle) -> String {
        match style {
            DateStyle::Full => self.format_pattern(dt, "h:mm:ss a zzzz"),
            DateStyle::Long => self.format_pattern(dt, "h:mm:ss a z"),
            DateStyle::Medium => self.format_pattern(dt, "h:mm:ss a"),
            DateStyle::Short => self.format_pattern(dt, "h:mm a"),
        }
    }

    pub fn format_pattern(&self, dt: &DateTime, pattern: &str) -> String {
        let mut result = String::new();
        let bytes = pattern.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let ch = bytes[i] as char;
            let mut count = 1;
            while i + count < bytes.len() && bytes[i + count] == bytes[i] {
                count += 1;
            }
            match ch {
                'y' => {
                    if count <= 2 {
                        result.push_str(&format_simple_u64((dt.year % 100) as u64));
                    } else {
                        result.push_str(&format_simple_u64(dt.year as u64));
                    }
                }
                'M' => match count {
                    1 => result.push_str(&format_simple_u64(dt.month as u64)),
                    2 => {
                        if dt.month < 10 {
                            result.push('0');
                        }
                        result.push_str(&format_simple_u64(dt.month as u64));
                    }
                    3 => result.push_str(short_month_name(dt.month)),
                    _ => result.push_str(long_month_name(dt.month)),
                },
                'd' => {
                    if count >= 2 && dt.day < 10 {
                        result.push('0');
                    }
                    result.push_str(&format_simple_u64(dt.day as u64));
                }
                'E' => {
                    let dow = dt.day_of_week();
                    if count <= 3 {
                        result.push_str(short_day_name(dow));
                    } else {
                        result.push_str(long_day_name(dow));
                    }
                }
                'h' => {
                    let h = if dt.hour == 0 {
                        12
                    } else if dt.hour > 12 {
                        dt.hour - 12
                    } else {
                        dt.hour
                    };
                    if count >= 2 && h < 10 {
                        result.push('0');
                    }
                    result.push_str(&format_simple_u64(h as u64));
                }
                'H' => {
                    if count >= 2 && dt.hour < 10 {
                        result.push('0');
                    }
                    result.push_str(&format_simple_u64(dt.hour as u64));
                }
                'm' => {
                    if count >= 2 && dt.minute < 10 {
                        result.push('0');
                    }
                    result.push_str(&format_simple_u64(dt.minute as u64));
                }
                's' => {
                    if count >= 2 && dt.second < 10 {
                        result.push('0');
                    }
                    result.push_str(&format_simple_u64(dt.second as u64));
                }
                'a' => {
                    result.push_str(match dt.day_period() {
                        DayPeriod::Am | DayPeriod::Midnight => "AM",
                        DayPeriod::Pm | DayPeriod::Noon => "PM",
                    });
                }
                'z' => {
                    let offset = dt.utc_offset_minutes;
                    let sign = if offset >= 0 { '+' } else { '-' };
                    let abs = offset.unsigned_abs();
                    result.push_str("UTC");
                    result.push(sign);
                    result.push_str(&format_simple_u64((abs / 60) as u64));
                    if abs % 60 != 0 {
                        result.push(':');
                        let m = abs % 60;
                        if m < 10 {
                            result.push('0');
                        }
                        result.push_str(&format_simple_u64(m as u64));
                    }
                }
                'G' => result.push_str(dt.era()),
                'Q' => result.push_str(&format_simple_u64(dt.quarter() as u64)),
                ' ' | ',' | '/' | ':' | '-' => {
                    for _ in 0..count {
                        result.push(ch);
                    }
                }
                _ => {
                    for _ in 0..count {
                        result.push(ch);
                    }
                }
            }
            i += count;
        }
        result
    }

    pub fn format_relative(&self, value: i64, unit: RelativeTimeUnit) -> String {
        let abs_val = value.unsigned_abs();
        let unit_name = match unit {
            RelativeTimeUnit::Second => {
                if abs_val == 1 {
                    "second"
                } else {
                    "seconds"
                }
            }
            RelativeTimeUnit::Minute => {
                if abs_val == 1 {
                    "minute"
                } else {
                    "minutes"
                }
            }
            RelativeTimeUnit::Hour => {
                if abs_val == 1 {
                    "hour"
                } else {
                    "hours"
                }
            }
            RelativeTimeUnit::Day => {
                if abs_val == 1 {
                    "day"
                } else {
                    "days"
                }
            }
            RelativeTimeUnit::Week => {
                if abs_val == 1 {
                    "week"
                } else {
                    "weeks"
                }
            }
            RelativeTimeUnit::Month => {
                if abs_val == 1 {
                    "month"
                } else {
                    "months"
                }
            }
            RelativeTimeUnit::Year => {
                if abs_val == 1 {
                    "year"
                } else {
                    "years"
                }
            }
        };
        let mut s = String::new();
        if value < 0 {
            s.push_str(&format_simple_u64(abs_val));
            s.push(' ');
            s.push_str(unit_name);
            s.push_str(" ago");
        } else if value > 0 {
            s.push_str("in ");
            s.push_str(&format_simple_u64(abs_val));
            s.push(' ');
            s.push_str(unit_name);
        } else {
            s.push_str("now");
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeTimeUnit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

fn short_month_name(m: u8) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "",
    }
}
fn long_month_name(m: u8) -> &'static str {
    match m {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "",
    }
}
fn short_day_name(d: u8) -> &'static str {
    match d {
        0 => "Sun",
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        6 => "Sat",
        _ => "",
    }
}
fn long_day_name(d: u8) -> &'static str {
    match d {
        0 => "Sunday",
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        _ => "",
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Collation — Unicode Collation Algorithm (UCA)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollationStrength {
    Primary,
    Secondary,
    Tertiary,
    Quaternary,
    Identical,
}

pub struct Collator {
    pub strength: CollationStrength,
    pub case_first: CaseFirst,
    pub numeric: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseFirst {
    Off,
    Upper,
    Lower,
}

impl Collator {
    pub fn new(strength: CollationStrength) -> Self {
        Self {
            strength,
            case_first: CaseFirst::Off,
            numeric: false,
        }
    }

    pub fn compare(&self, a: &str, b: &str) -> core::cmp::Ordering {
        if self.numeric {
            return self.compare_numeric(a, b);
        }
        let a_keys = self.sort_keys(a);
        let b_keys = self.sort_keys(b);
        a_keys.cmp(&b_keys)
    }

    fn sort_keys(&self, s: &str) -> Vec<u32> {
        let mut keys = Vec::new();
        for ch in s.chars() {
            let base = self.primary_weight(ch);
            let secondary = if self.strength as u8 >= CollationStrength::Secondary as u8 {
                self.secondary_weight(ch)
            } else {
                0
            };
            let tertiary = if self.strength as u8 >= CollationStrength::Tertiary as u8 {
                self.tertiary_weight(ch)
            } else {
                0
            };
            keys.push((base << 16) | ((secondary as u32) << 8) | tertiary as u32);
        }
        keys
    }

    fn primary_weight(&self, ch: char) -> u32 {
        let cp = ch as u32;
        match cp {
            0x0041..=0x005A => cp - 0x0041 + 1,   // A-Z
            0x0061..=0x007A => cp - 0x0061 + 1,   // a-z (same primary as A-Z)
            0x0030..=0x0039 => cp - 0x0030 + 100, // digits
            _ => cp + 1000,
        }
    }

    fn secondary_weight(&self, ch: char) -> u8 {
        let cp = ch as u32;
        match cp {
            0x00E0..=0x00E5 | 0x00C0..=0x00C5 => 2,
            0x00E8..=0x00EB | 0x00C8..=0x00CB => 2,
            0x00F2..=0x00F6 | 0x00D2..=0x00D6 => 2,
            _ => 0,
        }
    }

    fn tertiary_weight(&self, ch: char) -> u8 {
        if ch.is_uppercase() {
            match self.case_first {
                CaseFirst::Upper => 1,
                CaseFirst::Lower => 2,
                CaseFirst::Off => 2,
            }
        } else {
            match self.case_first {
                CaseFirst::Upper => 2,
                CaseFirst::Lower => 1,
                CaseFirst::Off => 1,
            }
        }
    }

    fn compare_numeric(&self, a: &str, b: &str) -> core::cmp::Ordering {
        let mut ai = a.chars().peekable();
        let mut bi = b.chars().peekable();
        loop {
            match (ai.peek(), bi.peek()) {
                (None, None) => return core::cmp::Ordering::Equal,
                (None, Some(_)) => return core::cmp::Ordering::Less,
                (Some(_), None) => return core::cmp::Ordering::Greater,
                (Some(&ac), Some(&bc)) => {
                    if ac.is_ascii_digit() && bc.is_ascii_digit() {
                        let an = Self::consume_number(&mut ai);
                        let bn = Self::consume_number(&mut bi);
                        match an.cmp(&bn) {
                            core::cmp::Ordering::Equal => continue,
                            other => return other,
                        }
                    }
                    let aw = self.primary_weight(ac);
                    let bw = self.primary_weight(bc);
                    match aw.cmp(&bw) {
                        core::cmp::Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }

    fn consume_number(iter: &mut core::iter::Peekable<core::str::Chars>) -> u64 {
        let mut n = 0u64;
        while let Some(&ch) = iter.peek() {
            if ch.is_ascii_digit() {
                n = n
                    .saturating_mul(10)
                    .saturating_add((ch as u64) - ('0' as u64));
                iter.next();
            } else {
                break;
            }
        }
        n
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Case mapping
// ═══════════════════════════════════════════════════════════════════════════

pub fn to_uppercase(s: &str) -> String {
    s.chars()
        .map(|c| {
            match c as u32 {
                0x0061..=0x007A => char::from_u32(c as u32 - 0x20).unwrap_or(c),
                0x00E0..=0x00F6 => char::from_u32(c as u32 - 0x20).unwrap_or(c),
                0x00F8..=0x00FE => char::from_u32(c as u32 - 0x20).unwrap_or(c),
                0x00DF => {
                    return c;
                } // eszett: special
                _ => c,
            }
        })
        .collect()
}

pub fn to_lowercase(s: &str) -> String {
    s.chars()
        .map(|c| match c as u32 {
            0x0041..=0x005A => char::from_u32(c as u32 + 0x20).unwrap_or(c),
            0x00C0..=0x00D6 => char::from_u32(c as u32 + 0x20).unwrap_or(c),
            0x00D8..=0x00DE => char::from_u32(c as u32 + 0x20).unwrap_or(c),
            _ => c,
        })
        .collect()
}

pub fn to_titlecase(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if capitalize_next && c.is_alphabetic() {
            for uc in c.to_uppercase() {
                result.push(uc);
            }
            capitalize_next = false;
        } else {
            result.push(c);
            if c.is_whitespace() || c == '-' {
                capitalize_next = true;
            }
        }
    }
    result
}

pub fn case_fold(s: &str) -> String {
    to_lowercase(s)
}

// ═══════════════════════════════════════════════════════════════════════════
// Plural rules
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluralCategory {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct PluralOperands {
    pub n: f64,
    pub i: u64,
    pub v: u32,
    pub w: u32,
    pub f: u64,
    pub t: u64,
    pub e: u32,
}

impl PluralOperands {
    pub fn from_f64(value: f64) -> Self {
        let abs = if value < 0.0 { -value } else { value };
        let i = abs as u64;
        let frac_str = format_frac(abs);
        let v = frac_str.len() as u32;
        let f_val: u64 = frac_str.iter().fold(0u64, |acc, &d| acc * 10 + d as u64);
        let w = frac_str.iter().rev().skip_while(|&&d| d == 0).count() as u32;
        let t_val: u64 = {
            let trimmed: Vec<u8> = frac_str.iter().copied().collect();
            let mut end = trimmed.len();
            while end > 0 && trimmed[end - 1] == 0 {
                end -= 1;
            }
            trimmed[..end]
                .iter()
                .fold(0u64, |acc, &d| acc * 10 + d as u64)
        };
        Self {
            n: abs,
            i,
            v,
            w,
            f: f_val,
            t: t_val,
            e: 0,
        }
    }

    pub fn from_i64(value: i64) -> Self {
        Self {
            n: value.unsigned_abs() as f64,
            i: value.unsigned_abs(),
            v: 0,
            w: 0,
            f: 0,
            t: 0,
            e: 0,
        }
    }
}

fn format_frac(value: f64) -> Vec<u8> {
    let frac = value - (value as u64 as f64);
    if frac < 0.000001 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut f = frac;
    for _ in 0..6 {
        f *= 10.0;
        let d = f as u8;
        result.push(d);
        f -= d as f64;
        if f < 0.000001 {
            break;
        }
    }
    result
}

pub fn cardinal_plural(lang: &str, ops: PluralOperands) -> PluralCategory {
    match lang {
        "ar" => {
            if ops.n == 0.0 {
                PluralCategory::Zero
            } else if ops.n == 1.0 {
                PluralCategory::One
            } else if ops.n == 2.0 {
                PluralCategory::Two
            } else if ops.i % 100 >= 3 && ops.i % 100 <= 10 {
                PluralCategory::Few
            } else if ops.i % 100 >= 11 && ops.i % 100 <= 99 {
                PluralCategory::Many
            } else {
                PluralCategory::Other
            }
        }
        "en" | "de" | "nl" | "sv" | "da" | "nb" | "nn" | "it" | "es" | "pt" | "el" | "fi"
        | "he" | "hi" | "hu" | "tr" => {
            if ops.i == 1 && ops.v == 0 {
                PluralCategory::One
            } else {
                PluralCategory::Other
            }
        }
        "fr" => {
            if ops.i == 0 || ops.i == 1 {
                PluralCategory::One
            } else {
                PluralCategory::Other
            }
        }
        "ru" | "uk" => {
            let mod10 = ops.i % 10;
            let mod100 = ops.i % 100;
            if ops.v == 0 && mod10 == 1 && mod100 != 11 {
                PluralCategory::One
            } else if ops.v == 0 && (2..=4).contains(&mod10) && !(12..=14).contains(&mod100) {
                PluralCategory::Few
            } else if ops.v == 0
                && (mod10 == 0 || (5..=9).contains(&mod10) || (11..=14).contains(&mod100))
            {
                PluralCategory::Many
            } else {
                PluralCategory::Other
            }
        }
        "pl" => {
            let mod10 = ops.i % 10;
            let mod100 = ops.i % 100;
            if ops.i == 1 && ops.v == 0 {
                PluralCategory::One
            } else if ops.v == 0 && (2..=4).contains(&mod10) && !(12..=14).contains(&mod100) {
                PluralCategory::Few
            } else if ops.v == 0
                && (mod10 == 0
                    || mod10 == 1
                    || (5..=9).contains(&mod10)
                    || (12..=14).contains(&mod100))
            {
                PluralCategory::Many
            } else {
                PluralCategory::Other
            }
        }
        "ja" | "ko" | "zh" | "vi" | "th" | "ms" | "id" => PluralCategory::Other,
        _ => {
            if ops.i == 1 && ops.v == 0 {
                PluralCategory::One
            } else {
                PluralCategory::Other
            }
        }
    }
}

pub fn ordinal_plural(lang: &str, ops: PluralOperands) -> PluralCategory {
    match lang {
        "en" => {
            let mod10 = ops.i % 10;
            let mod100 = ops.i % 100;
            if mod10 == 1 && mod100 != 11 {
                PluralCategory::One
            } else if mod10 == 2 && mod100 != 12 {
                PluralCategory::Two
            } else if mod10 == 3 && mod100 != 13 {
                PluralCategory::Few
            } else {
                PluralCategory::Other
            }
        }
        _ => PluralCategory::Other,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// ICU MessageFormat
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum MessagePart {
    Literal(String),
    Argument {
        name: String,
        format: ArgumentFormat,
    },
}

#[derive(Debug, Clone)]
pub enum ArgumentFormat {
    None,
    Number,
    Date(DateStyle),
    Time(DateStyle),
    Select(Vec<(String, Vec<MessagePart>)>),
    Plural {
        offset: i64,
        branches: Vec<(PluralSelector, Vec<MessagePart>)>,
    },
    SelectOrdinal {
        branches: Vec<(PluralSelector, Vec<MessagePart>)>,
    },
}

#[derive(Debug, Clone)]
pub enum PluralSelector {
    Exact(i64),
    Category(PluralCategory),
}

pub fn parse_message(pattern: &str) -> Vec<MessagePart> {
    let mut parts = Vec::new();
    let mut chars = pattern.chars().peekable();
    let mut literal = String::new();
    let mut depth = 0u32;

    while let Some(&ch) = chars.peek() {
        if ch == '{' && depth == 0 {
            if !literal.is_empty() {
                parts.push(MessagePart::Literal(core::mem::take(&mut literal)));
            }
            chars.next();
            let arg = parse_argument(&mut chars);
            parts.push(arg);
        } else if ch == '\'' {
            chars.next();
            if let Some(&next) = chars.peek() {
                if next == '\'' {
                    literal.push('\'');
                    chars.next();
                } else {
                    while let Some(&c) = chars.peek() {
                        if c == '\'' {
                            chars.next();
                            break;
                        }
                        literal.push(c);
                        chars.next();
                    }
                }
            }
        } else {
            literal.push(ch);
            chars.next();
        }
    }

    if !literal.is_empty() {
        parts.push(MessagePart::Literal(literal));
    }
    parts
}

fn parse_argument(chars: &mut core::iter::Peekable<core::str::Chars>) -> MessagePart {
    let mut name = String::new();
    while let Some(&ch) = chars.peek() {
        if ch == ',' || ch == '}' {
            break;
        }
        name.push(ch);
        chars.next();
    }
    let name = name.trim().into();

    if chars.peek() == Some(&'}') {
        chars.next();
        return MessagePart::Argument {
            name,
            format: ArgumentFormat::None,
        };
    }
    if chars.peek() == Some(&',') {
        chars.next();
    }

    let mut fmt_type = String::new();
    while let Some(&ch) = chars.peek() {
        if ch == ',' || ch == '}' {
            break;
        }
        fmt_type.push(ch);
        chars.next();
    }
    let fmt_type = fmt_type.trim().to_owned();

    if chars.peek() == Some(&'}') {
        chars.next();
        let format = match fmt_type.as_str() {
            "number" => ArgumentFormat::Number,
            "date" => ArgumentFormat::Date(DateStyle::Medium),
            "time" => ArgumentFormat::Time(DateStyle::Medium),
            _ => ArgumentFormat::None,
        };
        return MessagePart::Argument { name, format };
    }
    if chars.peek() == Some(&',') {
        chars.next();
    }

    let format = match fmt_type.as_str() {
        "select" => {
            let branches = parse_select_branches(chars);
            ArgumentFormat::Select(branches)
        }
        "plural" => {
            let (offset, branches) = parse_plural_branches(chars);
            ArgumentFormat::Plural { offset, branches }
        }
        "selectordinal" => {
            let (_, branches) = parse_plural_branches(chars);
            ArgumentFormat::SelectOrdinal { branches }
        }
        _ => {
            skip_to_closing_brace(chars);
            ArgumentFormat::None
        }
    };
    MessagePart::Argument { name, format }
}

fn parse_select_branches(
    chars: &mut core::iter::Peekable<core::str::Chars>,
) -> Vec<(String, Vec<MessagePart>)> {
    let mut branches = Vec::new();
    skip_whitespace(chars);
    while chars.peek().is_some() && chars.peek() != Some(&'}') {
        let mut key = String::new();
        while let Some(&ch) = chars.peek() {
            if ch == '{' || ch.is_whitespace() {
                break;
            }
            key.push(ch);
            chars.next();
        }
        skip_whitespace(chars);
        if chars.peek() == Some(&'{') {
            chars.next();
            let mut body = String::new();
            let mut depth = 1u32;
            while let Some(&ch) = chars.peek() {
                if ch == '{' {
                    depth += 1;
                }
                if ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        chars.next();
                        break;
                    }
                }
                body.push(ch);
                chars.next();
            }
            let parts = parse_message(&body);
            branches.push((key, parts));
        }
        skip_whitespace(chars);
    }
    if chars.peek() == Some(&'}') {
        chars.next();
    }
    branches
}

fn parse_plural_branches(
    chars: &mut core::iter::Peekable<core::str::Chars>,
) -> (i64, Vec<(PluralSelector, Vec<MessagePart>)>) {
    skip_whitespace(chars);
    let mut offset = 0i64;
    let mut branches = Vec::new();

    let mut preview = String::new();
    let saved = chars.clone();
    for &ch in chars.by_ref().take(20).collect::<Vec<_>>().iter() {
        preview.push(ch);
    }
    *chars = saved;

    while chars.peek().is_some() && chars.peek() != Some(&'}') {
        let mut key = String::new();
        while let Some(&ch) = chars.peek() {
            if ch == '{' || ch.is_whitespace() {
                break;
            }
            key.push(ch);
            chars.next();
        }
        skip_whitespace(chars);
        if chars.peek() == Some(&'{') {
            chars.next();
            let mut body = String::new();
            let mut depth = 1u32;
            while let Some(&ch) = chars.peek() {
                if ch == '{' {
                    depth += 1;
                }
                if ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        chars.next();
                        break;
                    }
                }
                body.push(ch);
                chars.next();
            }
            let parts = parse_message(&body);
            let selector = match key.as_str() {
                "zero" => PluralSelector::Category(PluralCategory::Zero),
                "one" => PluralSelector::Category(PluralCategory::One),
                "two" => PluralSelector::Category(PluralCategory::Two),
                "few" => PluralSelector::Category(PluralCategory::Few),
                "many" => PluralSelector::Category(PluralCategory::Many),
                "other" => PluralSelector::Category(PluralCategory::Other),
                s if s.starts_with('=') => {
                    let n: i64 = s[1..].parse().unwrap_or(0);
                    PluralSelector::Exact(n)
                }
                _ => PluralSelector::Category(PluralCategory::Other),
            };
            branches.push((selector, parts));
        }
        skip_whitespace(chars);
    }
    if chars.peek() == Some(&'}') {
        chars.next();
    }
    (offset, branches)
}

fn skip_whitespace(chars: &mut core::iter::Peekable<core::str::Chars>) {
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
}

fn skip_to_closing_brace(chars: &mut core::iter::Peekable<core::str::Chars>) {
    let mut depth = 1u32;
    while let Some(&ch) = chars.peek() {
        chars.next();
        if ch == '{' {
            depth += 1;
        }
        if ch == '}' {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
    }
}

// use alloc::string::ToString; (imported via alloc prelude in no_std)
fn to_owned(s: &str) -> String {
    String::from(s)
}

// ═══════════════════════════════════════════════════════════════════════════
// Resource bundles
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct ResourceBundle {
    pub locale: String,
    pub entries: BTreeMap<String, String>,
    pub parent: Option<Box<ResourceBundle>>,
}

impl ResourceBundle {
    pub fn new(locale: &str) -> Self {
        Self {
            locale: String::from(locale),
            entries: BTreeMap::new(),
            parent: None,
        }
    }

    pub fn set_parent(&mut self, parent: ResourceBundle) {
        self.parent = Some(Box::new(parent));
    }

    pub fn insert(&mut self, key: &str, value: &str) {
        self.entries.insert(String::from(key), String::from(value));
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        if let Some(v) = self.entries.get(key) {
            Some(v.as_str())
        } else if let Some(ref parent) = self.parent {
            parent.get(key)
        } else {
            None
        }
    }

    pub fn parse_properties(locale: &str, data: &str) -> Self {
        let mut bundle = ResourceBundle::new(locale);
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let value = line[eq_pos + 1..].trim();
                bundle.insert(key, value);
            }
        }
        bundle
    }
}

pub struct BundleCache {
    bundles: BTreeMap<String, ResourceBundle>,
}

impl BundleCache {
    pub fn new() -> Self {
        Self {
            bundles: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, locale: &str, bundle: ResourceBundle) {
        self.bundles.insert(String::from(locale), bundle);
    }

    pub fn get(&self, locale: &str) -> Option<&ResourceBundle> {
        self.bundles.get(locale)
    }

    pub fn resolve(&self, locale: &Locale) -> Option<&ResourceBundle> {
        for tag in locale.inheritance_chain() {
            if let Some(b) = self.bundles.get(&tag) {
                return Some(b);
            }
        }
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// List formatting
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListType {
    Conjunction,
    Disjunction,
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListWidth {
    Long,
    Short,
    Narrow,
}

pub fn format_list(items: &[&str], list_type: ListType, width: ListWidth) -> String {
    if items.is_empty() {
        return String::new();
    }
    if items.len() == 1 {
        return String::from(items[0]);
    }

    let (two_sep, middle_sep, end_sep) = match (list_type, width) {
        (ListType::Conjunction, ListWidth::Long) => (" and ", ", ", ", and "),
        (ListType::Conjunction, ListWidth::Short) => (" & ", ", ", ", & "),
        (ListType::Conjunction, ListWidth::Narrow) => (", ", ", ", ", "),
        (ListType::Disjunction, ListWidth::Long) => (" or ", ", ", ", or "),
        (ListType::Disjunction, ListWidth::Short) => (" or ", ", ", ", or "),
        (ListType::Disjunction, ListWidth::Narrow) => (", ", ", ", ", "),
        (ListType::Unit, ListWidth::Long) => (", ", ", ", ", "),
        (ListType::Unit, ListWidth::Short) => (", ", ", ", ", "),
        (ListType::Unit, ListWidth::Narrow) => (" ", " ", " "),
    };

    if items.len() == 2 {
        let mut s = String::from(items[0]);
        s.push_str(two_sep);
        s.push_str(items[1]);
        return s;
    }

    let mut s = String::from(items[0]);
    for (i, item) in items[1..].iter().enumerate() {
        if i < items.len() - 2 {
            s.push_str(middle_sep);
        } else {
            s.push_str(end_sep);
        }
        s.push_str(item);
    }
    s
}

// ═══════════════════════════════════════════════════════════════════════════
// Unit formatting
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementUnit {
    Meter,
    Kilometer,
    Centimeter,
    Millimeter,
    Mile,
    Yard,
    Foot,
    Inch,
    Kilogram,
    Gram,
    Milligram,
    Pound,
    Ounce,
    Stone,
    Liter,
    Milliliter,
    Gallon,
    Quart,
    Pint,
    FluidOunce,
    Cup,
    Celsius,
    Fahrenheit,
    Kelvin,
    MetersPerSecond,
    KilometersPerHour,
    MilesPerHour,
    Knot,
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
    SquareMeter,
    SquareKilometer,
    SquareMile,
    SquareFoot,
    Hectare,
    Acre,
    Byte,
    Kilobyte,
    Megabyte,
    Gigabyte,
    Terabyte,
    Petabyte,
    Joule,
    Kilojoule,
    Calorie,
    Kilocalorie,
    Watt,
    Kilowatt,
    Hertz,
    Kilohertz,
    Megahertz,
    Gigahertz,
    Pascal,
    Hectopascal,
    Millibar,
    Bar,
    Atmosphere,
    Psi,
}

impl MeasurementUnit {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Meter => "m",
            Self::Kilometer => "km",
            Self::Centimeter => "cm",
            Self::Millimeter => "mm",
            Self::Mile => "mi",
            Self::Yard => "yd",
            Self::Foot => "ft",
            Self::Inch => "in",
            Self::Kilogram => "kg",
            Self::Gram => "g",
            Self::Milligram => "mg",
            Self::Pound => "lb",
            Self::Ounce => "oz",
            Self::Stone => "st",
            Self::Liter => "L",
            Self::Milliliter => "mL",
            Self::Gallon => "gal",
            Self::Quart => "qt",
            Self::Pint => "pt",
            Self::FluidOunce => "fl oz",
            Self::Cup => "cup",
            Self::Celsius => "\u{00B0}C",
            Self::Fahrenheit => "\u{00B0}F",
            Self::Kelvin => "K",
            Self::MetersPerSecond => "m/s",
            Self::KilometersPerHour => "km/h",
            Self::MilesPerHour => "mph",
            Self::Knot => "kn",
            Self::Second => "s",
            Self::Millisecond => "ms",
            Self::Microsecond => "\u{00B5}s",
            Self::Nanosecond => "ns",
            Self::Minute => "min",
            Self::Hour => "h",
            Self::Day => "d",
            Self::Week => "wk",
            Self::Month => "mo",
            Self::Year => "yr",
            Self::SquareMeter => "m\u{00B2}",
            Self::SquareKilometer => "km\u{00B2}",
            Self::SquareMile => "mi\u{00B2}",
            Self::SquareFoot => "ft\u{00B2}",
            Self::Hectare => "ha",
            Self::Acre => "ac",
            Self::Byte => "B",
            Self::Kilobyte => "kB",
            Self::Megabyte => "MB",
            Self::Gigabyte => "GB",
            Self::Terabyte => "TB",
            Self::Petabyte => "PB",
            Self::Joule => "J",
            Self::Kilojoule => "kJ",
            Self::Calorie => "cal",
            Self::Kilocalorie => "kcal",
            Self::Watt => "W",
            Self::Kilowatt => "kW",
            Self::Hertz => "Hz",
            Self::Kilohertz => "kHz",
            Self::Megahertz => "MHz",
            Self::Gigahertz => "GHz",
            Self::Pascal => "Pa",
            Self::Hectopascal => "hPa",
            Self::Millibar => "mbar",
            Self::Bar => "bar",
            Self::Atmosphere => "atm",
            Self::Psi => "psi",
        }
    }

    pub fn convert(self, value: f64, to: MeasurementUnit) -> Option<f64> {
        let in_si = self.to_si(value)?;
        to.from_si(in_si)
    }

    fn to_si(self, value: f64) -> Option<f64> {
        Some(match self {
            Self::Meter => value,
            Self::Kilometer => value * 1000.0,
            Self::Centimeter => value * 0.01,
            Self::Millimeter => value * 0.001,
            Self::Mile => value * 1609.344,
            Self::Yard => value * 0.9144,
            Self::Foot => value * 0.3048,
            Self::Inch => value * 0.0254,
            Self::Kilogram => value,
            Self::Gram => value * 0.001,
            Self::Milligram => value * 0.000001,
            Self::Pound => value * 0.453592,
            Self::Ounce => value * 0.0283495,
            Self::Stone => value * 6.35029,
            Self::Liter => value,
            Self::Milliliter => value * 0.001,
            Self::Gallon => value * 3.78541,
            Self::Quart => value * 0.946353,
            Self::Pint => value * 0.473176,
            Self::FluidOunce => value * 0.0295735,
            Self::Cup => value * 0.236588,
            Self::Celsius => value + 273.15,
            Self::Fahrenheit => (value - 32.0) / 1.8 + 273.15,
            Self::Kelvin => value,
            _ => return None,
        })
    }

    fn from_si(self, value: f64) -> Option<f64> {
        Some(match self {
            Self::Meter => value,
            Self::Kilometer => value / 1000.0,
            Self::Centimeter => value / 0.01,
            Self::Millimeter => value / 0.001,
            Self::Mile => value / 1609.344,
            Self::Yard => value / 0.9144,
            Self::Foot => value / 0.3048,
            Self::Inch => value / 0.0254,
            Self::Kilogram => value,
            Self::Gram => value / 0.001,
            Self::Milligram => value / 0.000001,
            Self::Pound => value / 0.453592,
            Self::Ounce => value / 0.0283495,
            Self::Stone => value / 6.35029,
            Self::Liter => value,
            Self::Milliliter => value / 0.001,
            Self::Gallon => value / 3.78541,
            Self::Quart => value / 0.946353,
            Self::Pint => value / 0.473176,
            Self::FluidOunce => value / 0.0295735,
            Self::Cup => value / 0.236588,
            Self::Celsius => value - 273.15,
            Self::Fahrenheit => (value - 273.15) * 1.8 + 32.0,
            Self::Kelvin => value,
            _ => return None,
        })
    }
}

pub fn format_unit(value: f64, unit: MeasurementUnit, formatter: &NumberFormatter) -> String {
    let mut s = formatter.format_f64(value);
    s.push(' ');
    s.push_str(unit.symbol());
    s
}

// ═══════════════════════════════════════════════════════════════════════════
// Display names
// ═══════════════════════════════════════════════════════════════════════════

pub fn language_display_name(code: &str) -> &'static str {
    match code {
        "en" => "English",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "pt" => "Portuguese",
        "it" => "Italian",
        "nl" => "Dutch",
        "ru" => "Russian",
        "zh" => "Chinese",
        "ja" => "Japanese",
        "ko" => "Korean",
        "ar" => "Arabic",
        "hi" => "Hindi",
        "bn" => "Bengali",
        "pa" => "Punjabi",
        "te" => "Telugu",
        "mr" => "Marathi",
        "ta" => "Tamil",
        "ur" => "Urdu",
        "gu" => "Gujarati",
        "kn" => "Kannada",
        "ml" => "Malayalam",
        "th" => "Thai",
        "vi" => "Vietnamese",
        "pl" => "Polish",
        "uk" => "Ukrainian",
        "ro" => "Romanian",
        "cs" => "Czech",
        "hu" => "Hungarian",
        "sv" => "Swedish",
        "da" => "Danish",
        "fi" => "Finnish",
        "nb" => "Norwegian Bokm\u{00E5}l",
        "nn" => "Norwegian Nynorsk",
        "el" => "Greek",
        "he" => "Hebrew",
        "tr" => "Turkish",
        "id" => "Indonesian",
        "ms" => "Malay",
        "tl" => "Tagalog",
        "sw" => "Swahili",
        _ => "Unknown",
    }
}

pub fn region_display_name(code: &str) -> &'static str {
    match code {
        "US" => "United States",
        "GB" => "United Kingdom",
        "CA" => "Canada",
        "AU" => "Australia",
        "NZ" => "New Zealand",
        "IE" => "Ireland",
        "FR" => "France",
        "DE" => "Germany",
        "ES" => "Spain",
        "IT" => "Italy",
        "PT" => "Portugal",
        "NL" => "Netherlands",
        "BE" => "Belgium",
        "CH" => "Switzerland",
        "AT" => "Austria",
        "SE" => "Sweden",
        "DK" => "Denmark",
        "NO" => "Norway",
        "FI" => "Finland",
        "RU" => "Russia",
        "UA" => "Ukraine",
        "PL" => "Poland",
        "CZ" => "Czech Republic",
        "SK" => "Slovakia",
        "HU" => "Hungary",
        "RO" => "Romania",
        "BG" => "Bulgaria",
        "HR" => "Croatia",
        "CN" => "China",
        "JP" => "Japan",
        "KR" => "South Korea",
        "IN" => "India",
        "TH" => "Thailand",
        "VN" => "Vietnam",
        "ID" => "Indonesia",
        "MY" => "Malaysia",
        "PH" => "Philippines",
        "SA" => "Saudi Arabia",
        "AE" => "United Arab Emirates",
        "IL" => "Israel",
        "TR" => "Turkey",
        "EG" => "Egypt",
        "BR" => "Brazil",
        "MX" => "Mexico",
        "AR" => "Argentina",
        "CO" => "Colombia",
        "CL" => "Chile",
        "PE" => "Peru",
        "ZA" => "South Africa",
        "NG" => "Nigeria",
        "KE" => "Kenya",
        _ => "Unknown",
    }
}

pub fn script_display_name(code: &str) -> &'static str {
    match code {
        "Latn" => "Latin",
        "Cyrl" => "Cyrillic",
        "Grek" => "Greek",
        "Arab" => "Arabic",
        "Hebr" => "Hebrew",
        "Deva" => "Devanagari",
        "Beng" => "Bengali",
        "Guru" => "Gurmukhi",
        "Gujr" => "Gujarati",
        "Taml" => "Tamil",
        "Telu" => "Telugu",
        "Knda" => "Kannada",
        "Mlym" => "Malayalam",
        "Thai" => "Thai",
        "Khmr" => "Khmer",
        "Hans" => "Simplified Chinese",
        "Hant" => "Traditional Chinese",
        "Jpan" => "Japanese",
        "Kore" => "Korean",
        "Geor" => "Georgian",
        "Armn" => "Armenian",
        "Tibt" => "Tibetan",
        "Mymr" => "Myanmar",
        _ => "Unknown",
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Text direction
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDirection {
    Ltr,
    Rtl,
    Ttb,
}

pub fn layout_direction(locale: &Locale) -> LayoutDirection {
    match locale.language.as_str() {
        "ar" | "he" | "fa" | "ur" | "ps" | "sd" | "yi" | "dv" | "ku" | "ug" | "ckb" => {
            LayoutDirection::Rtl
        }
        _ => {
            if let Some(ref script) = locale.script {
                match script.as_str() {
                    "Arab" | "Hebr" | "Thaa" | "Syrc" | "Mand" | "Samr" | "Nkoo" => {
                        LayoutDirection::Rtl
                    }
                    _ => LayoutDirection::Ltr,
                }
            } else {
                LayoutDirection::Ltr
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Currency — ISO 4217
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CurrencyInfo {
    pub code: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub narrow_symbol: &'static str,
    pub decimal_digits: u8,
    pub rounding: u8,
}

pub fn currency_info(code: &str) -> Option<CurrencyInfo> {
    Some(match code {
        "USD" => CurrencyInfo {
            code: "USD",
            name: "US Dollar",
            symbol: "$",
            narrow_symbol: "$",
            decimal_digits: 2,
            rounding: 0,
        },
        "EUR" => CurrencyInfo {
            code: "EUR",
            name: "Euro",
            symbol: "\u{20AC}",
            narrow_symbol: "\u{20AC}",
            decimal_digits: 2,
            rounding: 0,
        },
        "GBP" => CurrencyInfo {
            code: "GBP",
            name: "British Pound",
            symbol: "\u{00A3}",
            narrow_symbol: "\u{00A3}",
            decimal_digits: 2,
            rounding: 0,
        },
        "JPY" => CurrencyInfo {
            code: "JPY",
            name: "Japanese Yen",
            symbol: "\u{00A5}",
            narrow_symbol: "\u{00A5}",
            decimal_digits: 0,
            rounding: 0,
        },
        "CNY" => CurrencyInfo {
            code: "CNY",
            name: "Chinese Yuan",
            symbol: "CN\u{00A5}",
            narrow_symbol: "\u{00A5}",
            decimal_digits: 2,
            rounding: 0,
        },
        "KRW" => CurrencyInfo {
            code: "KRW",
            name: "South Korean Won",
            symbol: "\u{20A9}",
            narrow_symbol: "\u{20A9}",
            decimal_digits: 0,
            rounding: 0,
        },
        "INR" => CurrencyInfo {
            code: "INR",
            name: "Indian Rupee",
            symbol: "\u{20B9}",
            narrow_symbol: "\u{20B9}",
            decimal_digits: 2,
            rounding: 0,
        },
        "RUB" => CurrencyInfo {
            code: "RUB",
            name: "Russian Ruble",
            symbol: "\u{20BD}",
            narrow_symbol: "\u{20BD}",
            decimal_digits: 2,
            rounding: 0,
        },
        "BRL" => CurrencyInfo {
            code: "BRL",
            name: "Brazilian Real",
            symbol: "R$",
            narrow_symbol: "R$",
            decimal_digits: 2,
            rounding: 0,
        },
        "CAD" => CurrencyInfo {
            code: "CAD",
            name: "Canadian Dollar",
            symbol: "CA$",
            narrow_symbol: "$",
            decimal_digits: 2,
            rounding: 0,
        },
        "AUD" => CurrencyInfo {
            code: "AUD",
            name: "Australian Dollar",
            symbol: "A$",
            narrow_symbol: "$",
            decimal_digits: 2,
            rounding: 0,
        },
        "CHF" => CurrencyInfo {
            code: "CHF",
            name: "Swiss Franc",
            symbol: "CHF",
            narrow_symbol: "CHF",
            decimal_digits: 2,
            rounding: 5,
        },
        "TRY" => CurrencyInfo {
            code: "TRY",
            name: "Turkish Lira",
            symbol: "\u{20BA}",
            narrow_symbol: "\u{20BA}",
            decimal_digits: 2,
            rounding: 0,
        },
        "MXN" => CurrencyInfo {
            code: "MXN",
            name: "Mexican Peso",
            symbol: "MX$",
            narrow_symbol: "$",
            decimal_digits: 2,
            rounding: 0,
        },
        "SEK" => CurrencyInfo {
            code: "SEK",
            name: "Swedish Krona",
            symbol: "kr",
            narrow_symbol: "kr",
            decimal_digits: 2,
            rounding: 0,
        },
        "NOK" => CurrencyInfo {
            code: "NOK",
            name: "Norwegian Krone",
            symbol: "kr",
            narrow_symbol: "kr",
            decimal_digits: 2,
            rounding: 0,
        },
        "DKK" => CurrencyInfo {
            code: "DKK",
            name: "Danish Krone",
            symbol: "kr",
            narrow_symbol: "kr",
            decimal_digits: 2,
            rounding: 0,
        },
        "PLN" => CurrencyInfo {
            code: "PLN",
            name: "Polish Zloty",
            symbol: "z\u{0142}",
            narrow_symbol: "z\u{0142}",
            decimal_digits: 2,
            rounding: 0,
        },
        "TWD" => CurrencyInfo {
            code: "TWD",
            name: "New Taiwan Dollar",
            symbol: "NT$",
            narrow_symbol: "$",
            decimal_digits: 2,
            rounding: 0,
        },
        "THB" => CurrencyInfo {
            code: "THB",
            name: "Thai Baht",
            symbol: "\u{0E3F}",
            narrow_symbol: "\u{0E3F}",
            decimal_digits: 2,
            rounding: 0,
        },
        "SAR" => CurrencyInfo {
            code: "SAR",
            name: "Saudi Riyal",
            symbol: "SAR",
            narrow_symbol: "SAR",
            decimal_digits: 2,
            rounding: 0,
        },
        "AED" => CurrencyInfo {
            code: "AED",
            name: "UAE Dirham",
            symbol: "AED",
            narrow_symbol: "AED",
            decimal_digits: 2,
            rounding: 0,
        },
        "ZAR" => CurrencyInfo {
            code: "ZAR",
            name: "South African Rand",
            symbol: "R",
            narrow_symbol: "R",
            decimal_digits: 2,
            rounding: 0,
        },
        _ => return None,
    })
}

pub fn currency_for_region(region: &str) -> &'static str {
    match region {
        "US" => "USD",
        "GB" => "GBP",
        "JP" => "JPY",
        "CN" => "CNY",
        "KR" => "KRW",
        "IN" => "INR",
        "RU" => "RUB",
        "BR" => "BRL",
        "CA" => "CAD",
        "AU" => "AUD",
        "CH" => "CHF",
        "TR" => "TRY",
        "MX" => "MXN",
        "SE" => "SEK",
        "NO" => "NOK",
        "DK" => "DKK",
        "PL" => "PLN",
        "TW" => "TWD",
        "TH" => "THB",
        "SA" => "SAR",
        "AE" => "AED",
        "ZA" => "ZAR",
        "FR" | "DE" | "ES" | "IT" | "PT" | "NL" | "BE" | "AT" | "IE" | "FI" | "GR" | "EE"
        | "LV" | "LT" | "SK" | "SI" | "MT" | "CY" | "LU" => "EUR",
        _ => "USD",
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Measurement systems
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementSystem {
    Metric,
    US,
    UK,
}

pub fn measurement_system(region: &str) -> MeasurementSystem {
    match region {
        "US" | "PR" | "GU" | "VI" | "AS" | "MH" | "FM" | "PW" => MeasurementSystem::US,
        "GB" | "MM" | "LR" => MeasurementSystem::UK,
        _ => MeasurementSystem::Metric,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Gettext compatibility — .po/.mo parser
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct GettextEntry {
    pub msgid: String,
    pub msgstr: String,
    pub msgctxt: Option<String>,
    pub plural_id: Option<String>,
    pub plural_strs: Vec<String>,
    pub fuzzy: bool,
}

#[derive(Debug, Clone)]
pub struct GettextCatalog {
    pub entries: Vec<GettextEntry>,
    pub nplurals: u32,
    pub plural_expr: String,
    pub domain: String,
}

impl GettextCatalog {
    pub fn new(domain: &str) -> Self {
        Self {
            entries: Vec::new(),
            nplurals: 2,
            plural_expr: String::from("n != 1"),
            domain: String::from(domain),
        }
    }

    pub fn parse_po(domain: &str, data: &str) -> Self {
        let mut catalog = GettextCatalog::new(domain);
        let mut current_msgid = String::new();
        let mut current_msgstr = String::new();
        let mut current_msgctxt: Option<String> = None;
        let mut current_plural_id: Option<String> = None;
        let mut current_plural_strs: Vec<String> = Vec::new();
        let mut current_fuzzy = false;
        let mut reading = PoField::None;

        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                if !current_msgid.is_empty() || !current_msgstr.is_empty() {
                    catalog.entries.push(GettextEntry {
                        msgid: core::mem::take(&mut current_msgid),
                        msgstr: core::mem::take(&mut current_msgstr),
                        msgctxt: current_msgctxt.take(),
                        plural_id: current_plural_id.take(),
                        plural_strs: core::mem::take(&mut current_plural_strs),
                        fuzzy: current_fuzzy,
                    });
                    current_fuzzy = false;
                }
                reading = PoField::None;
                continue;
            }
            if line.starts_with("#,") && line.contains("fuzzy") {
                current_fuzzy = true;
                continue;
            }
            if line.starts_with('#') {
                continue;
            }

            if let Some(rest) = line.strip_prefix("msgctxt ") {
                current_msgctxt = Some(unquote_po(rest));
                reading = PoField::Msgctxt;
            } else if let Some(rest) = line.strip_prefix("msgid_plural ") {
                current_plural_id = Some(unquote_po(rest));
                reading = PoField::MsgidPlural;
            } else if let Some(rest) = line.strip_prefix("msgid ") {
                current_msgid = unquote_po(rest);
                reading = PoField::Msgid;
            } else if let Some(rest) = line.strip_prefix("msgstr ") {
                current_msgstr = unquote_po(rest);
                reading = PoField::Msgstr;
            } else if line.starts_with("msgstr[") {
                if let Some(bracket_end) = line.find(']') {
                    let rest = &line[bracket_end + 1..].trim();
                    current_plural_strs.push(unquote_po(rest));
                    reading = PoField::MsgstrPlural;
                }
            } else if line.starts_with('"') {
                let continued = unquote_po(line);
                match reading {
                    PoField::Msgid => current_msgid.push_str(&continued),
                    PoField::Msgstr => current_msgstr.push_str(&continued),
                    PoField::Msgctxt => {
                        if let Some(ref mut ctx) = current_msgctxt {
                            ctx.push_str(&continued);
                        }
                    }
                    PoField::MsgidPlural => {
                        if let Some(ref mut pid) = current_plural_id {
                            pid.push_str(&continued);
                        }
                    }
                    PoField::MsgstrPlural => {
                        if let Some(last) = current_plural_strs.last_mut() {
                            last.push_str(&continued);
                        }
                    }
                    PoField::None => {}
                }
            }
        }
        if !current_msgid.is_empty() || !current_msgstr.is_empty() {
            catalog.entries.push(GettextEntry {
                msgid: current_msgid,
                msgstr: current_msgstr,
                msgctxt: current_msgctxt,
                plural_id: current_plural_id,
                plural_strs: current_plural_strs,
                fuzzy: current_fuzzy,
            });
        }

        if let Some(header) = catalog.entries.iter().find(|e| e.msgid.is_empty()) {
            for hline in header.msgstr.split("\\n") {
                if let Some(rest) = hline.strip_prefix("Plural-Forms:") {
                    let rest = rest.trim();
                    if let Some(np_start) = rest.find("nplurals=") {
                        let np_str = &rest[np_start + 9..];
                        if let Some(semi) = np_str.find(';') {
                            catalog.nplurals = np_str[..semi].trim().parse().unwrap_or(2);
                        }
                    }
                    if let Some(pl_start) = rest.find("plural=") {
                        let pl_str = &rest[pl_start + 7..];
                        let end = pl_str.find(';').unwrap_or(pl_str.len());
                        catalog.plural_expr = String::from(pl_str[..end].trim());
                    }
                }
            }
        }

        catalog
    }

    pub fn gettext<'a>(&'a self, msgid: &'a str) -> &'a str {
        for entry in &self.entries {
            if entry.msgid == msgid && !entry.fuzzy {
                if entry.msgstr.is_empty() {
                    return msgid;
                }
                return &entry.msgstr;
            }
        }
        msgid
    }

    pub fn pgettext<'a>(&'a self, context: &str, msgid: &'a str) -> &'a str {
        for entry in &self.entries {
            if entry.msgid == msgid && !entry.fuzzy {
                if let Some(ref ctx) = entry.msgctxt {
                    if ctx == context {
                        if entry.msgstr.is_empty() {
                            return msgid;
                        }
                        return &entry.msgstr;
                    }
                }
            }
        }
        msgid
    }

    pub fn ngettext<'a>(&'a self, msgid: &'a str, msgid_plural: &'a str, n: u64) -> &'a str {
        for entry in &self.entries {
            if entry.msgid == msgid && !entry.fuzzy {
                if !entry.plural_strs.is_empty() {
                    let idx = self.evaluate_plural(n) as usize;
                    if let Some(s) = entry.plural_strs.get(idx) {
                        if !s.is_empty() {
                            return s;
                        }
                    }
                    if let Some(s) = entry.plural_strs.first() {
                        if !s.is_empty() {
                            return s;
                        }
                    }
                }
            }
        }
        if n == 1 {
            msgid
        } else {
            msgid_plural
        }
    }

    fn evaluate_plural(&self, n: u64) -> u32 {
        if self.plural_expr == "n != 1" {
            return if n != 1 { 1 } else { 0 };
        }
        if self.plural_expr == "0" {
            return 0;
        }
        if self.plural_expr.contains("n%10") {
            let mod10 = n % 10;
            let mod100 = n % 100;
            if mod10 == 1 && mod100 != 11 {
                return 0;
            }
            if mod10 >= 2 && mod10 <= 4 && (mod100 < 10 || mod100 >= 20) {
                return 1;
            }
            return 2;
        }
        if n != 1 {
            1
        } else {
            0
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PoField {
    None,
    Msgid,
    Msgstr,
    Msgctxt,
    MsgidPlural,
    MsgstrPlural,
}

fn unquote_po(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        let mut result = String::new();
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some('\\') => result.push('\\'),
                    Some('"') => result.push('"'),
                    Some(other) => {
                        result.push('\\');
                        result.push(other);
                    }
                    None => result.push('\\'),
                }
            } else {
                result.push(ch);
            }
        }
        result
    } else {
        String::from(s)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Global LOCALE_ENGINE
// ═══════════════════════════════════════════════════════════════════════════

static LOCALE_ENGINE_INIT: AtomicBool = AtomicBool::new(false);

pub struct LocaleEngine {
    pub current_locale: Locale,
    pub bundle_cache: BundleCache,
    pub number_formatter: NumberFormatter,
    pub date_formatter: DateTimeFormatter,
    pub collator: Collator,
    pub catalogs: BTreeMap<String, GettextCatalog>,
}

static mut LOCALE_ENGINE: Option<LocaleEngine> = None;

impl LocaleEngine {
    pub fn init() {
        if LOCALE_ENGINE_INIT
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let locale = Locale::new("en");
            let symbols = symbols_for_locale(&locale);
            unsafe {
                LOCALE_ENGINE = Some(LocaleEngine {
                    number_formatter: NumberFormatter::new(symbols),
                    date_formatter: DateTimeFormatter::new(locale.clone()),
                    collator: Collator::new(CollationStrength::Tertiary),
                    current_locale: locale,
                    bundle_cache: BundleCache::new(),
                    catalogs: BTreeMap::new(),
                });
            }
        }
    }

    pub fn get() -> Option<&'static mut LocaleEngine> {
        if LOCALE_ENGINE_INIT.load(Ordering::SeqCst) {
            unsafe { LOCALE_ENGINE.as_mut() }
        } else {
            None
        }
    }

    pub fn set_locale(&mut self, locale: Locale) {
        let symbols = symbols_for_locale(&locale);
        self.number_formatter = NumberFormatter::new(symbols);
        self.date_formatter = DateTimeFormatter::new(locale.clone());
        self.current_locale = locale;
    }

    pub fn load_catalog(&mut self, domain: &str, po_data: &str) {
        let catalog = GettextCatalog::parse_po(domain, po_data);
        self.catalogs.insert(String::from(domain), catalog);
    }

    pub fn gettext<'a>(&'a self, domain: &str, msgid: &'a str) -> &'a str {
        if let Some(catalog) = self.catalogs.get(domain) {
            catalog.gettext(msgid)
        } else {
            msgid
        }
    }
}

// ── Host KATs (dev box, `cargo test -p raelocale`) ──────────────────────
// MasterChecklist i18n/raelocale: locale-aware number/currency formatting +
// CLDR plural rules + BCP-47 parsing. Pure logic — these FAIL-ably pin the
// global-readiness primitives so a renumbered rule or broken grouping is
// caught on the dev box, not by a user in another language.
#[cfg(test)]
mod kat {
    use super::*;

    #[test]
    fn locale_parse_bcp47() {
        let en = Locale::parse("en-US").unwrap();
        assert_eq!(en.language, "en");
        assert_eq!(en.region.as_deref(), Some("US"));
        assert!(en.script.is_none());

        // language-Script-REGION (4-char script, 2-char region).
        let zh = Locale::parse("zh-Hant-TW").unwrap();
        assert_eq!(zh.language, "zh");
        assert_eq!(zh.script.as_deref(), Some("Hant"));
        assert_eq!(zh.region.as_deref(), Some("TW"));

        let fr = Locale::parse("fr").unwrap();
        assert_eq!(fr.language, "fr");
        assert!(fr.region.is_none());

        assert!(Locale::parse("").is_none());
    }

    #[test]
    fn number_grouping_en_and_de() {
        // en-US groups by 3 with ',': 1,234,567
        let en = NumberFormatter::new(NumberFormatSymbols::default());
        assert_eq!(en.format_i64(1_234_567), "1,234,567");
        assert_eq!(en.format_i64(-1000), "-1,000");
        assert_eq!(en.format_i64(0), "0");
        assert_eq!(en.format_i64(42), "42");

        // de-DE swaps the separators: 1.234.567
        let de = NumberFormatter::new(symbols_for_locale(&Locale::parse("de-DE").unwrap()));
        assert_eq!(de.format_i64(1_234_567), "1.234.567");
    }

    #[test]
    fn fraction_percent_currency() {
        // Default formatter emits 3 fraction digits.
        let en = NumberFormatter::new(NumberFormatSymbols::default());
        assert_eq!(en.format_f64(1234.5), "1,234.500");

        // Currency to 2 places: $1,234.50
        let mut money = NumberFormatter::new(NumberFormatSymbols::default());
        money.max_fraction_digits = 2;
        assert_eq!(money.format_currency(1234.5), "$1,234.50");

        // Percent with no fraction digits: 0.25 -> 25%
        let mut pct = NumberFormatter::new(NumberFormatSymbols::default());
        pct.max_fraction_digits = 0;
        assert_eq!(pct.format_percent(0.25), "25%");
    }

    #[test]
    fn cardinal_plurals_cldr() {
        use PluralCategory::*;
        let n = PluralOperands::from_i64;
        // English: only 1 is singular.
        assert_eq!(cardinal_plural("en", n(1)), One);
        assert_eq!(cardinal_plural("en", n(2)), Other);
        assert_eq!(cardinal_plural("en", n(0)), Other);
        // French: 0 and 1 are singular.
        assert_eq!(cardinal_plural("fr", n(0)), One);
        assert_eq!(cardinal_plural("fr", n(2)), Other);
        // Russian: 1->one, 2-4->few, 5-9 & 11-14->many.
        assert_eq!(cardinal_plural("ru", n(1)), One);
        assert_eq!(cardinal_plural("ru", n(2)), Few);
        assert_eq!(cardinal_plural("ru", n(5)), Many);
        assert_eq!(cardinal_plural("ru", n(11)), Many);
        // CJK: no plural distinction.
        assert_eq!(cardinal_plural("ja", n(1)), Other);
    }

    #[test]
    fn ordinal_plurals_en() {
        use PluralCategory::*;
        let n = PluralOperands::from_i64;
        assert_eq!(ordinal_plural("en", n(1)), One); // 1st
        assert_eq!(ordinal_plural("en", n(2)), Two); // 2nd
        assert_eq!(ordinal_plural("en", n(3)), Few); // 3rd
        assert_eq!(ordinal_plural("en", n(4)), Other); // 4th
        assert_eq!(ordinal_plural("en", n(11)), Other); // 11th, not "11st"
        assert_eq!(ordinal_plural("en", n(21)), One); // 21st
    }

    #[test]
    fn currency_metadata() {
        let usd = currency_info("USD").unwrap();
        assert_eq!(usd.code, "USD");
        assert_eq!(usd.symbol, "$");
        assert_eq!(usd.decimal_digits, 2);
        let eur = currency_info("EUR").unwrap();
        assert_eq!(eur.symbol, "\u{20AC}");
        assert_eq!(eur.decimal_digits, 2);
    }
}
