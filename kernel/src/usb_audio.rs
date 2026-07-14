//! USB Audio Class (UAC) driver — playback + capture over isochronous endpoints.
//!
//! Concept §RaeAudio: a clean low-latency audio engine, none of the legacy
//! desktop-audio-daemon mess. USB DACs / headphone
//! amps are the common external-audio path on a gaming desktop, so RaeAudio
//! treats UAC as a first-class output device alongside the in-kernel HDA path
//! (`audio.rs`). This module owns the class-specific descriptor parse + stream
//! configuration; the live isochronous transfers ride the xHCI driver
//! (`xhci.rs`) once a device is claimed.
//!
//! MasterChecklist Phase 7: "USB Audio Class playback + capture".
//!
//! Scope today: UAC1 + UAC2 Audio Control / Audio Streaming descriptor parsing,
//! format-type-I (PCM) selection, and isochronous-endpoint configuration. The
//! parse is exercised by `run_boot_smoketest` against a synthetic descriptor so
//! the logic is QEMU-provable; real device streaming is wired when an external
//! UAC DAC is enumerated on hardware.

use crate::audio::{AudioStreamFormat, StreamDirection};
use alloc::vec::Vec;

// ─── USB Audio Class constants (USB Audio Device Class spec 1.0 / 2.0) ────────

/// bInterfaceClass = AUDIO.
pub const USB_CLASS_AUDIO: u8 = 0x01;
/// bInterfaceSubClass values.
pub const SUBCLASS_AUDIOCONTROL: u8 = 0x01;
pub const SUBCLASS_AUDIOSTREAMING: u8 = 0x02;
/// Class-specific descriptor type (CS_INTERFACE / CS_ENDPOINT).
pub const CS_INTERFACE: u8 = 0x24;
pub const CS_ENDPOINT: u8 = 0x25;
/// AudioControl interface descriptor subtypes.
pub const AC_HEADER: u8 = 0x01;
pub const AC_INPUT_TERMINAL: u8 = 0x02;
pub const AC_OUTPUT_TERMINAL: u8 = 0x03;
pub const AC_FEATURE_UNIT: u8 = 0x06;
/// AudioStreaming interface descriptor subtypes.
pub const AS_GENERAL: u8 = 0x01;
pub const AS_FORMAT_TYPE: u8 = 0x02;
/// Format Type I = PCM.
pub const FORMAT_TYPE_I: u8 = 0x01;
/// Terminal types we care about (USB Audio Terminal Types spec).
pub const TT_USB_STREAMING: u16 = 0x0101;
pub const TT_SPEAKER: u16 = 0x0301;
pub const TT_HEADPHONES: u16 = 0x0302;
pub const TT_MICROPHONE: u16 = 0x0201;

/// One PCM format a UAC streaming interface advertises (Format Type I).
#[derive(Debug, Clone, Copy)]
pub struct UacFormat {
    pub channels: u8,
    pub bit_resolution: u8,
    pub sample_rates: [u32; 8],
    pub rate_count: u8,
}

impl UacFormat {
    /// Pick the stream format closest to `preferred`, defaulting to the device's
    /// highest advertised rate at the requested depth.
    pub fn best_match(&self, preferred: &AudioStreamFormat) -> AudioStreamFormat {
        let mut rate = 0u32;
        for i in 0..self.rate_count as usize {
            let r = self.sample_rates[i];
            if r == preferred.sample_rate {
                rate = r;
                break;
            }
            if r > rate {
                rate = r;
            }
        }
        if rate == 0 {
            rate = preferred.sample_rate;
        }
        AudioStreamFormat {
            sample_rate: rate,
            bits_per_sample: if self.bit_resolution != 0 {
                self.bit_resolution
            } else {
                preferred.bits_per_sample
            },
            channels: if self.channels != 0 {
                self.channels
            } else {
                preferred.channels
            },
        }
    }
}

/// A discovered UAC streaming endpoint (one direction of one DAC/ADC).
#[derive(Debug, Clone)]
pub struct UacStream {
    pub interface: u8,
    pub alt_setting: u8,
    pub endpoint_addr: u8,
    pub direction: StreamDirection,
    pub terminal_type: u16,
    pub formats: Vec<UacFormat>,
    /// Bytes per isochronous service interval (1 ms frame at the chosen format).
    pub bytes_per_interval: u16,
}

impl UacStream {
    pub fn is_playback(&self) -> bool {
        matches!(self.direction, StreamDirection::Playback)
    }
}

/// A fully-parsed UAC device: control terminals + streaming interfaces.
#[derive(Debug, Default)]
pub struct UacDevice {
    pub uac_version: u16, // 0x0100 = UAC1, 0x0200 = UAC2
    pub streams: Vec<UacStream>,
}

impl UacDevice {
    pub fn playback_stream(&self) -> Option<&UacStream> {
        self.streams.iter().find(|s| s.is_playback())
    }
    pub fn capture_stream(&self) -> Option<&UacStream> {
        self.streams.iter().find(|s| !s.is_playback())
    }
}

/// Walk a USB configuration descriptor blob and extract every UAC streaming
/// interface. `config` is the raw bytes returned by GET_DESCRIPTOR(CONFIG).
///
/// The parse is defensive: every record is bounds-checked against `bLength`,
/// and a malformed/truncated descriptor yields whatever was validly parsed so
/// far rather than panicking — a hostile device cannot crash the kernel here.
pub fn parse_config(config: &[u8]) -> UacDevice {
    let mut dev = UacDevice::default();
    let mut i = 0usize;
    // Streaming-interface assembly state.
    let mut cur_iface: u8 = 0;
    let mut cur_alt: u8 = 0;
    let mut cur_dir = StreamDirection::Playback;
    let mut cur_terminal = 0u16;
    let mut cur_formats: Vec<UacFormat> = Vec::new();
    let mut cur_ep: u8 = 0;
    let mut in_streaming = false;
    let mut in_control = false;

    let flush = |dev: &mut UacDevice,
                 iface: u8,
                 alt: u8,
                 ep: u8,
                 dir: StreamDirection,
                 term: u16,
                 formats: &mut Vec<UacFormat>| {
        if ep != 0 && !formats.is_empty() {
            let bpi = formats
                .first()
                .map(|f| {
                    let bytes_per_frame = (f.bit_resolution as u32 / 8) * f.channels.max(1) as u32;
                    let rate = (0..f.rate_count as usize)
                        .map(|k| f.sample_rates[k])
                        .max()
                        .unwrap_or(48000);
                    ((bytes_per_frame * rate) / 1000) as u16
                })
                .unwrap_or(0);
            dev.streams.push(UacStream {
                interface: iface,
                alt_setting: alt,
                endpoint_addr: ep,
                direction: dir,
                terminal_type: term,
                formats: core::mem::take(formats),
                bytes_per_interval: bpi,
            });
        } else {
            formats.clear();
        }
    };

    while i + 2 <= config.len() {
        let blen = config[i] as usize;
        let btype = config[i + 1];
        if blen < 2 || i + blen > config.len() {
            break;
        }
        let rec = &config[i..i + blen];

        match btype {
            // INTERFACE descriptor (0x04): a new alt setting boundary.
            0x04 if blen >= 9 => {
                // Close any in-progress streaming interface.
                if in_streaming {
                    flush(
                        &mut dev,
                        cur_iface,
                        cur_alt,
                        cur_ep,
                        cur_dir,
                        cur_terminal,
                        &mut cur_formats,
                    );
                    cur_ep = 0;
                }
                let class = rec[5];
                let subclass = rec[6];
                cur_iface = rec[2];
                cur_alt = rec[3];
                in_streaming = class == USB_CLASS_AUDIO && subclass == SUBCLASS_AUDIOSTREAMING;
                in_control = class == USB_CLASS_AUDIO && subclass == SUBCLASS_AUDIOCONTROL;
            }
            // Class-specific AC header carries the UAC version (bcdADC). Gated to
            // AudioControl interfaces — AS_GENERAL shares subtype value 0x01 with
            // AC_HEADER, so without the context guard an AudioStreaming general
            // descriptor would clobber the parsed version.
            CS_INTERFACE if in_control && blen >= 4 && rec[2] == AC_HEADER => {
                dev.uac_version = u16::from_le_bytes([rec[3], rec.get(4).copied().unwrap_or(0)]);
            }
            // OUTPUT terminal → playback sink; INPUT terminal → capture source.
            CS_INTERFACE if in_control && blen >= 6 && rec[2] == AC_OUTPUT_TERMINAL => {
                cur_terminal = u16::from_le_bytes([rec[4], rec[5]]);
                if cur_terminal == TT_SPEAKER || cur_terminal == TT_HEADPHONES {
                    cur_dir = StreamDirection::Playback;
                }
            }
            CS_INTERFACE if in_control && blen >= 6 && rec[2] == AC_INPUT_TERMINAL => {
                let tt = u16::from_le_bytes([rec[4], rec[5]]);
                if tt == TT_MICROPHONE {
                    cur_terminal = tt;
                    cur_dir = StreamDirection::Capture;
                }
            }
            // AS_FORMAT_TYPE (Format Type I = PCM): channels, depth, rates.
            CS_INTERFACE if blen >= 8 && rec[2] == AS_FORMAT_TYPE && rec[3] == FORMAT_TYPE_I => {
                let channels = rec[4];
                let bit_resolution = rec[6];
                // UAC1 discrete-rate table: rec[7] = nrChannels of rates, then triplets.
                let mut sample_rates = [0u32; 8];
                let mut rate_count = 0u8;
                let n = rec[7] as usize;
                let mut off = 8;
                while rate_count < 8 && off + 3 <= blen && (rate_count as usize) < n {
                    let r = (rec[off] as u32)
                        | ((rec[off + 1] as u32) << 8)
                        | ((rec[off + 2] as u32) << 16);
                    sample_rates[rate_count as usize] = r;
                    rate_count += 1;
                    off += 3;
                }
                if rate_count == 0 {
                    // Continuous range or UAC2: default to 48 kHz.
                    sample_rates[0] = 48000;
                    rate_count = 1;
                }
                cur_formats.push(UacFormat {
                    channels,
                    bit_resolution,
                    sample_rates,
                    rate_count,
                });
            }
            // ENDPOINT descriptor (0x05): isochronous data endpoint for this alt.
            0x05 if blen >= 7 && in_streaming => {
                let addr = rec[2];
                let attrs = rec[3];
                // bits 1:0 == 01 => isochronous.
                if attrs & 0x03 == 0x01 {
                    cur_ep = addr;
                    cur_dir = if addr & 0x80 != 0 {
                        StreamDirection::Capture
                    } else {
                        StreamDirection::Playback
                    };
                }
            }
            _ => {}
        }
        i += blen;
    }

    if in_streaming {
        flush(
            &mut dev,
            cur_iface,
            cur_alt,
            cur_ep,
            cur_dir,
            cur_terminal,
            &mut cur_formats,
        );
    }
    dev
}

/// Build a synthetic UAC1 headphone-DAC config descriptor for the smoketest:
/// AudioControl header (UAC 1.0) + Output Terminal (headphones) + streaming
/// interface alt-1 with a 48 kHz / 16-bit / stereo PCM format on an OUT isoch
/// endpoint. Mirrors what a real USB DAC reports.
fn synthetic_dac_config() -> Vec<u8> {
    let mut c = Vec::new();
    // Configuration descriptor (9 bytes) — only the length/type matter to us.
    c.extend_from_slice(&[9, 0x02, 0, 0, 2, 1, 0, 0x80, 50]);
    // AudioControl interface (class 1 / subclass 1).
    c.extend_from_slice(&[
        9,
        0x04,
        0,
        0,
        0,
        USB_CLASS_AUDIO,
        SUBCLASS_AUDIOCONTROL,
        0,
        0,
    ]);
    // CS AC header, bcdADC = 0x0100 (UAC1).
    c.extend_from_slice(&[9, CS_INTERFACE, AC_HEADER, 0x00, 0x01, 30, 0, 1, 1]);
    // CS AC output terminal, wTerminalType = headphones (0x0302).
    c.extend_from_slice(&[9, CS_INTERFACE, AC_OUTPUT_TERMINAL, 1, 0x02, 0x03, 1, 2, 0]);
    // AudioStreaming interface, alt 0 (zero-bandwidth).
    c.extend_from_slice(&[
        9,
        0x04,
        1,
        0,
        0,
        USB_CLASS_AUDIO,
        SUBCLASS_AUDIOSTREAMING,
        0,
        0,
    ]);
    // AudioStreaming interface, alt 1 (operational, 1 endpoint).
    c.extend_from_slice(&[
        9,
        0x04,
        1,
        1,
        1,
        USB_CLASS_AUDIO,
        SUBCLASS_AUDIOSTREAMING,
        0,
        0,
    ]);
    // CS AS general.
    c.extend_from_slice(&[7, CS_INTERFACE, AS_GENERAL, 1, 1, 0x01, 0x00]);
    // CS AS format type I: 2ch, 16-bit, one rate (48000).
    c.extend_from_slice(&[
        11,
        CS_INTERFACE,
        AS_FORMAT_TYPE,
        FORMAT_TYPE_I,
        2,  // channels
        2,  // subframe size
        16, // bit resolution
        1,  // one discrete rate
        0x80,
        0xBB,
        0x00, // 48000 = 0x00BB80
    ]);
    // Isochronous OUT endpoint 0x01.
    c.extend_from_slice(&[9, 0x05, 0x01, 0x01, 0xC0, 0x00, 0x01, 0, 0]);
    c
}

pub fn init() {
    crate::serial_println!("[usb-audio] UAC1/UAC2 class driver ready (isoch playback+capture)");
}

pub fn run_boot_smoketest() {
    let cfg = synthetic_dac_config();
    let dev = parse_config(&cfg);
    let parsed_version = dev.uac_version == 0x0100;
    let pb = dev.playback_stream();
    let playback_found = pb.is_some();
    let (fmt_ok, ep_ok) = match pb {
        Some(s) => {
            let want = AudioStreamFormat::DVD_QUALITY;
            let chosen = s.formats.first().map(|f| f.best_match(&want));
            (
                chosen
                    .map(|f| f.sample_rate == 48000 && f.channels == 2)
                    .unwrap_or(false),
                s.endpoint_addr == 0x01 && s.is_playback(),
            )
        }
        None => (false, false),
    };
    let pass = parsed_version && playback_found && fmt_ok && ep_ok;
    crate::serial_println!(
        "[usb-audio] run_boot_smoketest: uac1={} playback={} fmt48k_stereo={} isoch_ep={} -> {}",
        parsed_version,
        playback_found,
        fmt_ok,
        ep_ok,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// `/proc/raeen/usb_audio` text.
pub fn dump_text() -> alloc::string::String {
    use alloc::string::String;
    let mut out = String::from("# RaeenOS USB Audio Class (UAC1/UAC2)\n");
    out.push_str("class: 0x01 (AUDIO)  subclasses: AC=0x01 AS=0x02\n");
    out.push_str("formats: PCM Format-Type-I  transport: isochronous\n");
    out.push_str("directions: playback (speaker/headphones) + capture (microphone)\n");
    out
}
