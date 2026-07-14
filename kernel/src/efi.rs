//! UEFI/EFI Runtime Services for RaeenOS.
//!
//! Provides EFI system table access, runtime service wrappers, variable storage,
//! secure boot chain, memory map management, device path construction, SMBIOS
//! parsing, capsule update delivery, and RNG services.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use spin::Mutex;

// ---------------------------------------------------------------------------
// EFI Status codes
// ---------------------------------------------------------------------------

#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EfiStatus {
    Success = 0,
    LoadError = 1 | (1 << 63),
    InvalidParameter = 2 | (1 << 63),
    Unsupported = 3 | (1 << 63),
    BadBufferSize = 4 | (1 << 63),
    BufferTooSmall = 5 | (1 << 63),
    NotReady = 6 | (1 << 63),
    DeviceError = 7 | (1 << 63),
    WriteProtected = 8 | (1 << 63),
    OutOfResources = 9 | (1 << 63),
    VolumeCorrupted = 10 | (1 << 63),
    VolumeFull = 11 | (1 << 63),
    NoMedia = 12 | (1 << 63),
    MediaChanged = 13 | (1 << 63),
    NotFound = 14 | (1 << 63),
    AccessDenied = 15 | (1 << 63),
    NoResponse = 16 | (1 << 63),
    NoMapping = 17 | (1 << 63),
    Timeout = 18 | (1 << 63),
    NotStarted = 19 | (1 << 63),
    AlreadyStarted = 20 | (1 << 63),
    Aborted = 21 | (1 << 63),
    SecurityViolation = 26 | (1 << 63),
    WarnUnknownGlyph = 1,
    WarnDeleteFailure = 2,
    WarnWriteFailure = 3,
    WarnBufferTooSmall = 4,
    WarnStaleData = 5,
    WarnFileSystem = 6,
}

impl EfiStatus {
    pub fn is_error(self) -> bool {
        (self as u64) & (1 << 63) != 0
    }
}

// ---------------------------------------------------------------------------
// EFI GUID
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EfiGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl EfiGuid {
    pub const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Self {
        Self {
            data1: d1,
            data2: d2,
            data3: d3,
            data4: d4,
        }
    }
}

impl fmt::Debug for EfiGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.data1,
            self.data2,
            self.data3,
            self.data4[0],
            self.data4[1],
            self.data4[2],
            self.data4[3],
            self.data4[4],
            self.data4[5],
            self.data4[6],
            self.data4[7],
        )
    }
}

impl fmt::Display for EfiGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

// Well-known GUIDs
pub const EFI_GLOBAL_VARIABLE_GUID: EfiGuid = EfiGuid::new(
    0x8BE4DF61,
    0x93CA,
    0x11D2,
    [0xAA, 0x0D, 0x00, 0xE0, 0x98, 0x03, 0x2B, 0x8C],
);
pub const EFI_RUNTIME_SERVICES_GUID: EfiGuid = EfiGuid::new(
    0x1E5B6655,
    0x28CD,
    0x4D2F,
    [0xAA, 0xFF, 0xD4, 0x8F, 0xB7, 0x65, 0x58, 0x44],
);
pub const EFI_LOADED_IMAGE_GUID: EfiGuid = EfiGuid::new(
    0x5B1B31A1,
    0x9562,
    0x11D2,
    [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_DEVICE_PATH_GUID: EfiGuid = EfiGuid::new(
    0x09576E91,
    0x6D3F,
    0x11D2,
    [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_SIMPLE_FILE_SYSTEM_GUID: EfiGuid = EfiGuid::new(
    0x0964E5B22,
    0x6459,
    0x11D2,
    [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_GRAPHICS_OUTPUT_GUID: EfiGuid = EfiGuid::new(
    0x9042A9DE,
    0x23DC,
    0x4A38,
    [0x96, 0xFB, 0x7A, 0xDE, 0xD0, 0x80, 0x51, 0x6A],
);
pub const EFI_SIMPLE_TEXT_INPUT_GUID: EfiGuid = EfiGuid::new(
    0x387477C1,
    0x69C7,
    0x11D2,
    [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_SIMPLE_TEXT_OUTPUT_GUID: EfiGuid = EfiGuid::new(
    0x387477C2,
    0x69C7,
    0x11D2,
    [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_BLOCK_IO_GUID: EfiGuid = EfiGuid::new(
    0x964E5B21,
    0x6459,
    0x11D2,
    [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_DISK_IO_GUID: EfiGuid = EfiGuid::new(
    0xCE345171,
    0xBA0B,
    0x11D2,
    [0x8E, 0x4F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_FILE_INFO_GUID: EfiGuid = EfiGuid::new(
    0x09576E92,
    0x6D3F,
    0x11D2,
    [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_FILE_SYSTEM_INFO_GUID: EfiGuid = EfiGuid::new(
    0x09576E93,
    0x6D3F,
    0x11D2,
    [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const EFI_CONFIGURATION_TABLE_GUID: EfiGuid = EfiGuid::new(
    0x8868E871,
    0xE4F1,
    0x11D3,
    [0xBC, 0x22, 0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81],
);
pub const EFI_ACPI_TABLE_GUID: EfiGuid = EfiGuid::new(
    0x8868E871,
    0xE4F1,
    0x11D3,
    [0xBC, 0x22, 0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81],
);
pub const EFI_ACPI_20_TABLE_GUID: EfiGuid = EfiGuid::new(
    0x8868E871,
    0xE4F1,
    0x11D3,
    [0xBC, 0x22, 0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81],
);
pub const EFI_SMBIOS_GUID: EfiGuid = EfiGuid::new(
    0xEB9D2D31,
    0x2D88,
    0x11D3,
    [0x9A, 0x16, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D],
);
pub const EFI_SMBIOS3_GUID: EfiGuid = EfiGuid::new(
    0xF2FD1544,
    0x9794,
    0x4A2C,
    [0x99, 0x2E, 0xE5, 0xBB, 0xCF, 0x20, 0xE3, 0x94],
);
pub const EFI_MEMORY_ATTRIBUTES_GUID: EfiGuid = EfiGuid::new(
    0xDCFA911D,
    0x26EB,
    0x469F,
    [0xA2, 0x20, 0x38, 0xB7, 0xDC, 0x46, 0x12, 0x20],
);
pub const EFI_RNG_GUID: EfiGuid = EfiGuid::new(
    0x3152BCA5,
    0xEADE,
    0x433D,
    [0x86, 0x2E, 0xC0, 0x1C, 0xDC, 0x29, 0x1F, 0x44],
);
pub const EFI_HII_GUID: EfiGuid = EfiGuid::new(
    0xEF9FC172,
    0xA1B2,
    0x4693,
    [0xB3, 0x27, 0x6D, 0x32, 0xFC, 0x41, 0x60, 0x42],
);
pub const EFI_DEVICE_TREE_GUID: EfiGuid = EfiGuid::new(
    0xB1B621D5,
    0xF19C,
    0x41A5,
    [0x83, 0x0B, 0xD9, 0x15, 0x2C, 0x69, 0xAA, 0xE0],
);

// RNG algorithm GUIDs
pub const EFI_RNG_ALGORITHM_RAW: EfiGuid = EfiGuid::new(
    0xE43176D7,
    0xB6E8,
    0x4827,
    [0xB7, 0x84, 0x7F, 0xFD, 0xC4, 0xB6, 0x85, 0x61],
);
pub const EFI_RNG_ALGORITHM_SP800_90_HASH256: EfiGuid = EfiGuid::new(
    0xA7AF67CB,
    0x603B,
    0x4D42,
    [0xBA, 0x21, 0x70, 0xBF, 0xB6, 0x29, 0x3F, 0x96],
);
pub const EFI_RNG_ALGORITHM_SP800_90_HMAC256: EfiGuid = EfiGuid::new(
    0xC5149B43,
    0xAE85,
    0x4F53,
    [0x99, 0x82, 0xB9, 0x43, 0x35, 0xD3, 0xA9, 0xE7],
);
pub const EFI_RNG_ALGORITHM_SP800_90_CTR256: EfiGuid = EfiGuid::new(
    0x44F0DE6E,
    0x4D8C,
    0x4045,
    [0xA8, 0xC7, 0x4D, 0xD1, 0x68, 0x85, 0x6B, 0x9E],
);

// Certificate type GUIDs for secure boot
pub const EFI_CERT_X509_GUID: EfiGuid = EfiGuid::new(
    0xA5C059A1,
    0x94E4,
    0x4AA7,
    [0x87, 0xB5, 0xAB, 0x15, 0x5C, 0x2B, 0xF0, 0x72],
);
pub const EFI_CERT_SHA256_GUID: EfiGuid = EfiGuid::new(
    0xC1C41626,
    0x504C,
    0x4092,
    [0xAC, 0xA9, 0x41, 0xF9, 0x36, 0x93, 0x43, 0x28],
);
pub const EFI_CERT_RSA2048_GUID: EfiGuid = EfiGuid::new(
    0x3C5766E8,
    0x269C,
    0x4E34,
    [0xAA, 0x14, 0xED, 0x77, 0x6E, 0x85, 0xB3, 0xB6],
);

// Image security database GUID
pub const EFI_IMAGE_SECURITY_DATABASE_GUID: EfiGuid = EfiGuid::new(
    0xD719B2CB,
    0x3D3A,
    0x4596,
    [0xA3, 0xBC, 0xDA, 0xD0, 0x0E, 0x67, 0x65, 0x6F],
);

// ---------------------------------------------------------------------------
// EFI Variable Attributes
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct VariableAttributes: u32 {
        const NON_VOLATILE                          = 0x0000_0001;
        const BOOTSERVICE_ACCESS                    = 0x0000_0002;
        const RUNTIME_ACCESS                        = 0x0000_0004;
        const HARDWARE_ERROR_RECORD                 = 0x0000_0008;
        const AUTHENTICATED_WRITE_ACCESS            = 0x0000_0010;
        const TIME_BASED_AUTHENTICATED_WRITE_ACCESS = 0x0000_0020;
        const APPEND_WRITE                          = 0x0000_0040;
        const ENHANCED_AUTHENTICATED_ACCESS         = 0x0000_0080;
        const NV  = Self::NON_VOLATILE.bits();
        const BS  = Self::BOOTSERVICE_ACCESS.bits();
        const RT  = Self::RUNTIME_ACCESS.bits();
        const AT  = Self::AUTHENTICATED_WRITE_ACCESS.bits();
        const TA  = Self::TIME_BASED_AUTHENTICATED_WRITE_ACCESS.bits();
        const AT_NOTIFY = Self::ENHANCED_AUTHENTICATED_ACCESS.bits();
    }
}

// ---------------------------------------------------------------------------
// EFI Time
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EfiTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub pad1: u8,
    pub nanosecond: u32,
    pub timezone: i16,
    pub daylight: u8,
    pub pad2: u8,
}

impl Default for EfiTime {
    fn default() -> Self {
        Self {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            pad1: 0,
            nanosecond: 0,
            timezone: 0,
            daylight: 0,
            pad2: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EfiTimeCapabilities {
    pub resolution: u32,
    pub accuracy: u32,
    pub sets_to_zero: bool,
}

// ---------------------------------------------------------------------------
// EFI Memory Map
// ---------------------------------------------------------------------------

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EfiMemoryType {
    Reserved = 0,
    LoaderCode = 1,
    LoaderData = 2,
    BootServicesCode = 3,
    BootServicesData = 4,
    RuntimeServicesCode = 5,
    RuntimeServicesData = 6,
    ConventionalMemory = 7,
    UnusableMemory = 8,
    AcpiReclaimMemory = 9,
    AcpiMemoryNvs = 10,
    MemoryMappedIO = 11,
    MemoryMappedIOPortSpace = 12,
    PalCode = 13,
    PersistentMemory = 14,
    MaxMemoryType = 15,
}

impl EfiMemoryType {
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::Reserved,
            1 => Self::LoaderCode,
            2 => Self::LoaderData,
            3 => Self::BootServicesCode,
            4 => Self::BootServicesData,
            5 => Self::RuntimeServicesCode,
            6 => Self::RuntimeServicesData,
            7 => Self::ConventionalMemory,
            8 => Self::UnusableMemory,
            9 => Self::AcpiReclaimMemory,
            10 => Self::AcpiMemoryNvs,
            11 => Self::MemoryMappedIO,
            12 => Self::MemoryMappedIOPortSpace,
            13 => Self::PalCode,
            14 => Self::PersistentMemory,
            _ => Self::Reserved,
        }
    }

    pub fn is_usable(self) -> bool {
        matches!(
            self,
            Self::LoaderCode
                | Self::LoaderData
                | Self::BootServicesCode
                | Self::BootServicesData
                | Self::ConventionalMemory
        )
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EfiMemoryAttribute: u64 {
        const UC       = 0x0000_0000_0000_0001;
        const WC       = 0x0000_0000_0000_0002;
        const WT       = 0x0000_0000_0000_0004;
        const WB       = 0x0000_0000_0000_0008;
        const UCE      = 0x0000_0000_0000_0010;
        const WP       = 0x0000_0000_0000_1000;
        const RP       = 0x0000_0000_0000_2000;
        const XP       = 0x0000_0000_0000_4000;
        const NV       = 0x0000_0000_0000_8000;
        const MORE_RELIABLE = 0x0000_0000_0001_0000;
        const RO       = 0x0000_0000_0002_0000;
        const SP       = 0x0000_0000_0004_0000;
        const CPU_CRYPTO = 0x0000_0000_0008_0000;
        const RUNTIME  = 0x8000_0000_0000_0000;
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EfiMemoryDescriptor {
    pub memory_type: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

impl EfiMemoryDescriptor {
    pub fn mem_type(&self) -> EfiMemoryType {
        EfiMemoryType::from_u32(self.memory_type)
    }

    pub fn size_bytes(&self) -> u64 {
        self.number_of_pages * 4096
    }

    pub fn end_address(&self) -> u64 {
        self.physical_start + self.size_bytes()
    }

    pub fn attributes(&self) -> EfiMemoryAttribute {
        EfiMemoryAttribute::from_bits_truncate(self.attribute)
    }
}

// ---------------------------------------------------------------------------
// EFI Device Path
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePathType {
    Hardware = 0x01,
    Acpi = 0x02,
    Messaging = 0x03,
    Media = 0x04,
    BiosBootSpec = 0x05,
    End = 0x7F,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareSubType {
    Pci = 0x01,
    PcCard = 0x02,
    MemoryMapped = 0x03,
    Vendor = 0x04,
    Controller = 0x05,
    Bmc = 0x06,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagingSubType {
    Atapi = 0x01,
    Scsi = 0x02,
    FibreChannel = 0x03,
    Ieee1394 = 0x04,
    Usb = 0x05,
    I2o = 0x06,
    MacAddr = 0x0B,
    Ipv4 = 0x0C,
    Ipv6 = 0x0D,
    Uart = 0x0E,
    UsbClass = 0x0F,
    UsbWwid = 0x10,
    Lun = 0x11,
    Sata = 0x12,
    Iscsi = 0x13,
    Vlan = 0x14,
    FibreChannelEx = 0x15,
    Sas = 0x16,
    SasEx = 0x20,
    Nvme = 0x17,
    Uri = 0x18,
    Ufs = 0x19,
    Sd = 0x1A,
    Bluetooth = 0x1B,
    Wifi = 0x1C,
    Emmc = 0x1D,
    BluetoothLe = 0x1E,
    Dns = 0x1F,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaSubType {
    HardDrive = 0x01,
    CdRom = 0x02,
    Vendor = 0x03,
    FilePath = 0x04,
    MediaProtocol = 0x05,
    PiwgFirmwareFile = 0x06,
    PiwgFirmwareVolume = 0x07,
    RelativeOffset = 0x08,
    RamDisk = 0x09,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DevicePathNode {
    pub node_type: u8,
    pub sub_type: u8,
    pub length: [u8; 2],
}

impl DevicePathNode {
    pub fn len(&self) -> u16 {
        u16::from_le_bytes(self.length)
    }

    pub fn is_end(&self) -> bool {
        self.node_type == DevicePathType::End as u8
    }

    pub fn path_type(&self) -> Option<DevicePathType> {
        match self.node_type {
            0x01 => Some(DevicePathType::Hardware),
            0x02 => Some(DevicePathType::Acpi),
            0x03 => Some(DevicePathType::Messaging),
            0x04 => Some(DevicePathType::Media),
            0x05 => Some(DevicePathType::BiosBootSpec),
            0x7F => Some(DevicePathType::End),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DevicePath {
    pub nodes: Vec<DevicePathEntry>,
}

#[derive(Debug, Clone)]
pub struct DevicePathEntry {
    pub node_type: u8,
    pub sub_type: u8,
    pub data: Vec<u8>,
}

impl DevicePath {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn add_pci(mut self, function: u8, device: u8) -> Self {
        let mut data = Vec::new();
        data.push(function);
        data.push(device);
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Hardware as u8,
            sub_type: HardwareSubType::Pci as u8,
            data,
        });
        self
    }

    pub fn add_vendor(mut self, vendor_guid: EfiGuid, vendor_data: &[u8]) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&vendor_guid.data1.to_le_bytes());
        data.extend_from_slice(&vendor_guid.data2.to_le_bytes());
        data.extend_from_slice(&vendor_guid.data3.to_le_bytes());
        data.extend_from_slice(&vendor_guid.data4);
        data.extend_from_slice(vendor_data);
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Hardware as u8,
            sub_type: HardwareSubType::Vendor as u8,
            data,
        });
        self
    }

    pub fn add_sata(mut self, hba_port: u16, port_multiplier: u16, lun: u16) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&hba_port.to_le_bytes());
        data.extend_from_slice(&port_multiplier.to_le_bytes());
        data.extend_from_slice(&lun.to_le_bytes());
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Sata as u8,
            data,
        });
        self
    }

    pub fn add_nvme(mut self, namespace_id: u32, eui64: u64) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&namespace_id.to_le_bytes());
        data.extend_from_slice(&eui64.to_le_bytes());
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Nvme as u8,
            data,
        });
        self
    }

    pub fn add_usb(mut self, parent_port: u8, interface: u8) -> Self {
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Usb as u8,
            data: vec![parent_port, interface],
        });
        self
    }

    pub fn add_scsi(mut self, target: u16, lun: u16) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&target.to_le_bytes());
        data.extend_from_slice(&lun.to_le_bytes());
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Scsi as u8,
            data,
        });
        self
    }

    pub fn add_atapi(mut self, primary_secondary: u8, slave_master: u8, lun: u16) -> Self {
        let mut data = Vec::new();
        data.push(primary_secondary);
        data.push(slave_master);
        data.extend_from_slice(&lun.to_le_bytes());
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Atapi as u8,
            data,
        });
        self
    }

    pub fn add_uri(mut self, uri: &str) -> Self {
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Uri as u8,
            data: uri.as_bytes().to_vec(),
        });
        self
    }

    pub fn add_ipv4(
        mut self,
        local: [u8; 4],
        remote: [u8; 4],
        local_port: u16,
        remote_port: u16,
        protocol: u16,
    ) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&local);
        data.extend_from_slice(&remote);
        data.extend_from_slice(&local_port.to_le_bytes());
        data.extend_from_slice(&remote_port.to_le_bytes());
        data.extend_from_slice(&protocol.to_le_bytes());
        data.push(0); // static address flag
        data.extend_from_slice(&[0u8; 4]); // gateway
        data.extend_from_slice(&[0u8; 4]); // subnet
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Ipv4 as u8,
            data,
        });
        self
    }

    pub fn add_ipv6(
        mut self,
        local: [u8; 16],
        remote: [u8; 16],
        local_port: u16,
        remote_port: u16,
        protocol: u16,
    ) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&local);
        data.extend_from_slice(&remote);
        data.extend_from_slice(&local_port.to_le_bytes());
        data.extend_from_slice(&remote_port.to_le_bytes());
        data.extend_from_slice(&protocol.to_le_bytes());
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Messaging as u8,
            sub_type: MessagingSubType::Ipv6 as u8,
            data,
        });
        self
    }

    pub fn add_hard_drive(
        mut self,
        partition_number: u32,
        partition_start: u64,
        partition_size: u64,
        signature: [u8; 16],
        mbr_type: u8,
        signature_type: u8,
    ) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&partition_number.to_le_bytes());
        data.extend_from_slice(&partition_start.to_le_bytes());
        data.extend_from_slice(&partition_size.to_le_bytes());
        data.extend_from_slice(&signature);
        data.push(mbr_type);
        data.push(signature_type);
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Media as u8,
            sub_type: MediaSubType::HardDrive as u8,
            data,
        });
        self
    }

    pub fn add_file_path(mut self, path: &str) -> Self {
        let mut data = Vec::new();
        for ch in path.encode_utf16() {
            data.extend_from_slice(&ch.to_le_bytes());
        }
        data.extend_from_slice(&[0u8; 2]); // null terminator
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Media as u8,
            sub_type: MediaSubType::FilePath as u8,
            data,
        });
        self
    }

    pub fn add_piwg_firmware(mut self, fv_name: EfiGuid) -> Self {
        let mut data = Vec::new();
        data.extend_from_slice(&fv_name.data1.to_le_bytes());
        data.extend_from_slice(&fv_name.data2.to_le_bytes());
        data.extend_from_slice(&fv_name.data3.to_le_bytes());
        data.extend_from_slice(&fv_name.data4);
        self.nodes.push(DevicePathEntry {
            node_type: DevicePathType::Media as u8,
            sub_type: MediaSubType::PiwgFirmwareVolume as u8,
            data,
        });
        self
    }

    pub fn to_string(&self) -> String {
        let mut s = String::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if i > 0 {
                s.push('/');
            }
            match node.node_type {
                0x01 => s.push_str("HW"),
                0x02 => s.push_str("ACPI"),
                0x03 => s.push_str("MSG"),
                0x04 => s.push_str("MEDIA"),
                _ => s.push_str("??"),
            }
            s.push_str(&alloc::format!("({:#x})", node.sub_type));
        }
        s
    }

    pub fn matches(&self, other: &DevicePath) -> bool {
        if self.nodes.len() != other.nodes.len() {
            return false;
        }
        for (a, b) in self.nodes.iter().zip(other.nodes.iter()) {
            if a.node_type != b.node_type || a.sub_type != b.sub_type || a.data != b.data {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// EFI Configuration Table
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EfiConfigurationTable {
    pub vendor_guid: EfiGuid,
    pub vendor_table: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTableType {
    Acpi20,
    Smbios,
    Smbios3,
    DeviceTreeBlob,
    DebugImageInfo,
    Unknown,
}

pub fn identify_config_table(guid: &EfiGuid) -> ConfigTableType {
    if *guid == EFI_ACPI_20_TABLE_GUID {
        ConfigTableType::Acpi20
    } else if *guid == EFI_SMBIOS_GUID {
        ConfigTableType::Smbios
    } else if *guid == EFI_SMBIOS3_GUID {
        ConfigTableType::Smbios3
    } else if *guid == EFI_DEVICE_TREE_GUID {
        ConfigTableType::DeviceTreeBlob
    } else {
        ConfigTableType::Unknown
    }
}

// ---------------------------------------------------------------------------
// EFI Variable Store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EfiVariable {
    pub name: String,
    pub vendor: EfiGuid,
    pub attributes: VariableAttributes,
    pub data: Vec<u8>,
}

pub struct VariableStore {
    variables: Vec<EfiVariable>,
    max_storage_size: usize,
    remaining_storage: usize,
    max_variable_size: usize,
}

impl VariableStore {
    pub fn new() -> Self {
        Self {
            variables: Vec::new(),
            max_storage_size: 256 * 1024,
            remaining_storage: 256 * 1024,
            max_variable_size: 32 * 1024,
        }
    }

    pub fn get_variable(&self, name: &str, vendor: &EfiGuid) -> Result<&EfiVariable, EfiStatus> {
        self.variables
            .iter()
            .find(|v| v.name == name && v.vendor == *vendor)
            .ok_or(EfiStatus::NotFound)
    }

    pub fn set_variable(
        &mut self,
        name: &str,
        vendor: &EfiGuid,
        attrs: VariableAttributes,
        data: &[u8],
    ) -> EfiStatus {
        if data.len() > self.max_variable_size {
            return EfiStatus::InvalidParameter;
        }

        if let Some(existing) = self
            .variables
            .iter_mut()
            .find(|v| v.name == name && v.vendor == *vendor)
        {
            let old_size = existing.data.len();
            if data.is_empty() {
                self.remaining_storage += old_size;
                self.variables
                    .retain(|v| !(v.name == name && v.vendor == *vendor));
                return EfiStatus::Success;
            }
            if data.len() > old_size && (data.len() - old_size) > self.remaining_storage {
                return EfiStatus::OutOfResources;
            }
            self.remaining_storage = self.remaining_storage + old_size - data.len();
            existing.attributes = attrs;
            existing.data = data.to_vec();
        } else {
            if data.is_empty() {
                return EfiStatus::NotFound;
            }
            if data.len() > self.remaining_storage {
                return EfiStatus::OutOfResources;
            }
            self.remaining_storage -= data.len();
            self.variables.push(EfiVariable {
                name: String::from(name),
                vendor: *vendor,
                attributes: attrs,
                data: data.to_vec(),
            });
        }
        EfiStatus::Success
    }

    pub fn get_next_variable_name(
        &self,
        current: Option<(&str, &EfiGuid)>,
    ) -> Option<(&str, &EfiGuid)> {
        match current {
            None => self.variables.first().map(|v| (v.name.as_str(), &v.vendor)),
            Some((name, vendor)) => {
                let pos = self
                    .variables
                    .iter()
                    .position(|v| v.name == name && v.vendor == *vendor)?;
                self.variables
                    .get(pos + 1)
                    .map(|v| (v.name.as_str(), &v.vendor))
            }
        }
    }

    pub fn query_variable_info(&self, attrs: VariableAttributes) -> (u64, u64, u64) {
        let _ = attrs;
        (
            self.max_storage_size as u64,
            self.remaining_storage as u64,
            self.max_variable_size as u64,
        )
    }

    fn populate_well_known(&mut self) {
        let nv_bs_rt = VariableAttributes::NON_VOLATILE
            | VariableAttributes::BOOTSERVICE_ACCESS
            | VariableAttributes::RUNTIME_ACCESS;
        let nv_bs_rt_ta = nv_bs_rt | VariableAttributes::TIME_BASED_AUTHENTICATED_WRITE_ACCESS;
        let bs_rt = VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;

        self.set_variable("SecureBoot", &EFI_GLOBAL_VARIABLE_GUID, bs_rt, &[0]);
        self.set_variable("SetupMode", &EFI_GLOBAL_VARIABLE_GUID, bs_rt, &[1]);
        self.set_variable("PK", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt_ta, &[]);
        self.set_variable("KEK", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt_ta, &[]);
        self.set_variable("db", &EFI_IMAGE_SECURITY_DATABASE_GUID, nv_bs_rt_ta, &[]);
        self.set_variable("dbx", &EFI_IMAGE_SECURITY_DATABASE_GUID, nv_bs_rt_ta, &[]);
        self.set_variable("dbr", &EFI_IMAGE_SECURITY_DATABASE_GUID, nv_bs_rt_ta, &[]);
        self.set_variable("dbt", &EFI_IMAGE_SECURITY_DATABASE_GUID, nv_bs_rt_ta, &[]);
        self.set_variable("MokList", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt, &[]);
        self.set_variable("MokListX", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt, &[]);

        self.set_variable("BootOrder", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt, &[0, 0]);
        self.set_variable(
            "Boot0000",
            &EFI_GLOBAL_VARIABLE_GUID,
            nv_bs_rt,
            &[1, 0, 0, 0],
        );
        self.set_variable("ConIn", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt, &[]);
        self.set_variable("ConOut", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt, &[]);
        self.set_variable("ErrOut", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt, &[]);
        self.set_variable("Timeout", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt, &[5, 0]);
        self.set_variable("Lang", &EFI_GLOBAL_VARIABLE_GUID, nv_bs_rt, b"eng");
        self.set_variable(
            "PlatformLang",
            &EFI_GLOBAL_VARIABLE_GUID,
            nv_bs_rt,
            b"en-US",
        );

        let os_ind_sup: u64 = 0x0000_0000_0000_0003;
        self.set_variable(
            "OsIndicationsSupported",
            &EFI_GLOBAL_VARIABLE_GUID,
            bs_rt,
            &os_ind_sup.to_le_bytes(),
        );
        self.set_variable(
            "OsIndications",
            &EFI_GLOBAL_VARIABLE_GUID,
            nv_bs_rt,
            &0u64.to_le_bytes(),
        );
    }
}

// ---------------------------------------------------------------------------
// Secure Boot — Signature Databases
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone)]
pub struct EfiSignatureList {
    pub signature_type: EfiGuid,
    pub signature_list_size: u32,
    pub signature_header_size: u32,
    pub signature_size: u32,
    pub signatures: Vec<EfiSignatureData>,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct EfiSignatureData {
    pub signature_owner: EfiGuid,
    pub signature_data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateType {
    X509,
    Sha256,
    Rsa2048,
    Unknown,
}

impl CertificateType {
    pub fn from_guid(guid: &EfiGuid) -> Self {
        if *guid == EFI_CERT_X509_GUID {
            Self::X509
        } else if *guid == EFI_CERT_SHA256_GUID {
            Self::Sha256
        } else if *guid == EFI_CERT_RSA2048_GUID {
            Self::Rsa2048
        } else {
            Self::Unknown
        }
    }

    pub fn to_guid(&self) -> EfiGuid {
        match self {
            Self::X509 => EFI_CERT_X509_GUID,
            Self::Sha256 => EFI_CERT_SHA256_GUID,
            Self::Rsa2048 => EFI_CERT_RSA2048_GUID,
            Self::Unknown => EfiGuid::new(0, 0, 0, [0; 8]),
        }
    }
}

pub struct SecureBootState {
    pub enabled: bool,
    pub setup_mode: bool,
    pub pk: Option<EfiSignatureList>,
    pub kek: Vec<EfiSignatureList>,
    pub db: Vec<EfiSignatureList>,
    pub dbx: Vec<EfiSignatureList>,
    pub mok_list: Vec<EfiSignatureList>,
    pub mok_list_x: Vec<EfiSignatureList>,
}

impl SecureBootState {
    pub fn new() -> Self {
        Self {
            enabled: false,
            setup_mode: true,
            pk: None,
            kek: Vec::new(),
            db: Vec::new(),
            dbx: Vec::new(),
            mok_list: Vec::new(),
            mok_list_x: Vec::new(),
        }
    }

    pub fn enroll_pk(&mut self, sig_list: EfiSignatureList) -> EfiStatus {
        self.pk = Some(sig_list);
        self.setup_mode = false;
        self.enabled = true;
        EfiStatus::Success
    }

    pub fn enroll_kek(&mut self, sig_list: EfiSignatureList) -> EfiStatus {
        self.kek.push(sig_list);
        EfiStatus::Success
    }

    pub fn enroll_db(&mut self, sig_list: EfiSignatureList) -> EfiStatus {
        self.db.push(sig_list);
        EfiStatus::Success
    }

    pub fn enroll_dbx(&mut self, sig_list: EfiSignatureList) -> EfiStatus {
        self.dbx.push(sig_list);
        EfiStatus::Success
    }

    pub fn enroll_mok(&mut self, sig_list: EfiSignatureList) -> EfiStatus {
        self.mok_list.push(sig_list);
        EfiStatus::Success
    }

    pub fn verify_image(&self, image_hash: &[u8]) -> Result<bool, EfiStatus> {
        if !self.enabled {
            return Ok(true);
        }

        if self.is_forbidden(image_hash) {
            return Ok(false);
        }

        if self.is_allowed(image_hash) {
            return Ok(true);
        }

        if self.is_mok_allowed(image_hash) {
            return Ok(true);
        }

        Ok(false)
    }

    fn is_forbidden(&self, hash: &[u8]) -> bool {
        for sig_list in &self.dbx {
            for sig in &sig_list.signatures {
                if sig.signature_data == hash {
                    return true;
                }
            }
        }
        for sig_list in &self.mok_list_x {
            for sig in &sig_list.signatures {
                if sig.signature_data == hash {
                    return true;
                }
            }
        }
        false
    }

    fn is_allowed(&self, hash: &[u8]) -> bool {
        for sig_list in &self.db {
            for sig in &sig_list.signatures {
                if sig.signature_data == hash {
                    return true;
                }
            }
        }
        false
    }

    fn is_mok_allowed(&self, hash: &[u8]) -> bool {
        for sig_list in &self.mok_list {
            for sig in &sig_list.signatures {
                if sig.signature_data == hash {
                    return true;
                }
            }
        }
        false
    }

    pub fn clear_pk(&mut self) -> EfiStatus {
        self.pk = None;
        self.setup_mode = true;
        self.enabled = false;
        EfiStatus::Success
    }
}

// ---------------------------------------------------------------------------
// EFI Capsule
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EfiCapsuleHeader {
    pub capsule_guid: EfiGuid,
    pub header_size: u32,
    pub flags: u32,
    pub capsule_image_size: u32,
}

pub const CAPSULE_FLAGS_PERSIST_ACROSS_RESET: u32 = 0x0001_0000;
pub const CAPSULE_FLAGS_POPULATE_SYSTEM_TABLE: u32 = 0x0002_0000;
pub const CAPSULE_FLAGS_INITIATE_RESET: u32 = 0x0004_0000;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EfiCapsuleBlockDescriptor {
    pub length: u64,
    pub data_block_or_continuation: u64,
}

pub struct CapsuleManager {
    pending_capsules: Vec<CapsulePending>,
    processed_count: u64,
}

#[derive(Debug, Clone)]
struct CapsulePending {
    header: EfiCapsuleHeader,
    data: Vec<u8>,
}

impl CapsuleManager {
    pub fn new() -> Self {
        Self {
            pending_capsules: Vec::new(),
            processed_count: 0,
        }
    }

    pub fn update_capsule(&mut self, header: EfiCapsuleHeader, data: Vec<u8>) -> EfiStatus {
        if header.capsule_image_size < header.header_size {
            return EfiStatus::InvalidParameter;
        }
        self.pending_capsules.push(CapsulePending { header, data });
        EfiStatus::Success
    }

    pub fn query_capsule_capabilities(
        &self,
        header: &EfiCapsuleHeader,
    ) -> Result<(u32, EfiResetType), EfiStatus> {
        let max_size = 16 * 1024 * 1024; // 16 MiB max capsule
        if header.capsule_image_size > max_size {
            return Err(EfiStatus::OutOfResources);
        }
        let reset = if header.flags & CAPSULE_FLAGS_INITIATE_RESET != 0 {
            EfiResetType::Warm
        } else {
            EfiResetType::Cold
        };
        Ok((max_size, reset))
    }

    pub fn process_pending(&mut self) -> usize {
        let count = self.pending_capsules.len();
        self.pending_capsules.clear();
        self.processed_count += count as u64;
        count
    }

    pub fn pending_count(&self) -> usize {
        self.pending_capsules.len()
    }
}

// ---------------------------------------------------------------------------
// EFI Reset Type
// ---------------------------------------------------------------------------

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EfiResetType {
    Cold = 0,
    Warm = 1,
    Shutdown = 2,
    PlatformSpecific = 3,
}

// ---------------------------------------------------------------------------
// SMBIOS Parsing
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SmbiosEntryPoint {
    pub anchor_string: [u8; 4],
    pub checksum: u8,
    pub entry_point_length: u8,
    pub major_version: u8,
    pub minor_version: u8,
    pub max_structure_size: u16,
    pub entry_point_revision: u8,
    pub formatted_area: [u8; 5],
    pub intermediate_anchor: [u8; 5],
    pub intermediate_checksum: u8,
    pub structure_table_length: u16,
    pub structure_table_address: u32,
    pub number_of_structures: u16,
    pub bcd_revision: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Smbios3EntryPoint {
    pub anchor_string: [u8; 5],
    pub checksum: u8,
    pub entry_point_length: u8,
    pub major_version: u8,
    pub minor_version: u8,
    pub docrev: u8,
    pub entry_point_revision: u8,
    pub reserved: u8,
    pub structure_table_max_size: u32,
    pub structure_table_address: u64,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SmbiosHeader {
    pub struct_type: u8,
    pub length: u8,
    pub handle: u16,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmbiosType {
    BiosInformation = 0,
    SystemInformation = 1,
    BaseBoardInformation = 2,
    ChassisInformation = 3,
    ProcessorInformation = 4,
    CacheInformation = 5,
    MemoryController = 5 + 1, // type 6 — avoid enum overlap
    MemoryModule = 6 + 1,     // type 7
    SystemSlots = 9,
    OemStrings = 11,
    PortConnector = 8,
    MemoryDevice = 17,
    SystemBoot = 32,
    ManagementDevice = 34,
    TpmDevice = 43,
    EndOfTable = 127,
}

impl SmbiosType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::BiosInformation),
            1 => Some(Self::SystemInformation),
            2 => Some(Self::BaseBoardInformation),
            3 => Some(Self::ChassisInformation),
            4 => Some(Self::ProcessorInformation),
            5 => Some(Self::CacheInformation),
            9 => Some(Self::SystemSlots),
            11 => Some(Self::OemStrings),
            8 => Some(Self::PortConnector),
            17 => Some(Self::MemoryDevice),
            32 => Some(Self::SystemBoot),
            34 => Some(Self::ManagementDevice),
            43 => Some(Self::TpmDevice),
            127 => Some(Self::EndOfTable),
            _ => None,
        }
    }
}

pub struct SmbiosParser {
    structures: Vec<SmbiosStructure>,
}

#[derive(Debug, Clone)]
pub struct SmbiosStructure {
    pub struct_type: u8,
    pub handle: u16,
    pub data: Vec<u8>,
    pub strings: Vec<String>,
}

impl SmbiosParser {
    pub fn new() -> Self {
        Self {
            structures: Vec::new(),
        }
    }

    pub fn parse_table(&mut self, table_base: u64, table_length: u32) {
        let mut offset = 0u32;
        while offset < table_length {
            let remaining = (table_length - offset) as usize;
            if remaining < 4 {
                break;
            }

            let struct_type = self.read_byte(table_base, offset);
            let length = self.read_byte(table_base, offset + 1);
            let handle = self.read_word(table_base, offset + 2);

            if length < 4 {
                break;
            }

            let mut data = Vec::new();
            for i in 0..length as u32 {
                data.push(self.read_byte(table_base, offset + i));
            }

            let string_start = offset + length as u32;
            let strings = self.parse_strings(table_base, string_start, table_length);

            let mut string_end = string_start;
            loop {
                if string_end + 1 >= table_length {
                    break;
                }
                let b = self.read_byte(table_base, string_end);
                if b == 0 {
                    let next = self.read_byte(table_base, string_end + 1);
                    if next == 0 {
                        string_end += 2;
                        break;
                    }
                }
                string_end += 1;
            }

            self.structures.push(SmbiosStructure {
                struct_type,
                handle,
                data,
                strings,
            });

            if struct_type == 127 {
                break;
            }
            offset = string_end;
        }
    }

    fn read_byte(&self, _base: u64, _offset: u32) -> u8 {
        0
    }

    fn read_word(&self, _base: u64, _offset: u32) -> u16 {
        0
    }

    fn parse_strings(&self, _base: u64, _offset: u32, _limit: u32) -> Vec<String> {
        Vec::new()
    }

    pub fn get_string(structure: &SmbiosStructure, index: u8) -> Option<&str> {
        if index == 0 {
            return None;
        }
        structure
            .strings
            .get((index - 1) as usize)
            .map(|s| s.as_str())
    }

    pub fn find_by_type(&self, stype: u8) -> Vec<&SmbiosStructure> {
        self.structures
            .iter()
            .filter(|s| s.struct_type == stype)
            .collect()
    }

    pub fn bios_vendor(&self) -> Option<&str> {
        let bioses = self.find_by_type(0);
        let bios = bioses.first()?;
        if bios.data.len() > 4 {
            Self::get_string(bios, bios.data[4])
        } else {
            None
        }
    }

    pub fn bios_version(&self) -> Option<&str> {
        let bioses = self.find_by_type(0);
        let bios = bioses.first()?;
        if bios.data.len() > 5 {
            Self::get_string(bios, bios.data[5])
        } else {
            None
        }
    }

    pub fn bios_release_date(&self) -> Option<&str> {
        let bioses = self.find_by_type(0);
        let bios = bioses.first()?;
        if bios.data.len() > 8 {
            Self::get_string(bios, bios.data[8])
        } else {
            None
        }
    }

    pub fn system_manufacturer(&self) -> Option<&str> {
        let systems = self.find_by_type(1);
        let sys = systems.first()?;
        if sys.data.len() > 4 {
            Self::get_string(sys, sys.data[4])
        } else {
            None
        }
    }

    pub fn system_product_name(&self) -> Option<&str> {
        let systems = self.find_by_type(1);
        let sys = systems.first()?;
        if sys.data.len() > 5 {
            Self::get_string(sys, sys.data[5])
        } else {
            None
        }
    }

    pub fn system_serial_number(&self) -> Option<&str> {
        let systems = self.find_by_type(1);
        let sys = systems.first()?;
        if sys.data.len() > 7 {
            Self::get_string(sys, sys.data[7])
        } else {
            None
        }
    }

    pub fn system_uuid(&self) -> Option<[u8; 16]> {
        let systems = self.find_by_type(1);
        let sys = systems.first()?;
        if sys.data.len() >= 24 {
            let mut uuid = [0u8; 16];
            uuid.copy_from_slice(&sys.data[8..24]);
            Some(uuid)
        } else {
            None
        }
    }

    pub fn board_manufacturer(&self) -> Option<&str> {
        let boards = self.find_by_type(2);
        let board = boards.first()?;
        if board.data.len() > 4 {
            Self::get_string(board, board.data[4])
        } else {
            None
        }
    }

    pub fn board_product_name(&self) -> Option<&str> {
        let boards = self.find_by_type(2);
        let board = boards.first()?;
        if board.data.len() > 5 {
            Self::get_string(board, board.data[5])
        } else {
            None
        }
    }

    pub fn chassis_type(&self) -> Option<u8> {
        let chassis = self.find_by_type(3);
        let ch = chassis.first()?;
        if ch.data.len() > 5 {
            Some(ch.data[5])
        } else {
            None
        }
    }

    pub fn processor_info(&self) -> Vec<ProcessorInfo> {
        let mut result = Vec::new();
        for s in self.find_by_type(4) {
            if s.data.len() >= 42 {
                result.push(ProcessorInfo {
                    socket: Self::get_string(s, s.data[4]).unwrap_or("").into(),
                    manufacturer: Self::get_string(s, s.data[7]).unwrap_or("").into(),
                    version: Self::get_string(s, s.data[16]).unwrap_or("").into(),
                    max_speed_mhz: u16::from_le_bytes([s.data[20], s.data[21]]),
                    current_speed_mhz: u16::from_le_bytes([s.data[22], s.data[23]]),
                    core_count: if s.data.len() > 35 { s.data[35] } else { 1 },
                    thread_count: if s.data.len() > 37 { s.data[37] } else { 1 },
                });
            }
        }
        result
    }

    pub fn memory_devices(&self) -> Vec<MemoryDeviceInfo> {
        let mut result = Vec::new();
        for s in self.find_by_type(17) {
            if s.data.len() >= 28 {
                result.push(MemoryDeviceInfo {
                    size_mb: u16::from_le_bytes([s.data[12], s.data[13]]),
                    form_factor: s.data[14],
                    locator: Self::get_string(s, s.data[16]).unwrap_or("").into(),
                    bank_locator: Self::get_string(s, s.data[17]).unwrap_or("").into(),
                    memory_type: s.data[18],
                    speed_mhz: u16::from_le_bytes([s.data[21], s.data[22]]),
                    manufacturer: Self::get_string(s, s.data[23]).unwrap_or("").into(),
                    serial_number: Self::get_string(s, s.data[24]).unwrap_or("").into(),
                    part_number: Self::get_string(s, s.data[26]).unwrap_or("").into(),
                });
            }
        }
        result
    }
}

#[derive(Debug, Clone)]
pub struct ProcessorInfo {
    pub socket: String,
    pub manufacturer: String,
    pub version: String,
    pub max_speed_mhz: u16,
    pub current_speed_mhz: u16,
    pub core_count: u8,
    pub thread_count: u8,
}

#[derive(Debug, Clone)]
pub struct MemoryDeviceInfo {
    pub size_mb: u16,
    pub form_factor: u8,
    pub locator: String,
    pub bank_locator: String,
    pub memory_type: u8,
    pub speed_mhz: u16,
    pub manufacturer: String,
    pub serial_number: String,
    pub part_number: String,
}

// ---------------------------------------------------------------------------
// EFI RNG (Random Number Generator)
// ---------------------------------------------------------------------------

pub struct EfiRng {
    algorithms: Vec<EfiGuid>,
    seed: u64,
}

impl EfiRng {
    pub fn new() -> Self {
        Self {
            algorithms: vec![
                EFI_RNG_ALGORITHM_RAW,
                EFI_RNG_ALGORITHM_SP800_90_HASH256,
                EFI_RNG_ALGORITHM_SP800_90_CTR256,
            ],
            seed: 0xDEAD_BEEF_CAFE_BABE,
        }
    }

    pub fn get_info(&self) -> &[EfiGuid] {
        &self.algorithms
    }

    pub fn get_rng(&mut self, algorithm: Option<&EfiGuid>, buf: &mut [u8]) -> EfiStatus {
        if let Some(alg) = algorithm {
            if !self.algorithms.contains(alg) {
                return EfiStatus::Unsupported;
            }
        }
        for byte in buf.iter_mut() {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            *byte = self.seed as u8;
        }
        EfiStatus::Success
    }
}

// ---------------------------------------------------------------------------
// EFI System Table & Runtime Services
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EfiTableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

pub struct EfiSystemTable {
    pub header: EfiTableHeader,
    pub firmware_vendor: String,
    pub firmware_revision: u32,
    pub config_tables: Vec<EfiConfigurationTable>,
    pub boot_services_active: bool,
}

impl EfiSystemTable {
    pub fn new(vendor: &str, revision: u32) -> Self {
        Self {
            header: EfiTableHeader {
                signature: 0x5453_5953_2049_4249, // "IBI SYST"
                revision: 0x0002_0046,            // UEFI 2.70
                header_size: 120,
                crc32: 0,
                reserved: 0,
            },
            firmware_vendor: String::from(vendor),
            firmware_revision: revision,
            config_tables: Vec::new(),
            boot_services_active: false,
        }
    }

    pub fn add_config_table(&mut self, guid: EfiGuid, address: u64) {
        if let Some(existing) = self
            .config_tables
            .iter_mut()
            .find(|t| t.vendor_guid == guid)
        {
            existing.vendor_table = address;
        } else {
            self.config_tables.push(EfiConfigurationTable {
                vendor_guid: guid,
                vendor_table: address,
            });
        }
    }

    pub fn find_config_table(&self, guid: &EfiGuid) -> Option<u64> {
        self.config_tables
            .iter()
            .find(|t| t.vendor_guid == *guid)
            .map(|t| t.vendor_table)
    }
}

// ---------------------------------------------------------------------------
// EFI Runtime — the unified runtime state object
// ---------------------------------------------------------------------------

pub struct EfiRuntime {
    pub system_table: EfiSystemTable,
    pub variable_store: VariableStore,
    pub secure_boot: SecureBootState,
    pub memory_map: Vec<EfiMemoryDescriptor>,
    pub memory_map_key: u64,
    pub descriptor_size: u32,
    pub descriptor_version: u32,
    pub capsule_manager: CapsuleManager,
    pub smbios_parser: SmbiosParser,
    pub rng: EfiRng,
    pub current_time: EfiTime,
    pub wakeup_enabled: bool,
    pub wakeup_pending: bool,
    pub wakeup_time: EfiTime,
    pub monotonic_count: u64,
    pub virtual_map_set: bool,
    pub initialized: bool,
}

impl EfiRuntime {
    pub fn new() -> Self {
        Self {
            system_table: EfiSystemTable::new("AthenaOS", 0x0001_0000),
            variable_store: VariableStore::new(),
            secure_boot: SecureBootState::new(),
            memory_map: Vec::new(),
            memory_map_key: 0,
            descriptor_size: core::mem::size_of::<EfiMemoryDescriptor>() as u32,
            descriptor_version: 1,
            capsule_manager: CapsuleManager::new(),
            smbios_parser: SmbiosParser::new(),
            rng: EfiRng::new(),
            current_time: EfiTime::default(),
            wakeup_enabled: false,
            wakeup_pending: false,
            wakeup_time: EfiTime::default(),
            monotonic_count: 0,
            virtual_map_set: false,
            initialized: false,
        }
    }

    // --- Runtime service: GetTime ---
    pub fn get_time(&self) -> (EfiTime, EfiTimeCapabilities) {
        let caps = EfiTimeCapabilities {
            resolution: 1,
            accuracy: 50_000_000,
            sets_to_zero: false,
        };
        (self.current_time, caps)
    }

    // --- Runtime service: SetTime ---
    pub fn set_time(&mut self, time: &EfiTime) -> EfiStatus {
        if time.month == 0 || time.month > 12 {
            return EfiStatus::InvalidParameter;
        }
        if time.day == 0 || time.day > 31 {
            return EfiStatus::InvalidParameter;
        }
        if time.hour > 23 {
            return EfiStatus::InvalidParameter;
        }
        if time.minute > 59 {
            return EfiStatus::InvalidParameter;
        }
        if time.second > 59 {
            return EfiStatus::InvalidParameter;
        }
        self.current_time = *time;
        EfiStatus::Success
    }

    // --- Runtime service: GetWakeupTime ---
    pub fn get_wakeup_time(&self) -> (bool, bool, EfiTime) {
        (self.wakeup_enabled, self.wakeup_pending, self.wakeup_time)
    }

    // --- Runtime service: SetWakeupTime ---
    pub fn set_wakeup_time(&mut self, enable: bool, time: Option<&EfiTime>) -> EfiStatus {
        self.wakeup_enabled = enable;
        self.wakeup_pending = false;
        if let Some(t) = time {
            self.wakeup_time = *t;
        }
        EfiStatus::Success
    }

    // --- Runtime service: SetVirtualAddressMap ---
    pub fn set_virtual_address_map(&mut self, map: &[EfiMemoryDescriptor]) -> EfiStatus {
        if self.virtual_map_set {
            return EfiStatus::Unsupported;
        }
        self.memory_map = map.to_vec();
        self.virtual_map_set = true;
        self.memory_map_key += 1;
        EfiStatus::Success
    }

    // --- Runtime service: ConvertPointer ---
    pub fn convert_pointer(&self, pointer: u64) -> Result<u64, EfiStatus> {
        if !self.virtual_map_set {
            return Err(EfiStatus::NotStarted);
        }
        for desc in &self.memory_map {
            let phys_end = desc.physical_start + desc.number_of_pages * 4096;
            if pointer >= desc.physical_start && pointer < phys_end {
                let offset = pointer - desc.physical_start;
                return Ok(desc.virtual_start + offset);
            }
        }
        Err(EfiStatus::NotFound)
    }

    // --- Runtime service: GetVariable ---
    pub fn get_variable(&self, name: &str, vendor: &EfiGuid) -> Result<&EfiVariable, EfiStatus> {
        self.variable_store.get_variable(name, vendor)
    }

    // --- Runtime service: GetNextVariableName ---
    pub fn get_next_variable_name(
        &self,
        current: Option<(&str, &EfiGuid)>,
    ) -> Option<(&str, &EfiGuid)> {
        self.variable_store.get_next_variable_name(current)
    }

    // --- Runtime service: SetVariable ---
    pub fn set_variable(
        &mut self,
        name: &str,
        vendor: &EfiGuid,
        attrs: VariableAttributes,
        data: &[u8],
    ) -> EfiStatus {
        self.variable_store.set_variable(name, vendor, attrs, data)
    }

    // --- Runtime service: GetNextHighMonotonicCount ---
    pub fn get_next_high_monotonic_count(&mut self) -> u32 {
        self.monotonic_count += 1;
        (self.monotonic_count >> 32) as u32
    }

    // --- Runtime service: ResetSystem ---
    pub fn reset_system(
        &self,
        reset_type: EfiResetType,
        status: EfiStatus,
        _data: Option<&[u8]>,
    ) -> ! {
        let _ = (reset_type, status);
        loop {
            core::hint::spin_loop();
        }
    }

    // --- Runtime service: UpdateCapsule ---
    pub fn update_capsule(&mut self, header: EfiCapsuleHeader, data: Vec<u8>) -> EfiStatus {
        self.capsule_manager.update_capsule(header, data)
    }

    // --- Runtime service: QueryCapsuleCapabilities ---
    pub fn query_capsule_capabilities(
        &self,
        header: &EfiCapsuleHeader,
    ) -> Result<(u32, EfiResetType), EfiStatus> {
        self.capsule_manager.query_capsule_capabilities(header)
    }

    // --- Runtime service: QueryVariableInfo ---
    pub fn query_variable_info(&self, attrs: VariableAttributes) -> (u64, u64, u64) {
        self.variable_store.query_variable_info(attrs)
    }

    // --- GetRNG ---
    pub fn get_rng(&mut self, algorithm: Option<&EfiGuid>, buf: &mut [u8]) -> EfiStatus {
        self.rng.get_rng(algorithm, buf)
    }

    // --- Memory map management ---
    pub fn add_memory_descriptor(&mut self, desc: EfiMemoryDescriptor) {
        self.memory_map.push(desc);
        self.memory_map_key += 1;
    }

    pub fn get_memory_map(&self) -> (&[EfiMemoryDescriptor], u64, u32, u32) {
        (
            &self.memory_map,
            self.memory_map_key,
            self.descriptor_size,
            self.descriptor_version,
        )
    }

    pub fn total_conventional_memory(&self) -> u64 {
        self.memory_map
            .iter()
            .filter(|d| d.mem_type() == EfiMemoryType::ConventionalMemory)
            .map(|d| d.size_bytes())
            .sum()
    }

    pub fn total_runtime_memory(&self) -> u64 {
        self.memory_map
            .iter()
            .filter(|d| {
                d.mem_type() == EfiMemoryType::RuntimeServicesCode
                    || d.mem_type() == EfiMemoryType::RuntimeServicesData
            })
            .map(|d| d.size_bytes())
            .sum()
    }
}

// ---------------------------------------------------------------------------
// Global EFI Runtime
// ---------------------------------------------------------------------------

pub static EFI_RUNTIME: Mutex<Option<EfiRuntime>> = Mutex::new(None);

pub fn init() {
    let mut rt = EfiRuntime::new();

    rt.system_table.add_config_table(EFI_ACPI_20_TABLE_GUID, 0);
    rt.system_table.add_config_table(EFI_SMBIOS_GUID, 0);
    rt.system_table.add_config_table(EFI_SMBIOS3_GUID, 0);

    rt.variable_store.populate_well_known();

    rt.add_memory_descriptor(EfiMemoryDescriptor {
        memory_type: EfiMemoryType::ConventionalMemory as u32,
        physical_start: 0x0010_0000,
        virtual_start: 0x0010_0000,
        number_of_pages: 256,
        attribute: EfiMemoryAttribute::WB.bits(),
    });
    rt.add_memory_descriptor(EfiMemoryDescriptor {
        memory_type: EfiMemoryType::RuntimeServicesCode as u32,
        physical_start: 0x0000_0000,
        virtual_start: 0x0000_0000,
        number_of_pages: 16,
        attribute: (EfiMemoryAttribute::WB | EfiMemoryAttribute::RUNTIME).bits(),
    });
    rt.add_memory_descriptor(EfiMemoryDescriptor {
        memory_type: EfiMemoryType::AcpiReclaimMemory as u32,
        physical_start: 0x000E_0000,
        virtual_start: 0x000E_0000,
        number_of_pages: 32,
        attribute: EfiMemoryAttribute::WB.bits(),
    });

    rt.initialized = true;
    *EFI_RUNTIME.lock() = Some(rt);
}
