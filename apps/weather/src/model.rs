//! Weather data model for the AthenaOS Weather app (conditions, forecasts, units,
//! weather codes). Moved from the former `raeshell::weather_app`; the app renders
//! a subset, so unused model surface is expected.
#![allow(dead_code, unused_imports, static_mut_refs)]

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ── Weather codes ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherCode {
    Clear,
    PartlyCloudy,
    Overcast,
    Fog,
    DrizzleLight,
    DrizzleModerate,
    RainLight,
    RainModerate,
    RainHeavy,
    FreezingRain,
    Sleet,
    SnowLight,
    SnowModerate,
    SnowHeavy,
    Blizzard,
    Thunderstorm,
    Hail,
    Tornado,
    Hurricane,
    DustStorm,
    Smoke,
    Haze,
}

impl WeatherCode {
    pub fn description(&self) -> &'static str {
        match self {
            WeatherCode::Clear => "Clear sky",
            WeatherCode::PartlyCloudy => "Partly cloudy",
            WeatherCode::Overcast => "Overcast",
            WeatherCode::Fog => "Fog",
            WeatherCode::DrizzleLight => "Light drizzle",
            WeatherCode::DrizzleModerate => "Moderate drizzle",
            WeatherCode::RainLight => "Light rain",
            WeatherCode::RainModerate => "Moderate rain",
            WeatherCode::RainHeavy => "Heavy rain",
            WeatherCode::FreezingRain => "Freezing rain",
            WeatherCode::Sleet => "Sleet",
            WeatherCode::SnowLight => "Light snow",
            WeatherCode::SnowModerate => "Moderate snow",
            WeatherCode::SnowHeavy => "Heavy snow",
            WeatherCode::Blizzard => "Blizzard",
            WeatherCode::Thunderstorm => "Thunderstorm",
            WeatherCode::Hail => "Hail",
            WeatherCode::Tornado => "Tornado",
            WeatherCode::Hurricane => "Hurricane / Typhoon",
            WeatherCode::DustStorm => "Dust storm",
            WeatherCode::Smoke => "Smoke",
            WeatherCode::Haze => "Haze",
        }
    }

    pub fn icon(&self) -> char {
        match self {
            WeatherCode::Clear => '☀',
            WeatherCode::PartlyCloudy => '⛅',
            WeatherCode::Overcast => '☁',
            WeatherCode::Fog | WeatherCode::Haze | WeatherCode::Smoke => '🌫',
            WeatherCode::DrizzleLight | WeatherCode::DrizzleModerate => '🌦',
            WeatherCode::RainLight | WeatherCode::RainModerate | WeatherCode::RainHeavy => '🌧',
            WeatherCode::FreezingRain | WeatherCode::Sleet => '🌨',
            WeatherCode::SnowLight
            | WeatherCode::SnowModerate
            | WeatherCode::SnowHeavy
            | WeatherCode::Blizzard => '❄',
            WeatherCode::Thunderstorm => '⛈',
            WeatherCode::Hail => '🌨',
            WeatherCode::Tornado => '🌪',
            WeatherCode::Hurricane => '🌀',
            WeatherCode::DustStorm => '🌪',
        }
    }

    pub fn is_severe(&self) -> bool {
        matches!(
            self,
            WeatherCode::Thunderstorm
                | WeatherCode::Hail
                | WeatherCode::Tornado
                | WeatherCode::Hurricane
                | WeatherCode::Blizzard
                | WeatherCode::DustStorm
        )
    }
}

// ── Units ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempUnit {
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedUnit {
    Kmh,
    Mph,
    Ms,
    Knots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecipUnit {
    Mm,
    Inches,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureUnit {
    Hpa,
    InHg,
    MmHg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceUnit {
    Km,
    Mi,
}

pub fn convert_temp(c: f32, to: TempUnit) -> f32 {
    match to {
        TempUnit::Celsius => c,
        TempUnit::Fahrenheit => c * 9.0 / 5.0 + 32.0,
    }
}

pub fn convert_speed(kmh: f32, to: SpeedUnit) -> f32 {
    match to {
        SpeedUnit::Kmh => kmh,
        SpeedUnit::Mph => kmh * 0.621371,
        SpeedUnit::Ms => kmh / 3.6,
        SpeedUnit::Knots => kmh * 0.539957,
    }
}

pub fn convert_precip(mm: f32, to: PrecipUnit) -> f32 {
    match to {
        PrecipUnit::Mm => mm,
        PrecipUnit::Inches => mm * 0.0393701,
    }
}

pub fn convert_pressure(hpa: f32, to: PressureUnit) -> f32 {
    match to {
        PressureUnit::Hpa => hpa,
        PressureUnit::InHg => hpa * 0.02953,
        PressureUnit::MmHg => hpa * 0.75006,
    }
}

pub fn convert_distance(km: f32, to: DistanceUnit) -> f32 {
    match to {
        DistanceUnit::Km => km,
        DistanceUnit::Mi => km * 0.621371,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UnitPrefs {
    pub temp: TempUnit,
    pub speed: SpeedUnit,
    pub precip: PrecipUnit,
    pub pressure: PressureUnit,
    pub distance: DistanceUnit,
}

impl UnitPrefs {
    pub fn metric() -> Self {
        Self {
            temp: TempUnit::Celsius,
            speed: SpeedUnit::Kmh,
            precip: PrecipUnit::Mm,
            pressure: PressureUnit::Hpa,
            distance: DistanceUnit::Km,
        }
    }

    pub fn imperial() -> Self {
        Self {
            temp: TempUnit::Fahrenheit,
            speed: SpeedUnit::Mph,
            precip: PrecipUnit::Inches,
            pressure: PressureUnit::InHg,
            distance: DistanceUnit::Mi,
        }
    }
}

// ── Current conditions ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CurrentConditions {
    pub temperature: f32,
    pub feels_like: f32,
    pub humidity: u8,
    pub dew_point: f32,
    pub pressure: f32,
    pub wind_speed: f32,
    pub wind_direction: u16,
    pub wind_gust: f32,
    pub visibility: f32,
    pub uv_index: u8,
    pub cloud_cover: u8,
    pub precipitation: f32,
    pub weather_code: WeatherCode,
    pub description: String,
    pub timestamp: u64,
}

impl CurrentConditions {
    pub fn new() -> Self {
        Self {
            temperature: 0.0,
            feels_like: 0.0,
            humidity: 0,
            dew_point: 0.0,
            pressure: 1013.25,
            wind_speed: 0.0,
            wind_direction: 0,
            wind_gust: 0.0,
            visibility: 10.0,
            uv_index: 0,
            cloud_cover: 0,
            precipitation: 0.0,
            weather_code: WeatherCode::Clear,
            description: String::from("Clear sky"),
            timestamp: 0,
        }
    }

    pub fn wind_direction_str(&self) -> &'static str {
        match self.wind_direction {
            0..=22 => "N",
            23..=67 => "NE",
            68..=112 => "E",
            113..=157 => "SE",
            158..=202 => "S",
            203..=247 => "SW",
            248..=292 => "W",
            293..=337 => "NW",
            _ => "N",
        }
    }

    pub fn temp_display(&self, unit: TempUnit) -> f32 {
        convert_temp(self.temperature, unit)
    }

    pub fn feels_like_display(&self, unit: TempUnit) -> f32 {
        convert_temp(self.feels_like, unit)
    }
}

// ── Hourly forecast ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HourlyForecast {
    pub timestamp: u64,
    pub temperature: f32,
    pub feels_like: f32,
    pub humidity: u8,
    pub precipitation_probability: u8,
    pub precipitation: f32,
    pub wind_speed: f32,
    pub wind_direction: u16,
    pub weather_code: WeatherCode,
    pub uv_index: u8,
    pub cloud_cover: u8,
}

// ── Daily forecast ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DailyForecast {
    pub date_timestamp: u64,
    pub temp_high: f32,
    pub temp_low: f32,
    pub humidity_avg: u8,
    pub precipitation_probability: u8,
    pub precipitation_total: f32,
    pub wind_speed_max: f32,
    pub weather_code: WeatherCode,
    pub uv_index_max: u8,
    pub sunrise: u64,
    pub sunset: u64,
    pub moonrise: u64,
    pub moonset: u64,
    pub description: String,
}

// ── Minutely precipitation ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MinutelyPrecip {
    pub timestamp: u64,
    pub intensity: f32,
}

// ── Air quality ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct AirQuality {
    pub aqi: u16,
    pub pm2_5: f32,
    pub pm10: f32,
    pub o3: f32,
    pub no2: f32,
    pub so2: f32,
    pub co: f32,
}

impl AirQuality {
    pub fn category(&self) -> &'static str {
        match self.aqi {
            0..=50 => "Good",
            51..=100 => "Moderate",
            101..=150 => "Unhealthy for Sensitive Groups",
            151..=200 => "Unhealthy",
            201..=300 => "Very Unhealthy",
            _ => "Hazardous",
        }
    }

    pub fn color(&self) -> u32 {
        match self.aqi {
            0..=50 => 0xFF_00_E4_00,
            51..=100 => 0xFF_FF_FF_00,
            101..=150 => 0xFF_FF_7E_00,
            151..=200 => 0xFF_FF_00_00,
            201..=300 => 0xFF_99_00_4C,
            _ => 0xFF_7E_00_23,
        }
    }
}

// ── Pollen ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct PollenCount {
    pub tree: u8,
    pub grass: u8,
    pub weed: u8,
    pub mold: u8,
}

impl PollenCount {
    pub fn overall_level(&self) -> &'static str {
        let max = self.tree.max(self.grass).max(self.weed).max(self.mold);
        match max {
            0..=2 => "Low",
            3..=5 => "Moderate",
            6..=8 => "High",
            _ => "Very High",
        }
    }
}

// ── Severe weather alerts ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Advisory,
    Watch,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertType {
    Thunderstorm,
    Tornado,
    Hurricane,
    Flood,
    WinterStorm,
    Heat,
    Cold,
    Wind,
    Fog,
    FireWeather,
}

#[derive(Debug, Clone)]
pub struct WeatherAlert {
    pub alert_type: AlertType,
    pub severity: AlertSeverity,
    pub title: String,
    pub description: String,
    pub start: u64,
    pub end: u64,
    pub source: String,
}

impl WeatherAlert {
    pub fn is_active(&self, now: u64) -> bool {
        now >= self.start && now <= self.end
    }

    pub fn color(&self) -> u32 {
        match self.severity {
            AlertSeverity::Advisory => 0xFF_FF_FF_00,
            AlertSeverity::Watch => 0xFF_FF_A5_00,
            AlertSeverity::Warning => 0xFF_FF_00_00,
        }
    }
}

// ── Location ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Location {
    pub id: u64,
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub country: String,
    pub timezone: String,
    pub is_current: bool,
}

impl Location {
    pub fn new(id: u64, name: &str, lat: f64, lon: f64) -> Self {
        Self {
            id,
            name: String::from(name),
            latitude: lat,
            longitude: lon,
            country: String::new(),
            timezone: String::new(),
            is_current: false,
        }
    }

    pub fn from_coordinates(id: u64, lat: f64, lon: f64) -> Self {
        let mut loc = Self::new(id, "Current Location", lat, lon);
        loc.is_current = true;
        loc
    }
}

// ── Astronomy ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoonPhase {
    NewMoon,
    WaxingCrescent,
    FirstQuarter,
    WaxingGibbous,
    FullMoon,
    WaningGibbous,
    ThirdQuarter,
    WaningCrescent,
}

impl MoonPhase {
    pub fn name(&self) -> &'static str {
        match self {
            MoonPhase::NewMoon => "New Moon",
            MoonPhase::WaxingCrescent => "Waxing Crescent",
            MoonPhase::FirstQuarter => "First Quarter",
            MoonPhase::WaxingGibbous => "Waxing Gibbous",
            MoonPhase::FullMoon => "Full Moon",
            MoonPhase::WaningGibbous => "Waning Gibbous",
            MoonPhase::ThirdQuarter => "Third Quarter",
            MoonPhase::WaningCrescent => "Waning Crescent",
        }
    }

    pub fn from_illumination(pct: f32, waxing: bool) -> Self {
        if pct < 1.0 {
            return MoonPhase::NewMoon;
        }
        if pct > 99.0 {
            return MoonPhase::FullMoon;
        }
        if waxing {
            if pct < 25.0 {
                MoonPhase::WaxingCrescent
            } else if pct < 50.0 {
                MoonPhase::FirstQuarter
            } else {
                MoonPhase::WaxingGibbous
            }
        } else {
            if pct > 75.0 {
                MoonPhase::WaningGibbous
            } else if pct > 50.0 {
                MoonPhase::ThirdQuarter
            } else {
                MoonPhase::WaningCrescent
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct AstronomyData {
    pub sunrise: u64,
    pub sunset: u64,
    pub golden_hour_start: u64,
    pub golden_hour_end: u64,
    pub blue_hour_start: u64,
    pub blue_hour_end: u64,
    pub civil_twilight_start: u64,
    pub civil_twilight_end: u64,
    pub nautical_twilight_start: u64,
    pub nautical_twilight_end: u64,
    pub astronomical_twilight_start: u64,
    pub astronomical_twilight_end: u64,
    pub moon_phase: MoonPhase,
    pub moon_illumination: f32,
    pub moonrise: u64,
    pub moonset: u64,
}

impl AstronomyData {
    pub fn daylight_hours(&self) -> f32 {
        if self.sunset > self.sunrise {
            (self.sunset - self.sunrise) as f32 / 3600.0
        } else {
            0.0
        }
    }
}

// ── Historical / averages ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HistoricalWeather {
    pub yesterday_high: f32,
    pub yesterday_low: f32,
    pub yesterday_precip: f32,
    pub monthly_avg_high: f32,
    pub monthly_avg_low: f32,
    pub monthly_avg_precip: f32,
    pub record_high: f32,
    pub record_high_year: u16,
    pub record_low: f32,
    pub record_low_year: u16,
}

// ── Widget sizes ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetSize {
    Compact,
    Medium,
    Large,
    LiveTile,
}

// ── Dynamic background ──────────────────────────────────────────────────

pub fn background_color_for(code: WeatherCode, is_day: bool) -> u32 {
    match (code, is_day) {
        (WeatherCode::Clear, true) => 0xFF_47_A6_FF,
        (WeatherCode::Clear, false) => 0xFF_0C_1A_3A,
        (WeatherCode::PartlyCloudy, true) => 0xFF_6B_B0_E0,
        (WeatherCode::PartlyCloudy, false) => 0xFF_1A_2A_4A,
        (WeatherCode::Overcast, _) => 0xFF_60_6B_7A,
        (WeatherCode::Fog | WeatherCode::Haze | WeatherCode::Smoke, _) => 0xFF_8A_8A_8A,
        (
            WeatherCode::RainLight
            | WeatherCode::RainModerate
            | WeatherCode::DrizzleLight
            | WeatherCode::DrizzleModerate,
            true,
        ) => 0xFF_4A_5A_6A,
        (
            WeatherCode::RainLight
            | WeatherCode::RainModerate
            | WeatherCode::DrizzleLight
            | WeatherCode::DrizzleModerate,
            false,
        ) => 0xFF_1A_2A_3A,
        (WeatherCode::RainHeavy | WeatherCode::FreezingRain, _) => 0xFF_2A_3A_4A,
        (WeatherCode::Thunderstorm, _) => 0xFF_1A_1A_2A,
        (
            WeatherCode::SnowLight
            | WeatherCode::SnowModerate
            | WeatherCode::SnowHeavy
            | WeatherCode::Blizzard
            | WeatherCode::Sleet,
            true,
        ) => 0xFF_C0_D0_E0,
        (
            WeatherCode::SnowLight
            | WeatherCode::SnowModerate
            | WeatherCode::SnowHeavy
            | WeatherCode::Blizzard
            | WeatherCode::Sleet,
            false,
        ) => 0xFF_40_50_60,
        (WeatherCode::Tornado | WeatherCode::Hurricane, _) => 0xFF_2A_1A_1A,
        (WeatherCode::DustStorm, _) => 0xFF_8A_7A_5A,
        (WeatherCode::Hail, _) => 0xFF_3A_4A_5A,
    }
}

// ── Weather data aggregate ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LocationWeather {
    pub location: Location,
    pub current: CurrentConditions,
    pub hourly: Vec<HourlyForecast>,
    pub daily: Vec<DailyForecast>,
    pub minutely: Vec<MinutelyPrecip>,
    pub air_quality: Option<AirQuality>,
    pub pollen: Option<PollenCount>,
    pub alerts: Vec<WeatherAlert>,
    pub astronomy: Option<AstronomyData>,
    pub historical: Option<HistoricalWeather>,
    pub last_updated: u64,
}

impl LocationWeather {
    pub fn new(location: Location) -> Self {
        Self {
            location,
            current: CurrentConditions::new(),
            hourly: Vec::new(),
            daily: Vec::new(),
            minutely: Vec::new(),
            air_quality: None,
            pollen: None,
            alerts: Vec::new(),
            astronomy: None,
            historical: None,
            last_updated: 0,
        }
    }

    pub fn active_alerts(&self, now: u64) -> Vec<&WeatherAlert> {
        self.alerts.iter().filter(|a| a.is_active(now)).collect()
    }

    pub fn has_severe_alerts(&self, now: u64) -> bool {
        self.active_alerts(now)
            .iter()
            .any(|a| a.severity == AlertSeverity::Warning)
    }

    pub fn next_hour_precip_expected(&self) -> bool {
        self.minutely.iter().any(|m| m.intensity > 0.0)
    }

    pub fn today_forecast(&self) -> Option<&DailyForecast> {
        self.daily.first()
    }
}

// ── Weather app ─────────────────────────────────────────────────────────

pub struct WeatherApp {
    pub locations: Vec<LocationWeather>,
    pub selected_location: usize,
    pub units: UnitPrefs,
    pub next_location_id: u64,
    pub widget_size: WidgetSize,
    pub auto_detect_location: bool,
    pub alert_notifications_enabled: bool,
}

impl WeatherApp {
    pub fn new() -> Self {
        Self {
            locations: Vec::new(),
            selected_location: 0,
            units: UnitPrefs::metric(),
            next_location_id: 1,
            widget_size: WidgetSize::Medium,
            auto_detect_location: true,
            alert_notifications_enabled: true,
        }
    }

    pub fn add_location(&mut self, name: &str, lat: f64, lon: f64) -> u64 {
        let id = self.next_location_id;
        self.next_location_id += 1;
        let loc = Location::new(id, name, lat, lon);
        self.locations.push(LocationWeather::new(loc));
        id
    }

    pub fn add_location_by_zip(&mut self, zip: &str) -> u64 {
        let id = self.next_location_id;
        self.next_location_id += 1;
        let mut loc = Location::new(id, zip, 0.0, 0.0);
        loc.name = format!("ZIP: {}", zip);
        self.locations.push(LocationWeather::new(loc));
        id
    }

    pub fn set_auto_location(&mut self, lat: f64, lon: f64) {
        let existing = self.locations.iter_mut().find(|l| l.location.is_current);
        if let Some(loc) = existing {
            loc.location.latitude = lat;
            loc.location.longitude = lon;
        } else {
            let id = self.next_location_id;
            self.next_location_id += 1;
            let loc = Location::from_coordinates(id, lat, lon);
            self.locations.insert(0, LocationWeather::new(loc));
        }
    }

    pub fn remove_location(&mut self, id: u64) {
        self.locations.retain(|l| l.location.id != id);
        if self.selected_location >= self.locations.len() {
            self.selected_location = self.locations.len().saturating_sub(1);
        }
    }

    pub fn select_location(&mut self, index: usize) {
        if index < self.locations.len() {
            self.selected_location = index;
        }
    }

    pub fn current_weather(&self) -> Option<&LocationWeather> {
        self.locations.get(self.selected_location)
    }

    pub fn current_weather_mut(&mut self) -> Option<&mut LocationWeather> {
        self.locations.get_mut(self.selected_location)
    }

    pub fn current_temp_display(&self) -> Option<f32> {
        self.current_weather()
            .map(|w| w.current.temp_display(self.units.temp))
    }

    pub fn set_units(&mut self, units: UnitPrefs) {
        self.units = units;
    }

    pub fn all_active_alerts(&self, now: u64) -> Vec<(&Location, &WeatherAlert)> {
        let mut alerts = Vec::new();
        for lw in &self.locations {
            for alert in lw.active_alerts(now) {
                alerts.push((&lw.location, alert));
            }
        }
        alerts
    }

    pub fn update_conditions(&mut self, location_id: u64, conditions: CurrentConditions) {
        if let Some(lw) = self
            .locations
            .iter_mut()
            .find(|l| l.location.id == location_id)
        {
            lw.current = conditions;
        }
    }

    pub fn update_hourly(&mut self, location_id: u64, forecast: Vec<HourlyForecast>) {
        if let Some(lw) = self
            .locations
            .iter_mut()
            .find(|l| l.location.id == location_id)
        {
            lw.hourly = forecast;
        }
    }

    pub fn update_daily(&mut self, location_id: u64, forecast: Vec<DailyForecast>) {
        if let Some(lw) = self
            .locations
            .iter_mut()
            .find(|l| l.location.id == location_id)
        {
            lw.daily = forecast;
        }
    }

    pub fn update_minutely(&mut self, location_id: u64, data: Vec<MinutelyPrecip>) {
        if let Some(lw) = self
            .locations
            .iter_mut()
            .find(|l| l.location.id == location_id)
        {
            lw.minutely = data;
        }
    }

    pub fn update_air_quality(&mut self, location_id: u64, aq: AirQuality) {
        if let Some(lw) = self
            .locations
            .iter_mut()
            .find(|l| l.location.id == location_id)
        {
            lw.air_quality = Some(aq);
        }
    }

    pub fn update_alerts(&mut self, location_id: u64, alerts: Vec<WeatherAlert>) {
        if let Some(lw) = self
            .locations
            .iter_mut()
            .find(|l| l.location.id == location_id)
        {
            lw.alerts = alerts;
        }
    }

    pub fn update_astronomy(&mut self, location_id: u64, data: AstronomyData) {
        if let Some(lw) = self
            .locations
            .iter_mut()
            .find(|l| l.location.id == location_id)
        {
            lw.astronomy = Some(data);
        }
    }

    pub fn location_count(&self) -> usize {
        self.locations.len()
    }
}

// ── Global instance ─────────────────────────────────────────────────────

static INITIALIZED: AtomicBool = AtomicBool::new(false);

static mut WEATHER_APP_INSTANCE: Option<WeatherApp> = None;

pub fn init() {
    if INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            WEATHER_APP_INSTANCE = Some(WeatherApp::new());
        }
    }
}

pub fn weather() -> &'static mut WeatherApp {
    unsafe {
        WEATHER_APP_INSTANCE
            .as_mut()
            .expect("WEATHER_APP not initialized; call init() first")
    }
}
