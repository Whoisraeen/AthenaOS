//! HDA pin "configuration default" decoding (MasterChecklist L1375 — record
//! from the mic). Every pin-complex widget on an HDA codec reports a 32-bit
//! *configuration default* (HDA spec §7.3.3.31) describing what's physically
//! wired to it: jack vs fixed, where on the chassis, and which device class
//! (speaker / line-in / **microphone** / …). The capture path needs this to
//! find the mic input pin to route to an ADC.
//!
//! Host-KAT-able pure logic — the in-kernel HDA driver (`kernel/src/audio.rs`)
//! does the same `(config_default >> 20) & 0xF` classification inline; this is
//! the testable extraction it can delegate to (same shape as `raehid` for HID).
//!
//! Validated against the live **Realtek ALC269VC** on the Athena (read over SSH
//! from `/proc/asound`, 2026-06-28): mic pin node 0x18 default = `0x04a19020`.

/// Port connectivity (config-default bits 31:30).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinConnectivity {
    /// A physical jack the user can plug into.
    Jack,
    /// No physical connection (pin unused).
    None,
    /// Fixed/integrated device (e.g. built-in speaker or mic).
    Fixed,
    /// Both a jack and a fixed device.
    Both,
}

/// Default device class (config-default bits 23:20) per the HDA spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdaDevice {
    LineOut,
    Speaker,
    HpOut,
    Cd,
    SpdifOut,
    DigOtherOut,
    ModemLine,
    ModemHandset,
    LineIn,
    Aux,
    MicIn,
    Telephony,
    SpdifIn,
    DigOtherIn,
    Reserved,
    Other,
}

impl HdaDevice {
    fn from_u4(v: u8) -> HdaDevice {
        match v & 0xF {
            0x0 => HdaDevice::LineOut,
            0x1 => HdaDevice::Speaker,
            0x2 => HdaDevice::HpOut,
            0x3 => HdaDevice::Cd,
            0x4 => HdaDevice::SpdifOut,
            0x5 => HdaDevice::DigOtherOut,
            0x6 => HdaDevice::ModemLine,
            0x7 => HdaDevice::ModemHandset,
            0x8 => HdaDevice::LineIn,
            0x9 => HdaDevice::Aux,
            0xA => HdaDevice::MicIn,
            0xB => HdaDevice::Telephony,
            0xC => HdaDevice::SpdifIn,
            0xD => HdaDevice::DigOtherIn,
            0xE => HdaDevice::Reserved,
            _ => HdaDevice::Other,
        }
    }
}

/// A decoded HDA pin configuration default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinConfig {
    pub connectivity: PinConnectivity,
    /// Bits 29:24 — gross location (5:4) | geometric location (3:0).
    pub location: u8,
    pub device: HdaDevice,
    /// Bits 19:16 — connection type (1/8", RCA, optical, …).
    pub connection_type: u8,
}

impl PinConfig {
    /// Decode the 32-bit configuration default a pin widget reports.
    pub fn decode(config_default: u32) -> Self {
        let connectivity = match (config_default >> 30) & 0x3 {
            0 => PinConnectivity::Jack,
            1 => PinConnectivity::None,
            2 => PinConnectivity::Fixed,
            _ => PinConnectivity::Both,
        };
        Self {
            connectivity,
            location: ((config_default >> 24) & 0x3F) as u8,
            device: HdaDevice::from_u4(((config_default >> 20) & 0xF) as u8),
            connection_type: ((config_default >> 16) & 0xF) as u8,
        }
    }

    /// A microphone input — the pin the L1375 capture path routes to an ADC.
    pub fn is_mic(&self) -> bool {
        self.device == HdaDevice::MicIn
    }

    /// Any capture/input device (mic, line-in, or a digital input).
    pub fn is_input(&self) -> bool {
        matches!(
            self.device,
            HdaDevice::LineIn | HdaDevice::MicIn | HdaDevice::SpdifIn | HdaDevice::DigOtherIn
        )
    }

    /// Something is physically present (a jack or a fixed device), not "None".
    pub fn is_connected(&self) -> bool {
        self.connectivity != PinConnectivity::None
    }

    /// Gross location is "External" (gross bits 5:4 == 0b00) — an external jack
    /// vs an internal/built-in device.
    pub fn is_external(&self) -> bool {
        (self.location >> 4) & 0x3 == 0
    }
}

#[cfg(test)]
mod hda_pin_kat {
    use super::*;

    #[test]
    fn real_alc269_mic_pin_decodes_as_external_mic_input() {
        // Ground truth: Athena Realtek ALC269VC, mic pin node 0x18, config
        // default 0x04a19020 (read over SSH from /proc/asound, 2026-06-28;
        // lsusb/proc reported "Mic at Ext Right").
        let pc = PinConfig::decode(0x04a1_9020);
        assert_eq!(pc.device, HdaDevice::MicIn);
        assert!(pc.is_mic());
        assert!(pc.is_input());
        assert_eq!(pc.connectivity, PinConnectivity::Jack);
        assert!(pc.is_connected());
        assert_eq!(pc.location, 0x04); // External (gross 0) / Right (geom 4)
        assert!(pc.is_external());
        assert_eq!(pc.connection_type, 0x1); // 1/8" stereo/mono
    }

    #[test]
    fn classifies_inputs_outputs_and_unused() {
        // Line In (device 0x8) is an input but not a mic.
        let line_in = PinConfig::decode(0x0080_0000);
        assert_eq!(line_in.device, HdaDevice::LineIn);
        assert!(line_in.is_input());
        assert!(!line_in.is_mic());
        // Speaker (device 0x1) is not an input.
        let spk = PinConfig::decode(0x0010_0000);
        assert_eq!(spk.device, HdaDevice::Speaker);
        assert!(!spk.is_input());
        // Connectivity "None" (bits 31:30 = 0b01) -> not connected.
        let unused = PinConfig::decode(0x4000_0000);
        assert_eq!(unused.connectivity, PinConnectivity::None);
        assert!(!unused.is_connected());
        // A built-in/fixed mic (connectivity Fixed, internal location) is still
        // a mic input — just not external. (The Athena's mic is the external
        // jack above; this covers the laptop/all-in-one built-in-mic case.)
        let int_mic = PinConfig::decode(0x90A0_0000);
        assert_eq!(int_mic.device, HdaDevice::MicIn);
        assert!(int_mic.is_mic());
        assert_eq!(int_mic.connectivity, PinConnectivity::Fixed);
        assert!(int_mic.is_connected());
        assert!(!int_mic.is_external());
    }
}
