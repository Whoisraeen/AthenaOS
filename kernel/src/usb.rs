#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbSpeed {
    Low,
    Full,
    High,
    Super,
    SuperPlus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbClass {
    Hid,
    MassStorage,
    Audio,
    Video,
    Printer,
    Hub,
    Vendor(u8, u8),
    Unknown,
}

#[derive(Debug)]
pub struct UsbDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub speed: UsbSpeed,
    pub class: UsbClass,
    pub bus: u8,
    pub address: u8,
    pub manufacturer: Option<&'static str>,
    pub product: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointDirection {
    In,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    Control,
    Bulk,
    Interrupt,
    Isochronous,
}

#[derive(Debug)]
pub struct UsbEndpoint {
    pub address: u8,
    pub direction: EndpointDirection,
    pub transfer_type: TransferType,
    pub max_packet_size: u16,
}

pub trait UsbDriver: Send {
    fn name(&self) -> &str;
    fn probe(&self, device: &UsbDeviceInfo) -> bool;
    fn attach(&mut self, device: &UsbDeviceInfo) -> Result<(), &'static str>;
    fn detach(&mut self, device: &UsbDeviceInfo);
}

pub struct UsbBus {
    devices: Vec<UsbDeviceInfo>,
    drivers: Vec<Box<dyn UsbDriver + Send>>,
}

impl UsbBus {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            drivers: Vec::new(),
        }
    }

    pub fn register_driver(&mut self, driver: Box<dyn UsbDriver + Send>) {
        self.drivers.push(driver);
    }

    pub fn enumerate(&mut self) {
        // Stub: real implementation would walk xHCI/EHCI port registers
        // and issue GET_DESCRIPTOR requests to each attached device.
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn list_devices(&self) -> &[UsbDeviceInfo] {
        &self.devices
    }
}

pub static USB_BUS: Mutex<Option<UsbBus>> = Mutex::new(None);

pub fn init() {
    let mut bus = UsbBus::new();
    bus.enumerate();
    *USB_BUS.lock() = Some(bus);
}
