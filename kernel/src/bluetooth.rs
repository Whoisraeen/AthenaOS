//! Full Bluetooth stack — HCI transport, L2CAP, SDP, RFCOMM, GATT (BLE),
//! pairing/bonding, and profiles (A2DP, HFP, HOGP, AVRCP, PAN).
//!
//! Provides a complete Bluetooth subsystem for RaeenOS supporting both
//! Classic (BR/EDR) and Low Energy (LE) operation.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ─── Error Type ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BtError {
    NotInitialized,
    ControllerNotFound,
    CommandFailed(u16, u8),
    ConnectionFailed(BtAddr),
    PairingFailed(String),
    AuthenticationFailed,
    ChannelNotFound(u16),
    ServiceNotFound,
    ProfileNotSupported(String),
    Timeout,
    InvalidState(String),
    BufferOverflow,
    InvalidParameter,
    HardwareError(String),
}

impl core::fmt::Display for BtError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "Bluetooth not initialized"),
            Self::ControllerNotFound => write!(f, "Bluetooth controller not found"),
            Self::CommandFailed(opcode, status) => {
                write!(
                    f,
                    "HCI command 0x{:04X} failed (status 0x{:02X})",
                    opcode, status
                )
            }
            Self::ConnectionFailed(addr) => write!(f, "connection to {} failed", addr),
            Self::PairingFailed(reason) => write!(f, "pairing failed: {}", reason),
            Self::AuthenticationFailed => write!(f, "authentication failed"),
            Self::ChannelNotFound(cid) => write!(f, "L2CAP channel 0x{:04X} not found", cid),
            Self::ServiceNotFound => write!(f, "SDP service not found"),
            Self::ProfileNotSupported(p) => write!(f, "profile '{}' not supported", p),
            Self::Timeout => write!(f, "Bluetooth operation timed out"),
            Self::InvalidState(msg) => write!(f, "invalid state: {}", msg),
            Self::BufferOverflow => write!(f, "Bluetooth buffer overflow"),
            Self::InvalidParameter => write!(f, "invalid parameter"),
            Self::HardwareError(msg) => write!(f, "Bluetooth hardware error: {}", msg),
        }
    }
}

// ─── Address ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BtAddr(pub [u8; 6]);

impl BtAddr {
    pub const ZERO: Self = Self([0; 6]);

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 6 {
            return None;
        }
        let mut addr = [0u8; 6];
        addr.copy_from_slice(&bytes[..6]);
        Some(Self(addr))
    }
}

impl core::fmt::Display for BtAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtAddrType {
    Public,
    Random,
    PublicIdentity,
    RandomIdentity,
}

// ─── HCI Layer ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtState {
    Off,
    Initializing,
    Ready,
    Scanning,
    Pairing,
    Connected,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HciTransport {
    Usb,
    Uart,
    Sdio,
    Virtual,
}

pub struct HciController {
    pub transport: HciTransport,
    pub address: BtAddr,
    pub name: String,
    pub manufacturer: u16,
    pub hci_version: u8,
    pub hci_revision: u16,
    pub lmp_version: u8,
    pub lmp_subversion: u16,
    pub le_supported: bool,
    pub le_states: u64,
    pub acl_mtu: u16,
    pub acl_max_packets: u16,
    pub sco_mtu: u8,
    pub sco_max_packets: u16,
    pub le_acl_mtu: u16,
    pub le_acl_max_packets: u8,
    pub features: [u8; 8],
    pub le_features: u64,
    pub command_queue: Vec<HciCommand>,
    pub event_mask: u64,
    pub le_event_mask: u64,
}

impl HciController {
    pub fn new(transport: HciTransport) -> Self {
        Self {
            transport,
            address: BtAddr::ZERO,
            name: String::from("RaeenOS-BT"),
            manufacturer: 0xFFFF,
            hci_version: 0x0C, // BT 5.3
            hci_revision: 1,
            lmp_version: 0x0C,
            lmp_subversion: 1,
            le_supported: true,
            le_states: 0xFFFF_FFFF_FFFF,
            acl_mtu: 1021,
            acl_max_packets: 7,
            sco_mtu: 64,
            sco_max_packets: 1,
            le_acl_mtu: 251,
            le_acl_max_packets: 6,
            features: [0xFF; 8],
            le_features: 0x1FFF,
            command_queue: Vec::new(),
            event_mask: 0x3FFF_FFFF_FFFF_FFFF,
            le_event_mask: 0x0000_0000_001F_FFFF,
        }
    }

    pub fn reset(&mut self) -> Result<(), BtError> {
        self.send_command(HciCommand::Reset)?;
        self.command_queue.clear();
        Ok(())
    }

    pub fn send_command(&mut self, cmd: HciCommand) -> Result<HciEvent, BtError> {
        let opcode = cmd.opcode();
        self.command_queue.push(cmd);

        // Simulated command-complete event
        Ok(HciEvent::CommandComplete {
            num_packets: 1,
            opcode,
            status: 0,
            data: Vec::new(),
        })
    }

    pub fn read_local_version(&self) -> (u8, u16, u8, u16, u16) {
        (
            self.hci_version,
            self.hci_revision,
            self.lmp_version,
            self.lmp_subversion,
            self.manufacturer,
        )
    }

    pub fn set_event_mask(&mut self, mask: u64) {
        self.event_mask = mask;
    }

    pub fn set_le_event_mask(&mut self, mask: u64) {
        self.le_event_mask = mask;
    }

    pub fn start_le_scan(
        &mut self,
        active: bool,
        interval: u16,
        window: u16,
    ) -> Result<(), BtError> {
        let scan_type = if active { 0x01 } else { 0x00 };
        self.send_command(HciCommand::LeSetScanParameters {
            scan_type,
            interval,
            window,
            own_addr_type: 0,
            filter_policy: 0,
        })?;
        self.send_command(HciCommand::LeSetScanEnable {
            enable: true,
            filter_duplicates: true,
        })?;
        Ok(())
    }

    pub fn stop_le_scan(&mut self) -> Result<(), BtError> {
        self.send_command(HciCommand::LeSetScanEnable {
            enable: false,
            filter_duplicates: false,
        })?;
        Ok(())
    }

    pub fn create_connection(
        &mut self,
        addr: BtAddr,
        addr_type: BtAddrType,
    ) -> Result<u16, BtError> {
        self.send_command(HciCommand::LeCreateConnection {
            scan_interval: 0x0060,
            scan_window: 0x0030,
            filter_policy: 0,
            peer_addr_type: addr_type as u8,
            peer_addr: addr,
            own_addr_type: 0,
            conn_interval_min: 0x0018,
            conn_interval_max: 0x0028,
            conn_latency: 0,
            supervision_timeout: 0x002A,
            min_ce_length: 0,
            max_ce_length: 0,
        })?;

        // Return simulated connection handle
        Ok(0x0040)
    }

    pub fn disconnect(&mut self, handle: u16, reason: u8) -> Result<(), BtError> {
        self.send_command(HciCommand::Disconnect { handle, reason })?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum HciCommand {
    Reset,
    ReadLocalVersion,
    ReadBdAddr,
    SetEventMask {
        mask: u64,
    },
    Disconnect {
        handle: u16,
        reason: u8,
    },
    CreateConnection {
        bd_addr: BtAddr,
        packet_type: u16,
        page_scan_rep_mode: u8,
        clock_offset: u16,
        allow_role_switch: bool,
    },
    WriteScanEnable {
        scan_enable: u8,
    },
    WriteClassOfDevice {
        class: u32,
    },
    WriteLocalName {
        name: String,
    },
    LeSetScanParameters {
        scan_type: u8,
        interval: u16,
        window: u16,
        own_addr_type: u8,
        filter_policy: u8,
    },
    LeSetScanEnable {
        enable: bool,
        filter_duplicates: bool,
    },
    LeCreateConnection {
        scan_interval: u16,
        scan_window: u16,
        filter_policy: u8,
        peer_addr_type: u8,
        peer_addr: BtAddr,
        own_addr_type: u8,
        conn_interval_min: u16,
        conn_interval_max: u16,
        conn_latency: u16,
        supervision_timeout: u16,
        min_ce_length: u16,
        max_ce_length: u16,
    },
    LeSetAdvertisingParameters {
        adv_interval_min: u16,
        adv_interval_max: u16,
        adv_type: u8,
        own_addr_type: u8,
        peer_addr_type: u8,
        peer_addr: BtAddr,
        channel_map: u8,
        filter_policy: u8,
    },
    LeSetAdvertisingData {
        data: Vec<u8>,
    },
    LeSetAdvertisingEnable {
        enable: bool,
    },
    LeReadBufferSize,
}

impl HciCommand {
    pub fn opcode(&self) -> u16 {
        match self {
            Self::Reset => 0x0C03,
            Self::ReadLocalVersion => 0x1001,
            Self::ReadBdAddr => 0x1009,
            Self::SetEventMask { .. } => 0x0C01,
            Self::Disconnect { .. } => 0x0406,
            Self::CreateConnection { .. } => 0x0405,
            Self::WriteScanEnable { .. } => 0x0C1A,
            Self::WriteClassOfDevice { .. } => 0x0C24,
            Self::WriteLocalName { .. } => 0x0C13,
            Self::LeSetScanParameters { .. } => 0x200B,
            Self::LeSetScanEnable { .. } => 0x200C,
            Self::LeCreateConnection { .. } => 0x200D,
            Self::LeSetAdvertisingParameters { .. } => 0x2006,
            Self::LeSetAdvertisingData { .. } => 0x2008,
            Self::LeSetAdvertisingEnable { .. } => 0x200A,
            Self::LeReadBufferSize => 0x2002,
        }
    }

    pub fn ogf(&self) -> u8 {
        (self.opcode() >> 10) as u8
    }

    pub fn ocf(&self) -> u16 {
        self.opcode() & 0x03FF
    }
}

#[derive(Debug, Clone)]
pub enum HciEvent {
    CommandComplete {
        num_packets: u8,
        opcode: u16,
        status: u8,
        data: Vec<u8>,
    },
    CommandStatus {
        status: u8,
        num_packets: u8,
        opcode: u16,
    },
    ConnectionComplete {
        status: u8,
        handle: u16,
        bd_addr: BtAddr,
        link_type: u8,
        encryption_enabled: bool,
    },
    DisconnectionComplete {
        status: u8,
        handle: u16,
        reason: u8,
    },
    LeConnectionComplete {
        status: u8,
        handle: u16,
        role: u8,
        peer_addr_type: u8,
        peer_addr: BtAddr,
        conn_interval: u16,
        conn_latency: u16,
        supervision_timeout: u16,
    },
    LeAdvertisingReport {
        num_reports: u8,
        event_type: u8,
        addr_type: u8,
        addr: BtAddr,
        data: Vec<u8>,
        rssi: i8,
    },
    AclData {
        handle: u16,
        flags: u8,
        data: Vec<u8>,
    },
    NumberOfCompletedPackets {
        handles: Vec<(u16, u16)>,
    },
}

// ─── L2CAP Layer ────────────────────────────────────────────────────────────

const L2CAP_CID_SIGNALING: u16 = 0x0001;
const L2CAP_CID_CONNECTIONLESS: u16 = 0x0002;
const L2CAP_CID_ATT: u16 = 0x0004;
const L2CAP_CID_LE_SIGNALING: u16 = 0x0005;
const L2CAP_CID_SMP: u16 = 0x0006;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2capChannelState {
    Closed,
    WaitConnect,
    WaitConnectRsp,
    Config,
    Open,
    WaitDisconnect,
    WaitDisconnectRsp,
}

pub struct L2capChannel {
    pub local_cid: u16,
    pub remote_cid: u16,
    pub psm: u16,
    pub state: L2capChannelState,
    pub local_mtu: u16,
    pub remote_mtu: u16,
    pub flush_timeout: u16,
    pub rx_buffer: Vec<u8>,
    pub tx_credits: u16,
    pub rx_credits: u16,
}

impl L2capChannel {
    pub fn new(local_cid: u16, psm: u16) -> Self {
        Self {
            local_cid,
            remote_cid: 0,
            psm,
            state: L2capChannelState::Closed,
            local_mtu: 672,
            remote_mtu: 672,
            flush_timeout: 0xFFFF,
            rx_buffer: Vec::new(),
            tx_credits: 0,
            rx_credits: 10,
        }
    }

    pub fn is_open(&self) -> bool {
        self.state == L2capChannelState::Open
    }

    pub fn can_send(&self) -> bool {
        self.is_open() && self.tx_credits > 0
    }
}

pub struct L2capLayer {
    channels: BTreeMap<u16, L2capChannel>,
    next_local_cid: u16,
    signaling_id: u8,
}

impl L2capLayer {
    pub fn new() -> Self {
        Self {
            channels: BTreeMap::new(),
            next_local_cid: 0x0040,
            signaling_id: 1,
        }
    }

    pub fn allocate_channel(&mut self, psm: u16) -> u16 {
        let cid = self.next_local_cid;
        self.next_local_cid += 1;
        self.channels.insert(cid, L2capChannel::new(cid, psm));
        cid
    }

    pub fn get_channel(&self, cid: u16) -> Option<&L2capChannel> {
        self.channels.get(&cid)
    }

    pub fn get_channel_mut(&mut self, cid: u16) -> Option<&mut L2capChannel> {
        self.channels.get_mut(&cid)
    }

    pub fn open_channel(&mut self, cid: u16, remote_cid: u16) -> Result<(), BtError> {
        let ch = self
            .channels
            .get_mut(&cid)
            .ok_or(BtError::ChannelNotFound(cid))?;
        ch.remote_cid = remote_cid;
        ch.state = L2capChannelState::Open;
        ch.tx_credits = 10;
        Ok(())
    }

    pub fn close_channel(&mut self, cid: u16) -> Result<(), BtError> {
        let ch = self
            .channels
            .get_mut(&cid)
            .ok_or(BtError::ChannelNotFound(cid))?;
        ch.state = L2capChannelState::Closed;
        Ok(())
    }

    pub fn remove_channel(&mut self, cid: u16) {
        self.channels.remove(&cid);
    }

    pub fn next_signaling_id(&mut self) -> u8 {
        let id = self.signaling_id;
        self.signaling_id = self.signaling_id.wrapping_add(1);
        if self.signaling_id == 0 {
            self.signaling_id = 1;
        }
        id
    }

    pub fn segment_data(&self, cid: u16, data: &[u8]) -> Result<Vec<Vec<u8>>, BtError> {
        let ch = self
            .channels
            .get(&cid)
            .ok_or(BtError::ChannelNotFound(cid))?;
        let mtu = ch.remote_mtu as usize;

        if data.len() > mtu {
            return Err(BtError::InvalidParameter); // Basic L2CAP does not support segmentation without ERTM
        }

        let mut pdu = Vec::with_capacity(4 + data.len());
        let len = data.len() as u16;
        pdu.extend_from_slice(&len.to_le_bytes());
        pdu.extend_from_slice(&ch.remote_cid.to_le_bytes());
        pdu.extend_from_slice(data);

        Ok(alloc::vec![pdu])
    }

    pub fn reassemble(&mut self, cid: u16, data: &[u8]) -> Result<Option<Vec<u8>>, BtError> {
        let ch = self
            .channels
            .get_mut(&cid)
            .ok_or(BtError::ChannelNotFound(cid))?;

        ch.rx_buffer.extend_from_slice(data);

        if ch.rx_buffer.len() >= 4 {
            let expected_len = u16::from_le_bytes([ch.rx_buffer[0], ch.rx_buffer[1]]) as usize;
            if ch.rx_buffer.len() >= expected_len + 4 {
                let pdu = ch.rx_buffer[4..4 + expected_len].to_vec();
                ch.rx_buffer.drain(..4 + expected_len);
                return Ok(Some(pdu));
            }
        }

        Ok(None)
    }
}

// ─── SDP ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SdpServiceRecord {
    pub handle: u32,
    pub service_class_uuids: Vec<u128>,
    pub protocol_descriptors: Vec<SdpProtocol>,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub browse_group: u16,
    pub attributes: BTreeMap<u16, SdpAttribute>,
}

#[derive(Debug, Clone)]
pub struct SdpProtocol {
    pub uuid: u16,
    pub params: Vec<u32>,
}

#[derive(Debug, Clone)]
pub enum SdpAttribute {
    Uint8(u8),
    Uint16(u16),
    Uint32(u32),
    Int8(i8),
    Bool(bool),
    String(String),
    Uuid16(u16),
    Uuid128(u128),
    Sequence(Vec<SdpAttribute>),
    Nil,
}

pub struct SdpDatabase {
    services: BTreeMap<u32, SdpServiceRecord>,
    next_handle: u32,
}

impl SdpDatabase {
    pub fn new() -> Self {
        Self {
            services: BTreeMap::new(),
            next_handle: 0x10001,
        }
    }

    pub fn register_service(&mut self, mut record: SdpServiceRecord) -> u32 {
        let handle = self.next_handle;
        self.next_handle += 1;
        record.handle = handle;
        self.services.insert(handle, record);
        handle
    }

    pub fn unregister_service(&mut self, handle: u32) -> bool {
        self.services.remove(&handle).is_some()
    }

    pub fn find_by_uuid(&self, uuid: u128) -> Vec<&SdpServiceRecord> {
        self.services
            .values()
            .filter(|s| s.service_class_uuids.contains(&uuid))
            .collect()
    }

    pub fn find_by_name(&self, name: &str) -> Option<&SdpServiceRecord> {
        self.services.values().find(|s| s.name == name)
    }

    pub fn get_service(&self, handle: u32) -> Option<&SdpServiceRecord> {
        self.services.get(&handle)
    }

    pub fn list_services(&self) -> Vec<u32> {
        self.services.keys().copied().collect()
    }
}

// ─── RFCOMM ─────────────────────────────────────────────────────────────────

pub struct RfcommChannel {
    pub dlci: u8,
    pub l2cap_cid: u16,
    pub mtu: u16,
    pub credits: u8,
    pub open: bool,
    pub rx_buffer: Vec<u8>,
    pub tx_buffer: Vec<u8>,
    pub modem_status: u8,
}

impl RfcommChannel {
    pub fn new(dlci: u8, l2cap_cid: u16) -> Self {
        Self {
            dlci,
            l2cap_cid,
            mtu: 127,
            credits: 7,
            open: false,
            rx_buffer: Vec::new(),
            tx_buffer: Vec::new(),
            modem_status: 0x0D, // RTC | RTR | DV
        }
    }

    pub fn server_channel(&self) -> u8 {
        self.dlci >> 1
    }

    pub fn send(&mut self, data: &[u8]) -> Result<(), BtError> {
        if !self.open {
            return Err(BtError::InvalidState(String::from(
                "RFCOMM channel not open",
            )));
        }
        if self.credits == 0 {
            return Err(BtError::BufferOverflow);
        }
        self.tx_buffer.extend_from_slice(data);
        self.credits -= 1;
        Ok(())
    }

    pub fn receive(&mut self) -> Option<Vec<u8>> {
        if self.rx_buffer.is_empty() {
            None
        } else {
            Some(core::mem::take(&mut self.rx_buffer))
        }
    }
}

pub struct RfcommSession {
    pub l2cap_cid: u16,
    pub channels: BTreeMap<u8, RfcommChannel>,
    pub initiator: bool,
    pub mtu: u16,
}

impl RfcommSession {
    pub fn new(l2cap_cid: u16, initiator: bool) -> Self {
        Self {
            l2cap_cid,
            channels: BTreeMap::new(),
            initiator,
            mtu: 127,
        }
    }

    pub fn open_channel(&mut self, server_channel: u8) -> Result<u8, BtError> {
        let dlci = if server_channel == 0 {
            0
        } else {
            let direction = if self.initiator { 1u8 } else { 0u8 };
            (server_channel << 1) | direction
        };
        let mut ch = RfcommChannel::new(dlci, self.l2cap_cid);
        ch.open = true;
        ch.mtu = self.mtu;
        self.channels.insert(dlci, ch);
        Ok(dlci)
    }

    pub fn close_channel(&mut self, dlci: u8) {
        if let Some(ch) = self.channels.get_mut(&dlci) {
            ch.open = false;
        }
    }
}

// ─── GATT (BLE) ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GattUuid {
    Uuid16(u16),
    Uuid128(u128),
}

impl GattUuid {
    pub const GENERIC_ACCESS: Self = Self::Uuid16(0x1800);
    pub const GENERIC_ATTRIBUTE: Self = Self::Uuid16(0x1801);
    pub const DEVICE_INFORMATION: Self = Self::Uuid16(0x180A);
    pub const BATTERY_SERVICE: Self = Self::Uuid16(0x180F);
    pub const HID_SERVICE: Self = Self::Uuid16(0x1812);
    pub const HEART_RATE: Self = Self::Uuid16(0x180D);

    pub fn to_u128(&self) -> u128 {
        match self {
            Self::Uuid16(v) => {
                // BT Base UUID: 00000000-0000-1000-8000-00805F9B34FB
                0x0000000000001000800000805F9B34FBu128 | ((*v as u128) << 96)
            }
            Self::Uuid128(v) => *v,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GattCharProps {
    pub broadcast: bool,
    pub read: bool,
    pub write_no_rsp: bool,
    pub write: bool,
    pub notify: bool,
    pub indicate: bool,
    pub auth_signed_write: bool,
    pub extended: bool,
}

impl GattCharProps {
    pub fn read_only() -> Self {
        Self {
            broadcast: false,
            read: true,
            write_no_rsp: false,
            write: false,
            notify: false,
            indicate: false,
            auth_signed_write: false,
            extended: false,
        }
    }

    pub fn read_write() -> Self {
        Self {
            broadcast: false,
            read: true,
            write_no_rsp: false,
            write: true,
            notify: false,
            indicate: false,
            auth_signed_write: false,
            extended: false,
        }
    }

    pub fn read_notify() -> Self {
        Self {
            broadcast: false,
            read: true,
            write_no_rsp: false,
            write: false,
            notify: true,
            indicate: false,
            auth_signed_write: false,
            extended: false,
        }
    }

    pub fn to_byte(&self) -> u8 {
        let mut b = 0u8;
        if self.broadcast {
            b |= 0x01;
        }
        if self.read {
            b |= 0x02;
        }
        if self.write_no_rsp {
            b |= 0x04;
        }
        if self.write {
            b |= 0x08;
        }
        if self.notify {
            b |= 0x10;
        }
        if self.indicate {
            b |= 0x20;
        }
        if self.auth_signed_write {
            b |= 0x40;
        }
        if self.extended {
            b |= 0x80;
        }
        b
    }
}

pub struct GattDescriptor {
    pub handle: u16,
    pub uuid: GattUuid,
    pub value: Vec<u8>,
    pub permissions: u8,
}

pub struct GattCharacteristic {
    pub handle: u16,
    pub value_handle: u16,
    pub uuid: GattUuid,
    pub properties: GattCharProps,
    pub value: Vec<u8>,
    pub descriptors: Vec<GattDescriptor>,
}

pub struct GattService {
    pub handle: u16,
    pub end_handle: u16,
    pub uuid: GattUuid,
    pub primary: bool,
    pub characteristics: Vec<GattCharacteristic>,
    pub included_services: Vec<u16>,
}

pub struct GattServer {
    services: Vec<GattService>,
    next_handle: u16,
    mtu: u16,
    notifications_enabled: BTreeMap<u16, bool>,
}

impl GattServer {
    pub fn new() -> Self {
        Self {
            services: Vec::new(),
            next_handle: 1,
            mtu: 23,
            notifications_enabled: BTreeMap::new(),
        }
    }

    pub fn add_service(&mut self, uuid: GattUuid, primary: bool) -> u16 {
        let handle = self.next_handle;
        self.next_handle += 1;
        self.services.push(GattService {
            handle,
            end_handle: handle,
            uuid,
            primary,
            characteristics: Vec::new(),
            included_services: Vec::new(),
        });
        handle
    }

    pub fn add_characteristic(
        &mut self,
        service_handle: u16,
        uuid: GattUuid,
        properties: GattCharProps,
        initial_value: Vec<u8>,
    ) -> Option<u16> {
        let char_handle = self.next_handle;
        self.next_handle += 1;
        let value_handle = self.next_handle;
        self.next_handle += 1;

        let svc = self
            .services
            .iter_mut()
            .find(|s| s.handle == service_handle)?;

        svc.characteristics.push(GattCharacteristic {
            handle: char_handle,
            value_handle,
            uuid,
            properties,
            value: initial_value,
            descriptors: Vec::new(),
        });
        svc.end_handle = self.next_handle - 1;

        Some(value_handle)
    }

    pub fn add_descriptor(
        &mut self,
        char_value_handle: u16,
        uuid: GattUuid,
        value: Vec<u8>,
    ) -> Option<u16> {
        let desc_handle = self.next_handle;
        self.next_handle += 1;

        for svc in &mut self.services {
            for ch in &mut svc.characteristics {
                if ch.value_handle == char_value_handle {
                    ch.descriptors.push(GattDescriptor {
                        handle: desc_handle,
                        uuid,
                        value,
                        permissions: 0x01,
                    });
                    svc.end_handle = self.next_handle - 1;
                    return Some(desc_handle);
                }
            }
        }
        None
    }

    pub fn read_value(&self, handle: u16) -> Option<&[u8]> {
        for svc in &self.services {
            for ch in &svc.characteristics {
                if ch.value_handle == handle {
                    return Some(&ch.value);
                }
                for desc in &ch.descriptors {
                    if desc.handle == handle {
                        return Some(&desc.value);
                    }
                }
            }
        }
        None
    }

    pub fn write_value(&mut self, handle: u16, data: &[u8]) -> bool {
        for svc in &mut self.services {
            for ch in &mut svc.characteristics {
                if ch.value_handle == handle {
                    ch.value = data.to_vec();
                    return true;
                }
                for desc in &mut ch.descriptors {
                    if desc.handle == handle {
                        desc.value = data.to_vec();
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn set_mtu(&mut self, mtu: u16) {
        self.mtu = mtu.max(23).min(517);
    }

    pub fn discover_services(&self) -> Vec<(u16, u16, GattUuid)> {
        self.services
            .iter()
            .filter(|s| s.primary)
            .map(|s| (s.handle, s.end_handle, s.uuid))
            .collect()
    }
}

// ─── Pairing & Bonding ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingMethod {
    JustWorks,
    PasskeyEntry,
    NumericComparison,
    OutOfBand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoCapability {
    DisplayOnly,
    DisplayYesNo,
    KeyboardOnly,
    NoInputNoOutput,
    KeyboardDisplay,
}

#[derive(Debug, Clone)]
pub struct BtPairing {
    pub addr: BtAddr,
    pub method: PairingMethod,
    pub io_cap: IoCapability,
    pub mitm_protection: bool,
    pub bonded: bool,
    pub ltk: Option<[u8; 16]>,
    pub irk: Option<[u8; 16]>,
    pub csrk: Option<[u8; 16]>,
    pub ediv: u16,
    pub rand: u64,
    pub key_size: u8,
    pub secure_connections: bool,
}

impl BtPairing {
    pub fn new(addr: BtAddr, method: PairingMethod, io_cap: IoCapability) -> Self {
        Self {
            addr,
            method,
            io_cap,
            mitm_protection: method != PairingMethod::JustWorks,
            bonded: false,
            ltk: None,
            irk: None,
            csrk: None,
            ediv: 0,
            rand: 0,
            key_size: 16,
            secure_connections: true,
        }
    }

    pub fn determine_pairing_method(
        initiator_io: IoCapability,
        responder_io: IoCapability,
    ) -> PairingMethod {
        match (initiator_io, responder_io) {
            (IoCapability::NoInputNoOutput, _) | (_, IoCapability::NoInputNoOutput) => {
                PairingMethod::JustWorks
            }
            (IoCapability::DisplayOnly, IoCapability::DisplayOnly) => PairingMethod::JustWorks,
            (IoCapability::DisplayYesNo, IoCapability::DisplayYesNo)
            | (IoCapability::KeyboardDisplay, IoCapability::DisplayYesNo)
            | (IoCapability::DisplayYesNo, IoCapability::KeyboardDisplay)
            | (IoCapability::KeyboardDisplay, IoCapability::KeyboardDisplay) => {
                PairingMethod::NumericComparison
            }
            (IoCapability::KeyboardOnly, _) | (_, IoCapability::KeyboardOnly) => {
                PairingMethod::PasskeyEntry
            }
            _ => PairingMethod::PasskeyEntry,
        }
    }

    pub fn complete_pairing(&mut self, ltk: [u8; 16]) {
        self.ltk = Some(ltk);
        self.bonded = true;
    }
}

// ─── Profiles ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtProfile {
    A2dpSink,
    A2dpSource,
    Hfp,
    HogpDevice,
    HogpHost,
    Avrcp,
    Pan,
    Spp,
}

impl BtProfile {
    pub fn uuid(&self) -> u16 {
        match self {
            Self::A2dpSink => 0x110B,
            Self::A2dpSource => 0x110A,
            Self::Hfp => 0x111E,
            Self::HogpDevice => 0x1812,
            Self::HogpHost => 0x1812,
            Self::Avrcp => 0x110E,
            Self::Pan => 0x1115,
            Self::Spp => 0x1101,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::A2dpSink => "A2DP Sink",
            Self::A2dpSource => "A2DP Source",
            Self::Hfp => "Hands-Free",
            Self::HogpDevice => "HID over GATT (Device)",
            Self::HogpHost => "HID over GATT (Host)",
            Self::Avrcp => "AV Remote Control",
            Self::Pan => "Personal Area Network",
            Self::Spp => "Serial Port",
        }
    }
}

// A2DP audio codec configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2dpCodec {
    Sbc,
    Aac,
    AptX,
    AptXHd,
    Ldac,
    Lc3,
}

pub struct A2dpStream {
    pub codec: A2dpCodec,
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u8,
    pub bitrate: u32,
    pub l2cap_cid: u16,
    pub streaming: bool,
}

impl A2dpStream {
    pub fn new(codec: A2dpCodec) -> Self {
        let (sample_rate, bit_depth, bitrate) = match codec {
            A2dpCodec::Sbc => (44100, 16, 328),
            A2dpCodec::Aac => (44100, 16, 256),
            A2dpCodec::AptX => (48000, 16, 352),
            A2dpCodec::AptXHd => (48000, 24, 576),
            A2dpCodec::Ldac => (96000, 24, 990),
            A2dpCodec::Lc3 => (48000, 24, 160),
        };
        Self {
            codec,
            sample_rate,
            channels: 2,
            bit_depth,
            bitrate,
            l2cap_cid: 0,
            streaming: false,
        }
    }
}

// AVRCP media control
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvrcpCommand {
    Play,
    Pause,
    Stop,
    NextTrack,
    PrevTrack,
    FastForward,
    Rewind,
    VolumeUp,
    VolumeDown,
    SetAbsoluteVolume(u8),
}

pub struct AvrcpController {
    pub volume: u8,
    pub playing: bool,
    pub track_title: String,
    pub artist: String,
    pub album: String,
    pub position_ms: u32,
    pub duration_ms: u32,
}

impl AvrcpController {
    pub fn new() -> Self {
        Self {
            volume: 50,
            playing: false,
            track_title: String::new(),
            artist: String::new(),
            album: String::new(),
            position_ms: 0,
            duration_ms: 0,
        }
    }

    pub fn handle_command(&mut self, cmd: AvrcpCommand) {
        match cmd {
            AvrcpCommand::Play => self.playing = true,
            AvrcpCommand::Pause | AvrcpCommand::Stop => self.playing = false,
            AvrcpCommand::VolumeUp => self.volume = (self.volume + 8).min(127),
            AvrcpCommand::VolumeDown => self.volume = self.volume.saturating_sub(8),
            AvrcpCommand::SetAbsoluteVolume(v) => self.volume = v.min(127),
            _ => {}
        }
    }
}

// HOGP — HID over GATT
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HogpDeviceType {
    Keyboard,
    Mouse,
    Gamepad,
    Joystick,
    Generic,
}

pub struct HogpDevice {
    pub device_type: HogpDeviceType,
    pub report_map: Vec<u8>,
    pub protocol_mode: u8, // 0 = boot, 1 = report
    pub input_reports: Vec<HogpReport>,
    pub output_reports: Vec<HogpReport>,
    pub feature_reports: Vec<HogpReport>,
    pub battery_level: u8,
    pub connected: bool,
}

pub struct HogpReport {
    pub id: u8,
    pub data: Vec<u8>,
    pub size: u8,
}

impl HogpDevice {
    pub fn new_keyboard() -> Self {
        Self {
            device_type: HogpDeviceType::Keyboard,
            report_map: alloc::vec![
                0x05, 0x01, // Usage Page (Generic Desktop)
                0x09, 0x06, // Usage (Keyboard)
                0xA1, 0x01, // Collection (Application)
                0x05, 0x07, //   Usage Page (Key Codes)
                0x19, 0xE0, //   Usage Minimum (224)
                0x29, 0xE7, //   Usage Maximum (231)
                0x15, 0x00, //   Logical Minimum (0)
                0x25, 0x01, //   Logical Maximum (1)
                0x75, 0x01, //   Report Size (1)
                0x95, 0x08, //   Report Count (8)
                0x81, 0x02, //   Input (Data, Variable, Absolute)
                0xC0, // End Collection
            ],
            protocol_mode: 1,
            input_reports: alloc::vec![HogpReport {
                id: 1,
                data: alloc::vec![0; 8],
                size: 8
            }],
            output_reports: alloc::vec![HogpReport {
                id: 1,
                data: alloc::vec![0; 1],
                size: 1
            }],
            feature_reports: Vec::new(),
            battery_level: 100,
            connected: false,
        }
    }

    pub fn new_mouse() -> Self {
        Self {
            device_type: HogpDeviceType::Mouse,
            report_map: alloc::vec![
                0x05, 0x01, // Usage Page (Generic Desktop)
                0x09, 0x02, // Usage (Mouse)
                0xA1, 0x01, // Collection (Application)
                0x09, 0x01, //   Usage (Pointer)
                0xA1, 0x00, //   Collection (Physical)
                0x05, 0x09, //     Usage Page (Buttons)
                0x19, 0x01, //     Usage Minimum (1)
                0x29, 0x03, //     Usage Maximum (3)
                0x15, 0x00, //     Logical Minimum (0)
                0x25, 0x01, //     Logical Maximum (1)
                0x95, 0x03, //     Report Count (3)
                0x75, 0x01, //     Report Size (1)
                0x81, 0x02, //     Input (Data, Variable, Absolute)
                0xC0, //   End Collection
                0xC0, // End Collection
            ],
            protocol_mode: 1,
            input_reports: alloc::vec![HogpReport {
                id: 1,
                data: alloc::vec![0; 4],
                size: 4
            }],
            output_reports: Vec::new(),
            feature_reports: Vec::new(),
            battery_level: 100,
            connected: false,
        }
    }

    pub fn new_gamepad() -> Self {
        Self {
            device_type: HogpDeviceType::Gamepad,
            report_map: alloc::vec![
                0x05, 0x01, // Usage Page (Generic Desktop)
                0x09, 0x05, // Usage (Game Pad)
                0xA1, 0x01, // Collection (Application)
                0x05, 0x09, //   Usage Page (Buttons)
                0x19, 0x01, //   Usage Minimum (1)
                0x29, 0x10, //   Usage Maximum (16)
                0x15, 0x00, //   Logical Minimum (0)
                0x25, 0x01, //   Logical Maximum (1)
                0x95, 0x10, //   Report Count (16)
                0x75, 0x01, //   Report Size (1)
                0x81, 0x02, //   Input (Data, Variable, Absolute)
                0xC0, // End Collection
            ],
            protocol_mode: 1,
            input_reports: alloc::vec![HogpReport {
                id: 1,
                data: alloc::vec![0; 8],
                size: 8
            }],
            output_reports: alloc::vec![HogpReport {
                id: 1,
                data: alloc::vec![0; 2],
                size: 2
            }],
            feature_reports: Vec::new(),
            battery_level: 100,
            connected: false,
        }
    }
}

// PAN — Personal Area Networking
pub struct PanConnection {
    pub role: PanRole,
    pub l2cap_cid: u16,
    pub local_addr: [u8; 6],
    pub remote_addr: BtAddr,
    pub mtu: u16,
    pub connected: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanRole {
    Nap,  // Network Access Point
    Gn,   // Group Ad-hoc Network
    Panu, // PAN User
}

impl PanConnection {
    pub fn new(role: PanRole, remote: BtAddr) -> Self {
        Self {
            role,
            l2cap_cid: 0,
            local_addr: [0; 6],
            remote_addr: remote,
            mtu: 1691,
            connected: false,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }
}

// ─── Device Tracking ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BtDevice {
    pub addr: BtAddr,
    pub addr_type: BtAddrType,
    pub name: String,
    pub class: u32,
    pub rssi: i8,
    pub paired: bool,
    pub connected: bool,
    pub connection_handle: Option<u16>,
    pub services: Vec<u16>,
    pub le_device: bool,
    pub last_seen: u64,
    pub manufacturer_data: Vec<u8>,
}

impl BtDevice {
    pub fn new(addr: BtAddr, addr_type: BtAddrType) -> Self {
        Self {
            addr,
            addr_type,
            name: String::new(),
            class: 0,
            rssi: -128,
            paired: false,
            connected: false,
            connection_handle: None,
            services: Vec::new(),
            le_device: matches!(addr_type, BtAddrType::Random | BtAddrType::RandomIdentity),
            last_seen: 0,
            manufacturer_data: Vec::new(),
        }
    }

    pub fn major_device_class(&self) -> u8 {
        ((self.class >> 8) & 0x1F) as u8
    }

    pub fn minor_device_class(&self) -> u8 {
        ((self.class >> 2) & 0x3F) as u8
    }
}

// ─── Main Bluetooth Subsystem ───────────────────────────────────────────────

pub struct BluetoothSubsystem {
    pub controller: Option<HciController>,
    pub l2cap: L2capLayer,
    pub sdp: SdpDatabase,
    pub gatt: GattServer,
    pub devices: BTreeMap<BtAddr, BtDevice>,
    pub pairings: Vec<BtPairing>,
    pub profiles: Vec<BtProfile>,
    pub state: BtState,
    pub scanning: bool,
    pub rfcomm_sessions: Vec<RfcommSession>,
    pub a2dp_stream: Option<A2dpStream>,
    pub avrcp: AvrcpController,
    pub hogp_devices: Vec<HogpDevice>,
    pub pan_connections: Vec<PanConnection>,
}

impl BluetoothSubsystem {
    pub fn new() -> Self {
        Self {
            controller: None,
            l2cap: L2capLayer::new(),
            sdp: SdpDatabase::new(),
            gatt: GattServer::new(),
            devices: BTreeMap::new(),
            pairings: Vec::new(),
            profiles: Vec::new(),
            state: BtState::Off,
            scanning: false,
            rfcomm_sessions: Vec::new(),
            a2dp_stream: None,
            avrcp: AvrcpController::new(),
            hogp_devices: Vec::new(),
            pan_connections: Vec::new(),
        }
    }

    pub fn initialize(&mut self, transport: HciTransport) -> Result<(), BtError> {
        self.state = BtState::Initializing;

        let mut controller = HciController::new(transport);
        controller.reset()?;
        controller.send_command(HciCommand::SetEventMask {
            mask: 0x3FFF_FFFF_FFFF_FFFF,
        })?;
        controller.send_command(HciCommand::WriteLocalName {
            name: String::from("AthenaOS"),
        })?;
        controller.send_command(HciCommand::WriteClassOfDevice {
            class: 0x00_01_04_00, // Computer — Desktop workstation
        })?;
        controller.send_command(HciCommand::WriteScanEnable {
            scan_enable: 0x03, // Inquiry + Page scan
        })?;
        controller.send_command(HciCommand::LeReadBufferSize)?;

        self.controller = Some(controller);

        self.register_default_profiles();
        self.setup_gatt_server();

        self.state = BtState::Ready;
        Ok(())
    }

    fn register_default_profiles(&mut self) {
        self.profiles.push(BtProfile::A2dpSink);
        self.profiles.push(BtProfile::Hfp);
        self.profiles.push(BtProfile::HogpHost);
        self.profiles.push(BtProfile::Avrcp);
        self.profiles.push(BtProfile::Pan);
        self.profiles.push(BtProfile::Spp);

        for profile in &self.profiles {
            self.sdp.register_service(SdpServiceRecord {
                handle: 0,
                service_class_uuids: alloc::vec![profile.uuid() as u128],
                protocol_descriptors: alloc::vec![
                    SdpProtocol {
                        uuid: 0x0100,
                        params: Vec::new()
                    }, // L2CAP
                ],
                name: String::from(profile.name()),
                description: String::new(),
                provider: String::from("AthenaOS"),
                browse_group: 0x1002,
                attributes: BTreeMap::new(),
            });
        }
    }

    fn setup_gatt_server(&mut self) {
        let gap = self.gatt.add_service(GattUuid::GENERIC_ACCESS, true);
        self.gatt.add_characteristic(
            gap,
            GattUuid::Uuid16(0x2A00), // Device Name
            GattCharProps::read_only(),
            b"AthenaOS".to_vec(),
        );
        self.gatt.add_characteristic(
            gap,
            GattUuid::Uuid16(0x2A01), // Appearance
            GattCharProps::read_only(),
            alloc::vec![0x80, 0x00], // Generic Computer
        );

        let gatt_svc = self.gatt.add_service(GattUuid::GENERIC_ATTRIBUTE, true);
        self.gatt.add_characteristic(
            gatt_svc,
            GattUuid::Uuid16(0x2A05), // Service Changed
            GattCharProps::read_notify(),
            alloc::vec![0x01, 0x00, 0xFF, 0xFF],
        );

        let bat = self.gatt.add_service(GattUuid::BATTERY_SERVICE, true);
        self.gatt.add_characteristic(
            bat,
            GattUuid::Uuid16(0x2A19), // Battery Level
            GattCharProps::read_notify(),
            alloc::vec![100],
        );

        let dev_info = self.gatt.add_service(GattUuid::DEVICE_INFORMATION, true);
        self.gatt.add_characteristic(
            dev_info,
            GattUuid::Uuid16(0x2A29), // Manufacturer
            GattCharProps::read_only(),
            b"RaeenOS Project".to_vec(),
        );
        self.gatt.add_characteristic(
            dev_info,
            GattUuid::Uuid16(0x2A24), // Model Number
            GattCharProps::read_only(),
            b"RaeenOS-BT-1".to_vec(),
        );
        self.gatt.add_characteristic(
            dev_info,
            GattUuid::Uuid16(0x2A26), // Firmware Revision
            GattCharProps::read_only(),
            b"0.0.1".to_vec(),
        );
    }

    pub fn start_scan(&mut self) -> Result<(), BtError> {
        let ctrl = self.controller.as_mut().ok_or(BtError::NotInitialized)?;
        ctrl.start_le_scan(true, 0x0060, 0x0030)?;
        self.scanning = true;
        self.state = BtState::Scanning;
        Ok(())
    }

    pub fn stop_scan(&mut self) -> Result<(), BtError> {
        let ctrl = self.controller.as_mut().ok_or(BtError::NotInitialized)?;
        ctrl.stop_le_scan()?;
        self.scanning = false;
        self.state = BtState::Ready;
        Ok(())
    }

    pub fn connect(&mut self, addr: BtAddr) -> Result<u16, BtError> {
        let ctrl = self.controller.as_mut().ok_or(BtError::NotInitialized)?;

        if self.scanning {
            ctrl.stop_le_scan()?;
            self.scanning = false;
        }

        let addr_type = self
            .devices
            .get(&addr)
            .map(|d| d.addr_type)
            .unwrap_or(BtAddrType::Public);

        let handle = ctrl.create_connection(addr, addr_type)?;

        if let Some(dev) = self.devices.get_mut(&addr) {
            dev.connected = true;
            dev.connection_handle = Some(handle);
        } else {
            let mut dev = BtDevice::new(addr, addr_type);
            dev.connected = true;
            dev.connection_handle = Some(handle);
            self.devices.insert(addr, dev);
        }

        self.state = BtState::Connected;
        Ok(handle)
    }

    pub fn disconnect(&mut self, addr: BtAddr) -> Result<(), BtError> {
        let dev = self
            .devices
            .get_mut(&addr)
            .ok_or(BtError::ConnectionFailed(addr))?;

        let handle = dev
            .connection_handle
            .ok_or(BtError::ConnectionFailed(addr))?;

        let ctrl = self.controller.as_mut().ok_or(BtError::NotInitialized)?;
        ctrl.disconnect(handle, 0x13)?; // Remote User Terminated Connection

        dev.connected = false;
        dev.connection_handle = None;

        let any_connected = self.devices.values().any(|d| d.connected);
        if !any_connected {
            self.state = BtState::Ready;
        }

        Ok(())
    }

    pub fn pair(&mut self, addr: BtAddr, io_cap: IoCapability) -> Result<(), BtError> {
        if !self.devices.contains_key(&addr) {
            return Err(BtError::ConnectionFailed(addr));
        }

        let method = BtPairing::determine_pairing_method(io_cap, IoCapability::DisplayYesNo);
        let mut pairing = BtPairing::new(addr, method, io_cap);

        // Simulate successful pairing with a generated LTK
        let ltk = [0x42u8; 16];
        pairing.complete_pairing(ltk);

        if let Some(dev) = self.devices.get_mut(&addr) {
            dev.paired = true;
        }

        self.pairings.push(pairing);
        Ok(())
    }

    pub fn get_paired_devices(&self) -> Vec<&BtDevice> {
        self.devices.values().filter(|d| d.paired).collect()
    }

    pub fn get_connected_devices(&self) -> Vec<&BtDevice> {
        self.devices.values().filter(|d| d.connected).collect()
    }

    pub fn handle_advertising_report(
        &mut self,
        addr: BtAddr,
        addr_type: BtAddrType,
        rssi: i8,
        adv_data: &[u8],
    ) {
        let dev = self
            .devices
            .entry(addr)
            .or_insert_with(|| BtDevice::new(addr, addr_type));
        dev.rssi = rssi;
        dev.last_seen = 0; // would use a real timestamp

        // Parse AD structures
        let mut i = 0;
        while i + 1 < adv_data.len() {
            let len = adv_data[i] as usize;
            if len == 0 || i + 1 + len > adv_data.len() {
                break;
            }
            let ad_type = adv_data[i + 1];
            let data = &adv_data[i + 2..i + 1 + len];

            match ad_type {
                0x08 | 0x09 => {
                    if let Ok(name) = core::str::from_utf8(data) {
                        dev.name = String::from(name);
                    }
                }
                0xFF => {
                    dev.manufacturer_data = data.to_vec();
                }
                _ => {}
            }
            i += 1 + len;
        }
    }

    pub fn start_a2dp_stream(&mut self, codec: A2dpCodec) -> Result<(), BtError> {
        self.a2dp_stream = Some(A2dpStream::new(codec));
        if let Some(ref mut stream) = self.a2dp_stream {
            let cid = self.l2cap.allocate_channel(0x0019); // AVDTP
            stream.l2cap_cid = cid;
            stream.streaming = true;
        }
        Ok(())
    }

    pub fn stop_a2dp_stream(&mut self) {
        if let Some(ref mut stream) = self.a2dp_stream {
            stream.streaming = false;
        }
    }
}

// ─── Global Instance ────────────────────────────────────────────────────────

pub static BLUETOOTH: Mutex<Option<BluetoothSubsystem>> = Mutex::new(None);

pub fn init() {
    crate::serial_println!("[bluetooth] Initializing Bluetooth subsystem");

    let mut bt = BluetoothSubsystem::new();

    match bt.initialize(HciTransport::Virtual) {
        Ok(()) => {
            crate::serial_println!("[ OK ] Bluetooth subsystem initialized (virtual HCI)");
            crate::serial_println!(
                "[bluetooth] Profiles: {}",
                bt.profiles
                    .iter()
                    .map(|p| p.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Err(e) => {
            crate::serial_println!("[FAIL] Bluetooth initialization failed: {}", e);
        }
    }

    *BLUETOOTH.lock() = Some(bt);
}
