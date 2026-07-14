//! winmm.dll — Windows Multimedia API: high-resolution timers, wave audio
//! I/O, MIDI output stubs, and legacy joystick access for RaeBridge.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::CompatContext;

// =========================================================================
// MMRESULT codes
// =========================================================================

pub const MMSYSERR_NOERROR: u32 = 0;
pub const MMSYSERR_ERROR: u32 = 1;
pub const MMSYSERR_BADDEVICEID: u32 = 2;
pub const MMSYSERR_NOTENABLED: u32 = 3;
pub const MMSYSERR_ALLOCATED: u32 = 4;
pub const MMSYSERR_INVALHANDLE: u32 = 5;
pub const MMSYSERR_NODRIVER: u32 = 6;
pub const MMSYSERR_NOMEM: u32 = 7;
pub const MMSYSERR_NOTSUPPORTED: u32 = 8;
pub const MMSYSERR_BADERRNUM: u32 = 9;
pub const MMSYSERR_INVALFLAG: u32 = 10;
pub const MMSYSERR_INVALPARAM: u32 = 11;

pub const WAVERR_BADFORMAT: u32 = 32;
pub const WAVERR_STILLPLAYING: u32 = 33;
pub const WAVERR_UNPREPARED: u32 = 34;
pub const WAVERR_SYNC: u32 = 35;

pub const TIMERR_NOERROR: u32 = 0;
pub const TIMERR_NOCANDO: u32 = 97;
pub const TIMERR_STRUCT: u32 = 129;

pub const JOYERR_NOERROR: u32 = 0;
pub const JOYERR_PARMS: u32 = 165;
pub const JOYERR_NOCANDO: u32 = 166;
pub const JOYERR_UNPLUGGED: u32 = 167;

// =========================================================================
// Wave format constants
// =========================================================================

pub const WAVE_FORMAT_PCM: u16 = 0x0001;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

pub const WAVE_MAPPER: u32 = 0xFFFFFFFF;

pub const CALLBACK_NULL: u32 = 0x00000000;
pub const CALLBACK_WINDOW: u32 = 0x00010000;
pub const CALLBACK_THREAD: u32 = 0x00020000;
pub const CALLBACK_FUNCTION: u32 = 0x00030000;
pub const CALLBACK_EVENT: u32 = 0x00050000;

// =========================================================================
// Wave header flags
// =========================================================================

pub const WHDR_DONE: u32 = 0x00000001;
pub const WHDR_PREPARED: u32 = 0x00000002;
pub const WHDR_BEGINLOOP: u32 = 0x00000004;
pub const WHDR_ENDLOOP: u32 = 0x00000008;
pub const WHDR_INQUEUE: u32 = 0x00000010;

// =========================================================================
// Timer event types
// =========================================================================

pub const TIME_ONESHOT: u32 = 0x0000;
pub const TIME_PERIODIC: u32 = 0x0001;
pub const TIME_CALLBACK_FUNCTION: u32 = 0x0000;
pub const TIME_CALLBACK_EVENT_SET: u32 = 0x0010;
pub const TIME_CALLBACK_EVENT_PULSE: u32 = 0x0020;
pub const TIME_KILL_SYNCHRONOUS: u32 = 0x0100;

// =========================================================================
// Joystick constants
// =========================================================================

pub const JOYSTICKID1: u32 = 0;
pub const JOYSTICKID2: u32 = 1;
pub const MAX_JOYSTICK_ID: u32 = 15;

pub const JOY_BUTTON1: u32 = 0x0001;
pub const JOY_BUTTON2: u32 = 0x0002;
pub const JOY_BUTTON3: u32 = 0x0004;
pub const JOY_BUTTON4: u32 = 0x0008;
pub const JOY_BUTTON5: u32 = 0x0010;
pub const JOY_BUTTON6: u32 = 0x0020;
pub const JOY_BUTTON7: u32 = 0x0040;
pub const JOY_BUTTON8: u32 = 0x0080;

pub const JOY_RETURNX: u32 = 0x00000001;
pub const JOY_RETURNY: u32 = 0x00000002;
pub const JOY_RETURNZ: u32 = 0x00000004;
pub const JOY_RETURNR: u32 = 0x00000008;
pub const JOY_RETURNU: u32 = 0x00000010;
pub const JOY_RETURNV: u32 = 0x00000020;
pub const JOY_RETURNPOV: u32 = 0x00000040;
pub const JOY_RETURNBUTTONS: u32 = 0x00000080;
pub const JOY_RETURNALL: u32 = 0x000000FF;

// =========================================================================
// Structures
// =========================================================================

#[derive(Debug, Clone, Copy)]
pub struct WaveFormatEx {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub cb_size: u16,
}

impl WaveFormatEx {
    pub fn pcm(channels: u16, sample_rate: u32, bits: u16) -> Self {
        let block_align = channels * bits / 8;
        Self {
            format_tag: WAVE_FORMAT_PCM,
            channels,
            samples_per_sec: sample_rate,
            avg_bytes_per_sec: sample_rate * block_align as u32,
            block_align,
            bits_per_sample: bits,
            cb_size: 0,
        }
    }

    pub fn cd_quality() -> Self {
        Self::pcm(2, 44100, 16)
    }
    pub fn dvd_quality() -> Self {
        Self::pcm(2, 48000, 16)
    }
}

#[derive(Debug, Clone)]
pub struct WaveHdr {
    pub data: Vec<u8>,
    pub buffer_length: u32,
    pub bytes_recorded: u32,
    pub flags: u32,
    pub loops: u32,
    pub user: u64,
}

impl WaveHdr {
    pub fn new(size: u32) -> Self {
        let mut data = Vec::new();
        data.resize(size as usize, 0u8);
        Self {
            data,
            buffer_length: size,
            bytes_recorded: 0,
            flags: 0,
            loops: 0,
            user: 0,
        }
    }

    pub fn is_prepared(&self) -> bool {
        self.flags & WHDR_PREPARED != 0
    }
    pub fn is_done(&self) -> bool {
        self.flags & WHDR_DONE != 0
    }
    pub fn is_in_queue(&self) -> bool {
        self.flags & WHDR_INQUEUE != 0
    }
}

#[derive(Debug, Clone)]
pub struct WaveOutCaps {
    pub manufacturer_id: u16,
    pub product_id: u16,
    pub driver_version: u32,
    pub product_name: String,
    pub formats: u32,
    pub channels: u16,
    pub support: u32,
}

impl WaveOutCaps {
    pub fn default_device() -> Self {
        Self {
            manufacturer_id: 1,
            product_id: 100,
            driver_version: 0x0500,
            product_name: String::from("RaeenOS Audio Output"),
            formats: 0x00000FFF,
            channels: 2,
            support: 0x000F,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WaveInCaps {
    pub manufacturer_id: u16,
    pub product_id: u16,
    pub driver_version: u32,
    pub product_name: String,
    pub formats: u32,
    pub channels: u16,
}

impl WaveInCaps {
    pub fn default_device() -> Self {
        Self {
            manufacturer_id: 1,
            product_id: 200,
            driver_version: 0x0500,
            product_name: String::from("RaeenOS Audio Input"),
            formats: 0x00000FFF,
            channels: 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MidiOutCaps {
    pub manufacturer_id: u16,
    pub product_id: u16,
    pub driver_version: u32,
    pub product_name: String,
    pub technology: u16,
    pub voices: u16,
    pub notes: u16,
    pub channel_mask: u16,
    pub support: u32,
}

impl MidiOutCaps {
    pub fn soft_synth() -> Self {
        Self {
            manufacturer_id: 1,
            product_id: 1,
            driver_version: 0x0100,
            product_name: String::from("RaeenOS Software Synth"),
            technology: 0x0001, // MOD_MIDIPORT
            voices: 16,
            notes: 128,
            channel_mask: 0xFFFF,
            support: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TimeCaps {
    pub period_min: u32,
    pub period_max: u32,
}

impl TimeCaps {
    pub fn system_default() -> Self {
        Self {
            period_min: 1,
            period_max: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct JoyCaps {
    pub manufacturer_id: u16,
    pub product_id: u16,
    pub x_min: u32,
    pub x_max: u32,
    pub y_min: u32,
    pub y_max: u32,
    pub z_min: u32,
    pub z_max: u32,
    pub num_buttons: u32,
    pub period_min: u32,
    pub period_max: u32,
    pub r_min: u32,
    pub r_max: u32,
    pub u_min: u32,
    pub u_max: u32,
    pub v_min: u32,
    pub v_max: u32,
    pub caps: u32,
    pub max_axes: u32,
    pub num_axes: u32,
    pub max_buttons: u32,
}

impl JoyCaps {
    pub fn default_gamepad() -> Self {
        Self {
            manufacturer_id: 0x045E,
            product_id: 0x028E,
            x_min: 0,
            x_max: 65535,
            y_min: 0,
            y_max: 65535,
            z_min: 0,
            z_max: 255,
            num_buttons: 16,
            period_min: 10,
            period_max: 1000,
            r_min: 0,
            r_max: 65535,
            u_min: 0,
            u_max: 65535,
            v_min: 0,
            v_max: 255,
            caps: 0x003F,
            max_axes: 6,
            num_axes: 6,
            max_buttons: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct JoyInfoEx {
    pub size: u32,
    pub flags: u32,
    pub x_pos: u32,
    pub y_pos: u32,
    pub z_pos: u32,
    pub r_pos: u32,
    pub u_pos: u32,
    pub v_pos: u32,
    pub buttons: u32,
    pub button_number: u32,
    pub pov: u32,
}

// =========================================================================
// Timer state
// =========================================================================

static TIMER_RESOLUTION_MS: AtomicU32 = AtomicU32::new(15);
static TIMER_COUNTER: AtomicU64 = AtomicU64::new(60_000);
static NEXT_TIMER_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Debug, Clone)]
pub struct TimerEvent {
    pub id: u32,
    pub delay_ms: u32,
    pub resolution_ms: u32,
    pub callback: u64,
    pub user_data: u64,
    pub event_type: u32,
    pub active: bool,
}

// =========================================================================
// Wave output device state
// =========================================================================

pub struct WaveOutDevice {
    pub id: u32,
    pub format: WaveFormatEx,
    pub open: bool,
    pub playing: bool,
    pub volume: u32,
    pub callback: u64,
    pub callback_type: u32,
    pub queued_headers: Vec<WaveHdr>,
    pub position_bytes: u64,
}

impl WaveOutDevice {
    fn new(id: u32, format: WaveFormatEx, callback: u64, callback_type: u32) -> Self {
        Self {
            id,
            format,
            open: true,
            playing: false,
            volume: 0xFFFFFFFF,
            callback,
            callback_type,
            queued_headers: Vec::new(),
            position_bytes: 0,
        }
    }
}

// =========================================================================
// Wave input device state
// =========================================================================

pub struct WaveInDevice {
    pub id: u32,
    pub format: WaveFormatEx,
    pub open: bool,
    pub recording: bool,
    pub callback: u64,
    pub callback_type: u32,
    pub queued_headers: Vec<WaveHdr>,
}

impl WaveInDevice {
    fn new(id: u32, format: WaveFormatEx, callback: u64, callback_type: u32) -> Self {
        Self {
            id,
            format,
            open: true,
            recording: false,
            callback,
            callback_type,
            queued_headers: Vec::new(),
        }
    }
}

// =========================================================================
// MIDI output device state
// =========================================================================

pub struct MidiOutDevice {
    pub id: u32,
    pub open: bool,
}

// =========================================================================
// Multimedia subsystem
// =========================================================================

pub struct WinmmSystem {
    pub wave_out_devices: Vec<WaveOutDevice>,
    pub wave_in_devices: Vec<WaveInDevice>,
    pub midi_out_devices: Vec<MidiOutDevice>,
    pub timer_events: Vec<TimerEvent>,
    pub next_wave_out_handle: u32,
    pub next_wave_in_handle: u32,
    pub next_midi_out_handle: u32,
}

impl WinmmSystem {
    pub fn new() -> Self {
        Self {
            wave_out_devices: Vec::new(),
            wave_in_devices: Vec::new(),
            midi_out_devices: Vec::new(),
            timer_events: Vec::new(),
            next_wave_out_handle: 1,
            next_wave_in_handle: 1,
            next_midi_out_handle: 1,
        }
    }
}

// =========================================================================
// Timer API
// =========================================================================

pub fn time_get_time() -> u32 {
    TIMER_COUNTER.fetch_add(
        TIMER_RESOLUTION_MS.load(Ordering::Relaxed) as u64,
        Ordering::Relaxed,
    ) as u32
}

pub fn time_begin_period(period: u32) -> u32 {
    if period == 0 {
        return TIMERR_NOCANDO;
    }
    let caps = TimeCaps::system_default();
    if period < caps.period_min || period > caps.period_max {
        return TIMERR_NOCANDO;
    }
    let current = TIMER_RESOLUTION_MS.load(Ordering::Relaxed);
    if period < current {
        TIMER_RESOLUTION_MS.store(period, Ordering::Relaxed);
    }
    TIMERR_NOERROR
}

pub fn time_end_period(period: u32) -> u32 {
    if period == 0 {
        return TIMERR_NOCANDO;
    }
    TIMER_RESOLUTION_MS.store(15, Ordering::Relaxed);
    TIMERR_NOERROR
}

pub fn time_get_dev_caps(caps: &mut TimeCaps) -> u32 {
    *caps = TimeCaps::system_default();
    TIMERR_NOERROR
}

pub fn time_set_event(
    system: &mut WinmmSystem,
    delay: u32,
    resolution: u32,
    callback: u64,
    user_data: u64,
    event_type: u32,
) -> u32 {
    if delay == 0 {
        return 0;
    }

    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    system.timer_events.push(TimerEvent {
        id,
        delay_ms: delay,
        resolution_ms: resolution,
        callback,
        user_data,
        event_type,
        active: true,
    });
    id
}

pub fn time_kill_event(system: &mut WinmmSystem, timer_id: u32) -> u32 {
    if let Some(timer) = system.timer_events.iter_mut().find(|t| t.id == timer_id) {
        timer.active = false;
        TIMERR_NOERROR
    } else {
        MMSYSERR_INVALPARAM
    }
}

// =========================================================================
// Wave Output API
// =========================================================================

pub fn wave_out_get_num_devs() -> u32 {
    1
}

pub fn wave_out_get_dev_caps_w(_device_id: u32, caps: &mut WaveOutCaps) -> u32 {
    *caps = WaveOutCaps::default_device();
    MMSYSERR_NOERROR
}

pub fn wave_out_get_dev_caps_a(_device_id: u32, caps: &mut WaveOutCaps) -> u32 {
    *caps = WaveOutCaps::default_device();
    MMSYSERR_NOERROR
}

pub fn wave_out_open(
    system: &mut WinmmSystem,
    device_id: u32,
    format: &WaveFormatEx,
    callback: u64,
    _instance: u64,
    flags: u32,
) -> Result<u32, u32> {
    if device_id != WAVE_MAPPER && device_id > 0 {
        return Err(MMSYSERR_BADDEVICEID);
    }

    if format.format_tag != WAVE_FORMAT_PCM
        && format.format_tag != WAVE_FORMAT_IEEE_FLOAT
        && format.format_tag != WAVE_FORMAT_EXTENSIBLE
    {
        return Err(WAVERR_BADFORMAT);
    }

    let handle = system.next_wave_out_handle;
    system.next_wave_out_handle += 1;

    let callback_type = flags & 0x00070000;
    system
        .wave_out_devices
        .push(WaveOutDevice::new(handle, *format, callback, callback_type));
    Ok(handle)
}

pub fn wave_out_close(system: &mut WinmmSystem, handle: u32) -> u32 {
    if let Some(dev) = system.wave_out_devices.iter_mut().find(|d| d.id == handle) {
        if !dev.queued_headers.is_empty() {
            return WAVERR_STILLPLAYING;
        }
        dev.open = false;
    } else {
        return MMSYSERR_INVALHANDLE;
    }
    system.wave_out_devices.retain(|d| d.open);
    MMSYSERR_NOERROR
}

pub fn wave_out_prepare_header(
    _system: &mut WinmmSystem,
    _handle: u32,
    header: &mut WaveHdr,
) -> u32 {
    header.flags |= WHDR_PREPARED;
    header.flags &= !WHDR_DONE;
    MMSYSERR_NOERROR
}

pub fn wave_out_unprepare_header(
    _system: &mut WinmmSystem,
    _handle: u32,
    header: &mut WaveHdr,
) -> u32 {
    if header.is_in_queue() {
        return WAVERR_STILLPLAYING;
    }
    header.flags &= !WHDR_PREPARED;
    MMSYSERR_NOERROR
}

pub fn wave_out_write(system: &mut WinmmSystem, handle: u32, header: &mut WaveHdr) -> u32 {
    if !header.is_prepared() {
        return WAVERR_UNPREPARED;
    }

    let dev = match system.wave_out_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };

    header.flags |= WHDR_INQUEUE;
    header.flags &= !WHDR_DONE;
    dev.position_bytes += header.buffer_length as u64;

    let mut completed = header.clone();
    completed.flags &= !WHDR_INQUEUE;
    completed.flags |= WHDR_DONE;
    dev.queued_headers.push(completed);

    MMSYSERR_NOERROR
}

pub fn wave_out_reset(system: &mut WinmmSystem, handle: u32) -> u32 {
    let dev = match system.wave_out_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };

    for hdr in &mut dev.queued_headers {
        hdr.flags &= !WHDR_INQUEUE;
        hdr.flags |= WHDR_DONE;
    }
    dev.playing = false;
    MMSYSERR_NOERROR
}

pub fn wave_out_pause(system: &mut WinmmSystem, handle: u32) -> u32 {
    let dev = match system.wave_out_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };
    dev.playing = false;
    MMSYSERR_NOERROR
}

pub fn wave_out_restart(system: &mut WinmmSystem, handle: u32) -> u32 {
    let dev = match system.wave_out_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };
    dev.playing = true;
    MMSYSERR_NOERROR
}

pub fn wave_out_get_volume(_system: &WinmmSystem, handle: u32, volume: &mut u32) -> u32 {
    let _ = handle;
    *volume = 0xFFFFFFFF;
    MMSYSERR_NOERROR
}

pub fn wave_out_set_volume(system: &mut WinmmSystem, handle: u32, volume: u32) -> u32 {
    let dev = match system.wave_out_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };
    dev.volume = volume;
    MMSYSERR_NOERROR
}

pub fn wave_out_get_position(system: &WinmmSystem, handle: u32) -> Result<u64, u32> {
    let dev = match system.wave_out_devices.iter().find(|d| d.id == handle) {
        Some(d) => d,
        None => return Err(MMSYSERR_INVALHANDLE),
    };
    Ok(dev.position_bytes)
}

pub fn wave_out_get_error_text_w(error: u32, text: &mut String) -> u32 {
    text.clear();
    match error {
        MMSYSERR_NOERROR => text.push_str("No error"),
        MMSYSERR_BADDEVICEID => text.push_str("Bad device ID"),
        MMSYSERR_ALLOCATED => text.push_str("Device already allocated"),
        MMSYSERR_INVALHANDLE => text.push_str("Invalid handle"),
        MMSYSERR_NODRIVER => text.push_str("No driver installed"),
        MMSYSERR_NOMEM => text.push_str("Out of memory"),
        WAVERR_BADFORMAT => text.push_str("Unsupported wave format"),
        WAVERR_STILLPLAYING => text.push_str("Buffers still playing"),
        WAVERR_UNPREPARED => text.push_str("Header not prepared"),
        _ => text.push_str("Unknown error"),
    }
    MMSYSERR_NOERROR
}

pub fn wave_out_break_loop(system: &mut WinmmSystem, handle: u32) -> u32 {
    if system.wave_out_devices.iter().any(|d| d.id == handle) {
        MMSYSERR_NOERROR
    } else {
        MMSYSERR_INVALHANDLE
    }
}

// =========================================================================
// Wave Input API
// =========================================================================

pub fn wave_in_get_num_devs() -> u32 {
    1
}

pub fn wave_in_get_dev_caps_w(_device_id: u32, caps: &mut WaveInCaps) -> u32 {
    *caps = WaveInCaps::default_device();
    MMSYSERR_NOERROR
}

pub fn wave_in_open(
    system: &mut WinmmSystem,
    device_id: u32,
    format: &WaveFormatEx,
    callback: u64,
    _instance: u64,
    flags: u32,
) -> Result<u32, u32> {
    if device_id != WAVE_MAPPER && device_id > 0 {
        return Err(MMSYSERR_BADDEVICEID);
    }

    let handle = system.next_wave_in_handle;
    system.next_wave_in_handle += 1;

    let callback_type = flags & 0x00070000;
    system
        .wave_in_devices
        .push(WaveInDevice::new(handle, *format, callback, callback_type));
    Ok(handle)
}

pub fn wave_in_close(system: &mut WinmmSystem, handle: u32) -> u32 {
    if let Some(dev) = system.wave_in_devices.iter_mut().find(|d| d.id == handle) {
        if dev.recording {
            return WAVERR_STILLPLAYING;
        }
        dev.open = false;
    } else {
        return MMSYSERR_INVALHANDLE;
    }
    system.wave_in_devices.retain(|d| d.open);
    MMSYSERR_NOERROR
}

pub fn wave_in_prepare_header(
    _system: &mut WinmmSystem,
    _handle: u32,
    header: &mut WaveHdr,
) -> u32 {
    header.flags |= WHDR_PREPARED;
    header.flags &= !WHDR_DONE;
    MMSYSERR_NOERROR
}

pub fn wave_in_unprepare_header(
    _system: &mut WinmmSystem,
    _handle: u32,
    header: &mut WaveHdr,
) -> u32 {
    if header.is_in_queue() {
        return WAVERR_STILLPLAYING;
    }
    header.flags &= !WHDR_PREPARED;
    MMSYSERR_NOERROR
}

pub fn wave_in_add_buffer(system: &mut WinmmSystem, handle: u32, header: &mut WaveHdr) -> u32 {
    if !header.is_prepared() {
        return WAVERR_UNPREPARED;
    }

    let dev = match system.wave_in_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };

    header.flags |= WHDR_INQUEUE;
    header.bytes_recorded = 0;
    dev.queued_headers.push(header.clone());
    MMSYSERR_NOERROR
}

pub fn wave_in_start(system: &mut WinmmSystem, handle: u32) -> u32 {
    let dev = match system.wave_in_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };
    dev.recording = true;
    MMSYSERR_NOERROR
}

pub fn wave_in_stop(system: &mut WinmmSystem, handle: u32) -> u32 {
    let dev = match system.wave_in_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };
    dev.recording = false;
    for hdr in &mut dev.queued_headers {
        hdr.flags &= !WHDR_INQUEUE;
        hdr.flags |= WHDR_DONE;
    }
    MMSYSERR_NOERROR
}

pub fn wave_in_reset(system: &mut WinmmSystem, handle: u32) -> u32 {
    let dev = match system.wave_in_devices.iter_mut().find(|d| d.id == handle) {
        Some(d) => d,
        None => return MMSYSERR_INVALHANDLE,
    };
    dev.recording = false;
    for hdr in &mut dev.queued_headers {
        hdr.flags &= !WHDR_INQUEUE;
        hdr.flags |= WHDR_DONE;
    }
    MMSYSERR_NOERROR
}

pub fn wave_in_get_position(system: &WinmmSystem, handle: u32) -> Result<u64, u32> {
    if system.wave_in_devices.iter().any(|d| d.id == handle) {
        Ok(0)
    } else {
        Err(MMSYSERR_INVALHANDLE)
    }
}

// =========================================================================
// MIDI Output API (stubs — return 0 or 1 soft synth device)
// =========================================================================

pub fn midi_out_get_num_devs() -> u32 {
    1
}

pub fn midi_out_get_dev_caps_w(_device_id: u32, caps: &mut MidiOutCaps) -> u32 {
    *caps = MidiOutCaps::soft_synth();
    MMSYSERR_NOERROR
}

pub fn midi_out_open(
    system: &mut WinmmSystem,
    _device_id: u32,
    _callback: u64,
    _instance: u64,
    _flags: u32,
) -> Result<u32, u32> {
    let handle = system.next_midi_out_handle;
    system.next_midi_out_handle += 1;
    system.midi_out_devices.push(MidiOutDevice {
        id: handle,
        open: true,
    });
    Ok(handle)
}

pub fn midi_out_close(system: &mut WinmmSystem, handle: u32) -> u32 {
    if let Some(dev) = system.midi_out_devices.iter_mut().find(|d| d.id == handle) {
        dev.open = false;
    } else {
        return MMSYSERR_INVALHANDLE;
    }
    system.midi_out_devices.retain(|d| d.open);
    MMSYSERR_NOERROR
}

pub fn midi_out_short_msg(system: &WinmmSystem, handle: u32, _msg: u32) -> u32 {
    if system.midi_out_devices.iter().any(|d| d.id == handle) {
        MMSYSERR_NOERROR
    } else {
        MMSYSERR_INVALHANDLE
    }
}

pub fn midi_out_long_msg(system: &WinmmSystem, handle: u32, _data: &[u8]) -> u32 {
    if system.midi_out_devices.iter().any(|d| d.id == handle) {
        MMSYSERR_NOERROR
    } else {
        MMSYSERR_INVALHANDLE
    }
}

pub fn midi_out_reset(system: &WinmmSystem, handle: u32) -> u32 {
    if system.midi_out_devices.iter().any(|d| d.id == handle) {
        MMSYSERR_NOERROR
    } else {
        MMSYSERR_INVALHANDLE
    }
}

pub fn midi_out_set_volume(system: &WinmmSystem, handle: u32, _volume: u32) -> u32 {
    if system.midi_out_devices.iter().any(|d| d.id == handle) {
        MMSYSERR_NOERROR
    } else {
        MMSYSERR_INVALHANDLE
    }
}

pub fn midi_out_get_volume(system: &WinmmSystem, handle: u32, volume: &mut u32) -> u32 {
    if system.midi_out_devices.iter().any(|d| d.id == handle) {
        *volume = 0xFFFFFFFF;
        MMSYSERR_NOERROR
    } else {
        MMSYSERR_INVALHANDLE
    }
}

// =========================================================================
// MIDI Input API (stubs)
// =========================================================================

pub fn midi_in_get_num_devs() -> u32 {
    0
}

// =========================================================================
// Joystick API
// =========================================================================

pub fn joy_get_num_devs() -> u32 {
    2
}

pub fn joy_get_dev_caps_w(joy_id: u32, caps: &mut JoyCaps) -> u32 {
    if joy_id > MAX_JOYSTICK_ID {
        return JOYERR_PARMS;
    }
    *caps = JoyCaps::default_gamepad();
    JOYERR_NOERROR
}

pub fn joy_get_dev_caps_a(joy_id: u32, caps: &mut JoyCaps) -> u32 {
    joy_get_dev_caps_w(joy_id, caps)
}

pub fn joy_get_pos_ex(joy_id: u32, info: &mut JoyInfoEx) -> u32 {
    if joy_id > MAX_JOYSTICK_ID {
        return JOYERR_PARMS;
    }

    info.size = core::mem::size_of::<JoyInfoEx>() as u32;
    info.x_pos = 32768;
    info.y_pos = 32768;
    info.z_pos = 0;
    info.r_pos = 32768;
    info.u_pos = 32768;
    info.v_pos = 0;
    info.buttons = 0;
    info.button_number = 0;
    info.pov = 0xFFFF;

    JOYERR_NOERROR
}

pub fn joy_get_pos(joy_id: u32) -> Result<(u32, u32, u32, u32), u32> {
    if joy_id > MAX_JOYSTICK_ID {
        return Err(JOYERR_PARMS);
    }
    Ok((32768, 32768, 0, 0))
}

pub fn joy_get_threshold(joy_id: u32, threshold: &mut u32) -> u32 {
    if joy_id > MAX_JOYSTICK_ID {
        return JOYERR_PARMS;
    }
    *threshold = 0;
    JOYERR_NOERROR
}

pub fn joy_set_threshold(joy_id: u32, _threshold: u32) -> u32 {
    if joy_id > MAX_JOYSTICK_ID {
        return JOYERR_PARMS;
    }
    JOYERR_NOERROR
}

pub fn joy_set_capture(_hwnd: u64, joy_id: u32, _period: u32, _changed: bool) -> u32 {
    if joy_id > MAX_JOYSTICK_ID {
        return JOYERR_PARMS;
    }
    JOYERR_NOERROR
}

pub fn joy_release_capture(joy_id: u32) -> u32 {
    if joy_id > MAX_JOYSTICK_ID {
        return JOYERR_PARMS;
    }
    JOYERR_NOERROR
}

// =========================================================================
// Miscellaneous multimedia functions
// =========================================================================

pub fn mci_send_string_w(
    _ctx: &mut CompatContext,
    _command: &str,
    return_string: &mut String,
) -> u32 {
    return_string.clear();
    MMSYSERR_NOERROR
}

pub fn mci_get_error_string_w(error: u32, text: &mut String) -> bool {
    text.clear();
    match error {
        0 => {
            text.push_str("No error");
            true
        }
        _ => {
            text.push_str("Unknown MCI error");
            true
        }
    }
}

pub fn play_sound_w(_sound: Option<&str>, _module: u64, _flags: u32) -> bool {
    true
}

pub fn snd_play_sound_w(_sound: Option<&str>, _flags: u32) -> bool {
    true
}

pub fn mmio_string_to_fourcc(s: &str) -> u32 {
    let bytes = s.as_bytes();
    let b = |i: usize| -> u32 {
        if i < bytes.len() {
            bytes[i] as u32
        } else {
            b' ' as u32
        }
    };
    b(0) | (b(1) << 8) | (b(2) << 16) | (b(3) << 24)
}

pub fn aux_get_num_devs() -> u32 {
    0
}

pub fn mixer_get_num_devs() -> u32 {
    1
}

pub fn mixer_open(
    _device_id: u32,
    _callback: u64,
    _instance: u64,
    _flags: u32,
) -> Result<u64, u32> {
    Ok(0xA0D10001)
}

pub fn mixer_close(_handle: u64) -> u32 {
    MMSYSERR_NOERROR
}
