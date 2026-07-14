//! Firmware Interface Layer for AthenaOS.
//!
//! Platform firmware abstraction covering DMI/SMBIOS queries, CMOS/RTC,
//! PIT 8254, PIC 8259A, PS/2 8042, PC speaker, BIOS data area,
//! system management, hardware detection, and platform quirks.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

// ---------------------------------------------------------------------------
// DMI / SMBIOS queries
// ---------------------------------------------------------------------------

pub static DMI_INFO: Mutex<Option<DmiInfo>> = Mutex::new(None);

pub fn init_dmi(info: DmiInfo) {
    *DMI_INFO.lock() = Some(info);
}

pub fn get_dmi() -> DmiInfo {
    DMI_INFO.lock().clone().unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct DmiInfo {
    pub system_manufacturer: String,
    pub product_name: String,
    pub serial_number: String,
    pub uuid: [u8; 16],
    pub chassis_type: ChassisType,
    pub bios_vendor: String,
    pub bios_version: String,
    pub bios_date: String,
    pub board_name: String,
    pub board_version: String,
    pub board_serial: String,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChassisType {
    Other = 0x01,
    Unknown = 0x02,
    Desktop = 0x03,
    LowProfileDesktop = 0x04,
    PizzaBox = 0x05,
    MiniTower = 0x06,
    Tower = 0x07,
    Portable = 0x08,
    Laptop = 0x09,
    Notebook = 0x0A,
    HandHeld = 0x0B,
    DockingStation = 0x0C,
    AllInOne = 0x0D,
    SubNotebook = 0x0E,
    RackMount = 0x11,
    SealedCasePC = 0x23,
    Tablet = 0x1E,
    Convertible = 0x1F,
    Detachable = 0x20,
    IoTGateway = 0x21,
    MiniPC = 0x22,
}

impl ChassisType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x03 => Self::Desktop,
            0x04 => Self::LowProfileDesktop,
            0x05 => Self::PizzaBox,
            0x06 => Self::MiniTower,
            0x07 => Self::Tower,
            0x08 => Self::Portable,
            0x09 => Self::Laptop,
            0x0A => Self::Notebook,
            0x0B => Self::HandHeld,
            0x0C => Self::DockingStation,
            0x0D => Self::AllInOne,
            0x0E => Self::SubNotebook,
            0x11 => Self::RackMount,
            0x1E => Self::Tablet,
            0x1F => Self::Convertible,
            0x20 => Self::Detachable,
            0x21 => Self::IoTGateway,
            0x22 => Self::MiniPC,
            0x23 => Self::SealedCasePC,
            _ => Self::Unknown,
        }
    }

    pub fn is_portable(self) -> bool {
        matches!(
            self,
            Self::Portable
                | Self::Laptop
                | Self::Notebook
                | Self::HandHeld
                | Self::SubNotebook
                | Self::Tablet
                | Self::Convertible
                | Self::Detachable
        )
    }
}

impl Default for DmiInfo {
    fn default() -> Self {
        Self {
            system_manufacturer: String::from("AthenaOS Virtual Machine"),
            product_name: String::from("AthenaOS QEMU"),
            serial_number: String::from("000000000000"),
            uuid: [0u8; 16],
            chassis_type: ChassisType::Desktop,
            bios_vendor: String::from("AthenaOS BIOS"),
            bios_version: String::from("0.1.0"),
            bios_date: String::from("05/25/2026"),
            board_name: String::from("AthenaOS Mainboard"),
            board_version: String::from("1.0"),
            board_serial: String::from("RAEEN-000001"),
        }
    }
}

impl DmiInfo {
    pub fn is_virtual_machine(&self) -> bool {
        let vm_vendors = [
            "QEMU",
            "KVM",
            "VMware",
            "VirtualBox",
            "Hyper-V",
            "Xen",
            "Bochs",
            "Parallels",
        ];
        for vendor in &vm_vendors {
            if self.system_manufacturer.contains(vendor)
                || self.product_name.contains(vendor)
                || self.bios_vendor.contains(vendor)
            {
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// CMOS / RTC
// ---------------------------------------------------------------------------

const CMOS_ADDR_PORT: u16 = 0x70;
const CMOS_DATA_PORT: u16 = 0x71;

const RTC_SECONDS: u8 = 0x00;
const RTC_MINUTES: u8 = 0x02;
const RTC_HOURS: u8 = 0x04;
const RTC_DAY_OF_WEEK: u8 = 0x06;
const RTC_DAY: u8 = 0x07;
const RTC_MONTH: u8 = 0x08;
const RTC_YEAR: u8 = 0x09;
const RTC_CENTURY: u8 = 0x32;
const RTC_STATUS_A: u8 = 0x0A;
const RTC_STATUS_B: u8 = 0x0B;
const RTC_STATUS_C: u8 = 0x0C;
const RTC_STATUS_D: u8 = 0x0D;

const RTC_ALARM_SECONDS: u8 = 0x01;
const RTC_ALARM_MINUTES: u8 = 0x03;
const RTC_ALARM_HOURS: u8 = 0x05;

const STATUS_B_24HR: u8 = 0x02;
const STATUS_B_BINARY: u8 = 0x04;
const STATUS_B_AIE: u8 = 0x20;
const STATUS_B_UIE: u8 = 0x10;
const STATUS_B_PIE: u8 = 0x40;
const STATUS_A_UIP: u8 = 0x80;

pub struct CmosRtc {
    storage: [u8; 256],
}

impl CmosRtc {
    pub fn new() -> Self {
        let mut storage = [0u8; 256];
        storage[RTC_STATUS_B as usize] = STATUS_B_24HR | STATUS_B_BINARY;
        storage[RTC_STATUS_D as usize] = 0x80; // valid RAM and time
        Self { storage }
    }

    pub fn read_cmos(&self, address: u8) -> u8 {
        self.storage[address as usize]
    }

    pub fn write_cmos(&mut self, address: u8, value: u8) {
        self.storage[address as usize] = value;
    }

    fn is_bcd_mode(&self) -> bool {
        self.storage[RTC_STATUS_B as usize] & STATUS_B_BINARY == 0
    }

    fn bcd_to_binary(bcd: u8) -> u8 {
        (bcd & 0x0F) + ((bcd >> 4) * 10)
    }

    fn binary_to_bcd(bin: u8) -> u8 {
        ((bin / 10) << 4) | (bin % 10)
    }

    fn read_register(&self, reg: u8) -> u8 {
        let raw = self.storage[reg as usize];
        if self.is_bcd_mode() {
            Self::bcd_to_binary(raw)
        } else {
            raw
        }
    }

    fn write_register(&mut self, reg: u8, value: u8) {
        let encoded = if self.is_bcd_mode() {
            Self::binary_to_bcd(value)
        } else {
            value
        };
        self.storage[reg as usize] = encoded;
    }

    pub fn is_update_in_progress(&self) -> bool {
        self.storage[RTC_STATUS_A as usize] & STATUS_A_UIP != 0
    }

    pub fn read_time(&self) -> RtcTime {
        RtcTime {
            seconds: self.read_register(RTC_SECONDS),
            minutes: self.read_register(RTC_MINUTES),
            hours: self.read_register(RTC_HOURS),
            day: self.read_register(RTC_DAY),
            month: self.read_register(RTC_MONTH),
            year: self.read_register(RTC_YEAR) as u16
                + (self.read_register(RTC_CENTURY) as u16) * 100,
        }
    }

    pub fn write_time(&mut self, time: &RtcTime) {
        self.write_register(RTC_SECONDS, time.seconds);
        self.write_register(RTC_MINUTES, time.minutes);
        self.write_register(RTC_HOURS, time.hours);
        self.write_register(RTC_DAY, time.day);
        self.write_register(RTC_MONTH, time.month);
        self.write_register(RTC_YEAR, (time.year % 100) as u8);
        self.write_register(RTC_CENTURY, (time.year / 100) as u8);
    }

    pub fn set_alarm(&mut self, hours: u8, minutes: u8, seconds: u8) {
        self.write_register(RTC_ALARM_HOURS, hours);
        self.write_register(RTC_ALARM_MINUTES, minutes);
        self.write_register(RTC_ALARM_SECONDS, seconds);
        self.storage[RTC_STATUS_B as usize] |= STATUS_B_AIE;
    }

    pub fn clear_alarm(&mut self) {
        self.storage[RTC_STATUS_B as usize] &= !STATUS_B_AIE;
    }

    pub fn alarm_fired(&self) -> bool {
        self.storage[RTC_STATUS_C as usize] & STATUS_B_AIE != 0
    }

    pub fn acknowledge_interrupt(&mut self) -> u8 {
        let status_c = self.storage[RTC_STATUS_C as usize];
        self.storage[RTC_STATUS_C as usize] = 0;
        status_c
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RtcTime {
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub day: u8,
    pub month: u8,
    pub year: u16,
}

impl RtcTime {
    pub fn to_unix_timestamp(&self) -> u64 {
        let y = self.year as u64;
        let m = self.month as u64;
        let d = self.day as u64;
        let mut days = 0u64;
        for yr in 1970..y {
            days += if yr % 4 == 0 && (yr % 100 != 0 || yr % 400 == 0) {
                366
            } else {
                365
            };
        }
        let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        for mo in 1..m {
            days += month_days[mo as usize] as u64;
            if mo == 2 && y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                days += 1;
            }
        }
        days += d - 1;
        days * 86400 + self.hours as u64 * 3600 + self.minutes as u64 * 60 + self.seconds as u64
    }
}

// ---------------------------------------------------------------------------
// CMOS Extended (non-volatile storage)
// ---------------------------------------------------------------------------

pub struct CmosExtended {
    storage: [u8; 256],
    checksum: u16,
}

impl CmosExtended {
    pub fn new() -> Self {
        let mut ext = Self {
            storage: [0u8; 256],
            checksum: 0,
        };
        ext.set_boot_device_order(&[
            BootDevice::Disk,
            BootDevice::Cdrom,
            BootDevice::Network,
            BootDevice::Usb,
        ]);
        ext.update_checksum();
        ext
    }

    pub fn read(&self, offset: u8) -> u8 {
        self.storage[offset as usize]
    }

    pub fn write(&mut self, offset: u8, value: u8) {
        self.storage[offset as usize] = value;
        self.update_checksum();
    }

    fn update_checksum(&mut self) {
        let mut sum: u16 = 0;
        for i in 16..126 {
            sum = sum.wrapping_add(self.storage[i] as u16);
        }
        self.checksum = sum;
        self.storage[126] = (sum >> 8) as u8;
        self.storage[127] = sum as u8;
    }

    pub fn verify_checksum(&self) -> bool {
        let mut sum: u16 = 0;
        for i in 16..126 {
            sum = sum.wrapping_add(self.storage[i] as u16);
        }
        sum == self.checksum
    }

    pub fn get_boot_device_order(&self) -> Vec<BootDevice> {
        let mut order = Vec::new();
        for i in 0..4 {
            let dev = self.storage[16 + i];
            order.push(BootDevice::from_u8(dev));
        }
        order
    }

    pub fn set_boot_device_order(&mut self, order: &[BootDevice]) {
        for (i, dev) in order.iter().enumerate().take(4) {
            self.storage[16 + i] = *dev as u8;
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootDevice {
    Disabled = 0x00,
    Floppy = 0x01,
    Disk = 0x02,
    Cdrom = 0x03,
    Network = 0x04,
    Usb = 0x05,
}

impl BootDevice {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x01 => Self::Floppy,
            0x02 => Self::Disk,
            0x03 => Self::Cdrom,
            0x04 => Self::Network,
            0x05 => Self::Usb,
            _ => Self::Disabled,
        }
    }
}

// ---------------------------------------------------------------------------
// PIT (Programmable Interval Timer) — Intel 8254
// ---------------------------------------------------------------------------

const PIT_CHANNEL0: u16 = 0x40;
const PIT_CHANNEL1: u16 = 0x41;
const PIT_CHANNEL2: u16 = 0x42;
const PIT_COMMAND: u16 = 0x43;

const PIT_BASE_FREQUENCY: u32 = 1_193_182; // 1.193182 MHz

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitMode {
    InterruptOnTerminalCount = 0,
    HardwareRetriggerable = 1,
    RateGenerator = 2,
    SquareWave = 3,
    SoftwareStrobe = 4,
    HardwareStrobe = 5,
}

impl PitMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::InterruptOnTerminalCount,
            1 => Self::HardwareRetriggerable,
            2 => Self::RateGenerator,
            3 => Self::SquareWave,
            4 => Self::SoftwareStrobe,
            5 => Self::HardwareStrobe,
            _ => Self::RateGenerator,
        }
    }
}

pub struct PitChannel {
    pub channel: u8,
    pub mode: PitMode,
    pub reload: u16,
    pub count: u16,
    pub latched: bool,
    pub latch_val: u16,
    pub access: PitAccess,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitAccess {
    LatchCount,
    LoByte,
    HiByte,
    LoHiByte,
}

impl PitChannel {
    pub fn new(channel: u8) -> Self {
        Self {
            channel,
            mode: PitMode::RateGenerator,
            reload: 0xFFFF,
            count: 0xFFFF,
            latched: false,
            latch_val: 0,
            access: PitAccess::LoHiByte,
            enabled: false,
        }
    }

    pub fn set_reload(&mut self, value: u16) {
        self.reload = if value == 0 { u16::MAX } else { value };
        self.count = self.reload;
    }

    pub fn frequency_hz(&self) -> u32 {
        if self.reload == 0 {
            return PIT_BASE_FREQUENCY;
        }
        PIT_BASE_FREQUENCY / self.reload as u32
    }

    pub fn period_us(&self) -> u32 {
        if self.reload == 0 {
            return 1;
        }
        (self.reload as u64 * 1_000_000 / PIT_BASE_FREQUENCY as u64) as u32
    }

    pub fn tick(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.count == 0 {
            self.count = self.reload;
            return true;
        }
        self.count -= 1;
        false
    }
}

pub struct Pit {
    pub channels: [PitChannel; 3],
}

impl Pit {
    pub fn new() -> Self {
        Self {
            channels: [PitChannel::new(0), PitChannel::new(1), PitChannel::new(2)],
        }
    }

    pub fn program_channel(&mut self, channel: u8, mode: PitMode, reload: u16) {
        if channel > 2 {
            return;
        }
        let ch = &mut self.channels[channel as usize];
        ch.mode = mode;
        ch.set_reload(reload);
        ch.enabled = true;
    }

    pub fn program_frequency(&mut self, channel: u8, freq_hz: u32) {
        if freq_hz == 0 || channel > 2 {
            return;
        }
        let divisor = PIT_BASE_FREQUENCY / freq_hz;
        let divisor = if divisor > 0xFFFF {
            0xFFFF
        } else if divisor == 0 {
            1
        } else {
            divisor as u16
        };
        self.program_channel(channel, PitMode::RateGenerator, divisor);
    }

    pub fn pit_delay_us(&self, microseconds: u32) -> u32 {
        let ticks = (microseconds as u64 * PIT_BASE_FREQUENCY as u64) / 1_000_000;
        ticks as u32
    }

    pub fn read_count(&self, channel: u8) -> u16 {
        if channel > 2 {
            return 0;
        }
        self.channels[channel as usize].count
    }
}

// ---------------------------------------------------------------------------
// PIC (Programmable Interrupt Controller) — Intel 8259A
// ---------------------------------------------------------------------------

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const ICW1_ICW4: u8 = 0x01;
const ICW1_SINGLE: u8 = 0x02;
const ICW1_INTERVAL4: u8 = 0x04;
const ICW1_LEVEL: u8 = 0x08;
const ICW1_INIT: u8 = 0x10;

const ICW4_8086: u8 = 0x01;
const ICW4_AUTO_EOI: u8 = 0x02;
const ICW4_BUF_SLAVE: u8 = 0x08;
const ICW4_BUF_MASTER: u8 = 0x0C;
const ICW4_SFNM: u8 = 0x10;

const OCW2_EOI: u8 = 0x20;
const OCW2_SPECIFIC_EOI: u8 = 0x60;
const OCW3_READ_IRR: u8 = 0x0A;
const OCW3_READ_ISR: u8 = 0x0B;

pub struct Pic8259 {
    pub master_offset: u8,
    pub slave_offset: u8,
    pub master_mask: u8,
    pub slave_mask: u8,
    pub master_icw4: u8,
    pub slave_icw4: u8,
    pub cascade_irq: u8,
    pub auto_eoi: bool,
    pub level_triggered: bool,
    pub initialized: bool,
}

impl Pic8259 {
    pub fn new() -> Self {
        Self {
            master_offset: 0x20,
            slave_offset: 0x28,
            master_mask: 0xFF,
            slave_mask: 0xFF,
            master_icw4: ICW4_8086,
            slave_icw4: ICW4_8086,
            cascade_irq: 2,
            auto_eoi: false,
            level_triggered: false,
            initialized: false,
        }
    }

    pub fn initialize(&mut self, master_offset: u8, slave_offset: u8) {
        self.master_offset = master_offset;
        self.slave_offset = slave_offset;

        let master_icw1 = ICW1_INIT | ICW1_ICW4 | if self.level_triggered { ICW1_LEVEL } else { 0 };
        let slave_icw1 = master_icw1;

        self.write_command(true, master_icw1);
        self.write_command(false, slave_icw1);

        self.write_data(true, master_offset);
        self.write_data(false, slave_offset);

        self.write_data(true, 1 << self.cascade_irq);
        self.write_data(false, self.cascade_irq);

        self.write_data(true, self.master_icw4);
        self.write_data(false, self.slave_icw4);

        self.write_data(true, self.master_mask);
        self.write_data(false, self.slave_mask);

        self.initialized = true;
    }

    pub fn mask_irq(&mut self, irq: u8) {
        if irq < 8 {
            self.master_mask |= 1 << irq;
        } else if irq < 16 {
            self.slave_mask |= 1 << (irq - 8);
        }
    }

    pub fn unmask_irq(&mut self, irq: u8) {
        if irq < 8 {
            self.master_mask &= !(1 << irq);
        } else if irq < 16 {
            self.slave_mask &= !(1 << (irq - 8));
            self.master_mask &= !(1 << self.cascade_irq);
        }
    }

    pub fn is_masked(&self, irq: u8) -> bool {
        if irq < 8 {
            self.master_mask & (1 << irq) != 0
        } else if irq < 16 {
            self.slave_mask & (1 << (irq - 8)) != 0
        } else {
            true
        }
    }

    pub fn send_eoi(&self, irq: u8) {
        if irq >= 8 {
            self.write_command(false, OCW2_EOI);
        }
        self.write_command(true, OCW2_EOI);
    }

    pub fn send_specific_eoi(&self, irq: u8) {
        if irq >= 8 {
            self.write_command(false, OCW2_SPECIFIC_EOI | (irq - 8));
        }
        self.write_command(true, OCW2_SPECIFIC_EOI | (irq.min(7)));
    }

    pub fn enable_auto_eoi(&mut self) {
        self.auto_eoi = true;
        self.master_icw4 |= ICW4_AUTO_EOI;
        self.slave_icw4 |= ICW4_AUTO_EOI;
    }

    pub fn set_level_triggered(&mut self) {
        self.level_triggered = true;
    }

    pub fn set_edge_triggered(&mut self) {
        self.level_triggered = false;
    }

    pub fn mask_all(&mut self) {
        self.master_mask = 0xFF;
        self.slave_mask = 0xFF;
    }

    pub fn unmask_all(&mut self) {
        self.master_mask = 0x00;
        self.slave_mask = 0x00;
    }

    pub fn read_isr(&self) -> u16 {
        self.write_command(true, OCW3_READ_ISR);
        self.write_command(false, OCW3_READ_ISR);
        let master = self.read_data(true) as u16;
        let slave = self.read_data(false) as u16;
        master | (slave << 8)
    }

    pub fn read_irr(&self) -> u16 {
        self.write_command(true, OCW3_READ_IRR);
        self.write_command(false, OCW3_READ_IRR);
        let master = self.read_data(true) as u16;
        let slave = self.read_data(false) as u16;
        master | (slave << 8)
    }

    pub fn irq_to_vector(&self, irq: u8) -> u8 {
        if irq < 8 {
            self.master_offset + irq
        } else {
            self.slave_offset + (irq - 8)
        }
    }

    fn write_command(&self, master: bool, value: u8) {
        let port = if master { PIC1_COMMAND } else { PIC2_COMMAND };
        unsafe {
            x86_64::instructions::port::Port::new(port).write(value);
        }
    }

    fn write_data(&self, master: bool, value: u8) {
        let port = if master { PIC1_DATA } else { PIC2_DATA };
        unsafe {
            x86_64::instructions::port::Port::new(port).write(value);
        }
    }

    fn read_data(&self, master: bool) -> u8 {
        let port = if master { PIC1_DATA } else { PIC2_DATA };
        unsafe { x86_64::instructions::port::Port::new(port).read() }
    }
}

// ---------------------------------------------------------------------------
// PS/2 Controller — Intel 8042
// ---------------------------------------------------------------------------

const PS2_DATA_PORT: u16 = 0x60;
const PS2_STATUS_PORT: u16 = 0x64;
const PS2_COMMAND_PORT: u16 = 0x64;

const PS2_STATUS_OUTPUT_FULL: u8 = 0x01;
const PS2_STATUS_INPUT_FULL: u8 = 0x02;
const PS2_STATUS_SYSTEM_FLAG: u8 = 0x04;
const PS2_STATUS_CMD_DATA: u8 = 0x08;
const PS2_STATUS_TIMEOUT: u8 = 0x40;
const PS2_STATUS_PARITY: u8 = 0x80;

const PS2_CMD_READ_CONFIG: u8 = 0x20;
const PS2_CMD_WRITE_CONFIG: u8 = 0x60;
const PS2_CMD_DISABLE_PORT2: u8 = 0xA7;
const PS2_CMD_ENABLE_PORT2: u8 = 0xA8;
const PS2_CMD_TEST_PORT2: u8 = 0xA9;
const PS2_CMD_SELF_TEST: u8 = 0xAA;
const PS2_CMD_TEST_PORT1: u8 = 0xAB;
const PS2_CMD_DISABLE_PORT1: u8 = 0xAD;
const PS2_CMD_ENABLE_PORT1: u8 = 0xAE;
const PS2_CMD_WRITE_PORT2: u8 = 0xD4;

const PS2_SELF_TEST_OK: u8 = 0x55;
const PS2_PORT_TEST_OK: u8 = 0x00;

const PS2_KB_CMD_SET_LEDS: u8 = 0xED;
const PS2_KB_CMD_ECHO: u8 = 0xEE;
const PS2_KB_CMD_SCAN_CODE_SET: u8 = 0xF0;
const PS2_KB_CMD_IDENTIFY: u8 = 0xF2;
const PS2_KB_CMD_TYPEMATIC: u8 = 0xF3;
const PS2_KB_CMD_ENABLE_SCAN: u8 = 0xF4;
const PS2_KB_CMD_DISABLE_SCAN: u8 = 0xF5;
const PS2_KB_CMD_RESET: u8 = 0xFF;

const PS2_MOUSE_CMD_SET_DEFAULTS: u8 = 0xF6;
const PS2_MOUSE_CMD_ENABLE: u8 = 0xF4;
const PS2_MOUSE_CMD_DISABLE: u8 = 0xF5;
const PS2_MOUSE_CMD_RESET: u8 = 0xFF;
const PS2_MOUSE_CMD_SET_SAMPLE: u8 = 0xF3;
const PS2_MOUSE_CMD_GET_ID: u8 = 0xF2;
const PS2_MOUSE_CMD_SET_RESOLUTION: u8 = 0xE8;
const PS2_MOUSE_CMD_STATUS: u8 = 0xE9;

const PS2_ACK: u8 = 0xFA;

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct KbLeds: u8 {
        const SCROLL_LOCK = 0x01;
        const NUM_LOCK    = 0x02;
        const CAPS_LOCK   = 0x04;
    }
}

pub struct Ps2Controller {
    pub config_byte: u8,
    pub port1_enabled: bool,
    pub port2_enabled: bool,
    pub port1_working: bool,
    pub port2_working: bool,
    pub dual_channel: bool,
    pub self_test_ok: bool,
    pub leds: KbLeds,
    pub typematic_byte: u8,
    pub scan_code_set: u8,
}

impl Ps2Controller {
    pub fn new() -> Self {
        Self {
            config_byte: 0x47,
            port1_enabled: false,
            port2_enabled: false,
            port1_working: false,
            port2_working: false,
            dual_channel: true,
            self_test_ok: false,
            leds: KbLeds::empty(),
            typematic_byte: 0x00,
            scan_code_set: 2,
        }
    }

    pub fn initialize(&mut self) {
        self.send_command(PS2_CMD_DISABLE_PORT1);
        self.send_command(PS2_CMD_DISABLE_PORT2);
        self.flush_output();

        self.send_command(PS2_CMD_READ_CONFIG);
        let config = self.read_data_wait();
        let config = config & !0x43; // disable IRQs and translation
        self.send_command(PS2_CMD_WRITE_CONFIG);
        self.send_data(config);
        self.config_byte = config;

        self.send_command(PS2_CMD_SELF_TEST);
        let result = self.read_data_wait();
        self.self_test_ok = result == PS2_SELF_TEST_OK;

        self.send_command(PS2_CMD_WRITE_CONFIG);
        self.send_data(config);

        self.send_command(PS2_CMD_ENABLE_PORT2);
        self.send_command(PS2_CMD_READ_CONFIG);
        let config2 = self.read_data_wait();
        self.dual_channel = config2 & 0x20 == 0;
        if self.dual_channel {
            self.send_command(PS2_CMD_DISABLE_PORT2);
        }

        self.send_command(PS2_CMD_TEST_PORT1);
        let port1_result = self.read_data_wait();
        self.port1_working = port1_result == PS2_PORT_TEST_OK;

        if self.dual_channel {
            self.send_command(PS2_CMD_TEST_PORT2);
            let port2_result = self.read_data_wait();
            self.port2_working = port2_result == PS2_PORT_TEST_OK;
        }

        if self.port1_working {
            self.send_command(PS2_CMD_ENABLE_PORT1);
            self.port1_enabled = true;
            let new_config = self.config_byte | 0x01; // enable port 1 IRQ
            self.send_command(PS2_CMD_WRITE_CONFIG);
            self.send_data(new_config);
            self.config_byte = new_config;
        }

        if self.port2_working && self.dual_channel {
            self.send_command(PS2_CMD_ENABLE_PORT2);
            self.port2_enabled = true;
            let new_config = self.config_byte | 0x02; // enable port 2 IRQ
            self.send_command(PS2_CMD_WRITE_CONFIG);
            self.send_data(new_config);
            self.config_byte = new_config;
        }
    }

    pub fn set_leds(&mut self, leds: KbLeds) {
        self.send_kb_command(PS2_KB_CMD_SET_LEDS);
        self.send_data(leds.bits());
        self.leds = leds;
    }

    pub fn set_typematic(&mut self, rate: u8, delay: u8) {
        let byte = (delay & 0x03) << 5 | (rate & 0x1F);
        self.send_kb_command(PS2_KB_CMD_TYPEMATIC);
        self.send_data(byte);
        self.typematic_byte = byte;
    }

    pub fn set_scan_code_set(&mut self, set: u8) {
        self.send_kb_command(PS2_KB_CMD_SCAN_CODE_SET);
        self.send_data(set);
        self.scan_code_set = set;
    }

    pub fn enable_scanning(&self) {
        self.send_kb_command_simple(PS2_KB_CMD_ENABLE_SCAN);
    }

    pub fn disable_scanning(&self) {
        self.send_kb_command_simple(PS2_KB_CMD_DISABLE_SCAN);
    }

    pub fn enable_mouse(&self) {
        self.send_mouse_command(PS2_MOUSE_CMD_ENABLE);
    }

    pub fn disable_mouse(&self) {
        self.send_mouse_command(PS2_MOUSE_CMD_DISABLE);
    }

    pub fn reset_mouse(&self) {
        self.send_mouse_command(PS2_MOUSE_CMD_RESET);
    }

    pub fn set_mouse_sample_rate(&self, rate: u8) {
        self.send_mouse_command(PS2_MOUSE_CMD_SET_SAMPLE);
        self.send_command(PS2_CMD_WRITE_PORT2);
        self.send_data(rate);
    }

    fn send_command(&self, cmd: u8) {
        self.wait_input_clear();
        unsafe {
            x86_64::instructions::port::Port::new(PS2_COMMAND_PORT).write(cmd);
        }
    }

    fn send_data(&self, data: u8) {
        self.wait_input_clear();
        unsafe {
            x86_64::instructions::port::Port::new(PS2_DATA_PORT).write(data);
        }
    }

    fn read_data_wait(&self) -> u8 {
        self.wait_output_ready();
        unsafe { x86_64::instructions::port::Port::new(PS2_DATA_PORT).read() }
    }

    fn wait_input_clear(&self) {
        for _ in 0..100_000 {
            let status: u8 =
                unsafe { x86_64::instructions::port::Port::new(PS2_STATUS_PORT).read() };
            if status & PS2_STATUS_INPUT_FULL == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    fn wait_output_ready(&self) {
        for _ in 0..100_000 {
            let status: u8 =
                unsafe { x86_64::instructions::port::Port::new(PS2_STATUS_PORT).read() };
            if status & PS2_STATUS_OUTPUT_FULL != 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    fn flush_output(&self) {
        for _ in 0..100 {
            let status: u8 =
                unsafe { x86_64::instructions::port::Port::new(PS2_STATUS_PORT).read() };
            if status & PS2_STATUS_OUTPUT_FULL == 0 {
                break;
            }
            let _: u8 = unsafe { x86_64::instructions::port::Port::new(PS2_DATA_PORT).read() };
        }
    }

    fn send_kb_command(&self, cmd: u8) {
        self.send_data(cmd);
        let _ = self.read_data_wait(); // ACK
    }

    fn send_kb_command_simple(&self, cmd: u8) {
        self.send_data(cmd);
    }

    fn send_mouse_command(&self, cmd: u8) {
        self.send_command(PS2_CMD_WRITE_PORT2);
        self.send_data(cmd);
    }
}

// ---------------------------------------------------------------------------
// PC Speaker
// ---------------------------------------------------------------------------

const SPEAKER_PORT: u16 = 0x61;

pub struct PcSpeaker {
    pub enabled: bool,
    pub frequency_hz: u32,
}

impl PcSpeaker {
    pub fn new() -> Self {
        Self {
            enabled: false,
            frequency_hz: 0,
        }
    }

    pub fn play_tone(&mut self, frequency_hz: u32) {
        if frequency_hz == 0 {
            self.stop();
            return;
        }

        let divisor = PIT_BASE_FREQUENCY / frequency_hz;
        let divisor = if divisor > 0xFFFF {
            0xFFFF
        } else if divisor == 0 {
            1
        } else {
            divisor as u16
        };

        unsafe {
            let mut cmd_port: x86_64::instructions::port::Port<u8> =
                x86_64::instructions::port::Port::new(PIT_COMMAND);
            cmd_port.write(0xB6); // channel 2, lobyte/hibyte, square wave

            let mut ch2: x86_64::instructions::port::Port<u8> =
                x86_64::instructions::port::Port::new(PIT_CHANNEL2);
            ch2.write(divisor as u8);
            ch2.write((divisor >> 8) as u8);

            let mut speaker: x86_64::instructions::port::Port<u8> =
                x86_64::instructions::port::Port::new(SPEAKER_PORT);
            let val = speaker.read();
            speaker.write(val | 0x03); // enable PIT channel 2 gate + speaker
        }

        self.enabled = true;
        self.frequency_hz = frequency_hz;
    }

    pub fn stop(&mut self) {
        unsafe {
            let mut speaker: x86_64::instructions::port::Port<u8> =
                x86_64::instructions::port::Port::new(SPEAKER_PORT);
            let val = speaker.read();
            speaker.write(val & !0x03);
        }
        self.enabled = false;
        self.frequency_hz = 0;
    }

    pub fn beep(&mut self, frequency_hz: u32, duration_ticks: u32) {
        self.play_tone(frequency_hz);
        for _ in 0..duration_ticks {
            core::hint::spin_loop();
        }
        self.stop();
    }

    pub fn boot_beep(&mut self) {
        self.beep(1000, 500_000);
    }

    pub fn error_beep(&mut self) {
        self.beep(440, 1_000_000);
        for _ in 0..200_000 {
            core::hint::spin_loop();
        }
        self.beep(440, 1_000_000);
    }
}

// ---------------------------------------------------------------------------
// BIOS Data Area (BDA) at 0x0400–0x04FF
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BiosDataArea {
    pub com1_base: u16,             // 0x400
    pub com2_base: u16,             // 0x402
    pub com3_base: u16,             // 0x404
    pub com4_base: u16,             // 0x406
    pub lpt1_base: u16,             // 0x408
    pub lpt2_base: u16,             // 0x40A
    pub lpt3_base: u16,             // 0x40C
    pub ebda_segment: u16,          // 0x40E
    pub equipment_list: u16,        // 0x410
    pub reserved1: u8,              // 0x412
    pub memory_size_kb: u16,        // 0x413
    pub reserved2: [u8; 2],         // 0x415
    pub keyboard_flags1: u8,        // 0x417
    pub keyboard_flags2: u8,        // 0x418
    pub numpad_buffer: u8,          // 0x419
    pub kbd_buf_head: u16,          // 0x41A
    pub kbd_buf_tail: u16,          // 0x41C
    pub kbd_buffer: [u16; 16],      // 0x41E
    pub floppy_recal: u8,           // 0x43E
    pub floppy_motor: u8,           // 0x43F
    pub floppy_timeout: u8,         // 0x440
    pub floppy_status: u8,          // 0x441
    pub reserved3: [u8; 7],         // 0x442
    pub video_mode: u8,             // 0x449
    pub screen_columns: u16,        // 0x44A
    pub video_page_size: u16,       // 0x44C
    pub video_page_offset: u16,     // 0x44E
    pub cursor_positions: [u16; 8], // 0x450
    pub cursor_shape: u16,          // 0x460
    pub active_page: u8,            // 0x462
    pub crtc_base: u16,             // 0x463
    pub current_msr: u8,            // 0x465
    pub current_palette: u8,        // 0x466
    pub reserved4: [u8; 5],         // 0x467
    pub timer_count: u32,           // 0x46C
    pub timer_overflow: u8,         // 0x470
    pub ctrl_break: u8,             // 0x471
    pub soft_reset_flag: u16,       // 0x472
    pub hd_last_status: u8,         // 0x474
    pub hd_count: u8,               // 0x475
}

impl BiosDataArea {
    pub fn new() -> Self {
        Self {
            com1_base: 0x3F8,
            com2_base: 0x2F8,
            com3_base: 0x3E8,
            com4_base: 0x2E8,
            lpt1_base: 0x378,
            lpt2_base: 0x278,
            lpt3_base: 0,
            ebda_segment: 0x9FC0,
            equipment_list: 0x0021,
            reserved1: 0,
            memory_size_kb: 640,
            reserved2: [0; 2],
            keyboard_flags1: 0,
            keyboard_flags2: 0,
            numpad_buffer: 0,
            kbd_buf_head: 0x41E,
            kbd_buf_tail: 0x41E,
            kbd_buffer: [0; 16],
            floppy_recal: 0,
            floppy_motor: 0,
            floppy_timeout: 0,
            floppy_status: 0,
            reserved3: [0; 7],
            video_mode: 0x03,
            screen_columns: 80,
            video_page_size: 4096,
            video_page_offset: 0,
            cursor_positions: [0; 8],
            cursor_shape: 0x0607,
            active_page: 0,
            crtc_base: 0x3D4,
            current_msr: 0x29,
            current_palette: 0x30,
            reserved4: [0; 5],
            timer_count: 0,
            timer_overflow: 0,
            ctrl_break: 0,
            soft_reset_flag: 0,
            hd_last_status: 0,
            hd_count: 0,
        }
    }

    pub fn com_port_address(&self, port: u8) -> u16 {
        match port {
            1 => self.com1_base,
            2 => self.com2_base,
            3 => self.com3_base,
            4 => self.com4_base,
            _ => 0,
        }
    }

    pub fn lpt_port_address(&self, port: u8) -> u16 {
        match port {
            1 => self.lpt1_base,
            2 => self.lpt2_base,
            3 => self.lpt3_base,
            _ => 0,
        }
    }

    pub fn has_fpu(&self) -> bool {
        self.equipment_list & 0x02 != 0
    }

    pub fn number_of_com_ports(&self) -> u8 {
        ((self.equipment_list >> 9) & 0x07) as u8
    }

    pub fn number_of_lpt_ports(&self) -> u8 {
        ((self.equipment_list >> 14) & 0x03) as u8
    }

    pub fn ebda_address(&self) -> u32 {
        (self.ebda_segment as u32) << 4
    }
}

// ---------------------------------------------------------------------------
// EBDA (Extended BIOS Data Area) search helpers
// ---------------------------------------------------------------------------

pub struct EbdaSearch {
    pub ebda_base: u64,
    pub ebda_size: usize,
}

impl EbdaSearch {
    pub fn new(ebda_base: u64) -> Self {
        Self {
            ebda_base,
            ebda_size: 1024,
        }
    }

    pub fn find_smp_floating_pointer(&self) -> Option<u64> {
        self.search_signature(b"_MP_", 16)
    }

    pub fn find_acpi_rsdp(&self) -> Option<u64> {
        let ebda_result = self.search_signature(b"RSD PTR ", 16);
        if ebda_result.is_some() {
            return ebda_result;
        }
        self.search_bios_area(b"RSD PTR ", 16)
    }

    fn search_signature(&self, signature: &[u8], alignment: usize) -> Option<u64> {
        let _ = (signature, alignment);
        None
    }

    fn search_bios_area(&self, signature: &[u8], alignment: usize) -> Option<u64> {
        let _ = (signature, alignment);
        None
    }
}

// ---------------------------------------------------------------------------
// System Management
// ---------------------------------------------------------------------------

pub struct SystemManagement {
    pub smi_enabled: bool,
    pub smm_base: u64,
    pub smm_size: u64,
    pub smi_count: u64,
    pub smm_aware: bool,
    pub locked: bool,
}

impl SystemManagement {
    pub fn new() -> Self {
        Self {
            smi_enabled: true,
            smm_base: 0x000A_0000,
            smm_size: 0x0001_0000,
            smi_count: 0,
            smm_aware: false,
            locked: false,
        }
    }

    pub fn disable_smi(&mut self) -> bool {
        if self.locked {
            return false;
        }
        self.smi_enabled = false;
        true
    }

    pub fn enable_smi(&mut self) -> bool {
        if self.locked {
            return false;
        }
        self.smi_enabled = true;
        true
    }

    pub fn lock(&mut self) {
        self.locked = true;
    }

    pub fn record_smi(&mut self) {
        self.smi_count += 1;
    }

    pub fn is_in_smm_region(&self, addr: u64) -> bool {
        addr >= self.smm_base && addr < self.smm_base + self.smm_size
    }
}

// ---------------------------------------------------------------------------
// Hardware Detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusType {
    Isa,
    Pci,
    PciExpress,
    Usb,
    AcpiEnum,
}

#[derive(Debug, Clone)]
pub struct DetectedDevice {
    pub name: String,
    pub bus_type: BusType,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub irq: Option<u8>,
}

pub struct HardwareDetector {
    pub devices: Vec<DetectedDevice>,
}

impl HardwareDetector {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    pub fn probe_isa(&mut self) {
        const ISA_COM_PORTS: [(u16, &str); 4] = [
            (0x3F8, "COM1"),
            (0x2F8, "COM2"),
            (0x3E8, "COM3"),
            (0x2E8, "COM4"),
        ];
        for &(port, name) in &ISA_COM_PORTS {
            let scratch: u8 = unsafe {
                let mut p: x86_64::instructions::port::Port<u8> =
                    x86_64::instructions::port::Port::new(port + 7);
                p.write(0xAA);
                p.read()
            };
            if scratch == 0xAA {
                self.devices.push(DetectedDevice {
                    name: String::from(name),
                    bus_type: BusType::Isa,
                    vendor: 0,
                    device: 0,
                    class: 0x07,
                    subclass: 0x00,
                    irq: Some(if port == 0x3F8 || port == 0x3E8 { 4 } else { 3 }),
                });
            }
        }
    }

    pub fn probe_pci(&mut self) {
        for bus in 0..=255u8 {
            for device in 0..32u8 {
                let addr: u32 = 0x8000_0000 | ((bus as u32) << 16) | ((device as u32) << 11);
                unsafe {
                    x86_64::instructions::port::Port::new(0xCF8).write(addr);
                    let id: u32 = x86_64::instructions::port::Port::new(0xCFC).read();
                    if id == 0xFFFF_FFFF {
                        continue;
                    }
                    let vendor = (id & 0xFFFF) as u16;
                    let dev_id = (id >> 16) as u16;

                    x86_64::instructions::port::Port::new(0xCF8).write(addr | 0x08);
                    let class_word: u32 = x86_64::instructions::port::Port::new(0xCFC).read();
                    let class = (class_word >> 24) as u8;
                    let subclass = ((class_word >> 16) & 0xFF) as u8;

                    x86_64::instructions::port::Port::new(0xCF8).write(addr | 0x3C);
                    let irq_reg: u32 = x86_64::instructions::port::Port::new(0xCFC).read();
                    let irq = (irq_reg & 0xFF) as u8;

                    self.devices.push(DetectedDevice {
                        name: alloc::format!(
                            "PCI {:02x}:{:02x}.0 {:04x}:{:04x}",
                            bus,
                            device,
                            vendor,
                            dev_id
                        ),
                        bus_type: BusType::Pci,
                        vendor,
                        device: dev_id,
                        class,
                        subclass,
                        irq: if irq > 0 && irq < 255 {
                            Some(irq)
                        } else {
                            None
                        },
                    });
                }
            }
        }
    }

    pub fn enumerate_acpi(&mut self) {
        // Placeholder: real implementation walks ACPI namespace via AML interpreter
    }

    pub fn find_by_class(&self, class: u8, subclass: u8) -> Vec<&DetectedDevice> {
        self.devices
            .iter()
            .filter(|d| d.class == class && d.subclass == subclass)
            .collect()
    }

    pub fn find_by_vendor(&self, vendor: u16, device: u16) -> Option<&DetectedDevice> {
        self.devices
            .iter()
            .find(|d| d.vendor == vendor && d.device == device)
    }
}

// ---------------------------------------------------------------------------
// Platform Quirks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PlatformQuirk {
    pub name: String,
    pub description: String,
    pub vendor: String,
    pub product: String,
    pub action: QuirkAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuirkAction {
    DisableAcpiTimer,
    ForceApicMode,
    OverrideIrqRouting,
    DisableHpet,
    ForcePs2,
    SkipSelfTest,
    DisableMsi,
    ForceAhci,
    UsePollMode,
    WorkaroundBrokenBios,
}

pub struct QuirkDatabase {
    pub quirks: Vec<PlatformQuirk>,
}

impl QuirkDatabase {
    pub fn new() -> Self {
        let mut db = Self { quirks: Vec::new() };
        db.populate_defaults();
        db
    }

    fn populate_defaults(&mut self) {
        self.quirks.push(PlatformQuirk {
            name: String::from("QEMU-APIC"),
            description: String::from("Force APIC mode on QEMU (8259 unreliable)"),
            vendor: String::from("QEMU"),
            product: String::from("Standard PC"),
            action: QuirkAction::ForceApicMode,
        });
        self.quirks.push(PlatformQuirk {
            name: String::from("VBox-HPET"),
            description: String::from("Disable HPET on VirtualBox (broken implementation)"),
            vendor: String::from("innotek GmbH"),
            product: String::from("VirtualBox"),
            action: QuirkAction::DisableHpet,
        });
        self.quirks.push(PlatformQuirk {
            name: String::from("VMware-Timer"),
            description: String::from("Disable ACPI timer on older VMware (too slow)"),
            vendor: String::from("VMware, Inc."),
            product: String::from("VMware Virtual Platform"),
            action: QuirkAction::DisableAcpiTimer,
        });
        self.quirks.push(PlatformQuirk {
            name: String::from("Legacy-PS2"),
            description: String::from("Force PS/2 on platforms that lie about USB-HID"),
            vendor: String::from("*"),
            product: String::from("*"),
            action: QuirkAction::ForcePs2,
        });
        self.quirks.push(PlatformQuirk {
            name: String::from("BIOS-IRQ-Fix"),
            description: String::from("Override IRQ routing on boards with broken MP tables"),
            vendor: String::from("Award"),
            product: String::from("*"),
            action: QuirkAction::OverrideIrqRouting,
        });
        self.quirks.push(PlatformQuirk {
            name: String::from("Broken-BIOS"),
            description: String::from("Workaround for BIOSes that don't clear BSS properly"),
            vendor: String::from("American Megatrends"),
            product: String::from("*"),
            action: QuirkAction::WorkaroundBrokenBios,
        });
    }

    pub fn find_quirks_for(&self, vendor: &str, product: &str) -> Vec<&PlatformQuirk> {
        self.quirks
            .iter()
            .filter(|q| {
                (q.vendor == "*" || vendor.contains(&q.vendor))
                    && (q.product == "*" || product.contains(&q.product))
            })
            .collect()
    }

    pub fn has_quirk(&self, vendor: &str, product: &str, action: QuirkAction) -> bool {
        self.find_quirks_for(vendor, product)
            .iter()
            .any(|q| q.action == action)
    }
}

// ---------------------------------------------------------------------------
// ACPI Blacklist
// ---------------------------------------------------------------------------

pub struct AcpiBlacklist {
    entries: Vec<AcpiBlacklistEntry>,
}

#[derive(Debug, Clone)]
struct AcpiBlacklistEntry {
    oem_id: String,
    oem_table: String,
    reason: String,
}

impl AcpiBlacklist {
    pub fn new() -> Self {
        let mut bl = Self {
            entries: Vec::new(),
        };
        bl.entries.push(AcpiBlacklistEntry {
            oem_id: String::from("PTLTD"),
            oem_table: String::from("RSDT"),
            reason: String::from("Known broken RSDT on old Phoenix BIOS"),
        });
        bl.entries.push(AcpiBlacklistEntry {
            oem_id: String::from("NVIDIA"),
            oem_table: String::from("NFORCE"),
            reason: String::from("nForce ACPI timer issues"),
        });
        bl
    }

    pub fn is_blacklisted(&self, oem_id: &str, oem_table: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| oem_id.starts_with(&e.oem_id) && oem_table.starts_with(&e.oem_table))
            .map(|e| e.reason.as_str())
    }
}

// ---------------------------------------------------------------------------
// Firmware Password
// ---------------------------------------------------------------------------

pub struct FirmwarePassword {
    enabled: bool,
    hash: [u8; 32],
    attempts: u32,
    max_attempts: u32,
    locked_out: bool,
}

impl FirmwarePassword {
    pub fn new() -> Self {
        Self {
            enabled: false,
            hash: [0u8; 32],
            attempts: 0,
            max_attempts: 3,
            locked_out: false,
        }
    }

    pub fn set_password(&mut self, password_hash: &[u8; 32]) {
        self.hash = *password_hash;
        self.enabled = true;
        self.attempts = 0;
        self.locked_out = false;
    }

    pub fn clear_password(&mut self) {
        self.hash = [0u8; 32];
        self.enabled = false;
    }

    pub fn verify(&mut self, candidate_hash: &[u8; 32]) -> bool {
        if !self.enabled {
            return true;
        }
        if self.locked_out {
            return false;
        }

        if self.hash == *candidate_hash {
            self.attempts = 0;
            true
        } else {
            self.attempts += 1;
            if self.attempts >= self.max_attempts {
                self.locked_out = true;
            }
            false
        }
    }

    pub fn is_locked_out(&self) -> bool {
        self.locked_out
    }

    pub fn reset_lockout(&mut self) {
        self.attempts = 0;
        self.locked_out = false;
    }
}

// ---------------------------------------------------------------------------
// Global Firmware State
// ---------------------------------------------------------------------------

pub struct FirmwareState {
    pub dmi: DmiInfo,
    pub cmos_rtc: CmosRtc,
    pub cmos_extended: CmosExtended,
    pub pit: Pit,
    pub pic: Pic8259,
    pub ps2: Ps2Controller,
    pub speaker: PcSpeaker,
    pub bda: BiosDataArea,
    pub ebda_search: EbdaSearch,
    pub sys_mgmt: SystemManagement,
    pub hw_detector: HardwareDetector,
    pub quirk_db: QuirkDatabase,
    pub acpi_bl: AcpiBlacklist,
    pub fw_password: FirmwarePassword,
    pub initialized: bool,
}

impl FirmwareState {
    pub fn new() -> Self {
        let bda = BiosDataArea::new();
        Self {
            dmi: DmiInfo::default(),
            cmos_rtc: CmosRtc::new(),
            cmos_extended: CmosExtended::new(),
            pit: Pit::new(),
            pic: Pic8259::new(),
            ps2: Ps2Controller::new(),
            speaker: PcSpeaker::new(),
            bda,
            ebda_search: EbdaSearch::new(0x9FC00),
            sys_mgmt: SystemManagement::new(),
            hw_detector: HardwareDetector::new(),
            quirk_db: QuirkDatabase::new(),
            acpi_bl: AcpiBlacklist::new(),
            fw_password: FirmwarePassword::new(),
            initialized: false,
        }
    }

    pub fn apply_quirks(&mut self) {
        let quirks = self
            .quirk_db
            .find_quirks_for(&self.dmi.system_manufacturer, &self.dmi.product_name);
        for q in quirks {
            match q.action {
                QuirkAction::ForceApicMode => { /* handled in APIC init */ }
                QuirkAction::DisableHpet => { /* handled in HPET init */ }
                QuirkAction::ForcePs2 => { /* handled in PS/2 init */ }
                _ => {}
            }
        }
    }
}

pub static FIRMWARE: Mutex<Option<FirmwareState>> = Mutex::new(None);

pub fn init() {
    let mut fw = FirmwareState::new();

    fw.cmos_rtc.write_time(&RtcTime {
        seconds: 0,
        minutes: 0,
        hours: 0,
        day: 25,
        month: 5,
        year: 2026,
    });

    fw.pit.program_frequency(0, 1000); // 1 kHz system timer
    fw.pit.channels[1].enabled = false; // DRAM refresh — unused in modern hardware
    fw.pit.channels[2].enabled = false; // speaker — enabled on demand

    fw.hw_detector.probe_isa();

    fw.apply_quirks();

    fw.initialized = true;
    *FIRMWARE.lock() = Some(fw);
}
