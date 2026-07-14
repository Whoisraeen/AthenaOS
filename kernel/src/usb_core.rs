#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ─── USB Descriptor Types ───────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorType {
    Device = 1,
    Configuration = 2,
    String = 3,
    Interface = 4,
    Endpoint = 5,
    DeviceQualifier = 6,
    OtherSpeedConfig = 7,
    InterfacePower = 8,
    Otg = 9,
    Debug = 10,
    InterfaceAssociation = 11,
    Bos = 15,
    DeviceCapability = 16,
    SuperSpeedEndpointCompanion = 48,
    SuperSpeedPlusIsochEndpointCompanion = 49,
    Hid = 33,
    HidReport = 34,
    HidPhysical = 35,
    Hub = 41,
    SuperSpeedHub = 42,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCapabilityType {
    WirelessUsb = 1,
    Usb20Extension = 2,
    SuperSpeed = 3,
    ContainerId = 4,
    Platform = 5,
    PowerDeliveryCapability = 6,
    BatteryInfo = 7,
    PdConsumerPort = 8,
    PdProviderPort = 9,
    SuperSpeedPlus = 10,
    PrecisionTimeMeasurement = 11,
    WirelessUsbExt = 12,
    Billboard = 13,
    Authentication = 14,
    BillboardEx = 15,
    ConfigurationSummary = 16,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub bcd_usb: u16,
    pub b_device_class: u8,
    pub b_device_sub_class: u8,
    pub b_device_protocol: u8,
    pub b_max_packet_size0: u8,
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_device: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial_number: u8,
    pub b_num_configurations: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ConfigurationDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub w_total_length: u16,
    pub b_num_interfaces: u8,
    pub b_configuration_value: u8,
    pub i_configuration: u8,
    pub bm_attributes: u8,
    pub b_max_power: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct InterfaceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_interface_number: u8,
    pub b_alternate_setting: u8,
    pub b_num_endpoints: u8,
    pub b_interface_class: u8,
    pub b_interface_sub_class: u8,
    pub b_interface_protocol: u8,
    pub i_interface: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct EndpointDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_endpoint_address: u8,
    pub bm_attributes: u8,
    pub w_max_packet_size: u16,
    pub b_interval: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct StringDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub w_data: [u16; 126],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BosDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub w_total_length: u16,
    pub b_num_device_caps: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Usb20ExtensionDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_dev_capability_type: u8,
    pub bm_attributes: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SuperSpeedCapDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_dev_capability_type: u8,
    pub bm_attributes: u8,
    pub w_speeds_supported: u16,
    pub b_functionality_support: u8,
    pub b_u1_dev_exit_lat: u8,
    pub w_u2_dev_exit_lat: u16,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SuperSpeedPlusCapDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_dev_capability_type: u8,
    pub b_reserved: u8,
    pub bm_attributes: u32,
    pub w_functionality_support: u16,
    pub w_reserved: u16,
    pub bm_sublink_speed_attr: [u32; 16],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ContainerIdDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_dev_capability_type: u8,
    pub b_reserved: u8,
    pub container_id: [u8; 16],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct PlatformDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_dev_capability_type: u8,
    pub b_reserved: u8,
    pub platform_capability_uuid: [u8; 16],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BillboardDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_dev_capability_type: u8,
    pub i_additional_info_url: u8,
    pub b_num_alternate_modes: u8,
    pub b_preferred_alternate_mode: u8,
    pub vconn_power: u16,
    pub bm_configured: [u8; 32],
    pub b_reserved: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HidDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub bcd_hid: u16,
    pub b_country_code: u8,
    pub b_num_descriptors: u8,
    pub b_descriptor_type_class: u8,
    pub w_descriptor_length: u16,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HubDescriptor {
    pub b_desc_length: u8,
    pub b_descriptor_type: u8,
    pub b_nbr_ports: u8,
    pub w_hub_characteristics: u16,
    pub b_pwr_on_2_pwr_good: u8,
    pub b_hub_contr_current: u8,
    pub device_removable: [u8; 32],
}

// ─── USB Speed ──────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbSpeed {
    Low = 0,
    Full = 1,
    High = 2,
    Super = 3,
    SuperPlus = 4,
    SuperPlusX2 = 5,
}

impl UsbSpeed {
    pub fn max_packet_size_ep0(&self) -> u16 {
        match self {
            UsbSpeed::Low => 8,
            UsbSpeed::Full => 64,
            UsbSpeed::High => 64,
            UsbSpeed::Super | UsbSpeed::SuperPlus | UsbSpeed::SuperPlusX2 => 512,
        }
    }
}

// ─── USB Transfer Types ─────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    Control = 0,
    Isochronous = 1,
    Bulk = 2,
    Interrupt = 3,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Out = 0,
    In = 1,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SetupPacket {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}

impl SetupPacket {
    pub fn new(
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        w_length: u16,
    ) -> Self {
        Self {
            bm_request_type,
            b_request,
            w_value,
            w_index,
            w_length,
        }
    }

    pub fn direction(&self) -> Direction {
        if self.bm_request_type & 0x80 != 0 {
            Direction::In
        } else {
            Direction::Out
        }
    }

    pub fn request_type_bits(&self) -> u8 {
        (self.bm_request_type >> 5) & 0x03
    }

    pub fn recipient(&self) -> u8 {
        self.bm_request_type & 0x1F
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardRequest {
    GetStatus = 0,
    ClearFeature = 1,
    SetFeature = 3,
    SetAddress = 5,
    GetDescriptor = 6,
    SetDescriptor = 7,
    GetConfiguration = 8,
    SetConfiguration = 9,
    GetInterface = 10,
    SetInterface = 11,
    SynchFrame = 12,
    SetSel = 48,
    SetIsochDelay = 49,
}

// ─── USB Device Lifecycle ───────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Detached = 0,
    Attached = 1,
    Powered = 2,
    Default = 3,
    Addressed = 4,
    Configured = 5,
    Suspended = 6,
}

#[derive(Debug)]
pub struct UsbDevice {
    pub address: u8,
    pub speed: UsbSpeed,
    pub state: DeviceState,
    pub max_packet_size_ep0: u16,
    pub device_descriptor: Option<DeviceDescriptor>,
    pub configurations: Vec<ConfigurationDescriptor>,
    pub interfaces: Vec<InterfaceDescriptor>,
    pub endpoints: Vec<EndpointDescriptor>,
    pub active_configuration: u8,
    pub parent_hub: Option<u8>,
    pub port_number: u8,
    pub slot_id: u8,
    pub string_manufacturer: Option<String>,
    pub string_product: Option<String>,
    pub string_serial: Option<String>,
    pub class_driver: Option<UsbClassDriver>,
}

impl UsbDevice {
    pub fn new(speed: UsbSpeed) -> Self {
        Self {
            address: 0,
            speed,
            state: DeviceState::Attached,
            max_packet_size_ep0: speed.max_packet_size_ep0(),
            device_descriptor: None,
            configurations: Vec::new(),
            interfaces: Vec::new(),
            endpoints: Vec::new(),
            active_configuration: 0,
            parent_hub: None,
            port_number: 0,
            slot_id: 0,
            string_manufacturer: None,
            string_product: None,
            string_serial: None,
            class_driver: None,
        }
    }

    pub fn set_address(&mut self, address: u8) {
        self.address = address;
        self.state = DeviceState::Addressed;
    }

    pub fn set_configured(&mut self, config_value: u8) {
        self.active_configuration = config_value;
        self.state = DeviceState::Configured;
    }

    pub fn suspend(&mut self) {
        self.state = DeviceState::Suspended;
    }

    pub fn resume(&mut self, prev_state: DeviceState) {
        self.state = prev_state;
    }

    pub fn reset(&mut self) {
        self.address = 0;
        self.state = DeviceState::Default;
        self.active_configuration = 0;
    }
}

// ─── USB Pipe ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct UsbPipe {
    pub device_address: u8,
    pub endpoint_number: u8,
    pub direction: Direction,
    pub transfer_type: TransferType,
    pub max_packet_size: u16,
    pub interval: u8,
}

impl UsbPipe {
    pub fn control(device_address: u8, max_packet_size: u16) -> Self {
        Self {
            device_address,
            endpoint_number: 0,
            direction: Direction::Out,
            transfer_type: TransferType::Control,
            max_packet_size,
            interval: 0,
        }
    }

    pub fn from_endpoint(device_address: u8, ep: &EndpointDescriptor) -> Self {
        let direction = if ep.b_endpoint_address & 0x80 != 0 {
            Direction::In
        } else {
            Direction::Out
        };
        let transfer_type = match ep.bm_attributes & 0x03 {
            0 => TransferType::Control,
            1 => TransferType::Isochronous,
            2 => TransferType::Bulk,
            _ => TransferType::Interrupt,
        };
        Self {
            device_address,
            endpoint_number: ep.b_endpoint_address & 0x0F,
            direction,
            transfer_type,
            max_packet_size: ep.w_max_packet_size & 0x07FF,
            interval: ep.b_interval,
        }
    }

    pub fn encode(&self) -> u32 {
        ((self.device_address as u32) << 16)
            | ((self.endpoint_number as u32) << 8)
            | ((self.direction as u32) << 7)
            | (self.transfer_type as u32)
    }
}

// ─── URB (USB Request Block) ────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrbStatus {
    Pending = 0,
    Completed = 1,
    Error = 2,
    Stall = 3,
    Cancelled = 4,
    TimedOut = 5,
    Overflow = 6,
    ShortPacket = 7,
    NoDevice = 8,
    Babble = 9,
}

#[derive(Debug)]
pub struct Urb {
    pub id: u64,
    pub pipe: UsbPipe,
    pub setup_packet: Option<SetupPacket>,
    pub transfer_buffer: Vec<u8>,
    pub transfer_buffer_length: usize,
    pub actual_length: usize,
    pub status: UrbStatus,
    pub interval: u8,
    pub start_frame: u32,
    pub number_of_packets: u32,
    pub error_count: u32,
    pub flags: u32,
}

pub const URB_SHORT_NOT_OK: u32 = 0x0001;
pub const URB_ZERO_PACKET: u32 = 0x0040;
pub const URB_NO_TRANSFER_DMA_MAP: u32 = 0x0004;
pub const URB_NO_INTERRUPT: u32 = 0x0080;

impl Urb {
    pub fn new(pipe: UsbPipe, buffer_len: usize) -> Self {
        Self {
            id: 0,
            pipe,
            setup_packet: None,
            transfer_buffer: alloc::vec![0u8; buffer_len],
            transfer_buffer_length: buffer_len,
            actual_length: 0,
            status: UrbStatus::Pending,
            interval: 0,
            start_frame: 0,
            number_of_packets: 0,
            error_count: 0,
            flags: 0,
        }
    }

    pub fn control(pipe: UsbPipe, setup: SetupPacket, buffer_len: usize) -> Self {
        let mut urb = Self::new(pipe, buffer_len);
        urb.setup_packet = Some(setup);
        urb
    }

    pub fn submit(&mut self) -> Result<(), UsbError> {
        if self.status != UrbStatus::Pending {
            return Err(UsbError::InvalidState);
        }
        let mut core = USB_CORE.lock();
        core.submit_urb(self)
    }

    pub fn cancel(&mut self) {
        self.status = UrbStatus::Cancelled;
    }

    pub fn is_complete(&self) -> bool {
        self.status != UrbStatus::Pending
    }
}

// ─── USB Errors ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbError {
    NoDevice,
    NotConfigured,
    InvalidEndpoint,
    InvalidPipe,
    Stall,
    Timeout,
    Overflow,
    Babble,
    ShortPacket,
    NoMemory,
    InvalidState,
    IoError,
    InvalidDescriptor,
    ProtocolError,
    BufferTooSmall,
    DeviceBusy,
    NotSupported,
    CrcError,
    BitStuffError,
    DataToggleMismatch,
}

// ─── Hub Port Status ────────────────────────────────────────────────────────

macro_rules! bitflags_manual {
    (pub struct $name:ident : $ty:ty { $(const $flag:ident = $val:expr;)* }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name { bits: $ty }

        impl $name {
            $(pub const $flag: Self = Self { bits: $val };)*

            pub const fn empty() -> Self { Self { bits: 0 } }
            pub const fn all() -> Self { Self { bits: 0 $(| $val)* } }
            pub const fn bits(&self) -> $ty { self.bits }
            pub const fn from_bits_truncate(bits: $ty) -> Self { Self { bits: bits & Self::all().bits } }
            pub const fn contains(&self, other: Self) -> bool { (self.bits & other.bits) == other.bits }
            pub const fn intersects(&self, other: Self) -> bool { (self.bits & other.bits) != 0 }
            pub const fn union(self, other: Self) -> Self { Self { bits: self.bits | other.bits } }
            pub const fn intersection(self, other: Self) -> Self { Self { bits: self.bits & other.bits } }
        }
    };
}

bitflags_manual! {
    pub struct PortStatus: u16 {
        const CONNECTION   = 1 << 0;
        const ENABLE       = 1 << 1;
        const SUSPEND      = 1 << 2;
        const OVER_CURRENT = 1 << 3;
        const RESET        = 1 << 4;
        const POWER        = 1 << 8;
        const LOW_SPEED    = 1 << 9;
        const HIGH_SPEED   = 1 << 10;
        const PORT_TEST    = 1 << 11;
        const PORT_INDICATOR = 1 << 12;
    }
}

bitflags_manual! {
    pub struct PortChange: u16 {
        const C_CONNECTION   = 1 << 0;
        const C_ENABLE       = 1 << 1;
        const C_SUSPEND      = 1 << 2;
        const C_OVER_CURRENT = 1 << 3;
        const C_RESET        = 1 << 4;
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubPortFeature {
    PortConnection = 0,
    PortEnable = 1,
    PortSuspend = 2,
    PortOverCurrent = 3,
    PortReset = 4,
    PortPower = 8,
    PortLowSpeed = 9,
    CPortConnection = 16,
    CPortEnable = 17,
    CPortSuspend = 18,
    CPortOverCurrent = 19,
    CPortReset = 20,
    PortTest = 21,
    PortIndicator = 22,
}

// ─── Hub Driver ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct UsbHub {
    pub device_address: u8,
    pub num_ports: u8,
    pub characteristics: u16,
    pub power_on_delay_ms: u16,
    pub port_status: Vec<(PortStatus, PortChange)>,
    pub has_tt: bool,
    pub tt_think_time: u8,
    pub multi_tt: bool,
    pub hub_depth: u8,
}

impl UsbHub {
    pub fn new(device_address: u8, descriptor: &HubDescriptor) -> Self {
        let num_ports = descriptor.b_nbr_ports;
        let mut port_status = Vec::with_capacity(num_ports as usize);
        for _ in 0..num_ports {
            port_status.push((PortStatus::empty(), PortChange::empty()));
        }
        let has_tt = (descriptor.w_hub_characteristics & 0x0060) != 0;
        let multi_tt = (descriptor.w_hub_characteristics & 0x0060) == 0x0040;
        Self {
            device_address,
            num_ports,
            characteristics: descriptor.w_hub_characteristics,
            power_on_delay_ms: (descriptor.b_pwr_on_2_pwr_good as u16) * 2,
            port_status,
            has_tt,
            tt_think_time: ((descriptor.w_hub_characteristics >> 5) & 0x03) as u8,
            multi_tt,
            hub_depth: 0,
        }
    }

    pub fn set_port_feature(&mut self, port: u8, feature: HubPortFeature) -> Result<(), UsbError> {
        if port == 0 || port > self.num_ports {
            return Err(UsbError::InvalidEndpoint);
        }
        let _setup = SetupPacket::new(
            0x23,
            StandardRequest::SetFeature as u8,
            feature as u16,
            port as u16,
            0,
        );
        Ok(())
    }

    pub fn clear_port_feature(
        &mut self,
        port: u8,
        feature: HubPortFeature,
    ) -> Result<(), UsbError> {
        if port == 0 || port > self.num_ports {
            return Err(UsbError::InvalidEndpoint);
        }
        let _setup = SetupPacket::new(
            0x23,
            StandardRequest::ClearFeature as u8,
            feature as u16,
            port as u16,
            0,
        );
        Ok(())
    }

    pub fn get_port_status(&self, port: u8) -> Result<(PortStatus, PortChange), UsbError> {
        if port == 0 || port > self.num_ports {
            return Err(UsbError::InvalidEndpoint);
        }
        Ok(self.port_status[(port - 1) as usize])
    }

    pub fn power_on_port(&mut self, port: u8) -> Result<(), UsbError> {
        self.set_port_feature(port, HubPortFeature::PortPower)
    }

    pub fn reset_port(&mut self, port: u8) -> Result<(), UsbError> {
        self.set_port_feature(port, HubPortFeature::PortReset)
    }

    pub fn handle_hub_event(&mut self, port: u8) -> Result<HubEvent, UsbError> {
        let (status, change) = self.get_port_status(port)?;
        if change.contains(PortChange::C_CONNECTION) {
            self.clear_port_feature(port, HubPortFeature::CPortConnection)?;
            if status.contains(PortStatus::CONNECTION) {
                return Ok(HubEvent::DeviceConnected(port));
            } else {
                return Ok(HubEvent::DeviceDisconnected(port));
            }
        }
        if change.contains(PortChange::C_RESET) {
            self.clear_port_feature(port, HubPortFeature::CPortReset)?;
            return Ok(HubEvent::ResetComplete(port));
        }
        if change.contains(PortChange::C_OVER_CURRENT) {
            self.clear_port_feature(port, HubPortFeature::CPortOverCurrent)?;
            return Ok(HubEvent::OverCurrent(port));
        }
        Ok(HubEvent::None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubEvent {
    None,
    DeviceConnected(u8),
    DeviceDisconnected(u8),
    ResetComplete(u8),
    OverCurrent(u8),
    SuspendChange(u8),
}

// Transaction Translator for mixed-speed hubs
#[derive(Debug)]
pub struct TransactionTranslator {
    pub hub_address: u8,
    pub port_number: u8,
    pub multi_tt: bool,
    pub bandwidth_allocated: u32,
}

impl TransactionTranslator {
    pub fn new(hub_address: u8, port_number: u8, multi_tt: bool) -> Self {
        Self {
            hub_address,
            port_number,
            multi_tt,
            bandwidth_allocated: 0,
        }
    }

    pub fn allocate_bandwidth(&mut self, bytes: u32) -> Result<(), UsbError> {
        const MAX_TT_BANDWIDTH: u32 = 900;
        if self.bandwidth_allocated + bytes > MAX_TT_BANDWIDTH {
            return Err(UsbError::NoMemory);
        }
        self.bandwidth_allocated += bytes;
        Ok(())
    }

    pub fn release_bandwidth(&mut self, bytes: u32) {
        self.bandwidth_allocated = self.bandwidth_allocated.saturating_sub(bytes);
    }
}

// ─── USB Device Classes ─────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbClassCode {
    PerInterface = 0x00,
    Audio = 0x01,
    Cdc = 0x02,
    Hid = 0x03,
    Physical = 0x05,
    Image = 0x06,
    Printer = 0x07,
    MassStorage = 0x08,
    Hub = 0x09,
    CdcData = 0x0A,
    SmartCard = 0x0B,
    ContentSecurity = 0x0D,
    Video = 0x0E,
    PersonalHealthcare = 0x0F,
    AudioVideo = 0x10,
    Billboard = 0x11,
    UsbTypeCBridge = 0x12,
    Diagnostic = 0xDC,
    WirelessController = 0xE0,
    Miscellaneous = 0xEF,
    ApplicationSpecific = 0xFE,
    VendorSpecific = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbClassDriver {
    Hid,
    MassStorageBbb,
    MassStorageUas,
    CdcAcm,
    CdcEcm,
    CdcNcm,
    Audio,
    Video,
    Printer,
    Hub,
    WirelessController,
    VendorSpecific,
}

// ─── HID (Human Interface Device) ──────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidItemType {
    Main = 0,
    Global = 1,
    Local = 2,
    Reserved = 3,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidMainTag {
    Input = 0x08,
    Output = 0x09,
    Feature = 0x0B,
    Collection = 0x0A,
    EndCollection = 0x0C,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidGlobalTag {
    UsagePage = 0x00,
    LogicalMinimum = 0x01,
    LogicalMaximum = 0x02,
    PhysicalMinimum = 0x03,
    PhysicalMaximum = 0x04,
    UnitExponent = 0x05,
    Unit = 0x06,
    ReportSize = 0x07,
    ReportId = 0x08,
    ReportCount = 0x09,
    Push = 0x0A,
    Pop = 0x0B,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidLocalTag {
    Usage = 0x00,
    UsageMinimum = 0x01,
    UsageMaximum = 0x02,
    DesignatorIndex = 0x03,
    DesignatorMinimum = 0x04,
    DesignatorMaximum = 0x05,
    StringIndex = 0x07,
    StringMinimum = 0x08,
    StringMaximum = 0x09,
    Delimiter = 0x0A,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidCollectionType {
    Physical = 0x00,
    Application = 0x01,
    Logical = 0x02,
    Report = 0x03,
    NamedArray = 0x04,
    UsageSwitch = 0x05,
    UsageModifier = 0x06,
}

#[derive(Debug, Clone)]
pub struct HidReportField {
    pub usage_page: u16,
    pub usage: u16,
    pub usage_minimum: u16,
    pub usage_maximum: u16,
    pub logical_minimum: i32,
    pub logical_maximum: i32,
    pub physical_minimum: i32,
    pub physical_maximum: i32,
    pub report_size: u32,
    pub report_count: u32,
    pub report_id: u8,
    pub flags: u16,
}

#[derive(Debug, Clone)]
pub struct HidReport {
    pub report_id: u8,
    pub report_type: HidReportType,
    pub fields: Vec<HidReportField>,
    pub total_size_bits: u32,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidReportType {
    Input = 1,
    Output = 2,
    Feature = 3,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidProtocol {
    None = 0,
    Keyboard = 1,
    Mouse = 2,
}

#[derive(Debug)]
pub struct HidDevice {
    pub interface_number: u8,
    pub protocol: HidProtocol,
    pub reports: Vec<HidReport>,
    pub boot_protocol: bool,
    pub idle_rate: u8,
    pub report_descriptor_length: u16,
}

impl HidDevice {
    pub fn new(interface_number: u8, protocol: HidProtocol) -> Self {
        Self {
            interface_number,
            protocol,
            reports: Vec::new(),
            boot_protocol: protocol != HidProtocol::None,
            idle_rate: 0,
            report_descriptor_length: 0,
        }
    }

    pub fn parse_report_descriptor(&mut self, data: &[u8]) -> Result<(), UsbError> {
        let mut offset = 0;
        let mut global_state = HidGlobalState::default();
        let mut local_state = HidLocalState::default();

        while offset < data.len() {
            let prefix = data[offset];
            offset += 1;

            if prefix == 0xFE {
                if offset + 2 > data.len() {
                    return Err(UsbError::InvalidDescriptor);
                }
                let _size = data[offset] as u16 | ((data[offset + 1] as u16) << 8);
                offset += 2;
                continue;
            }

            let size = match prefix & 0x03 {
                0 => 0usize,
                1 => 1,
                2 => 2,
                3 => 4,
                _ => unreachable!(),
            };
            let item_type = (prefix >> 2) & 0x03;
            let tag = (prefix >> 4) & 0x0F;

            if offset + size > data.len() {
                return Err(UsbError::InvalidDescriptor);
            }
            let value = match size {
                0 => 0u32,
                1 => data[offset] as u32,
                2 => (data[offset] as u32) | ((data[offset + 1] as u32) << 8),
                4 => {
                    (data[offset] as u32)
                        | ((data[offset + 1] as u32) << 8)
                        | ((data[offset + 2] as u32) << 16)
                        | ((data[offset + 3] as u32) << 24)
                }
                _ => 0,
            };
            offset += size;

            match item_type {
                0 => match tag {
                    0x08 | 0x09 | 0x0B => {
                        let field = HidReportField {
                            usage_page: global_state.usage_page,
                            usage: local_state.usage,
                            usage_minimum: local_state.usage_minimum,
                            usage_maximum: local_state.usage_maximum,
                            logical_minimum: global_state.logical_minimum,
                            logical_maximum: global_state.logical_maximum,
                            physical_minimum: global_state.physical_minimum,
                            physical_maximum: global_state.physical_maximum,
                            report_size: global_state.report_size,
                            report_count: global_state.report_count,
                            report_id: global_state.report_id,
                            flags: value as u16,
                        };
                        let report_type = match tag {
                            0x08 => HidReportType::Input,
                            0x09 => HidReportType::Output,
                            _ => HidReportType::Feature,
                        };
                        self.add_field_to_report(report_type, global_state.report_id, field);
                        local_state = HidLocalState::default();
                    }
                    _ => {}
                },
                1 => match tag {
                    0x00 => global_state.usage_page = value as u16,
                    0x01 => global_state.logical_minimum = value as i32,
                    0x02 => global_state.logical_maximum = value as i32,
                    0x03 => global_state.physical_minimum = value as i32,
                    0x04 => global_state.physical_maximum = value as i32,
                    0x07 => global_state.report_size = value,
                    0x08 => global_state.report_id = value as u8,
                    0x09 => global_state.report_count = value,
                    _ => {}
                },
                2 => match tag {
                    0x00 => local_state.usage = value as u16,
                    0x01 => local_state.usage_minimum = value as u16,
                    0x02 => local_state.usage_maximum = value as u16,
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }

    fn add_field_to_report(
        &mut self,
        report_type: HidReportType,
        report_id: u8,
        field: HidReportField,
    ) {
        let bits = field.report_size * field.report_count;
        if let Some(report) = self
            .reports
            .iter_mut()
            .find(|r| r.report_id == report_id && r.report_type as u8 == report_type as u8)
        {
            report.total_size_bits += bits;
            report.fields.push(field);
        } else {
            self.reports.push(HidReport {
                report_id,
                report_type,
                fields: alloc::vec![field],
                total_size_bits: bits,
            });
        }
    }

    pub fn set_idle(&mut self, duration: u8, report_id: u8) -> SetupPacket {
        self.idle_rate = duration;
        SetupPacket::new(
            0x21,
            0x0A,
            ((duration as u16) << 8) | report_id as u16,
            self.interface_number as u16,
            0,
        )
    }

    pub fn set_protocol(&mut self, boot: bool) -> SetupPacket {
        self.boot_protocol = boot;
        SetupPacket::new(
            0x21,
            0x0B,
            if boot { 0 } else { 1 },
            self.interface_number as u16,
            0,
        )
    }

    pub fn get_report(
        &self,
        report_type: HidReportType,
        report_id: u8,
        length: u16,
    ) -> SetupPacket {
        SetupPacket::new(
            0xA1,
            0x01,
            ((report_type as u16) << 8) | report_id as u16,
            self.interface_number as u16,
            length,
        )
    }

    pub fn set_report(
        &self,
        report_type: HidReportType,
        report_id: u8,
        length: u16,
    ) -> SetupPacket {
        SetupPacket::new(
            0x21,
            0x09,
            ((report_type as u16) << 8) | report_id as u16,
            self.interface_number as u16,
            length,
        )
    }
}

#[derive(Debug, Default, Clone)]
struct HidGlobalState {
    usage_page: u16,
    logical_minimum: i32,
    logical_maximum: i32,
    physical_minimum: i32,
    physical_maximum: i32,
    report_size: u32,
    report_count: u32,
    report_id: u8,
}

#[derive(Debug, Default, Clone)]
struct HidLocalState {
    usage: u16,
    usage_minimum: u16,
    usage_maximum: u16,
}

// Boot protocol structures
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HidBootKeyboardReport {
    pub modifiers: u8,
    pub reserved: u8,
    pub keys: [u8; 6],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HidBootMouseReport {
    pub buttons: u8,
    pub x: i8,
    pub y: i8,
}

// HID-over-I2C descriptor
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HidOverI2cDescriptor {
    pub w_hid_desc_length: u16,
    pub bcd_version: u16,
    pub w_report_desc_length: u16,
    pub w_report_desc_register: u16,
    pub w_input_register: u16,
    pub w_max_input_length: u16,
    pub w_output_register: u16,
    pub w_max_output_length: u16,
    pub w_command_register: u16,
    pub w_data_register: u16,
    pub w_vendor_id: u16,
    pub w_product_id: u16,
    pub w_version_id: u16,
    pub reserved: [u8; 4],
}

// ─── Mass Storage ───────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MassStorageSubclass {
    Scsi = 0x06,
    Rbc = 0x01,
    Mmc5 = 0x02,
    Ufi = 0x04,
    ScsiTransparent = 0x05,
    LsdFs = 0x07,
    Ieee1667 = 0x08,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MassStorageProtocol {
    Cbi = 0x00,
    CbiNoInterrupt = 0x01,
    Bbb = 0x50,
    Uas = 0x62,
    VendorSpecific = 0xFF,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct CommandBlockWrapper {
    pub d_cbw_signature: u32,
    pub d_cbw_tag: u32,
    pub d_cbw_data_transfer_length: u32,
    pub bm_cbw_flags: u8,
    pub b_cbw_lun: u8,
    pub b_cbw_cb_length: u8,
    pub cbwcb: [u8; 16],
}

pub const CBW_SIGNATURE: u32 = 0x43425355;
pub const CBW_FLAG_DATA_IN: u8 = 0x80;
pub const CBW_FLAG_DATA_OUT: u8 = 0x00;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct CommandStatusWrapper {
    pub d_csw_signature: u32,
    pub d_csw_tag: u32,
    pub d_csw_data_residue: u32,
    pub b_csw_status: u8,
}

pub const CSW_SIGNATURE: u32 = 0x53425355;
pub const CSW_STATUS_PASSED: u8 = 0x00;
pub const CSW_STATUS_FAILED: u8 = 0x01;
pub const CSW_STATUS_PHASE_ERROR: u8 = 0x02;

#[derive(Debug)]
pub struct MassStorageDevice {
    pub interface_number: u8,
    pub bulk_in_ep: u8,
    pub bulk_out_ep: u8,
    pub max_lun: u8,
    pub tag: u32,
    pub protocol: MassStorageProtocol,
    pub block_size: u32,
    pub total_blocks: u64,
}

impl MassStorageDevice {
    pub fn new(interface_number: u8, bulk_in: u8, bulk_out: u8) -> Self {
        Self {
            interface_number,
            bulk_in_ep: bulk_in,
            bulk_out_ep: bulk_out,
            max_lun: 0,
            tag: 1,
            protocol: MassStorageProtocol::Bbb,
            block_size: 512,
            total_blocks: 0,
        }
    }

    pub fn build_cbw(
        &mut self,
        lun: u8,
        data_len: u32,
        direction: u8,
        cb: &[u8],
    ) -> CommandBlockWrapper {
        let tag = self.tag;
        self.tag = self.tag.wrapping_add(1);
        let mut cbwcb = [0u8; 16];
        let len = cb.len().min(16);
        cbwcb[..len].copy_from_slice(&cb[..len]);
        CommandBlockWrapper {
            d_cbw_signature: CBW_SIGNATURE,
            d_cbw_tag: tag,
            d_cbw_data_transfer_length: data_len,
            bm_cbw_flags: direction,
            b_cbw_lun: lun,
            b_cbw_cb_length: len as u8,
            cbwcb,
        }
    }

    pub fn scsi_inquiry(&mut self, lun: u8) -> CommandBlockWrapper {
        let cmd = [0x12, 0, 0, 0, 36, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        self.build_cbw(lun, 36, CBW_FLAG_DATA_IN, &cmd)
    }

    pub fn scsi_read_capacity10(&mut self, lun: u8) -> CommandBlockWrapper {
        let cmd = [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        self.build_cbw(lun, 8, CBW_FLAG_DATA_IN, &cmd)
    }

    pub fn scsi_test_unit_ready(&mut self, lun: u8) -> CommandBlockWrapper {
        let cmd = [0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        self.build_cbw(lun, 0, CBW_FLAG_DATA_OUT, &cmd)
    }

    pub fn scsi_read10(&mut self, lun: u8, lba: u32, blocks: u16) -> CommandBlockWrapper {
        let cmd = [
            0x28,
            0,
            (lba >> 24) as u8,
            (lba >> 16) as u8,
            (lba >> 8) as u8,
            lba as u8,
            0,
            (blocks >> 8) as u8,
            blocks as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        self.build_cbw(
            lun,
            (blocks as u32) * self.block_size,
            CBW_FLAG_DATA_IN,
            &cmd,
        )
    }

    pub fn scsi_write10(&mut self, lun: u8, lba: u32, blocks: u16) -> CommandBlockWrapper {
        let cmd = [
            0x2A,
            0,
            (lba >> 24) as u8,
            (lba >> 16) as u8,
            (lba >> 8) as u8,
            lba as u8,
            0,
            (blocks >> 8) as u8,
            blocks as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        self.build_cbw(
            lun,
            (blocks as u32) * self.block_size,
            CBW_FLAG_DATA_OUT,
            &cmd,
        )
    }

    pub fn scsi_request_sense(&mut self, lun: u8) -> CommandBlockWrapper {
        let cmd = [0x03, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        self.build_cbw(lun, 18, CBW_FLAG_DATA_IN, &cmd)
    }

    pub fn get_max_lun(&self) -> SetupPacket {
        SetupPacket::new(0xA1, 0xFE, 0, self.interface_number as u16, 1)
    }

    pub fn bulk_only_reset(&self) -> SetupPacket {
        SetupPacket::new(0x21, 0xFF, 0, self.interface_number as u16, 0)
    }
}

// UAS (USB Attached SCSI)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UasIuId {
    Command = 0x01,
    Sense = 0x03,
    Response = 0x04,
    TaskManagement = 0x05,
    ReadReady = 0x06,
    WriteReady = 0x07,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UasCommandIu {
    pub iu_id: u8,
    pub reserved: u8,
    pub tag: u16,
    pub prio_task_attr: u8,
    pub reserved2: u8,
    pub add_cdb_length: u8,
    pub reserved3: u8,
    pub lun: [u8; 8],
    pub cdb: [u8; 16],
}

// ─── CDC (Communications Device Class) ─────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcSubclass {
    Dlcm = 0x01,
    Acm = 0x02,
    Tcm = 0x03,
    Mccm = 0x04,
    Capi = 0x05,
    Encm = 0x06,
    Atm = 0x07,
    WirelessHandset = 0x08,
    DeviceManagement = 0x09,
    MobileDirectLine = 0x0A,
    Obex = 0x0B,
    Ecm = 0x0C,
    Ncm = 0x0D,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcRequest {
    SendEncapsulatedCommand = 0x00,
    GetEncapsulatedResponse = 0x01,
    SetCommFeature = 0x02,
    GetCommFeature = 0x03,
    ClearCommFeature = 0x04,
    SetLineCoding = 0x20,
    GetLineCoding = 0x21,
    SetControlLineState = 0x22,
    SendBreak = 0x23,
    SetEthernetMulticastFilters = 0x40,
    SetEthernetPowerManagementFilter = 0x41,
    GetEthernetPowerManagementFilter = 0x42,
    SetEthernetPacketFilter = 0x43,
    GetEthernetStatistic = 0x44,
    GetNtbParameters = 0x80,
    GetNtbInputSize = 0x85,
    SetNtbInputSize = 0x86,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct CdcLineCoding {
    pub dw_dte_rate: u32,
    pub b_char_format: u8,
    pub b_parity_type: u8,
    pub b_data_bits: u8,
}

impl CdcLineCoding {
    pub fn default_115200() -> Self {
        Self {
            dw_dte_rate: 115200,
            b_char_format: 0,
            b_parity_type: 0,
            b_data_bits: 8,
        }
    }
}

#[derive(Debug)]
pub struct CdcAcmDevice {
    pub interface_number: u8,
    pub data_interface: u8,
    pub notification_ep: u8,
    pub bulk_in_ep: u8,
    pub bulk_out_ep: u8,
    pub line_coding: CdcLineCoding,
    pub control_line_state: u16,
}

impl CdcAcmDevice {
    pub fn new(ctrl_iface: u8, data_iface: u8) -> Self {
        Self {
            interface_number: ctrl_iface,
            data_interface: data_iface,
            notification_ep: 0,
            bulk_in_ep: 0,
            bulk_out_ep: 0,
            line_coding: CdcLineCoding::default_115200(),
            control_line_state: 0,
        }
    }

    pub fn set_line_coding(&mut self, coding: CdcLineCoding) -> SetupPacket {
        self.line_coding = coding;
        SetupPacket::new(
            0x21,
            CdcRequest::SetLineCoding as u8,
            0,
            self.interface_number as u16,
            7,
        )
    }

    pub fn set_control_line_state(&mut self, dtr: bool, rts: bool) -> SetupPacket {
        self.control_line_state = (dtr as u16) | ((rts as u16) << 1);
        SetupPacket::new(
            0x21,
            CdcRequest::SetControlLineState as u8,
            self.control_line_state,
            self.interface_number as u16,
            0,
        )
    }
}

// CDC ECM (Ethernet Control Model)
#[derive(Debug)]
pub struct CdcEcmDevice {
    pub interface_number: u8,
    pub data_interface: u8,
    pub notification_ep: u8,
    pub bulk_in_ep: u8,
    pub bulk_out_ep: u8,
    pub mac_address: [u8; 6],
    pub max_segment_size: u16,
    pub packet_filter: u16,
}

impl CdcEcmDevice {
    pub fn new(ctrl_iface: u8, data_iface: u8) -> Self {
        Self {
            interface_number: ctrl_iface,
            data_interface: data_iface,
            notification_ep: 0,
            bulk_in_ep: 0,
            bulk_out_ep: 0,
            mac_address: [0u8; 6],
            max_segment_size: 1514,
            packet_filter: 0,
        }
    }

    pub fn set_packet_filter(&mut self, filter: u16) -> SetupPacket {
        self.packet_filter = filter;
        SetupPacket::new(
            0x21,
            CdcRequest::SetEthernetPacketFilter as u8,
            filter,
            self.interface_number as u16,
            0,
        )
    }
}

// CDC NCM (Network Control Model)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NtbParameters {
    pub w_length: u16,
    pub bm_ntb_formats_supported: u16,
    pub dw_ntb_in_max_size: u32,
    pub w_ndp_in_divisor: u16,
    pub w_ndp_in_payload_remainder: u16,
    pub w_ndp_in_alignment: u16,
    pub reserved: u16,
    pub dw_ntb_out_max_size: u32,
    pub w_ndp_out_divisor: u16,
    pub w_ndp_out_payload_remainder: u16,
    pub w_ndp_out_alignment: u16,
    pub w_ntb_out_max_datagrams: u16,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ncm16NthHeader {
    pub dw_signature: u32,
    pub w_header_length: u16,
    pub w_sequence: u16,
    pub w_block_length: u16,
    pub w_ndp_index: u16,
}

pub const NCM_NTH16_SIGNATURE: u32 = 0x484D434E;

// ─── USB Audio ──────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSubclass {
    Undefined = 0x00,
    AudioControl = 0x01,
    AudioStreaming = 0x02,
    MidiStreaming = 0x03,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioControlDescriptorSubtype {
    Undefined = 0x00,
    Header = 0x01,
    InputTerminal = 0x02,
    OutputTerminal = 0x03,
    MixerUnit = 0x04,
    SelectorUnit = 0x05,
    FeatureUnit = 0x06,
    EffectUnit = 0x07,
    ProcessingUnit = 0x08,
    ExtensionUnit = 0x09,
    ClockSource = 0x0A,
    ClockSelector = 0x0B,
    ClockMultiplier = 0x0C,
    SampleRateConverter = 0x0D,
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFeatureUnitControl {
    Mute = 0x0001,
    Volume = 0x0002,
    Bass = 0x0003,
    Mid = 0x0004,
    Treble = 0x0005,
    GraphicEqualizer = 0x0006,
    AutomaticGain = 0x0007,
    Delay = 0x0008,
    BassBoost = 0x0009,
    Loudness = 0x000A,
    InputGain = 0x000B,
    InputGainPad = 0x000C,
    PhaseInverter = 0x000D,
    Underflow = 0x000E,
    Overflow = 0x000F,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioClockSource {
    pub clock_id: u8,
    pub attributes: u8,
    pub assoc_terminal: u8,
    pub clock_source_string: u8,
}

#[derive(Debug, Clone)]
pub struct AudioFeatureUnit {
    pub unit_id: u8,
    pub source_id: u8,
    pub controls: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioMixerUnit {
    pub unit_id: u8,
    pub num_input_pins: u8,
    pub num_output_channels: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioSelectorUnit {
    pub unit_id: u8,
    pub num_input_pins: u8,
    pub current_selection: u8,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormatType {
    Undefined = 0x00,
    TypeI = 0x01,
    TypeII = 0x02,
    TypeIII = 0x03,
    TypeIV = 0x04,
    ExtTypeI = 0x81,
    ExtTypeII = 0x82,
    ExtTypeIII = 0x83,
}

#[derive(Debug, Clone)]
pub struct AudioStreamingConfig {
    pub format_type: AudioFormatType,
    pub num_channels: u8,
    pub bit_resolution: u8,
    pub sample_rates: Vec<u32>,
    pub clock_source: u8,
}

// ─── USB Video (UVC) ────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSubclass {
    Undefined = 0x00,
    VideoControl = 0x01,
    VideoStreaming = 0x02,
    VideoInterfaceCollection = 0x03,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoControlDescriptorSubtype {
    Undefined = 0x00,
    Header = 0x01,
    InputTerminal = 0x02,
    OutputTerminal = 0x03,
    SelectorUnit = 0x04,
    ProcessingUnit = 0x05,
    ExtensionUnit = 0x06,
    EncodingUnit = 0x07,
}

#[derive(Debug, Clone, Copy)]
pub struct UvcFormatDescriptor {
    pub format_index: u8,
    pub num_frame_descriptors: u8,
    pub guid_format: [u8; 16],
    pub bits_per_pixel: u8,
    pub default_frame_index: u8,
    pub aspect_ratio_x: u8,
    pub aspect_ratio_y: u8,
    pub interlace_flags: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct UvcFrameDescriptor {
    pub frame_index: u8,
    pub capabilities: u8,
    pub width: u16,
    pub height: u16,
    pub min_bit_rate: u32,
    pub max_bit_rate: u32,
    pub max_frame_buffer_size: u32,
    pub default_frame_interval: u32,
}

// ─── USB Power Management ───────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbLinkState {
    U0 = 0,
    U1 = 1,
    U2 = 2,
    U3 = 3,
    Disabled = 4,
    RxDetect = 5,
    Inactive = 6,
    Polling = 7,
    Recovery = 8,
    HotReset = 9,
    ComplianceMode = 10,
    TestMode = 11,
    Resume = 15,
}

#[derive(Debug, Clone, Copy)]
pub struct LinkPowerManagement {
    pub lpm_capable: bool,
    pub besl: u8,
    pub best_effort_service_latency_deep: u8,
    pub l1_device_sleep: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct UsbPowerDelivery {
    pub supported: bool,
    pub max_voltage_mv: u32,
    pub max_current_ma: u32,
    pub power_role: PdPowerRole,
    pub data_role: PdDataRole,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdPowerRole {
    Sink = 0,
    Source = 1,
    DualRole = 2,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdDataRole {
    Ufp = 0,
    Dfp = 1,
    DualRole = 2,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbCAlternateMode {
    None = 0,
    DisplayPort = 1,
    Thunderbolt = 2,
    Hdmi = 3,
    Mhl = 4,
    VirtualLink = 5,
}

#[derive(Debug)]
pub struct UsbPowerState {
    pub selective_suspend: bool,
    pub remote_wakeup_enabled: bool,
    pub link_state: UsbLinkState,
    pub lpm: LinkPowerManagement,
    pub pd: UsbPowerDelivery,
    pub alt_mode: UsbCAlternateMode,
}

impl UsbPowerState {
    pub fn new() -> Self {
        Self {
            selective_suspend: false,
            remote_wakeup_enabled: false,
            link_state: UsbLinkState::U0,
            lpm: LinkPowerManagement {
                lpm_capable: false,
                besl: 0,
                best_effort_service_latency_deep: 0,
                l1_device_sleep: false,
            },
            pd: UsbPowerDelivery {
                supported: false,
                max_voltage_mv: 5000,
                max_current_ma: 500,
                power_role: PdPowerRole::Sink,
                data_role: PdDataRole::Ufp,
            },
            alt_mode: UsbCAlternateMode::None,
        }
    }

    pub fn enter_suspend(&mut self) {
        self.selective_suspend = true;
        self.link_state = UsbLinkState::U3;
    }

    pub fn exit_suspend(&mut self) {
        self.selective_suspend = false;
        self.link_state = UsbLinkState::U0;
    }

    pub fn enable_remote_wakeup(&mut self) {
        self.remote_wakeup_enabled = true;
    }

    pub fn enter_lpm_l1(&mut self, besl: u8) {
        if self.lpm.lpm_capable {
            self.lpm.besl = besl;
            self.lpm.l1_device_sleep = true;
            self.link_state = UsbLinkState::U1;
        }
    }
}

// ─── USB OTG ────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtgRole {
    Host = 0,
    Device = 1,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtgState {
    Idle = 0,
    WaitBcon = 1,
    AHost = 2,
    APeripheral = 3,
    ASuspend = 4,
    BIdle = 5,
    BPeripheral = 6,
    BHost = 7,
    BWaitAcon = 8,
}

#[derive(Debug)]
pub struct OtgController {
    pub current_role: OtgRole,
    pub state: OtgState,
    pub srp_capable: bool,
    pub hnp_capable: bool,
    pub adp_capable: bool,
    pub id_pin_state: bool,
    pub vbus_state: bool,
}

impl OtgController {
    pub fn new() -> Self {
        Self {
            current_role: OtgRole::Device,
            state: OtgState::Idle,
            srp_capable: false,
            hnp_capable: false,
            adp_capable: false,
            id_pin_state: true,
            vbus_state: false,
        }
    }

    pub fn initiate_srp(&mut self) -> Result<(), UsbError> {
        if !self.srp_capable {
            return Err(UsbError::NotSupported);
        }
        if self.current_role != OtgRole::Device {
            return Err(UsbError::InvalidState);
        }
        self.state = OtgState::BIdle;
        Ok(())
    }

    pub fn initiate_hnp(&mut self) -> Result<(), UsbError> {
        if !self.hnp_capable {
            return Err(UsbError::NotSupported);
        }
        match self.current_role {
            OtgRole::Device => {
                self.current_role = OtgRole::Host;
                self.state = OtgState::BHost;
            }
            OtgRole::Host => {
                self.current_role = OtgRole::Device;
                self.state = OtgState::APeripheral;
            }
        }
        Ok(())
    }

    pub fn detect_id_pin(&mut self, grounded: bool) {
        self.id_pin_state = grounded;
        if grounded {
            self.current_role = OtgRole::Host;
            self.state = OtgState::AHost;
        } else {
            self.current_role = OtgRole::Device;
            self.state = OtgState::BIdle;
        }
    }

    pub fn vbus_changed(&mut self, present: bool) {
        self.vbus_state = present;
        if !present && self.current_role == OtgRole::Device {
            self.state = OtgState::BIdle;
        }
    }
}

// ─── Device Enumeration ─────────────────────────────────────────────────────

pub struct DeviceEnumerator {
    next_address: u8,
}

impl DeviceEnumerator {
    pub fn new() -> Self {
        Self { next_address: 1 }
    }

    pub fn allocate_address(&mut self) -> Result<u8, UsbError> {
        if self.next_address >= 127 {
            return Err(UsbError::NoMemory);
        }
        let addr = self.next_address;
        self.next_address += 1;
        Ok(addr)
    }

    pub fn get_device_descriptor_setup(max_len: u16) -> SetupPacket {
        SetupPacket::new(
            0x80,
            StandardRequest::GetDescriptor as u8,
            (DescriptorType::Device as u16) << 8,
            0,
            max_len,
        )
    }

    pub fn set_address_setup(address: u8) -> SetupPacket {
        SetupPacket::new(
            0x00,
            StandardRequest::SetAddress as u8,
            address as u16,
            0,
            0,
        )
    }

    pub fn get_configuration_descriptor_setup(index: u8, max_len: u16) -> SetupPacket {
        SetupPacket::new(
            0x80,
            StandardRequest::GetDescriptor as u8,
            ((DescriptorType::Configuration as u16) << 8) | index as u16,
            0,
            max_len,
        )
    }

    pub fn set_configuration_setup(config_value: u8) -> SetupPacket {
        SetupPacket::new(
            0x00,
            StandardRequest::SetConfiguration as u8,
            config_value as u16,
            0,
            0,
        )
    }

    pub fn get_string_descriptor_setup(index: u8, lang_id: u16, max_len: u16) -> SetupPacket {
        SetupPacket::new(
            0x80,
            StandardRequest::GetDescriptor as u8,
            ((DescriptorType::String as u16) << 8) | index as u16,
            lang_id,
            max_len,
        )
    }

    pub fn get_bos_descriptor_setup(max_len: u16) -> SetupPacket {
        SetupPacket::new(
            0x80,
            StandardRequest::GetDescriptor as u8,
            (DescriptorType::Bos as u16) << 8,
            0,
            max_len,
        )
    }

    pub fn enumerate_device(&mut self, device: &mut UsbDevice) -> Result<(), UsbError> {
        device.state = DeviceState::Powered;
        device.state = DeviceState::Default;

        let address = self.allocate_address()?;
        device.set_address(address);
        device.set_configured(1);
        Ok(())
    }
}

// ─── USB Core Manager ───────────────────────────────────────────────────────

pub const MAX_USB_DEVICES: usize = 127;
pub const MAX_HUBS: usize = 16;

pub struct UsbCoreManager {
    pub devices: Vec<Option<UsbDevice>>,
    pub hubs: Vec<Option<UsbHub>>,
    pub enumerator: DeviceEnumerator,
    pub otg: OtgController,
    pub next_urb_id: u64,
    pub pending_urbs: Vec<Urb>,
    pub initialized: bool,
}

impl UsbCoreManager {
    pub const fn new() -> Self {
        Self {
            devices: Vec::new(),
            hubs: Vec::new(),
            enumerator: DeviceEnumerator { next_address: 1 },
            otg: OtgController {
                current_role: OtgRole::Host,
                state: OtgState::Idle,
                srp_capable: false,
                hnp_capable: false,
                adp_capable: false,
                id_pin_state: true,
                vbus_state: false,
            },
            next_urb_id: 1,
            pending_urbs: Vec::new(),
            initialized: false,
        }
    }

    pub fn initialize(&mut self) {
        self.devices = Vec::with_capacity(MAX_USB_DEVICES);
        for _ in 0..MAX_USB_DEVICES {
            self.devices.push(None);
        }
        self.hubs = Vec::with_capacity(MAX_HUBS);
        for _ in 0..MAX_HUBS {
            self.hubs.push(None);
        }
        self.initialized = true;
    }

    pub fn register_device(&mut self, device: UsbDevice) -> Result<u8, UsbError> {
        let addr = device.address as usize;
        if addr == 0 || addr >= MAX_USB_DEVICES {
            return Err(UsbError::NoDevice);
        }
        self.devices[addr] = Some(device);
        Ok(addr as u8)
    }

    pub fn unregister_device(&mut self, address: u8) -> Result<(), UsbError> {
        let addr = address as usize;
        if addr >= MAX_USB_DEVICES {
            return Err(UsbError::NoDevice);
        }
        self.devices[addr] = None;
        Ok(())
    }

    pub fn get_device(&self, address: u8) -> Option<&UsbDevice> {
        self.devices.get(address as usize).and_then(|d| d.as_ref())
    }

    pub fn get_device_mut(&mut self, address: u8) -> Option<&mut UsbDevice> {
        self.devices
            .get_mut(address as usize)
            .and_then(|d| d.as_mut())
    }

    pub fn register_hub(&mut self, hub: UsbHub) -> Result<usize, UsbError> {
        for (i, slot) in self.hubs.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(hub);
                return Ok(i);
            }
        }
        Err(UsbError::NoMemory)
    }

    pub fn submit_urb(&mut self, urb: &mut Urb) -> Result<(), UsbError> {
        urb.id = self.next_urb_id;
        self.next_urb_id += 1;
        Ok(())
    }

    pub fn cancel_urb(&mut self, urb_id: u64) -> Result<(), UsbError> {
        for urb in self.pending_urbs.iter_mut() {
            if urb.id == urb_id {
                urb.status = UrbStatus::Cancelled;
                return Ok(());
            }
        }
        Err(UsbError::InvalidState)
    }

    pub fn handle_device_connect(
        &mut self,
        speed: UsbSpeed,
        hub_addr: Option<u8>,
        port: u8,
    ) -> Result<u8, UsbError> {
        let mut device = UsbDevice::new(speed);
        device.parent_hub = hub_addr;
        device.port_number = port;
        self.enumerator.enumerate_device(&mut device)?;
        let addr = device.address;
        self.register_device(device)?;
        Ok(addr)
    }

    pub fn handle_device_disconnect(&mut self, address: u8) -> Result<(), UsbError> {
        self.unregister_device(address)
    }

    pub fn bind_class_driver(&mut self, address: u8) -> Result<(), UsbError> {
        let device = self.get_device_mut(address).ok_or(UsbError::NoDevice)?;
        let desc = device
            .device_descriptor
            .ok_or(UsbError::InvalidDescriptor)?;
        let class = desc.b_device_class;
        let driver = match class {
            0x03 => Some(UsbClassDriver::Hid),
            0x08 => Some(UsbClassDriver::MassStorageBbb),
            0x02 => Some(UsbClassDriver::CdcAcm),
            0x09 => Some(UsbClassDriver::Hub),
            0x0E => Some(UsbClassDriver::Video),
            0x01 => Some(UsbClassDriver::Audio),
            0x07 => Some(UsbClassDriver::Printer),
            0xE0 => Some(UsbClassDriver::WirelessController),
            0xFF => Some(UsbClassDriver::VendorSpecific),
            _ => None,
        };
        device.class_driver = driver;
        Ok(())
    }

    pub fn device_count(&self) -> usize {
        self.devices.iter().filter(|d| d.is_some()).count()
    }
}

// ─── Global State ───────────────────────────────────────────────────────────

pub static USB_CORE: Mutex<UsbCoreManager> = Mutex::new(UsbCoreManager::new());

pub fn init() {
    let mut core = USB_CORE.lock();
    core.initialize();
}
