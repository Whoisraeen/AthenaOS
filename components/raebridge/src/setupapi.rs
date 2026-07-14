//! setupapi.dll — Device installation, enumeration, driver management, INF file
//! parsing, and Configuration Manager (CM) devnode APIs for AthBridge.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    DWord, WinHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, INVALID_HANDLE_VALUE, NULL_HANDLE,
};

// =========================================================================
// DIGCF flags — SetupDiGetClassDevsW
// =========================================================================

pub const DIGCF_DEFAULT: u32 = 0x00000001;
pub const DIGCF_PRESENT: u32 = 0x00000002;
pub const DIGCF_ALLCLASSES: u32 = 0x00000004;
pub const DIGCF_PROFILE: u32 = 0x00000008;
pub const DIGCF_DEVICEINTERFACE: u32 = 0x00000010;

// =========================================================================
// SPDRP — Device registry property codes
// =========================================================================

pub const SPDRP_DEVICEDESC: u32 = 0x00000000;
pub const SPDRP_HARDWAREID: u32 = 0x00000001;
pub const SPDRP_COMPATIBLEIDS: u32 = 0x00000002;
pub const SPDRP_SERVICE: u32 = 0x00000005;
pub const SPDRP_CLASS: u32 = 0x00000007;
pub const SPDRP_CLASSGUID: u32 = 0x00000008;
pub const SPDRP_DRIVER: u32 = 0x00000009;
pub const SPDRP_MFG: u32 = 0x0000000B;
pub const SPDRP_FRIENDLYNAME: u32 = 0x0000000C;
pub const SPDRP_LOCATION_INFORMATION: u32 = 0x0000000D;
pub const SPDRP_PHYSICAL_DEVICE_OBJECT_NAME: u32 = 0x0000000E;
pub const SPDRP_CAPABILITIES: u32 = 0x0000000F;
pub const SPDRP_UI_NUMBER: u32 = 0x00000010;
pub const SPDRP_ADDRESS: u32 = 0x0000001C;
pub const SPDRP_BUSNUMBER: u32 = 0x00000015;
pub const SPDRP_ENUMERATOR_NAME: u32 = 0x00000016;
pub const SPDRP_DEVTYPE: u32 = 0x00000019;
pub const SPDRP_EXCLUSIVE: u32 = 0x0000001A;
pub const SPDRP_INSTALL_STATE: u32 = 0x00000022;
pub const SPDRP_REMOVAL_POLICY: u32 = 0x0000001F;
pub const SPDRP_REMOVAL_POLICY_HW_DEFAULT: u32 = 0x00000020;
pub const SPDRP_REMOVAL_POLICY_OVERRIDE: u32 = 0x00000021;
pub const SPDRP_BASE_CONTAINERID: u32 = 0x00000024;

// =========================================================================
// DIF — Device installation function codes
// =========================================================================

pub const DIF_SELECTDEVICE: u32 = 0x00000001;
pub const DIF_INSTALLDEVICE: u32 = 0x00000002;
pub const DIF_REMOVE: u32 = 0x00000005;
pub const DIF_SELECTBESTCOMPATDRV: u32 = 0x0000000D;
pub const DIF_PROPERTYCHANGE: u32 = 0x00000012;

// =========================================================================
// CM_DEVCAP — Device capabilities
// =========================================================================

pub const CM_DEVCAP_LOCKSUPPORTED: u32 = 0x00000001;
pub const CM_DEVCAP_EJECTSUPPORTED: u32 = 0x00000002;
pub const CM_DEVCAP_REMOVABLE: u32 = 0x00000004;
pub const CM_DEVCAP_DOCKDEVICE: u32 = 0x00000008;
pub const CM_DEVCAP_UNIQUEID: u32 = 0x00000010;
pub const CM_DEVCAP_SILENTINSTALL: u32 = 0x00000020;
pub const CM_DEVCAP_RAWDEVICEOK: u32 = 0x00000040;
pub const CM_DEVCAP_SURPRISEREMOVALOK: u32 = 0x00000080;

// =========================================================================
// CM return codes
// =========================================================================

pub const CR_SUCCESS: u32 = 0x00000000;
pub const CR_DEFAULT: u32 = 0x00000001;
pub const CR_OUT_OF_MEMORY: u32 = 0x00000002;
pub const CR_INVALID_POINTER: u32 = 0x00000003;
pub const CR_INVALID_DEVNODE: u32 = 0x0000000D;
pub const CR_NO_SUCH_DEVNODE: u32 = 0x0000000D;
pub const CR_INVALID_PROPERTY: u32 = 0x00000027;
pub const CR_NO_SUCH_VALUE: u32 = 0x00000025;
pub const CR_BUFFER_SMALL: u32 = 0x0000001A;

// =========================================================================
// CM_REENUMERATE flags
// =========================================================================

pub const CM_REENUMERATE_NORMAL: u32 = 0x00000000;
pub const CM_REENUMERATE_SYNCHRONOUS: u32 = 0x00000001;
pub const CM_REENUMERATE_RETRY_INSTALLATION: u32 = 0x00000002;

// =========================================================================
// Device class GUIDs (as 16-byte arrays for no_std)
// =========================================================================

pub const GUID_DEVCLASS_DISPLAY: [u8; 16] = [
    0xD3, 0x2A, 0x6C, 0x4D, 0x14, 0x6C, 0x11, 0xD0, 0xB0, 0x84, 0x00, 0x60, 0x97, 0x13, 0x05, 0x4F,
];
pub const GUID_DEVCLASS_NET: [u8; 16] = [
    0xAD, 0x49, 0x8A, 0x4D, 0x5D, 0x19, 0x11, 0xD2, 0x96, 0x0C, 0x00, 0xC0, 0x4F, 0xB9, 0x38, 0x6D,
];
pub const GUID_DEVCLASS_USB: [u8; 16] = [
    0x36, 0xFC, 0x9E, 0x60, 0xC4, 0x65, 0x11, 0xCE, 0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18,
];
pub const GUID_DEVCLASS_DISKDRIVE: [u8; 16] = [
    0x53, 0xF5, 0x63, 0x07, 0xB6, 0xBF, 0x11, 0xD0, 0x94, 0xF2, 0x00, 0xA0, 0xC9, 0x1E, 0xFB, 0x8B,
];
pub const GUID_DEVCLASS_CDROM: [u8; 16] = [
    0x53, 0xF5, 0x63, 0x08, 0xB6, 0xBF, 0x11, 0xD0, 0x94, 0xF2, 0x00, 0xA0, 0xC9, 0x1E, 0xFB, 0x8B,
];
pub const GUID_DEVCLASS_HIDCLASS: [u8; 16] = [
    0x74, 0x5A, 0x17, 0xA0, 0x74, 0xD8, 0x11, 0xD0, 0xB0, 0x24, 0x00, 0xC0, 0x4F, 0xC2, 0x95, 0xEE,
];
pub const GUID_DEVCLASS_MOUSE: [u8; 16] = [
    0x4D, 0x36, 0xE9, 0x6F, 0xE3, 0x25, 0x11, 0xCE, 0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18,
];
pub const GUID_DEVCLASS_KEYBOARD: [u8; 16] = [
    0x4D, 0x36, 0xE9, 0x6B, 0xE3, 0x25, 0x11, 0xCE, 0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18,
];
pub const GUID_DEVCLASS_MONITOR: [u8; 16] = [
    0x4D, 0x36, 0xE9, 0x6E, 0xE3, 0x25, 0x11, 0xCE, 0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18,
];
pub const GUID_DEVCLASS_PORTS: [u8; 16] = [
    0x4D, 0x36, 0xE9, 0x78, 0xE3, 0x25, 0x11, 0xCE, 0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18,
];
pub const GUID_DEVCLASS_BLUETOOTH: [u8; 16] = [
    0xE0, 0xCB, 0xF0, 0x6C, 0xCD, 0x8B, 0x4C, 0x47, 0x83, 0x12, 0xA7, 0x42, 0x72, 0xEF, 0x4C, 0x30,
];
pub const GUID_DEVCLASS_CAMERA: [u8; 16] = [
    0xCA, 0x3E, 0x7A, 0xB9, 0xB4, 0xC3, 0x4A, 0xE6, 0x82, 0x51, 0x57, 0x9E, 0xF9, 0x33, 0x89, 0x0F,
];
pub const GUID_DEVCLASS_AUDIO_ENDPOINT: [u8; 16] = [
    0xC1, 0x66, 0x52, 0x3C, 0x0F, 0x26, 0x46, 0x8D, 0xA0, 0x20, 0x7D, 0x81, 0xF0, 0x64, 0x7D, 0x12,
];
pub const GUID_DEVCLASS_IMAGE: [u8; 16] = [
    0x6B, 0xDD, 0x1F, 0xC6, 0x81, 0x0F, 0x11, 0xD0, 0xBE, 0xC7, 0x08, 0x00, 0x2B, 0xE2, 0x09, 0x2F,
];
pub const GUID_DEVCLASS_PRINTER: [u8; 16] = [
    0x4D, 0x36, 0xE9, 0x79, 0xE3, 0x25, 0x11, 0xCE, 0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18,
];
pub const GUID_DEVCLASS_WPD: [u8; 16] = [
    0xEE, 0xC5, 0xAD, 0x98, 0x80, 0x80, 0x42, 0x5F, 0x92, 0x2A, 0xDA, 0xBF, 0x3D, 0xE3, 0xF6, 0x9A,
];
pub const GUID_DEVCLASS_SMARTCARD: [u8; 16] = [
    0x99, 0x0A, 0x2B, 0xD7, 0xE7, 0x38, 0x46, 0x48, 0xB0, 0xEF, 0x5F, 0x3E, 0x31, 0x62, 0x6D, 0x26,
];
pub const GUID_DEVCLASS_SYSTEM: [u8; 16] = [
    0x4D, 0x36, 0xE9, 0x7D, 0xE3, 0x25, 0x11, 0xCE, 0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18,
];
pub const GUID_DEVCLASS_VOLUME: [u8; 16] = [
    0x71, 0xA2, 0x7C, 0xDD, 0x81, 0x2A, 0x11, 0xD0, 0xBE, 0xC7, 0x08, 0x00, 0x2B, 0xE2, 0x09, 0x2F,
];
pub const GUID_DEVCLASS_BATTERY: [u8; 16] = [
    0x72, 0x63, 0x1E, 0x54, 0xC3, 0x6F, 0x11, 0xD2, 0x8A, 0xD9, 0x00, 0xA0, 0xC9, 0xA0, 0x6D, 0x45,
];
pub const GUID_DEVCLASS_BIOMETRIC: [u8; 16] = [
    0x53, 0xD2, 0x9E, 0xF7, 0x37, 0x7C, 0x48, 0x27, 0x87, 0x35, 0x12, 0x81, 0xA3, 0xC5, 0xAA, 0x2F,
];

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct SpDevInfoData {
    pub size: u32,
    pub class_guid: [u8; 16],
    pub dev_inst: u32,
    pub reserved: u64,
}

impl SpDevInfoData {
    pub fn new() -> Self {
        Self {
            size: 32,
            class_guid: [0u8; 16],
            dev_inst: 0,
            reserved: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpDevInterfaceData {
    pub size: u32,
    pub interface_class_guid: [u8; 16],
    pub flags: u32,
    pub reserved: u64,
}

impl SpDevInterfaceData {
    pub fn new() -> Self {
        Self {
            size: 32,
            interface_class_guid: [0u8; 16],
            flags: 0,
            reserved: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpDevInterfaceDetailData {
    pub size: u32,
    pub device_path: String,
}

#[derive(Debug, Clone)]
pub struct SpDriverInfoData {
    pub size: u32,
    pub driver_type: u32,
    pub reserved: u64,
    pub description: String,
    pub mfg_name: String,
    pub provider_name: String,
    pub driver_date: u64,
    pub driver_version: u64,
}

#[derive(Debug, Clone)]
pub struct SpDriverInfoDetailData {
    pub size: u32,
    pub inf_date: u64,
    pub compat_ids_offset: u32,
    pub compat_ids_length: u32,
    pub reserved: u64,
    pub section_name: String,
    pub inf_file_name: String,
    pub drv_description: String,
    pub hardware_id: String,
}

#[derive(Debug, Clone)]
pub struct InfContext {
    pub inf_handle: WinHandle,
    pub section: String,
    pub line: u32,
}

#[derive(Debug, Clone)]
struct EmulatedDevice {
    instance_id: String,
    description: String,
    hardware_id: String,
    class: String,
    class_guid: [u8; 16],
    manufacturer: String,
    friendly_name: String,
    service: String,
    location: String,
    capabilities: u32,
    dev_inst: u32,
    parent_inst: u32,
    children: Vec<u32>,
    status: u32,
    problem: u32,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct DevInfoSet {
    handle: WinHandle,
    class_guid: Option<[u8; 16]>,
    flags: u32,
    devices: Vec<EmulatedDevice>,
}

struct InfFile {
    handle: WinHandle,
    sections: BTreeMap<String, Vec<Vec<String>>>,
}

// =========================================================================
// Global state
// =========================================================================

pub struct SetupApi {
    next_handle: u64,
    dev_info_sets: BTreeMap<u64, DevInfoSet>,
    inf_files: BTreeMap<u64, InfFile>,
    all_devices: Vec<EmulatedDevice>,
    next_dev_inst: u32,
}

impl SetupApi {
    const fn new() -> Self {
        Self {
            next_handle: 0xD000_0000,
            dev_info_sets: BTreeMap::new(),
            inf_files: BTreeMap::new(),
            all_devices: Vec::new(),
            next_dev_inst: 1,
        }
    }

    fn alloc_handle(&mut self) -> WinHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        WinHandle(h)
    }

    fn populate_default_devices(&mut self) {
        let devs = [
            (
                "PCI\\VEN_8086&DEV_1234&SUBSYS_00000000&REV_01",
                "Intel UHD Graphics",
                "PCI\\VEN_8086&DEV_1234",
                "Display",
                GUID_DEVCLASS_DISPLAY,
                "Intel Corporation",
                "Intel UHD Graphics 630",
                "igfx",
                "PCI bus 0, device 2, function 0",
                CM_DEVCAP_SURPRISEREMOVALOK,
            ),
            (
                "PCI\\VEN_8086&DEV_A370&SUBSYS_00000000&REV_20",
                "Intel Ethernet Controller",
                "PCI\\VEN_8086&DEV_A370",
                "Net",
                GUID_DEVCLASS_NET,
                "Intel Corporation",
                "Intel I219-V",
                "e1d",
                "PCI bus 0, device 31, function 6",
                0,
            ),
            (
                "USB\\VID_046D&PID_C08B",
                "Logitech USB Mouse",
                "USB\\VID_046D&PID_C08B",
                "Mouse",
                GUID_DEVCLASS_MOUSE,
                "Logitech",
                "Logitech G502",
                "mouhid",
                "Port_#0001.Hub_#0001",
                CM_DEVCAP_REMOVABLE | CM_DEVCAP_SURPRISEREMOVALOK,
            ),
            (
                "USB\\VID_046D&PID_C336",
                "USB Keyboard",
                "USB\\VID_046D&PID_C336",
                "Keyboard",
                GUID_DEVCLASS_KEYBOARD,
                "Logitech",
                "Logitech G Pro Keyboard",
                "kbdhid",
                "Port_#0002.Hub_#0001",
                CM_DEVCAP_REMOVABLE | CM_DEVCAP_SURPRISEREMOVALOK,
            ),
            (
                "IDE\\DISKWDC_WD10EZEX",
                "WDC WD10EZEX SATA Disk",
                "IDE\\DISKWDC_WD10EZEX",
                "DiskDrive",
                GUID_DEVCLASS_DISKDRIVE,
                "Western Digital",
                "WDC WD10EZEX-00WN4A0",
                "disk",
                "Bus 0, Target 0, LUN 0",
                0,
            ),
            (
                "MONITOR\\SAM0E0C",
                "Samsung Monitor",
                "MONITOR\\SAM0E0C",
                "Monitor",
                GUID_DEVCLASS_MONITOR,
                "Samsung",
                "Samsung 27\" Monitor",
                "monitor",
                "DISPLAY1",
                0,
            ),
            (
                "BTHENUM\\{00001101-0000-1000-8000-00805F9B34FB}",
                "Bluetooth Device",
                "BTHENUM\\{00001101}",
                "Bluetooth",
                GUID_DEVCLASS_BLUETOOTH,
                "Microsoft",
                "Bluetooth Radio",
                "bthusb",
                "Bluetooth Module",
                CM_DEVCAP_REMOVABLE,
            ),
        ];

        for (inst_id, desc, hw_id, class, guid, mfg, friendly, svc, loc, caps) in devs {
            let id = self.next_dev_inst;
            self.next_dev_inst += 1;
            self.all_devices.push(EmulatedDevice {
                instance_id: String::from(inst_id),
                description: String::from(desc),
                hardware_id: String::from(hw_id),
                class: String::from(class),
                class_guid: guid,
                manufacturer: String::from(mfg),
                friendly_name: String::from(friendly),
                service: String::from(svc),
                location: String::from(loc),
                capabilities: caps,
                dev_inst: id,
                parent_inst: 0,
                children: Vec::new(),
                status: 0x0180_0000, // DN_DRIVER_LOADED | DN_STARTED
                problem: 0,
                enabled: true,
            });
        }
    }
}

static mut SETUP_API: Option<SetupApi> = None;

pub fn init() {
    unsafe {
        let mut api = SetupApi::new();
        api.populate_default_devices();
        SETUP_API = Some(api);
    }
}

fn api() -> &'static mut SetupApi {
    unsafe {
        SETUP_API
            .as_mut()
            .expect("setupapi not initialized — call init()")
    }
}

// =========================================================================
// Device information set operations
// =========================================================================

pub fn setup_di_create_device_info_list(
    class_guid: Option<&[u8; 16]>,
    _hwnd_parent: u64,
) -> WinHandle {
    let sa = api();
    let handle = sa.alloc_handle();
    let set = DevInfoSet {
        handle,
        class_guid: class_guid.copied(),
        flags: 0,
        devices: Vec::new(),
    };
    sa.dev_info_sets.insert(handle.0, set);
    handle
}

pub fn setup_di_destroy_device_info_list(dev_info: WinHandle) -> bool {
    api().dev_info_sets.remove(&dev_info.0).is_some()
}

pub fn setup_di_get_class_devs_w(
    class_guid: Option<&[u8; 16]>,
    _enumerator: Option<&str>,
    _hwnd_parent: u64,
    flags: u32,
) -> WinHandle {
    let sa = api();
    let handle = sa.alloc_handle();

    let mut matched: Vec<EmulatedDevice> = Vec::new();
    for dev in &sa.all_devices {
        if flags & DIGCF_PRESENT != 0 && !dev.enabled {
            continue;
        }
        if let Some(guid) = class_guid {
            if flags & DIGCF_ALLCLASSES == 0 && dev.class_guid != *guid {
                continue;
            }
        }
        matched.push(dev.clone());
    }

    let set = DevInfoSet {
        handle,
        class_guid: class_guid.copied(),
        flags,
        devices: matched,
    };
    sa.dev_info_sets.insert(handle.0, set);
    handle
}

pub fn setup_di_enum_device_info(
    dev_info: WinHandle,
    member_index: u32,
    data: &mut SpDevInfoData,
) -> bool {
    let sa = api();
    let set = match sa.dev_info_sets.get(&dev_info.0) {
        Some(s) => s,
        None => return false,
    };
    let idx = member_index as usize;
    if idx >= set.devices.len() {
        return false;
    }
    let dev = &set.devices[idx];
    data.class_guid = dev.class_guid;
    data.dev_inst = dev.dev_inst;
    true
}

pub fn setup_di_get_device_instance_id_w(
    dev_info: WinHandle,
    data: &SpDevInfoData,
    id_buf: &mut [u16],
    buf_size: u32,
    required_size: &mut u32,
) -> bool {
    let sa = api();
    let set = match sa.dev_info_sets.get(&dev_info.0) {
        Some(s) => s,
        None => return false,
    };
    let dev = match set.devices.iter().find(|d| d.dev_inst == data.dev_inst) {
        Some(d) => d,
        None => return false,
    };
    let wide = crate::string_to_wide(&dev.instance_id);
    *required_size = wide.len() as u32;
    if (buf_size as usize) < wide.len() || id_buf.len() < wide.len() {
        return false;
    }
    let n = wide.len().min(id_buf.len());
    id_buf[..n].copy_from_slice(&wide[..n]);
    true
}

// =========================================================================
// Device interface operations
// =========================================================================

pub fn setup_di_enum_device_interfaces(
    dev_info: WinHandle,
    _data: Option<&SpDevInfoData>,
    _interface_class: &[u8; 16],
    member_index: u32,
    iface: &mut SpDevInterfaceData,
) -> bool {
    let sa = api();
    let set = match sa.dev_info_sets.get(&dev_info.0) {
        Some(s) => s,
        None => return false,
    };
    let idx = member_index as usize;
    if idx >= set.devices.len() {
        return false;
    }
    iface.interface_class_guid = set.devices[idx].class_guid;
    iface.flags = 1;
    true
}

pub fn setup_di_get_device_interface_detail_w(
    dev_info: WinHandle,
    iface: &SpDevInterfaceData,
    detail: &mut SpDevInterfaceDetailData,
    _detail_size: u32,
    required_size: &mut u32,
    data: Option<&mut SpDevInfoData>,
) -> bool {
    let sa = api();
    let set = match sa.dev_info_sets.get(&dev_info.0) {
        Some(s) => s,
        None => return false,
    };
    let dev = match set
        .devices
        .iter()
        .find(|d| d.class_guid == iface.interface_class_guid)
    {
        Some(d) => d,
        None => return false,
    };
    let path = {
        let mut p = String::from("\\\\?\\");
        p.push_str(&dev.instance_id);
        p
    };
    *required_size = (path.len() as u32) + 8;
    detail.device_path = path;
    detail.size = *required_size;
    if let Some(d) = data {
        d.class_guid = dev.class_guid;
        d.dev_inst = dev.dev_inst;
    }
    true
}

pub fn setup_di_create_device_interface_reg_key(
    _dev_info: WinHandle,
    _iface: &SpDevInterfaceData,
    _sam_desired: u32,
) -> WinHandle {
    WinHandle(0xEEEE_0001)
}

pub fn setup_di_open_device_interface_reg_key(
    _dev_info: WinHandle,
    _iface: &SpDevInterfaceData,
    _sam_desired: u32,
) -> WinHandle {
    WinHandle(0xEEEE_0002)
}

// =========================================================================
// Device registry property operations
// =========================================================================

pub fn setup_di_get_device_registry_property_w(
    dev_info: WinHandle,
    data: &SpDevInfoData,
    property: u32,
    property_type: &mut u32,
    buf: &mut [u8],
    buf_size: u32,
    required_size: &mut u32,
) -> bool {
    let sa = api();
    let set = match sa.dev_info_sets.get(&dev_info.0) {
        Some(s) => s,
        None => return false,
    };
    let dev = match set.devices.iter().find(|d| d.dev_inst == data.dev_inst) {
        Some(d) => d,
        None => return false,
    };

    let value: String = match property {
        SPDRP_DEVICEDESC => dev.description.clone(),
        SPDRP_HARDWAREID => dev.hardware_id.clone(),
        SPDRP_COMPATIBLEIDS => dev.hardware_id.clone(),
        SPDRP_SERVICE => dev.service.clone(),
        SPDRP_CLASS => dev.class.clone(),
        SPDRP_CLASSGUID => {
            *property_type = 3; // REG_BINARY
            *required_size = 16;
            if buf_size < 16 || buf.len() < 16 {
                return false;
            }
            buf[..16].copy_from_slice(&dev.class_guid);
            return true;
        }
        SPDRP_DRIVER => dev.service.clone(),
        SPDRP_MFG => dev.manufacturer.clone(),
        SPDRP_FRIENDLYNAME => dev.friendly_name.clone(),
        SPDRP_LOCATION_INFORMATION => dev.location.clone(),
        SPDRP_PHYSICAL_DEVICE_OBJECT_NAME => {
            let mut pdo = String::from("\\Device\\");
            pdo.push_str(&dev.service);
            pdo
        }
        SPDRP_CAPABILITIES => {
            *property_type = 4; // REG_DWORD
            *required_size = 4;
            if buf_size < 4 || buf.len() < 4 {
                return false;
            }
            buf[..4].copy_from_slice(&dev.capabilities.to_le_bytes());
            return true;
        }
        SPDRP_UI_NUMBER | SPDRP_ADDRESS | SPDRP_BUSNUMBER => {
            *property_type = 4;
            *required_size = 4;
            if buf_size < 4 || buf.len() < 4 {
                return false;
            }
            buf[..4].copy_from_slice(&0u32.to_le_bytes());
            return true;
        }
        SPDRP_ENUMERATOR_NAME => {
            let end = dev.instance_id.find('\\').unwrap_or(dev.instance_id.len());
            String::from(&dev.instance_id[..end])
        }
        SPDRP_DEVTYPE
        | SPDRP_EXCLUSIVE
        | SPDRP_INSTALL_STATE
        | SPDRP_REMOVAL_POLICY
        | SPDRP_REMOVAL_POLICY_HW_DEFAULT
        | SPDRP_REMOVAL_POLICY_OVERRIDE => {
            *property_type = 4;
            *required_size = 4;
            if buf_size < 4 || buf.len() < 4 {
                return false;
            }
            buf[..4].copy_from_slice(&0u32.to_le_bytes());
            return true;
        }
        SPDRP_BASE_CONTAINERID => String::from("{00000000-0000-0000-0000-000000000000}"),
        _ => return false,
    };

    *property_type = 1; // REG_SZ
    let wide = crate::string_to_wide(&value);
    let byte_len = wide.len() * 2;
    *required_size = byte_len as u32;
    if (buf_size as usize) < byte_len || buf.len() < byte_len {
        return false;
    }
    for (i, &w) in wide.iter().enumerate() {
        let b = w.to_le_bytes();
        buf[i * 2] = b[0];
        buf[i * 2 + 1] = b[1];
    }
    true
}

pub fn setup_di_set_device_registry_property_w(
    dev_info: WinHandle,
    data: &SpDevInfoData,
    property: u32,
    _value: &[u8],
    _value_size: u32,
) -> bool {
    let sa = api();
    let set = match sa.dev_info_sets.get_mut(&dev_info.0) {
        Some(s) => s,
        None => return false,
    };
    set.devices.iter().any(|d| d.dev_inst == data.dev_inst) && property <= SPDRP_BASE_CONTAINERID
}

// =========================================================================
// Driver enumeration
// =========================================================================

pub fn setup_di_enum_driver_info_w(
    dev_info: WinHandle,
    _data: Option<&SpDevInfoData>,
    driver_type: u32,
    member_index: u32,
    info: &mut SpDriverInfoData,
) -> bool {
    let sa = api();
    let set = match sa.dev_info_sets.get(&dev_info.0) {
        Some(s) => s,
        None => return false,
    };
    if member_index > 0 || set.devices.is_empty() {
        return false;
    }
    let dev = &set.devices[0];
    info.size = 80;
    info.driver_type = driver_type;
    info.description = dev.description.clone();
    info.mfg_name = dev.manufacturer.clone();
    info.provider_name = String::from("Microsoft");
    info.driver_date = 0;
    info.driver_version = 0x000A_0000_0000;
    true
}

pub fn setup_di_get_driver_info_detail_w(
    _dev_info: WinHandle,
    _data: Option<&SpDevInfoData>,
    _driver: &SpDriverInfoData,
    detail: &mut SpDriverInfoDetailData,
    _buf_size: u32,
    required_size: &mut u32,
) -> bool {
    detail.size = 100;
    detail.inf_file_name = String::from("C:\\Windows\\INF\\oem0.inf");
    detail.section_name = String::from("[Manufacturer]");
    detail.drv_description = String::from("Default Driver");
    detail.hardware_id = String::from("PCI\\VEN_0000&DEV_0000");
    *required_size = 200;
    true
}

pub fn setup_di_get_selected_driver_w(
    dev_info: WinHandle,
    data: Option<&SpDevInfoData>,
    info: &mut SpDriverInfoData,
) -> bool {
    setup_di_enum_driver_info_w(dev_info, data, 1, 0, info)
}

pub fn setup_di_set_selected_driver_w(
    _dev_info: WinHandle,
    _data: Option<&mut SpDevInfoData>,
    _info: Option<&SpDriverInfoData>,
) -> bool {
    true
}

// =========================================================================
// INF file operations
// =========================================================================

pub fn setup_open_inf_file_w(
    file_name: &str,
    _inf_class: Option<&str>,
    _inf_style: u32,
    _error_line: &mut u32,
) -> WinHandle {
    if file_name.is_empty() {
        return INVALID_HANDLE_VALUE;
    }
    let sa = api();
    let handle = sa.alloc_handle();
    let mut sections = BTreeMap::new();
    let mut version_lines = Vec::new();
    version_lines.push({
        let mut v = Vec::new();
        v.push(String::from("Signature"));
        v.push(String::from("\"$WINDOWS NT$\""));
        v
    });
    sections.insert(String::from("Version"), version_lines);
    sa.inf_files.insert(handle.0, InfFile { handle, sections });
    handle
}

pub fn setup_close_inf_file(inf_handle: WinHandle) {
    api().inf_files.remove(&inf_handle.0);
}

pub fn setup_find_first_line_w(
    inf_handle: WinHandle,
    section: &str,
    _key: Option<&str>,
    context: &mut InfContext,
) -> bool {
    let sa = api();
    let inf = match sa.inf_files.get(&inf_handle.0) {
        Some(f) => f,
        None => return false,
    };
    if !inf.sections.contains_key(section) {
        return false;
    }
    context.inf_handle = inf_handle;
    context.section = String::from(section);
    context.line = 0;
    true
}

pub fn setup_find_next_line(context: &mut InfContext) -> bool {
    let sa = api();
    let inf = match sa.inf_files.get(&context.inf_handle.0) {
        Some(f) => f,
        None => return false,
    };
    let lines = match inf.sections.get(&context.section) {
        Some(l) => l,
        None => return false,
    };
    context.line += 1;
    (context.line as usize) < lines.len()
}

pub fn setup_get_string_field_w(
    context: &InfContext,
    field_index: u32,
    buf: &mut [u16],
    buf_size: u32,
    required_size: &mut u32,
) -> bool {
    let sa = api();
    let inf = match sa.inf_files.get(&context.inf_handle.0) {
        Some(f) => f,
        None => return false,
    };
    let lines = match inf.sections.get(&context.section) {
        Some(l) => l,
        None => return false,
    };
    let line_idx = context.line as usize;
    if line_idx >= lines.len() {
        return false;
    }
    let fields = &lines[line_idx];
    let fi = field_index as usize;
    if fi >= fields.len() {
        return false;
    }
    let wide = crate::string_to_wide(&fields[fi]);
    *required_size = wide.len() as u32;
    if (buf_size as usize) < wide.len() || buf.len() < wide.len() {
        return false;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    true
}

pub fn setup_get_int_field(context: &InfContext, field_index: u32, value: &mut i32) -> bool {
    let sa = api();
    let inf = match sa.inf_files.get(&context.inf_handle.0) {
        Some(f) => f,
        None => return false,
    };
    let lines = match inf.sections.get(&context.section) {
        Some(l) => l,
        None => return false,
    };
    let line_idx = context.line as usize;
    if line_idx >= lines.len() {
        return false;
    }
    let fields = &lines[line_idx];
    let fi = field_index as usize;
    if fi >= fields.len() {
        return false;
    }
    *value = 0;
    true
}

pub fn setup_get_line_text_w(
    context: &InfContext,
    _inf_handle: WinHandle,
    _section: Option<&str>,
    _key: Option<&str>,
    buf: &mut [u16],
    buf_size: u32,
    required_size: &mut u32,
) -> bool {
    let sa = api();
    let inf = match sa.inf_files.get(&context.inf_handle.0) {
        Some(f) => f,
        None => return false,
    };
    let lines = match inf.sections.get(&context.section) {
        Some(l) => l,
        None => return false,
    };
    let line_idx = context.line as usize;
    if line_idx >= lines.len() {
        return false;
    }
    let mut text = String::new();
    for (i, f) in lines[line_idx].iter().enumerate() {
        if i > 0 {
            text.push(',');
        }
        text.push_str(f);
    }
    let wide = crate::string_to_wide(&text);
    *required_size = wide.len() as u32;
    if (buf_size as usize) < wide.len() || buf.len() < wide.len() {
        return false;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    true
}

pub fn setup_get_line_count_w(inf_handle: WinHandle, section: &str) -> i32 {
    let sa = api();
    let inf = match sa.inf_files.get(&inf_handle.0) {
        Some(f) => f,
        None => return -1,
    };
    match inf.sections.get(section) {
        Some(lines) => lines.len() as i32,
        None => -1,
    }
}

// =========================================================================
// Device installation
// =========================================================================

pub fn setup_di_call_class_installer(
    install_function: u32,
    dev_info: WinHandle,
    data: &SpDevInfoData,
) -> bool {
    let sa = api();
    let set = match sa.dev_info_sets.get_mut(&dev_info.0) {
        Some(s) => s,
        None => return false,
    };
    let dev = match set.devices.iter_mut().find(|d| d.dev_inst == data.dev_inst) {
        Some(d) => d,
        None => return false,
    };
    match install_function {
        DIF_INSTALLDEVICE => {
            dev.status |= 0x0080_0000; // DN_STARTED
            true
        }
        DIF_REMOVE => {
            dev.enabled = false;
            dev.status = 0;
            true
        }
        DIF_SELECTBESTCOMPATDRV => true,
        DIF_PROPERTYCHANGE => true,
        _ => false,
    }
}

// =========================================================================
// Configuration Manager — devnode operations
// =========================================================================

pub fn cm_locate_dev_node_w(dev_inst: &mut u32, device_id: Option<&str>, _flags: u32) -> u32 {
    let sa = api();
    match device_id {
        Some(id) => {
            for dev in &sa.all_devices {
                if dev.instance_id == id {
                    *dev_inst = dev.dev_inst;
                    return CR_SUCCESS;
                }
            }
            CR_NO_SUCH_DEVNODE
        }
        None => {
            *dev_inst = 0;
            CR_SUCCESS
        }
    }
}

pub fn cm_get_dev_node_status(
    status: &mut u32,
    problem_number: &mut u32,
    dev_inst: u32,
    _flags: u32,
) -> u32 {
    let sa = api();
    for dev in &sa.all_devices {
        if dev.dev_inst == dev_inst {
            *status = dev.status;
            *problem_number = dev.problem;
            return CR_SUCCESS;
        }
    }
    CR_NO_SUCH_DEVNODE
}

pub fn cm_get_dev_node_property_w(
    dev_inst: u32,
    _property_key: &[u8; 20],
    property_type: &mut u32,
    buf: &mut [u8],
    buf_size: &mut u32,
    _flags: u32,
) -> u32 {
    let sa = api();
    for dev in &sa.all_devices {
        if dev.dev_inst == dev_inst {
            let wide = crate::string_to_wide(&dev.description);
            let byte_len = wide.len() * 2;
            *property_type = 1;
            if (*buf_size as usize) < byte_len || buf.len() < byte_len {
                *buf_size = byte_len as u32;
                return CR_BUFFER_SMALL;
            }
            for (i, &w) in wide.iter().enumerate() {
                let b = w.to_le_bytes();
                buf[i * 2] = b[0];
                buf[i * 2 + 1] = b[1];
            }
            *buf_size = byte_len as u32;
            return CR_SUCCESS;
        }
    }
    CR_NO_SUCH_DEVNODE
}

pub fn cm_get_child(child: &mut u32, dev_inst: u32, _flags: u32) -> u32 {
    let sa = api();
    for dev in &sa.all_devices {
        if dev.dev_inst == dev_inst {
            if let Some(&c) = dev.children.first() {
                *child = c;
                return CR_SUCCESS;
            }
            return CR_NO_SUCH_DEVNODE;
        }
    }
    CR_NO_SUCH_DEVNODE
}

pub fn cm_get_sibling(sibling: &mut u32, dev_inst: u32, _flags: u32) -> u32 {
    let sa = api();
    for (i, dev) in sa.all_devices.iter().enumerate() {
        if dev.dev_inst == dev_inst {
            if i + 1 < sa.all_devices.len() && sa.all_devices[i + 1].parent_inst == dev.parent_inst
            {
                *sibling = sa.all_devices[i + 1].dev_inst;
                return CR_SUCCESS;
            }
            return CR_NO_SUCH_DEVNODE;
        }
    }
    CR_NO_SUCH_DEVNODE
}

pub fn cm_get_parent(parent: &mut u32, dev_inst: u32, _flags: u32) -> u32 {
    let sa = api();
    for dev in &sa.all_devices {
        if dev.dev_inst == dev_inst {
            *parent = dev.parent_inst;
            return CR_SUCCESS;
        }
    }
    CR_NO_SUCH_DEVNODE
}

pub fn cm_enable_dev_node(dev_inst: u32, _flags: u32) -> u32 {
    let sa = api();
    for dev in &mut sa.all_devices {
        if dev.dev_inst == dev_inst {
            dev.enabled = true;
            dev.status |= 0x0080_0000;
            return CR_SUCCESS;
        }
    }
    CR_NO_SUCH_DEVNODE
}

pub fn cm_disable_dev_node(dev_inst: u32, _flags: u32) -> u32 {
    let sa = api();
    for dev in &mut sa.all_devices {
        if dev.dev_inst == dev_inst {
            dev.enabled = false;
            dev.status &= !0x0080_0000;
            return CR_SUCCESS;
        }
    }
    CR_NO_SUCH_DEVNODE
}

pub fn cm_reenumerate_dev_node(dev_inst: u32, _flags: u32) -> u32 {
    let sa = api();
    for dev in &sa.all_devices {
        if dev.dev_inst == dev_inst {
            return CR_SUCCESS;
        }
    }
    CR_NO_SUCH_DEVNODE
}
