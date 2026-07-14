//! AthenaOS Weather — the bundled weather app (Concept §Three User Experiences:
//! the daily-driver parity bar vs Win11 Weather / macOS Weather).
//!
//! A standalone userspace ELF (`exec_path = "weather"`). The rich data model
//! (`model` — conditions, forecasts, units, weather codes) was the previously
//! UNWIRED `athshell::weather_app`; it moved here and became a live app. This
//! `main.rs` is the app shell: it builds a demo `LocationWeather` (there is no
//! live weather-service syscall yet — a `NEEDS-INTERFACE` follow-up), renders
//! the current conditions + a short daily forecast on the OBSIDIAN design
//! language, and re-skins to the live desktop accent via `SYS_THEME_GET`.
//!
//! PROOF: a `#![no_main]` ELF can't run `cargo test`, so `design_proof()` (a
//! fail-able runtime gate at `_start`) asserts the model builds a sane demo
//! (named location, plausible temperature) AND the chrome is wired to the shared
//! `ath_tokens` tokens — `exit(3)` on any drift.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;

#[allow(unused_imports)]
use athkit;

use ath_tokens::{DARK, RAEBLUE, TYPE_BODY, TYPE_CAPTION, TYPE_DISPLAY, TYPE_LABEL, TYPE_TITLE};
use athgfx::text::FontFamily;
use athgfx::Canvas;

mod model;
use model::{Location, LocationWeather, WeatherCode};

const WIN_W: usize = 720;
const WIN_H: usize = 460;
const SURFACE_VIRT: u64 = 0x0000_7C00_0000;

// OBSIDIAN chrome on the shared `ath_tokens::DARK` palette — near-black tiers,
// no frost, accent glow. Live Vibe accent via SYS_THEME_GET (whole-OS cohesion).
const BG: u32 = DARK.bg_base;
const CARD_BG: u32 = DARK.bg_raised;
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_DIM: u32 = DARK.text_secondary;
const TEXT_MUTE: u32 = DARK.text_tertiary;

fn accent() -> u32 {
    ath_tokens::derive_accent(athkit::sys::theme_accent(), &DARK).base
}

/// A 5-day forecast row: (day, high°C, low°C, code). Kept local (the live
/// service syscall is a follow-up); the model's `WeatherCode` labels it.
const FORECAST: [(&str, i32, i32, WeatherCode); 5] = [
    ("Mon", 19, 12, WeatherCode::Clear),
    ("Tue", 17, 11, WeatherCode::Overcast),
    ("Wed", 16, 10, WeatherCode::RainModerate),
    ("Thu", 18, 12, WeatherCode::PartlyCloudy),
    ("Fri", 21, 13, WeatherCode::Clear),
];

/// Short human label for a weather code (font-safe — the bitmap/AA font has no
/// weather emoji, so we render words, not glyphs).
fn code_label(c: WeatherCode) -> &'static str {
    match c {
        WeatherCode::Clear => "Clear",
        WeatherCode::PartlyCloudy => "Partly cloudy",
        WeatherCode::Overcast => "Overcast",
        WeatherCode::Fog => "Fog",
        WeatherCode::DrizzleLight | WeatherCode::DrizzleModerate => "Drizzle",
        WeatherCode::RainLight => "Light rain",
        WeatherCode::RainModerate => "Rain",
        WeatherCode::RainHeavy | WeatherCode::FreezingRain => "Heavy rain",
        WeatherCode::Sleet => "Sleet",
        WeatherCode::SnowLight | WeatherCode::SnowModerate | WeatherCode::SnowHeavy => "Snow",
        _ => "Cloudy",
    }
}

/// Build the demo location + current conditions from the reused model.
fn demo() -> LocationWeather {
    let mut lw = LocationWeather::new(Location::new(1, "San Francisco", 37.77, -122.42));
    lw.current.temperature = 18.0;
    lw.current.feels_like = 17.0;
    lw.current.humidity = 72;
    lw.current.wind_speed = 12.0;
    lw.current.wind_direction = 270; // W
    lw.current.uv_index = 4;
    lw.current.weather_code = WeatherCode::PartlyCloudy;
    lw.current.description = String::from("Partly cloudy");
    lw
}

fn render(lw: &LocationWeather, canvas: &mut Canvas) {
    let acc = accent();
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // ── Header: location ────────────────────────────────────────────────
    canvas.draw_text_aa(
        28,
        26,
        &lw.location.name,
        TYPE_TITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(28, 56, "Now", TYPE_CAPTION, TEXT_MUTE, FontFamily::Sans);

    // ── Hero: big current temperature + condition ───────────────────────
    let temp = format!("{}\u{b0}", lw.current.temperature as i32);
    canvas.draw_text_aa(24, 92, &temp, TYPE_DISPLAY, TEXT_FG, FontFamily::Sans);
    canvas.draw_text_aa(
        200,
        108,
        &lw.current.description,
        TYPE_TITLE,
        acc,
        FontFamily::Sans,
    );
    let feels = format!("Feels like {}\u{b0}", lw.current.feels_like as i32);
    canvas.draw_text_aa(200, 138, &feels, TYPE_BODY, TEXT_DIM, FontFamily::Sans);

    // ── Stat pills: humidity / wind / UV ────────────────────────────────
    let stats = [
        format!("Humidity  {}%", lw.current.humidity),
        format!(
            "Wind  {} {} km/h",
            lw.current.wind_direction_str(),
            lw.current.wind_speed as i32
        ),
        format!("UV index  {}", lw.current.uv_index),
    ];
    let pill_w = 210usize;
    let pill_h = 44usize;
    for (i, s) in stats.iter().enumerate() {
        let x = 28 + i * (pill_w + 14);
        let y = 180usize;
        canvas.fill_rounded_rect(
            x,
            y,
            pill_w,
            pill_h,
            ath_tokens::RADIUS_MD as usize,
            CARD_BG,
        );
        canvas.draw_text_aa(
            x as i32 + 16,
            y as i32 + 13,
            s,
            TYPE_BODY,
            TEXT_FG,
            FontFamily::Sans,
        );
    }

    // ── Forecast card: 5 days ───────────────────────────────────────────
    let fc_y = 250usize;
    let fc_h = 176usize;
    canvas.fill_rounded_rect(
        28,
        fc_y,
        WIN_W - 56,
        fc_h,
        ath_tokens::RADIUS_LG as usize,
        CARD_BG,
    );
    canvas.draw_text_aa(
        48,
        fc_y as i32 + 16,
        "5-day forecast",
        TYPE_LABEL,
        TEXT_DIM,
        FontFamily::Sans,
    );
    let col_w = (WIN_W - 56 - 40) / FORECAST.len();
    for (i, (day, hi, lo, code)) in FORECAST.iter().enumerate() {
        let cx = 48 + i * col_w;
        let cy = fc_y as i32 + 56;
        // Selected/today column gets a subtle accent wash.
        if i == 0 {
            canvas.fill_rounded_rect(
                cx - 8,
                cy as usize - 6,
                col_w - 8,
                104,
                ath_tokens::RADIUS_MD as usize,
                ath_tokens::derive_accent(athkit::sys::theme_accent(), &DARK).subtle,
            );
        }
        canvas.draw_text_aa(cx as i32, cy, day, TYPE_BODY, TEXT_FG, FontFamily::Sans);
        canvas.draw_text_aa(
            cx as i32,
            cy + 30,
            code_label(*code),
            TYPE_CAPTION,
            TEXT_MUTE,
            FontFamily::Sans,
        );
        let hilo = format!("{}\u{b0}  {}\u{b0}", hi, lo);
        canvas.draw_text_aa(
            cx as i32,
            cy + 66,
            &hilo,
            TYPE_BODY,
            TEXT_DIM,
            FontFamily::Sans,
        );
    }

    // ── Footer hint ─────────────────────────────────────────────────────
    canvas.draw_text_aa(
        28,
        WIN_H as i32 - 22,
        "Esc  Close",
        TYPE_CAPTION,
        TEXT_MUTE,
        FontFamily::Sans,
    );
}

/// Fail-able design/wiring proof — the ELF's stand-in for `cargo test`.
pub fn design_proof() -> bool {
    let lw = demo();
    let named = !lw.location.name.is_empty();
    let sane_temp = lw.current.temperature > -90.0 && lw.current.temperature < 60.0;
    let wind_ok = lw.current.wind_direction_str() == "W";
    let tokens_ok = BG == DARK.bg_base
        && TEXT_FG == DARK.text_primary
        && athkit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;
    named && sane_temp && wind_ok && tokens_ok
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    if !design_proof() {
        athkit::sys::exit(3);
    }
    let sid = athkit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        athkit::sys::exit(1);
    }
    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };
    let lw = demo();
    render(&lw, &mut canvas);
    athkit::sys::surface_present(sid, 180, 90);

    let mut extended = false;
    loop {
        let key = athkit::sys::read_key();
        if key == 0 {
            athkit::sys::yield_now();
            continue;
        }
        let sc = key as u8;
        if sc == 0xE0 {
            extended = true;
            continue;
        }
        let _ext = core::mem::replace(&mut extended, false);
        if sc & 0x80 != 0 {
            continue;
        }
        // Esc closes.
        if sc & 0x7F == 0x01 {
            athkit::sys::exit(0);
        }
    }
}
