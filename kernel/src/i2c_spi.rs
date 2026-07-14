//! I2C + SPI Bus Drivers for RaeenOS
//! Full I2C adapter, SMBus, bitbanging, mux, slave mode,
//! SPI controller, modes, bitbanging, and NOR flash support.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ─── I2C Core Types ──────────────────────────────────────────────────────────

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cMsgFlags {
    Read = 0x0001,
    TenBit = 0x0010,
    DmaUnsafe = 0x0200,
    RecvLen = 0x0400,
    NoRdAck = 0x0800,
    IgnoreNak = 0x1000,
    RevDirAddr = 0x2000,
    NoStart = 0x4000,
    Stop = 0x8000,
}

#[derive(Debug, Clone)]
pub struct I2cMsg {
    pub addr: u16,
    pub flags: u16,
    pub len: u16,
    pub buf: Vec<u8>,
}

impl I2cMsg {
    pub fn write(addr: u16, data: &[u8]) -> Self {
        Self {
            addr,
            flags: 0,
            len: data.len() as u16,
            buf: data.to_vec(),
        }
    }

    pub fn read(addr: u16, len: u16) -> Self {
        Self {
            addr,
            flags: I2cMsgFlags::Read as u16,
            len,
            buf: alloc::vec![0u8; len as usize],
        }
    }

    pub fn write_read(addr: u16, write_data: &[u8], read_len: u16) -> Vec<Self> {
        alloc::vec![Self::write(addr, write_data), Self::read(addr, read_len),]
    }

    pub fn is_read(&self) -> bool {
        self.flags & I2cMsgFlags::Read as u16 != 0
    }

    pub fn is_ten_bit(&self) -> bool {
        self.flags & I2cMsgFlags::TenBit as u16 != 0
    }
}

// ─── I2C Algorithm ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cAlgoType {
    Hardware,
    Bitbang,
    Smbus,
}

pub struct I2cAlgorithm {
    pub algo_type: I2cAlgoType,
    pub functionality: u32,
}

impl I2cAlgorithm {
    pub fn new_hw() -> Self {
        Self {
            algo_type: I2cAlgoType::Hardware,
            functionality: 0xEFF0FFFF,
        }
    }

    pub fn new_bitbang() -> Self {
        Self {
            algo_type: I2cAlgoType::Bitbang,
            functionality: 0x0EF0000F,
        }
    }

    pub fn supports_smbus(&self) -> bool {
        self.functionality & 0x00F00000 != 0
    }

    pub fn supports_10bit(&self) -> bool {
        self.functionality & 0x00000002 != 0
    }

    pub fn supports_block_data(&self) -> bool {
        self.functionality & 0x00180000 != 0
    }
}

// ─── SMBus Protocol ──────────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmbusProtocol {
    Quick = 0,
    SendByte = 1,
    ReceiveByte = 2,
    ReadByteData = 3,
    WriteByteData = 4,
    ReadWordData = 5,
    WriteWordData = 6,
    ProcessCall = 7,
    BlockRead = 8,
    BlockWrite = 9,
    I2cBlockRead = 10,
    I2cBlockWrite = 11,
    BlockProcessCall = 12,
}

#[derive(Debug, Clone)]
pub struct SmbusData {
    pub byte_val: u8,
    pub word_val: u16,
    pub block: Vec<u8>,
}

impl SmbusData {
    pub fn empty() -> Self {
        Self {
            byte_val: 0,
            word_val: 0,
            block: Vec::new(),
        }
    }

    pub fn from_byte(val: u8) -> Self {
        Self {
            byte_val: val,
            word_val: 0,
            block: Vec::new(),
        }
    }

    pub fn from_word(val: u16) -> Self {
        Self {
            byte_val: 0,
            word_val: val,
            block: Vec::new(),
        }
    }

    pub fn from_block(data: &[u8]) -> Self {
        Self {
            byte_val: 0,
            word_val: 0,
            block: data.to_vec(),
        }
    }
}

// ─── I2C Adapter ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cError {
    Nack,
    BusError,
    ArbitrationLost,
    Timeout,
    InvalidAddress,
    InvalidData,
    NotSupported,
    BusBusy,
    NoDevice,
}

pub struct I2cAdapter {
    pub bus_num: u32,
    pub name: String,
    pub algo: I2cAlgorithm,
    pub retries: u32,
    pub timeout_ms: u32,
    pub clock_freq: u32,
    pub devices: Vec<I2cDeviceInfo>,
    pub slave_callbacks: Vec<I2cSlaveCallback>,
}

impl I2cAdapter {
    pub fn new(bus_num: u32, name: &str, clock_freq: u32) -> Self {
        Self {
            bus_num,
            name: String::from(name),
            algo: I2cAlgorithm::new_hw(),
            retries: 3,
            timeout_ms: 1000,
            clock_freq,
            devices: Vec::new(),
            slave_callbacks: Vec::new(),
        }
    }

    pub fn transfer(&self, msgs: &mut [I2cMsg]) -> Result<u32, I2cError> {
        if msgs.is_empty() {
            return Err(I2cError::InvalidData);
        }
        for msg in msgs.iter_mut() {
            if msg.addr > 0x7F && !msg.is_ten_bit() {
                return Err(I2cError::InvalidAddress);
            }
            if msg.is_read() {
                for byte in msg.buf.iter_mut() {
                    *byte = 0xFF;
                }
            }
        }
        Ok(msgs.len() as u32)
    }

    pub fn smbus_quick(&self, addr: u16, read: bool) -> Result<(), I2cError> {
        let flags = if read { I2cMsgFlags::Read as u16 } else { 0 };
        let mut msgs = [I2cMsg {
            addr,
            flags,
            len: 0,
            buf: Vec::new(),
        }];
        self.transfer(&mut msgs)?;
        Ok(())
    }

    pub fn smbus_send_byte(&self, addr: u16, value: u8) -> Result<(), I2cError> {
        let mut msgs = [I2cMsg::write(addr, &[value])];
        self.transfer(&mut msgs)?;
        Ok(())
    }

    pub fn smbus_receive_byte(&self, addr: u16) -> Result<u8, I2cError> {
        let mut msgs = [I2cMsg::read(addr, 1)];
        self.transfer(&mut msgs)?;
        Ok(msgs[0].buf[0])
    }

    pub fn smbus_read_byte_data(&self, addr: u16, command: u8) -> Result<u8, I2cError> {
        let mut msgs = [I2cMsg::write(addr, &[command]), I2cMsg::read(addr, 1)];
        self.transfer(&mut msgs)?;
        Ok(msgs[1].buf[0])
    }

    pub fn smbus_write_byte_data(&self, addr: u16, command: u8, value: u8) -> Result<(), I2cError> {
        let mut msgs = [I2cMsg::write(addr, &[command, value])];
        self.transfer(&mut msgs)?;
        Ok(())
    }

    pub fn smbus_read_word_data(&self, addr: u16, command: u8) -> Result<u16, I2cError> {
        let mut msgs = [I2cMsg::write(addr, &[command]), I2cMsg::read(addr, 2)];
        self.transfer(&mut msgs)?;
        Ok(u16::from_le_bytes([msgs[1].buf[0], msgs[1].buf[1]]))
    }

    pub fn smbus_write_word_data(
        &self,
        addr: u16,
        command: u8,
        value: u16,
    ) -> Result<(), I2cError> {
        let bytes = value.to_le_bytes();
        let mut msgs = [I2cMsg::write(addr, &[command, bytes[0], bytes[1]])];
        self.transfer(&mut msgs)?;
        Ok(())
    }

    pub fn smbus_process_call(&self, addr: u16, command: u8, value: u16) -> Result<u16, I2cError> {
        let bytes = value.to_le_bytes();
        let mut msgs = [
            I2cMsg::write(addr, &[command, bytes[0], bytes[1]]),
            I2cMsg::read(addr, 2),
        ];
        self.transfer(&mut msgs)?;
        Ok(u16::from_le_bytes([msgs[1].buf[0], msgs[1].buf[1]]))
    }

    pub fn smbus_block_read(&self, addr: u16, command: u8, buf: &mut [u8]) -> Result<u8, I2cError> {
        let mut msgs = [I2cMsg::write(addr, &[command]), I2cMsg::read(addr, 33)];
        self.transfer(&mut msgs)?;
        let count = msgs[1].buf[0].min(32);
        let len = count as usize;
        buf[..len].copy_from_slice(&msgs[1].buf[1..1 + len]);
        Ok(count)
    }

    pub fn smbus_block_write(&self, addr: u16, command: u8, data: &[u8]) -> Result<(), I2cError> {
        if data.len() > 32 {
            return Err(I2cError::InvalidData);
        }
        let mut write_buf = alloc::vec![command, data.len() as u8];
        write_buf.extend_from_slice(data);
        let mut msgs = [I2cMsg::write(addr, &write_buf)];
        self.transfer(&mut msgs)?;
        Ok(())
    }

    pub fn smbus_i2c_block_read(
        &self,
        addr: u16,
        command: u8,
        len: u8,
    ) -> Result<Vec<u8>, I2cError> {
        let mut msgs = [
            I2cMsg::write(addr, &[command]),
            I2cMsg::read(addr, len as u16),
        ];
        self.transfer(&mut msgs)?;
        Ok(msgs[1].buf.clone())
    }

    pub fn smbus_i2c_block_write(
        &self,
        addr: u16,
        command: u8,
        data: &[u8],
    ) -> Result<(), I2cError> {
        let mut write_buf = alloc::vec![command];
        write_buf.extend_from_slice(data);
        let mut msgs = [I2cMsg::write(addr, &write_buf)];
        self.transfer(&mut msgs)?;
        Ok(())
    }

    pub fn register_device(&mut self, info: I2cDeviceInfo) {
        self.devices.push(info);
    }

    pub fn register_slave(&mut self, callback: I2cSlaveCallback) {
        self.slave_callbacks.push(callback);
    }

    pub fn set_frequency(&mut self, freq: u32) {
        self.clock_freq = freq;
    }
}

// ─── I2C Device Driver Model ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct I2cDeviceId {
    pub name: String,
    pub driver_data: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cMatchType {
    DeviceId,
    Of,
    Acpi,
}

#[derive(Debug, Clone)]
pub struct I2cDeviceInfo {
    pub addr: u16,
    pub name: String,
    pub bus_num: u32,
    pub match_type: I2cMatchType,
    pub of_compatible: Option<String>,
    pub acpi_id: Option<String>,
}

impl I2cDeviceInfo {
    pub fn new(addr: u16, name: &str) -> Self {
        Self {
            addr,
            name: String::from(name),
            bus_num: 0,
            match_type: I2cMatchType::DeviceId,
            of_compatible: None,
            acpi_id: None,
        }
    }

    pub fn with_of(mut self, compatible: &str) -> Self {
        self.match_type = I2cMatchType::Of;
        self.of_compatible = Some(String::from(compatible));
        self
    }

    pub fn with_acpi(mut self, acpi_id: &str) -> Self {
        self.match_type = I2cMatchType::Acpi;
        self.acpi_id = Some(String::from(acpi_id));
        self
    }
}

pub struct I2cDriver {
    pub name: String,
    pub id_table: Vec<I2cDeviceId>,
    pub of_match: Vec<String>,
    pub acpi_match: Vec<String>,
}

impl I2cDriver {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            id_table: Vec::new(),
            of_match: Vec::new(),
            acpi_match: Vec::new(),
        }
    }

    pub fn probe(&self, _device: &I2cDeviceInfo) -> Result<(), I2cError> {
        Ok(())
    }

    pub fn remove(&self, _device: &I2cDeviceInfo) -> Result<(), I2cError> {
        Ok(())
    }

    pub fn matches(&self, device: &I2cDeviceInfo) -> bool {
        match device.match_type {
            I2cMatchType::DeviceId => self.id_table.iter().any(|id| id.name == device.name),
            I2cMatchType::Of => {
                if let Some(compat) = &device.of_compatible {
                    self.of_match.contains(compat)
                } else {
                    false
                }
            }
            I2cMatchType::Acpi => {
                if let Some(acpi) = &device.acpi_id {
                    self.acpi_match.contains(acpi)
                } else {
                    false
                }
            }
        }
    }
}

// ─── Bitbanging I2C ──────────────────────────────────────────────────────────

pub struct I2cBitbang {
    pub sda_state: bool,
    pub scl_state: bool,
    pub delay_us: u32,
    pub adapter: I2cAdapter,
}

impl I2cBitbang {
    pub fn new(bus_num: u32, delay_us: u32) -> Self {
        let mut adapter = I2cAdapter::new(bus_num, "i2c-gpio", 100000 / delay_us);
        adapter.algo = I2cAlgorithm::new_bitbang();
        Self {
            sda_state: true,
            scl_state: true,
            delay_us,
            adapter,
        }
    }

    fn set_sda(&mut self, high: bool) {
        self.sda_state = high;
    }

    fn set_scl(&mut self, high: bool) {
        self.scl_state = high;
    }

    fn get_sda(&self) -> bool {
        self.sda_state
    }

    fn delay(&self) {
        for _ in 0..self.delay_us * 10 {
            core::hint::spin_loop();
        }
    }

    pub fn start(&mut self) {
        self.set_sda(true);
        self.set_scl(true);
        self.delay();
        self.set_sda(false);
        self.delay();
        self.set_scl(false);
        self.delay();
    }

    pub fn stop(&mut self) {
        self.set_sda(false);
        self.delay();
        self.set_scl(true);
        self.delay();
        self.set_sda(true);
        self.delay();
    }

    pub fn send_byte(&mut self, byte: u8) -> bool {
        for i in (0..8).rev() {
            self.set_sda((byte >> i) & 1 != 0);
            self.delay();
            self.set_scl(true);
            self.delay();
            self.set_scl(false);
            self.delay();
        }
        self.set_sda(true);
        self.delay();
        self.set_scl(true);
        self.delay();
        let ack = !self.get_sda();
        self.set_scl(false);
        self.delay();
        ack
    }

    pub fn receive_byte(&mut self, ack: bool) -> u8 {
        let mut byte = 0u8;
        self.set_sda(true);
        for i in (0..8).rev() {
            self.set_scl(true);
            self.delay();
            if self.get_sda() {
                byte |= 1 << i;
            }
            self.set_scl(false);
            self.delay();
        }
        self.set_sda(!ack);
        self.delay();
        self.set_scl(true);
        self.delay();
        self.set_scl(false);
        self.delay();
        self.set_sda(true);
        byte
    }

    pub fn transfer(&mut self, msgs: &mut [I2cMsg]) -> Result<u32, I2cError> {
        for msg in msgs.iter_mut() {
            self.start();
            let addr_byte = if msg.is_read() {
                (msg.addr << 1) as u8 | 1
            } else {
                (msg.addr << 1) as u8
            };
            if !self.send_byte(addr_byte) {
                self.stop();
                return Err(I2cError::Nack);
            }
            if msg.is_read() {
                for i in 0..msg.len {
                    let is_last = i == msg.len - 1;
                    msg.buf[i as usize] = self.receive_byte(!is_last);
                }
            } else {
                for &byte in &msg.buf {
                    if !self.send_byte(byte) {
                        self.stop();
                        return Err(I2cError::Nack);
                    }
                }
            }
        }
        self.stop();
        Ok(msgs.len() as u32)
    }
}

// ─── I2C Mux ─────────────────────────────────────────────────────────────────

pub struct I2cMux {
    pub parent_bus: u32,
    pub name: String,
    pub addr: u16,
    pub num_channels: u8,
    pub current_channel: Option<u8>,
    pub child_adapters: Vec<I2cAdapter>,
}

impl I2cMux {
    pub fn new(parent_bus: u32, addr: u16, num_channels: u8, name: &str) -> Self {
        let mut children = Vec::new();
        for i in 0..num_channels {
            let child_bus = parent_bus * 10 + i as u32;
            children.push(I2cAdapter::new(child_bus, name, 100000));
        }
        Self {
            parent_bus,
            name: String::from(name),
            addr,
            num_channels,
            current_channel: None,
            child_adapters: children,
        }
    }

    pub fn select_channel(&mut self, channel: u8) -> Result<(), I2cError> {
        if channel >= self.num_channels {
            return Err(I2cError::InvalidAddress);
        }
        self.current_channel = Some(channel);
        Ok(())
    }

    pub fn deselect(&mut self) {
        self.current_channel = None;
    }

    pub fn get_child_adapter(&self, channel: u8) -> Option<&I2cAdapter> {
        self.child_adapters.get(channel as usize)
    }
}

// ─── I2C Slave Mode ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cSlaveEvent {
    WriteRequested,
    ReadRequested,
    WriteReceived,
    ReadProcessed,
    Stop,
}

pub struct I2cSlaveCallback {
    pub addr: u16,
    pub buffer: Vec<u8>,
    pub reg_ptr: u8,
    pub active: bool,
}

impl I2cSlaveCallback {
    pub fn new(addr: u16, register_count: u8) -> Self {
        Self {
            addr,
            buffer: alloc::vec![0u8; register_count as usize],
            reg_ptr: 0,
            active: true,
        }
    }

    pub fn handle_event(&mut self, event: I2cSlaveEvent, data: Option<u8>) -> Option<u8> {
        match event {
            I2cSlaveEvent::WriteRequested => {
                self.reg_ptr = 0;
                None
            }
            I2cSlaveEvent::WriteReceived => {
                if let Some(byte) = data {
                    if self.reg_ptr == 0 {
                        self.reg_ptr = byte;
                    } else if (self.reg_ptr as usize) < self.buffer.len() {
                        self.buffer[self.reg_ptr as usize] = byte;
                        self.reg_ptr = self.reg_ptr.wrapping_add(1);
                    }
                }
                None
            }
            I2cSlaveEvent::ReadRequested | I2cSlaveEvent::ReadProcessed => {
                let val = if (self.reg_ptr as usize) < self.buffer.len() {
                    self.buffer[self.reg_ptr as usize]
                } else {
                    0xFF
                };
                self.reg_ptr = self.reg_ptr.wrapping_add(1);
                Some(val)
            }
            I2cSlaveEvent::Stop => None,
        }
    }
}

// ─── SPI Core Types ──────────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiMode {
    Mode0 = 0x00,
    Mode1 = 0x01,
    Mode2 = 0x02,
    Mode3 = 0x03,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiFlags {
    CpolHigh = 0x02,
    CphaSecond = 0x01,
    LsbFirst = 0x08,
    ThreeWire = 0x10,
    Loop = 0x20,
    NoCs = 0x40,
    Ready = 0x80,
    CsHigh = 0x04,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiError {
    BusError,
    Timeout,
    InvalidConfig,
    NoDevice,
    TransferFailed,
    BusBusy,
}

// ─── SPI Transfer ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SpiTransfer {
    pub tx_buf: Vec<u8>,
    pub rx_buf: Vec<u8>,
    pub len: u32,
    pub speed_hz: u32,
    pub bits_per_word: u8,
    pub cs_change: bool,
    pub delay_usecs: u16,
    pub tx_nbits: u8,
    pub rx_nbits: u8,
}

impl SpiTransfer {
    pub fn new(len: u32) -> Self {
        Self {
            tx_buf: alloc::vec![0u8; len as usize],
            rx_buf: alloc::vec![0u8; len as usize],
            len,
            speed_hz: 0,
            bits_per_word: 8,
            cs_change: false,
            delay_usecs: 0,
            tx_nbits: 1,
            rx_nbits: 1,
        }
    }

    pub fn write(data: &[u8]) -> Self {
        Self {
            tx_buf: data.to_vec(),
            rx_buf: alloc::vec![0u8; data.len()],
            len: data.len() as u32,
            speed_hz: 0,
            bits_per_word: 8,
            cs_change: false,
            delay_usecs: 0,
            tx_nbits: 1,
            rx_nbits: 1,
        }
    }

    pub fn read(len: u32) -> Self {
        Self {
            tx_buf: alloc::vec![0u8; len as usize],
            rx_buf: alloc::vec![0u8; len as usize],
            len,
            speed_hz: 0,
            bits_per_word: 8,
            cs_change: false,
            delay_usecs: 0,
            tx_nbits: 1,
            rx_nbits: 1,
        }
    }

    pub fn write_then_read(tx: &[u8], rx_len: u32) -> Vec<Self> {
        alloc::vec![Self::write(tx), Self::read(rx_len),]
    }
}

#[derive(Debug, Clone)]
pub struct SpiMessage {
    pub transfers: Vec<SpiTransfer>,
    pub status: i32,
    pub actual_length: u32,
}

impl SpiMessage {
    pub fn new() -> Self {
        Self {
            transfers: Vec::new(),
            status: 0,
            actual_length: 0,
        }
    }

    pub fn add_transfer(&mut self, transfer: SpiTransfer) {
        self.transfers.push(transfer);
    }

    pub fn total_length(&self) -> u32 {
        self.transfers.iter().map(|t| t.len).sum()
    }
}

// ─── SPI Device ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SpiDevice {
    pub bus_num: u32,
    pub chip_select: u8,
    pub mode: SpiMode,
    pub bits_per_word: u8,
    pub max_speed_hz: u32,
    pub flags: u32,
    pub name: String,
    pub of_compatible: Option<String>,
}

impl SpiDevice {
    pub fn new(bus_num: u32, cs: u8, name: &str, max_speed: u32) -> Self {
        Self {
            bus_num,
            chip_select: cs,
            mode: SpiMode::Mode0,
            bits_per_word: 8,
            max_speed_hz: max_speed,
            flags: 0,
            name: String::from(name),
            of_compatible: None,
        }
    }

    pub fn set_mode(&mut self, mode: SpiMode) {
        self.mode = mode;
    }

    pub fn set_lsb_first(&mut self) {
        self.flags |= SpiFlags::LsbFirst as u32;
    }

    pub fn set_cs_high(&mut self) {
        self.flags |= SpiFlags::CsHigh as u32;
    }

    pub fn set_three_wire(&mut self) {
        self.flags |= SpiFlags::ThreeWire as u32;
    }
}

// ─── SPI Controller ──────────────────────────────────────────────────────────

pub struct SpiController {
    pub bus_num: u32,
    pub name: String,
    pub num_chipselect: u8,
    pub mode_bits: u32,
    pub bits_per_word_mask: u32,
    pub min_speed_hz: u32,
    pub max_speed_hz: u32,
    pub devices: Vec<SpiDevice>,
    pub queue: Vec<SpiMessage>,
    pub busy: bool,
    pub dma_capable: bool,
}

impl SpiController {
    pub fn new(bus_num: u32, name: &str, num_cs: u8) -> Self {
        Self {
            bus_num,
            name: String::from(name),
            num_chipselect: num_cs,
            mode_bits: 0x03FF,
            bits_per_word_mask: 0xFF,
            min_speed_hz: 100_000,
            max_speed_hz: 50_000_000,
            devices: Vec::new(),
            queue: Vec::new(),
            busy: false,
            dma_capable: false,
        }
    }

    pub fn register_device(&mut self, device: SpiDevice) -> Result<(), SpiError> {
        if device.chip_select >= self.num_chipselect {
            return Err(SpiError::InvalidConfig);
        }
        if self
            .devices
            .iter()
            .any(|d| d.chip_select == device.chip_select)
        {
            return Err(SpiError::BusBusy);
        }
        self.devices.push(device);
        Ok(())
    }

    pub fn transfer(&mut self, cs: u8, msg: &mut SpiMessage) -> Result<(), SpiError> {
        if cs >= self.num_chipselect {
            return Err(SpiError::InvalidConfig);
        }
        if self.busy {
            self.queue.push(msg.clone());
            return Ok(());
        }
        self.busy = true;
        for transfer in &mut msg.transfers {
            let speed = if transfer.speed_hz > 0 {
                transfer.speed_hz.min(self.max_speed_hz)
            } else {
                self.max_speed_hz
            };
            let _ = speed;
            for i in 0..transfer.len as usize {
                if i < transfer.tx_buf.len() {
                    transfer.rx_buf[i] = transfer.tx_buf[i];
                }
            }
            msg.actual_length += transfer.len;
        }
        msg.status = 0;
        self.busy = false;
        Ok(())
    }

    pub fn sync_transfer(
        &mut self,
        cs: u8,
        transfers: &mut [SpiTransfer],
    ) -> Result<u32, SpiError> {
        let mut msg = SpiMessage::new();
        for t in transfers.iter() {
            msg.add_transfer(t.clone());
        }
        self.transfer(cs, &mut msg)?;
        for (i, t) in msg.transfers.iter().enumerate() {
            if i < transfers.len() {
                transfers[i].rx_buf = t.rx_buf.clone();
            }
        }
        Ok(msg.actual_length)
    }
}

// ─── Bitbanging SPI ──────────────────────────────────────────────────────────

pub struct SpiBitbang {
    pub mosi_state: bool,
    pub miso_state: bool,
    pub sck_state: bool,
    pub cs_states: Vec<bool>,
    pub mode: SpiMode,
    pub delay_ns: u32,
    pub controller: SpiController,
}

impl SpiBitbang {
    pub fn new(bus_num: u32, num_cs: u8) -> Self {
        Self {
            mosi_state: false,
            miso_state: false,
            sck_state: false,
            cs_states: alloc::vec![true; num_cs as usize],
            mode: SpiMode::Mode0,
            delay_ns: 100,
            controller: SpiController::new(bus_num, "spi-gpio", num_cs),
        }
    }

    fn set_mosi(&mut self, high: bool) {
        self.mosi_state = high;
    }

    fn get_miso(&self) -> bool {
        self.miso_state
    }

    fn set_sck(&mut self, high: bool) {
        self.sck_state = high;
    }

    fn set_cs(&mut self, cs: u8, active: bool) {
        if let Some(state) = self.cs_states.get_mut(cs as usize) {
            *state = !active;
        }
    }

    fn delay(&self) {
        for _ in 0..self.delay_ns {
            core::hint::spin_loop();
        }
    }

    pub fn transfer_byte(&mut self, tx: u8) -> u8 {
        let mut rx: u8 = 0;
        let cpol = matches!(self.mode, SpiMode::Mode2 | SpiMode::Mode3);
        let cpha = matches!(self.mode, SpiMode::Mode1 | SpiMode::Mode3);

        for i in (0..8).rev() {
            if !cpha {
                self.set_mosi((tx >> i) & 1 != 0);
                self.delay();
            }
            self.set_sck(!cpol);
            self.delay();
            if cpha {
                self.set_mosi((tx >> i) & 1 != 0);
                self.delay();
            }
            if self.get_miso() {
                rx |= 1 << i;
            }
            self.set_sck(cpol);
            self.delay();
            if !cpha && self.get_miso() {
                rx |= 1 << i;
            }
        }
        rx
    }

    pub fn transfer(&mut self, cs: u8, tx: &[u8], rx: &mut [u8]) -> Result<(), SpiError> {
        self.set_cs(cs, true);
        self.delay();
        for (i, &byte) in tx.iter().enumerate() {
            let received = self.transfer_byte(byte);
            if i < rx.len() {
                rx[i] = received;
            }
        }
        self.set_cs(cs, false);
        Ok(())
    }
}

// ─── SPI NOR Flash ───────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiNorCmd {
    WriteEnable = 0x06,
    WriteDisable = 0x04,
    ReadStatusReg1 = 0x05,
    ReadStatusReg2 = 0x35,
    WriteStatusReg = 0x01,
    PageProgram = 0x02,
    Read = 0x03,
    FastRead = 0x0B,
    SectorErase4k = 0x20,
    BlockErase32k = 0x52,
    BlockErase64k = 0xD8,
    ChipErase = 0xC7,
    ReadJedecId = 0x9F,
    EnableQpi = 0x38,
    EnableReset = 0x66,
    Reset = 0x99,
    ReadSfdp = 0x5A,
    DualRead = 0x3B,
    QuadRead = 0x6B,
    QuadPageProgram = 0x32,
    PowerDown = 0xB9,
    ReleasePowerDown = 0xAB,
}

#[derive(Debug, Clone)]
pub struct SpiNorFlashInfo {
    pub name: String,
    pub jedec_id: [u8; 3],
    pub sector_size: u32,
    pub n_sectors: u32,
    pub page_size: u32,
    pub flags: u32,
    pub quad_enable_bit: u8,
}

pub struct SpiNorFlash {
    pub info: SpiNorFlashInfo,
    pub bus_num: u32,
    pub chip_select: u8,
    pub write_protected: bool,
    pub qpi_enabled: bool,
    pub status_reg1: u8,
    pub status_reg2: u8,
    pub storage: Vec<u8>,
}

impl SpiNorFlash {
    pub fn new(bus_num: u32, cs: u8, info: SpiNorFlashInfo) -> Self {
        let total_size = info.sector_size * info.n_sectors;
        Self {
            info,
            bus_num,
            chip_select: cs,
            write_protected: false,
            qpi_enabled: false,
            status_reg1: 0,
            status_reg2: 0,
            storage: alloc::vec![0xFF; total_size as usize],
        }
    }

    pub fn total_size(&self) -> u32 {
        self.info.sector_size * self.info.n_sectors
    }

    pub fn read_jedec_id(&self) -> [u8; 3] {
        self.info.jedec_id
    }

    pub fn read_status_reg1(&self) -> u8 {
        self.status_reg1
    }

    pub fn read_status_reg2(&self) -> u8 {
        self.status_reg2
    }

    pub fn is_busy(&self) -> bool {
        self.status_reg1 & 0x01 != 0
    }

    pub fn is_write_enabled(&self) -> bool {
        self.status_reg1 & 0x02 != 0
    }

    pub fn write_enable(&mut self) {
        self.status_reg1 |= 0x02;
    }

    pub fn write_disable(&mut self) {
        self.status_reg1 &= !0x02;
    }

    pub fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), SpiError> {
        let start = addr as usize;
        let end = start + buf.len();
        if end > self.storage.len() {
            return Err(SpiError::InvalidConfig);
        }
        buf.copy_from_slice(&self.storage[start..end]);
        Ok(())
    }

    pub fn page_program(&mut self, addr: u32, data: &[u8]) -> Result<(), SpiError> {
        if !self.is_write_enabled() {
            return Err(SpiError::TransferFailed);
        }
        if self.write_protected {
            return Err(SpiError::TransferFailed);
        }
        let page_offset = addr % self.info.page_size;
        let max_write = (self.info.page_size - page_offset) as usize;
        let write_len = data.len().min(max_write);
        let start = addr as usize;
        if start + write_len > self.storage.len() {
            return Err(SpiError::InvalidConfig);
        }
        for i in 0..write_len {
            self.storage[start + i] &= data[i];
        }
        self.write_disable();
        Ok(())
    }

    pub fn sector_erase(&mut self, addr: u32) -> Result<(), SpiError> {
        if !self.is_write_enabled() || self.write_protected {
            return Err(SpiError::TransferFailed);
        }
        let sector_start = (addr / self.info.sector_size) * self.info.sector_size;
        let start = sector_start as usize;
        let end = start + self.info.sector_size as usize;
        if end > self.storage.len() {
            return Err(SpiError::InvalidConfig);
        }
        for byte in &mut self.storage[start..end] {
            *byte = 0xFF;
        }
        self.write_disable();
        Ok(())
    }

    pub fn block_erase_32k(&mut self, addr: u32) -> Result<(), SpiError> {
        if !self.is_write_enabled() || self.write_protected {
            return Err(SpiError::TransferFailed);
        }
        let block_start = (addr / 32768) * 32768;
        let start = block_start as usize;
        let end = (start + 32768).min(self.storage.len());
        for byte in &mut self.storage[start..end] {
            *byte = 0xFF;
        }
        self.write_disable();
        Ok(())
    }

    pub fn block_erase_64k(&mut self, addr: u32) -> Result<(), SpiError> {
        if !self.is_write_enabled() || self.write_protected {
            return Err(SpiError::TransferFailed);
        }
        let block_start = (addr / 65536) * 65536;
        let start = block_start as usize;
        let end = (start + 65536).min(self.storage.len());
        for byte in &mut self.storage[start..end] {
            *byte = 0xFF;
        }
        self.write_disable();
        Ok(())
    }

    pub fn chip_erase(&mut self) -> Result<(), SpiError> {
        if !self.is_write_enabled() || self.write_protected {
            return Err(SpiError::TransferFailed);
        }
        for byte in &mut self.storage {
            *byte = 0xFF;
        }
        self.write_disable();
        Ok(())
    }

    pub fn set_write_protect(&mut self, protect: bool) {
        self.write_protected = protect;
        if protect {
            self.status_reg1 |= 0x1C;
        } else {
            self.status_reg1 &= !0x1C;
        }
    }

    pub fn enable_qpi(&mut self) -> Result<(), SpiError> {
        self.status_reg2 |= 1 << self.info.quad_enable_bit;
        self.qpi_enabled = true;
        Ok(())
    }

    pub fn disable_qpi(&mut self) {
        self.status_reg2 &= !(1 << self.info.quad_enable_bit);
        self.qpi_enabled = false;
    }

    pub fn power_down(&mut self) {
        self.status_reg1 |= 0x80;
    }

    pub fn release_power_down(&mut self) {
        self.status_reg1 &= !0x80;
    }
}

// ─── Device Tree Binding Model ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DeviceTreeBinding {
    pub compatible: String,
    pub reg: Vec<u32>,
    pub interrupts: Vec<u32>,
    pub clocks: Vec<u32>,
    pub clock_frequency: Option<u32>,
    pub properties: Vec<(String, String)>,
}

impl DeviceTreeBinding {
    pub fn new(compatible: &str) -> Self {
        Self {
            compatible: String::from(compatible),
            reg: Vec::new(),
            interrupts: Vec::new(),
            clocks: Vec::new(),
            clock_frequency: None,
            properties: Vec::new(),
        }
    }

    pub fn set_reg(&mut self, addr: u32, size: u32) {
        self.reg = alloc::vec![addr, size];
    }

    pub fn add_interrupt(&mut self, irq: u32) {
        self.interrupts.push(irq);
    }

    pub fn set_clock_frequency(&mut self, freq: u32) {
        self.clock_frequency = Some(freq);
    }

    pub fn add_property(&mut self, key: &str, value: &str) {
        self.properties
            .push((String::from(key), String::from(value)));
    }
}

// ─── Runtime Power Management ────────────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusPowerState {
    Active = 0,
    Idle = 1,
    Suspended = 2,
    Off = 3,
}

pub struct BusPowerManager {
    pub state: BusPowerState,
    pub auto_suspend_delay_ms: u32,
    pub usage_count: i32,
    pub last_busy_time: u64,
    pub autosuspend_enabled: bool,
}

impl BusPowerManager {
    pub fn new() -> Self {
        Self {
            state: BusPowerState::Active,
            auto_suspend_delay_ms: 2000,
            usage_count: 0,
            last_busy_time: 0,
            autosuspend_enabled: true,
        }
    }

    pub fn get(&mut self) -> Result<(), I2cError> {
        self.usage_count += 1;
        if self.state != BusPowerState::Active {
            self.state = BusPowerState::Active;
        }
        Ok(())
    }

    pub fn put(&mut self) {
        self.usage_count -= 1;
        if self.usage_count <= 0 && self.autosuspend_enabled {
            self.state = BusPowerState::Idle;
        }
    }

    pub fn suspend(&mut self) -> Result<(), I2cError> {
        if self.usage_count > 0 {
            return Err(I2cError::BusBusy);
        }
        self.state = BusPowerState::Suspended;
        Ok(())
    }

    pub fn resume(&mut self) {
        self.state = BusPowerState::Active;
    }

    pub fn mark_busy(&mut self) {
        self.last_busy_time += 1;
    }
}

// ─── Bus Frequency Scaling ───────────────────────────────────────────────────

pub struct BusFrequencyScaler {
    pub current_freq: u32,
    pub min_freq: u32,
    pub max_freq: u32,
    pub available_freqs: Vec<u32>,
    pub auto_scale: bool,
    pub load_threshold_up: u32,
    pub load_threshold_down: u32,
    pub current_load: u32,
}

impl BusFrequencyScaler {
    pub fn new_i2c() -> Self {
        Self {
            current_freq: 100_000,
            min_freq: 100_000,
            max_freq: 3_400_000,
            available_freqs: alloc::vec![100_000, 400_000, 1_000_000, 3_400_000],
            auto_scale: false,
            load_threshold_up: 80,
            load_threshold_down: 20,
            current_load: 0,
        }
    }

    pub fn new_spi() -> Self {
        Self {
            current_freq: 1_000_000,
            min_freq: 100_000,
            max_freq: 100_000_000,
            available_freqs: alloc::vec![
                100_000,
                1_000_000,
                10_000_000,
                25_000_000,
                50_000_000,
                100_000_000
            ],
            auto_scale: false,
            load_threshold_up: 80,
            load_threshold_down: 20,
            current_load: 0,
        }
    }

    pub fn set_frequency(&mut self, freq: u32) -> Result<(), I2cError> {
        if freq < self.min_freq || freq > self.max_freq {
            return Err(I2cError::InvalidData);
        }
        let closest = self
            .available_freqs
            .iter()
            .min_by_key(|&&f| (f as i64 - freq as i64).unsigned_abs())
            .copied()
            .unwrap_or(freq);
        self.current_freq = closest;
        Ok(())
    }

    pub fn update_load(&mut self, load: u32) {
        self.current_load = load;
        if !self.auto_scale {
            return;
        }
        if load > self.load_threshold_up {
            self.scale_up();
        } else if load < self.load_threshold_down {
            self.scale_down();
        }
    }

    fn scale_up(&mut self) {
        if let Some(&next) = self
            .available_freqs
            .iter()
            .find(|&&f| f > self.current_freq)
        {
            self.current_freq = next;
        }
    }

    fn scale_down(&mut self) {
        if let Some(&prev) = self
            .available_freqs
            .iter()
            .rev()
            .find(|&&f| f < self.current_freq)
        {
            self.current_freq = prev;
        }
    }
}

// ─── Global I2C Bus ──────────────────────────────────────────────────────────

pub struct I2cBusState {
    pub adapters: Vec<I2cAdapter>,
    pub muxes: Vec<I2cMux>,
    pub drivers: Vec<I2cDriver>,
    pub bitbang_adapters: Vec<I2cBitbang>,
    pub power_managers: Vec<BusPowerManager>,
    pub freq_scaler: BusFrequencyScaler,
    pub next_bus_num: u32,
    pub initialized: bool,
}

impl I2cBusState {
    pub const fn new() -> Self {
        Self {
            adapters: Vec::new(),
            muxes: Vec::new(),
            drivers: Vec::new(),
            bitbang_adapters: Vec::new(),
            power_managers: Vec::new(),
            freq_scaler: BusFrequencyScaler {
                current_freq: 100_000,
                min_freq: 100_000,
                max_freq: 3_400_000,
                available_freqs: Vec::new(),
                auto_scale: false,
                load_threshold_up: 80,
                load_threshold_down: 20,
                current_load: 0,
            },
            next_bus_num: 0,
            initialized: false,
        }
    }

    pub fn register_adapter(&mut self, name: &str, clock_freq: u32) -> u32 {
        let bus = self.next_bus_num;
        self.next_bus_num += 1;
        self.adapters.push(I2cAdapter::new(bus, name, clock_freq));
        self.power_managers.push(BusPowerManager::new());
        bus
    }

    pub fn get_adapter(&self, bus: u32) -> Option<&I2cAdapter> {
        self.adapters.iter().find(|a| a.bus_num == bus)
    }

    pub fn get_adapter_mut(&mut self, bus: u32) -> Option<&mut I2cAdapter> {
        self.adapters.iter_mut().find(|a| a.bus_num == bus)
    }

    pub fn register_mux(&mut self, parent_bus: u32, addr: u16, channels: u8) {
        self.muxes
            .push(I2cMux::new(parent_bus, addr, channels, "i2c-mux-pca9548"));
    }

    pub fn register_driver(&mut self, driver: I2cDriver) {
        self.drivers.push(driver);
    }
}

pub static I2C_BUS: Mutex<I2cBusState> = Mutex::new(I2cBusState::new());

// ─── Global SPI Bus ──────────────────────────────────────────────────────────

pub struct SpiBusState {
    pub controllers: Vec<SpiController>,
    pub bitbang_controllers: Vec<SpiBitbang>,
    pub nor_flash_devices: Vec<SpiNorFlash>,
    pub power_managers: Vec<BusPowerManager>,
    pub freq_scaler: BusFrequencyScaler,
    pub next_bus_num: u32,
    pub initialized: bool,
}

impl SpiBusState {
    pub const fn new() -> Self {
        Self {
            controllers: Vec::new(),
            bitbang_controllers: Vec::new(),
            nor_flash_devices: Vec::new(),
            power_managers: Vec::new(),
            freq_scaler: BusFrequencyScaler {
                current_freq: 1_000_000,
                min_freq: 100_000,
                max_freq: 100_000_000,
                available_freqs: Vec::new(),
                auto_scale: false,
                load_threshold_up: 80,
                load_threshold_down: 20,
                current_load: 0,
            },
            next_bus_num: 0,
            initialized: false,
        }
    }

    pub fn register_controller(&mut self, name: &str, num_cs: u8) -> u32 {
        let bus = self.next_bus_num;
        self.next_bus_num += 1;
        self.controllers.push(SpiController::new(bus, name, num_cs));
        self.power_managers.push(BusPowerManager::new());
        bus
    }

    pub fn get_controller(&self, bus: u32) -> Option<&SpiController> {
        self.controllers.iter().find(|c| c.bus_num == bus)
    }

    pub fn get_controller_mut(&mut self, bus: u32) -> Option<&mut SpiController> {
        self.controllers.iter_mut().find(|c| c.bus_num == bus)
    }

    pub fn register_nor_flash(
        &mut self,
        bus: u32,
        cs: u8,
        name: &str,
        jedec: [u8; 3],
        size_kb: u32,
    ) {
        let info = SpiNorFlashInfo {
            name: String::from(name),
            jedec_id: jedec,
            sector_size: 4096,
            n_sectors: size_kb * 1024 / 4096,
            page_size: 256,
            flags: 0,
            quad_enable_bit: 1,
        };
        self.nor_flash_devices.push(SpiNorFlash::new(bus, cs, info));
    }
}

pub static SPI_BUS: Mutex<SpiBusState> = Mutex::new(SpiBusState::new());

// ─── Initialization ──────────────────────────────────────────────────────────

pub fn init() {
    let mut i2c = I2C_BUS.lock();
    let bus0 = i2c.register_adapter("i2c-raeen-0", 400_000);
    let bus1 = i2c.register_adapter("i2c-raeen-1", 100_000);

    if let Some(adapter) = i2c.get_adapter_mut(bus0) {
        adapter.register_device(I2cDeviceInfo::new(0x50, "eeprom"));
        adapter.register_device(I2cDeviceInfo::new(0x68, "rtc-ds3231"));
        adapter.register_device(I2cDeviceInfo::new(0x76, "bme280"));
    }
    if let Some(adapter) = i2c.get_adapter_mut(bus1) {
        adapter.register_device(I2cDeviceInfo::new(0x3C, "ssd1306-oled"));
        adapter.register_device(I2cDeviceInfo::new(0x27, "pcf8574-gpio"));
    }

    i2c.register_mux(bus0, 0x70, 8);
    i2c.freq_scaler = BusFrequencyScaler::new_i2c();
    i2c.initialized = true;
    drop(i2c);

    let mut spi = SPI_BUS.lock();
    let spi_bus0 = spi.register_controller("spi-raeen-0", 4);
    let _spi_bus1 = spi.register_controller("spi-raeen-1", 2);

    if let Some(ctrl) = spi.get_controller_mut(spi_bus0) {
        let _ = ctrl.register_device(SpiDevice::new(spi_bus0, 0, "w25q128", 50_000_000));
        let _ = ctrl.register_device(SpiDevice::new(spi_bus0, 1, "enc28j60", 20_000_000));
    }

    spi.register_nor_flash(spi_bus0, 0, "W25Q128FV", [0xEF, 0x40, 0x18], 16384);
    spi.freq_scaler = BusFrequencyScaler::new_spi();
    spi.initialized = true;
}
