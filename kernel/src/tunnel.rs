//! Network Tunneling Protocols — GRE, VXLAN, GENEVE, IP-in-IP, WireGuard,
//! L2TP, PPPoE, TUN/TAP, IPVS, and tunnel metadata infrastructure.

#![allow(dead_code)]

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeMap, string::String, vec, vec::Vec};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Common types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EtherType {
    Ipv4 = 0x0800,
    Ipv6 = 0x86DD,
    Arp = 0x0806,
    Vlan = 0x8100,
    TransparentEthBridge = 0x6558,
    Erspan = 0x88BE,
    Ppp = 0x880B,
    PppDisc = 0x8863,
    PppSess = 0x8864,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IpProtocol {
    Tcp = 6,
    Udp = 17,
    Gre = 47,
    Ipv4InIp = 4,
    Ipv6InIp = 41,
    IpComp = 108,
    L2tp = 115,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv4Addr(pub [u8; 4]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv6Addr(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpAddr {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xFF; 6]);
    pub const ZERO: MacAddr = MacAddr([0; 6]);

    pub fn is_multicast(&self) -> bool {
        self.0[0] & 0x01 != 0
    }

    pub fn is_broadcast(&self) -> bool {
        self.0 == [0xFF; 6]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelError {
    InvalidHeader,
    BufferTooShort,
    ChecksumMismatch,
    UnsupportedVersion,
    VniNotFound,
    NoRoute,
    MtuExceeded,
    SessionNotFound,
    DeviceNotFound,
    HandshakeRequired,
    EncryptionFailed,
    DecryptionFailed,
    QueueFull,
    InvalidState,
    ServiceNotFound,
    NoRealServer,
    AllocationFailed,
    CookieInvalid,
}

// ─────────────────────────────────────────────────────────────────────────────
// GRE — Generic Routing Encapsulation (RFC 2784/2890, GREv1/PPTP, ERSPAN)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct GreHeader {
    pub flags_version: u16,
    pub protocol_type: u16,
}

impl GreHeader {
    pub const FLAG_CHECKSUM: u16 = 0x8000;
    pub const FLAG_KEY: u16 = 0x2000;
    pub const FLAG_SEQUENCE: u16 = 0x1000;
    pub const FLAG_ACK: u16 = 0x0080;

    pub fn version(&self) -> u8 {
        (self.flags_version & 0x0007) as u8
    }

    pub fn has_checksum(&self) -> bool {
        self.flags_version & Self::FLAG_CHECKSUM != 0
    }

    pub fn has_key(&self) -> bool {
        self.flags_version & Self::FLAG_KEY != 0
    }

    pub fn has_sequence(&self) -> bool {
        self.flags_version & Self::FLAG_SEQUENCE != 0
    }

    pub fn has_ack(&self) -> bool {
        self.flags_version & Self::FLAG_ACK != 0
    }
}

#[derive(Debug, Clone)]
pub struct GreOptionalFields {
    pub checksum: Option<u16>,
    pub key: Option<u32>,
    pub sequence: Option<u32>,
    pub ack: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErspanType {
    TypeI,
    TypeII,
    TypeIII,
}

#[derive(Debug, Clone)]
pub struct ErspanHeader {
    pub span_type: ErspanType,
    pub version: u8,
    pub vlan: u16,
    pub cos: u8,
    pub truncated: bool,
    pub session_id: u16,
    pub port_index: u32,
    pub timestamp: Option<u32>,
    pub sgt: Option<u16>,
    pub direction: u8,
}

#[derive(Debug, Clone)]
pub struct GreTunnel {
    pub id: u32,
    pub name: String,
    pub local: IpAddr,
    pub remote: IpAddr,
    pub key: Option<u32>,
    pub use_checksum: bool,
    pub use_sequence: bool,
    pub sequence_number: u32,
    pub ttl: u8,
    pub tos: u8,
    pub mtu: u16,
    pub version: u8,
    pub erspan: Option<ErspanHeader>,
    pub is_tap: bool,
    pub stats: TunnelStats,
}

impl GreTunnel {
    pub fn new_v0(id: u32, name: String, local: IpAddr, remote: IpAddr) -> Self {
        Self {
            id,
            name,
            local,
            remote,
            key: None,
            use_checksum: false,
            use_sequence: false,
            sequence_number: 0,
            ttl: 64,
            tos: 0,
            mtu: 1476,
            version: 0,
            erspan: None,
            is_tap: false,
            stats: TunnelStats::new(),
        }
    }

    pub fn new_v1_pptp(id: u32, name: String, local: IpAddr, remote: IpAddr, call_id: u16) -> Self {
        Self {
            id,
            name,
            local,
            remote,
            key: Some(call_id as u32),
            use_checksum: false,
            use_sequence: true,
            sequence_number: 0,
            ttl: 64,
            tos: 0,
            mtu: 1400,
            version: 1,
            erspan: None,
            is_tap: false,
            stats: TunnelStats::new(),
        }
    }

    pub fn new_erspan(
        id: u32,
        name: String,
        local: IpAddr,
        remote: IpAddr,
        span_type: ErspanType,
        session_id: u16,
    ) -> Self {
        Self {
            id,
            name,
            local,
            remote,
            key: None,
            use_checksum: false,
            use_sequence: true,
            sequence_number: 0,
            ttl: 64,
            tos: 0,
            mtu: 1450,
            version: 0,
            erspan: Some(ErspanHeader {
                span_type,
                version: match span_type {
                    ErspanType::TypeI => 0,
                    ErspanType::TypeII => 1,
                    ErspanType::TypeIII => 2,
                },
                vlan: 0,
                cos: 0,
                truncated: false,
                session_id,
                port_index: 0,
                timestamp: None,
                sgt: None,
                direction: 0,
            }),
            is_tap: false,
            stats: TunnelStats::new(),
        }
    }

    pub fn new_gre_tap(id: u32, name: String, local: IpAddr, remote: IpAddr) -> Self {
        let mut t = Self::new_v0(id, name, local, remote);
        t.is_tap = true;
        t.mtu = 1462;
        t
    }

    pub fn encapsulate(&mut self, inner: &[u8], buf: &mut Vec<u8>) -> Result<(), TunnelError> {
        if inner.len() > self.mtu as usize {
            return Err(TunnelError::MtuExceeded);
        }
        let mut flags: u16 = (self.version as u16) & 0x07;
        if self.use_checksum {
            flags |= GreHeader::FLAG_CHECKSUM;
        }
        if self.key.is_some() {
            flags |= GreHeader::FLAG_KEY;
        }
        if self.use_sequence {
            flags |= GreHeader::FLAG_SEQUENCE;
        }
        let proto = if self.is_tap {
            EtherType::TransparentEthBridge as u16
        } else if self.erspan.is_some() {
            EtherType::Erspan as u16
        } else {
            EtherType::Ipv4 as u16
        };
        buf.push((flags >> 8) as u8);
        buf.push(flags as u8);
        buf.push((proto >> 8) as u8);
        buf.push(proto as u8);
        if self.use_checksum {
            buf.extend_from_slice(&[0, 0, 0, 0]);
        }
        if let Some(key) = self.key {
            buf.extend_from_slice(&key.to_be_bytes());
        }
        if self.use_sequence {
            buf.extend_from_slice(&self.sequence_number.to_be_bytes());
            self.sequence_number = self.sequence_number.wrapping_add(1);
        }
        if let Some(ref erspan) = self.erspan {
            self.encode_erspan(erspan, buf);
        }
        buf.extend_from_slice(inner);
        if self.use_checksum {
            let cksum = Self::gre_checksum(buf);
            buf[4] = (cksum >> 8) as u8;
            buf[5] = cksum as u8;
        }
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += buf.len() as u64;
        Ok(())
    }

    pub fn decapsulate(&mut self, packet: &[u8]) -> Result<(u16, Vec<u8>), TunnelError> {
        if packet.len() < 4 {
            return Err(TunnelError::BufferTooShort);
        }
        let flags = ((packet[0] as u16) << 8) | packet[1] as u16;
        let proto = ((packet[2] as u16) << 8) | packet[3] as u16;
        let version = flags & 0x07;
        if version != self.version as u16 {
            return Err(TunnelError::UnsupportedVersion);
        }
        let mut offset = 4usize;
        if flags & GreHeader::FLAG_CHECKSUM != 0 {
            if packet.len() < offset + 4 {
                return Err(TunnelError::BufferTooShort);
            }
            let stored_cksum = ((packet[offset] as u16) << 8) | packet[offset + 1] as u16;
            let mut verify_buf = packet.to_vec();
            verify_buf[offset] = 0;
            verify_buf[offset + 1] = 0;
            let computed = Self::gre_checksum(&verify_buf);
            if stored_cksum != computed {
                return Err(TunnelError::ChecksumMismatch);
            }
            offset += 4;
        }
        if flags & GreHeader::FLAG_KEY != 0 {
            if packet.len() < offset + 4 {
                return Err(TunnelError::BufferTooShort);
            }
            offset += 4;
        }
        if flags & GreHeader::FLAG_SEQUENCE != 0 {
            if packet.len() < offset + 4 {
                return Err(TunnelError::BufferTooShort);
            }
            offset += 4;
        }
        if flags & GreHeader::FLAG_ACK != 0 {
            if packet.len() < offset + 4 {
                return Err(TunnelError::BufferTooShort);
            }
            offset += 4;
        }
        if offset > packet.len() {
            return Err(TunnelError::BufferTooShort);
        }
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.len() as u64;
        Ok((proto, packet[offset..].to_vec()))
    }

    fn encode_erspan(&self, hdr: &ErspanHeader, buf: &mut Vec<u8>) {
        match hdr.span_type {
            ErspanType::TypeI => {}
            ErspanType::TypeII => {
                let w0: u32 = ((hdr.version as u32) << 28)
                    | ((hdr.vlan as u32) << 16)
                    | ((hdr.cos as u32) << 13)
                    | if hdr.truncated { 1 << 10 } else { 0 }
                    | (hdr.session_id as u32);
                buf.extend_from_slice(&w0.to_be_bytes());
                buf.extend_from_slice(&hdr.port_index.to_be_bytes());
            }
            ErspanType::TypeIII => {
                let w0: u32 = ((hdr.version as u32) << 28)
                    | ((hdr.vlan as u32) << 16)
                    | ((hdr.cos as u32) << 13)
                    | if hdr.truncated { 1 << 10 } else { 0 }
                    | (hdr.session_id as u32);
                buf.extend_from_slice(&w0.to_be_bytes());
                buf.extend_from_slice(&hdr.timestamp.unwrap_or(0).to_be_bytes());
                let w2: u32 = ((hdr.sgt.unwrap_or(0) as u32) << 16) | ((hdr.direction as u32) << 3);
                buf.extend_from_slice(&w2.to_be_bytes());
            }
        }
    }

    fn gre_checksum(data: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        let mut i = 0;
        while i + 1 < data.len() {
            sum += ((data[i] as u32) << 8) | data[i + 1] as u32;
            i += 2;
        }
        if i < data.len() {
            sum += (data[i] as u32) << 8;
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        !(sum as u16)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VXLAN — Virtual Extensible LAN (RFC 7348, VXLAN-GPE RFC draft)
// ─────────────────────────────────────────────────────────────────────────────

pub const VXLAN_UDP_PORT: u16 = 4789;
pub const VXLAN_GPE_UDP_PORT: u16 = 4790;
pub const VXLAN_HEADER_LEN: usize = 8;

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct VxlanHeader {
    pub flags: u8,
    pub reserved1: [u8; 3],
    pub vni_reserved: [u8; 4],
}

impl VxlanHeader {
    pub const FLAG_I: u8 = 0x08;
    pub const FLAG_GPE_P: u8 = 0x04;
    pub const FLAG_GPE_B: u8 = 0x80;
    pub const FLAG_GPE_O: u8 = 0x01;

    pub fn vni(&self) -> u32 {
        ((self.vni_reserved[0] as u32) << 16)
            | ((self.vni_reserved[1] as u32) << 8)
            | (self.vni_reserved[2] as u32)
    }

    pub fn set_vni(vni: u32) -> [u8; 4] {
        [
            ((vni >> 16) & 0xFF) as u8,
            ((vni >> 8) & 0xFF) as u8,
            (vni & 0xFF) as u8,
            0,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VxlanGpeProtocol {
    Ipv4 = 0x01,
    Ipv6 = 0x02,
    Ethernet = 0x03,
    Nsh = 0x04,
}

#[derive(Debug, Clone)]
pub struct VxlanFdbEntry {
    pub mac: MacAddr,
    pub vtep_ip: IpAddr,
    pub vni: u32,
    pub port: u16,
    pub is_static: bool,
    pub age_ticks: u64,
}

#[derive(Debug)]
pub struct VxlanInstance {
    pub vni: u32,
    pub local_ip: IpAddr,
    pub local_port: u16,
    pub group: Option<IpAddr>,
    pub ttl: u8,
    pub tos: u8,
    pub mtu: u16,
    pub learning: bool,
    pub arp_suppress: bool,
    pub nd_suppress: bool,
    pub is_gpe: bool,
    pub fdb: Vec<VxlanFdbEntry>,
    pub stats: TunnelStats,
}

impl VxlanInstance {
    pub fn new(vni: u32, local_ip: IpAddr) -> Self {
        Self {
            vni,
            local_ip,
            local_port: VXLAN_UDP_PORT,
            group: None,
            ttl: 64,
            tos: 0,
            mtu: 1450,
            learning: true,
            arp_suppress: false,
            nd_suppress: false,
            is_gpe: false,
            fdb: Vec::new(),
            stats: TunnelStats::new(),
        }
    }

    pub fn new_gpe(vni: u32, local_ip: IpAddr) -> Self {
        let mut inst = Self::new(vni, local_ip);
        inst.is_gpe = true;
        inst.local_port = VXLAN_GPE_UDP_PORT;
        inst
    }

    pub fn fdb_learn(&mut self, src_mac: MacAddr, vtep_ip: IpAddr) {
        if !self.learning {
            return;
        }
        for entry in &mut self.fdb {
            if entry.mac.0 == src_mac.0 && entry.vni == self.vni {
                entry.vtep_ip = vtep_ip;
                entry.age_ticks = 0;
                return;
            }
        }
        self.fdb.push(VxlanFdbEntry {
            mac: src_mac,
            vtep_ip,
            vni: self.vni,
            port: self.local_port,
            is_static: false,
            age_ticks: 0,
        });
    }

    pub fn fdb_add_static(&mut self, mac: MacAddr, vtep_ip: IpAddr) {
        self.fdb.push(VxlanFdbEntry {
            mac,
            vtep_ip,
            vni: self.vni,
            port: self.local_port,
            is_static: true,
            age_ticks: 0,
        });
    }

    pub fn fdb_lookup(&self, dst_mac: &MacAddr) -> Option<&VxlanFdbEntry> {
        self.fdb
            .iter()
            .find(|e| e.mac.0 == dst_mac.0 && e.vni == self.vni)
    }

    pub fn fdb_age(&mut self, max_age: u64) {
        self.fdb.retain(|e| e.is_static || e.age_ticks < max_age);
        for entry in &mut self.fdb {
            entry.age_ticks += 1;
        }
    }

    pub fn encapsulate(
        &mut self,
        inner_frame: &[u8],
        buf: &mut Vec<u8>,
    ) -> Result<(), TunnelError> {
        if inner_frame.len() > self.mtu as usize {
            return Err(TunnelError::MtuExceeded);
        }
        let flags = if self.is_gpe {
            VxlanHeader::FLAG_I | VxlanHeader::FLAG_GPE_P
        } else {
            VxlanHeader::FLAG_I
        };
        buf.push(flags);
        buf.extend_from_slice(&[0, 0, 0]);
        let vni_bytes = VxlanHeader::set_vni(self.vni);
        buf.extend_from_slice(&vni_bytes);
        buf.extend_from_slice(inner_frame);
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += buf.len() as u64;
        Ok(())
    }

    pub fn decapsulate(&mut self, packet: &[u8]) -> Result<(u32, Vec<u8>), TunnelError> {
        if packet.len() < VXLAN_HEADER_LEN {
            return Err(TunnelError::BufferTooShort);
        }
        let flags = packet[0];
        if flags & VxlanHeader::FLAG_I == 0 {
            return Err(TunnelError::InvalidHeader);
        }
        let vni = ((packet[4] as u32) << 16) | ((packet[5] as u32) << 8) | packet[6] as u32;
        if vni != self.vni {
            return Err(TunnelError::VniNotFound);
        }
        let inner = packet[VXLAN_HEADER_LEN..].to_vec();
        if self.learning && inner.len() >= 14 {
            let src_mac = MacAddr([inner[6], inner[7], inner[8], inner[9], inner[10], inner[11]]);
            if !src_mac.is_multicast() {
                // vtep_ip would be extracted from outer IP header in real impl
            }
        }
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.len() as u64;
        Ok((vni, inner))
    }

    pub fn handle_bum(&self, frame: &[u8]) -> Vec<IpAddr> {
        if frame.len() < 6 {
            return Vec::new();
        }
        let dst_mac = MacAddr([frame[0], frame[1], frame[2], frame[3], frame[4], frame[5]]);
        if dst_mac.is_broadcast() || dst_mac.is_multicast() || self.fdb_lookup(&dst_mac).is_none() {
            if let Some(group) = self.group {
                return vec![group];
            }
            return self
                .fdb
                .iter()
                .filter(|e| e.vni == self.vni)
                .map(|e| e.vtep_ip)
                .collect();
        }
        if let Some(entry) = self.fdb_lookup(&dst_mac) {
            vec![entry.vtep_ip]
        } else {
            Vec::new()
        }
    }

    pub fn should_suppress_arp(&self, frame: &[u8]) -> bool {
        if !self.arp_suppress || frame.len() < 42 {
            return false;
        }
        let ethertype = ((frame[12] as u16) << 8) | frame[13] as u16;
        ethertype == EtherType::Arp as u16
    }

    pub fn should_suppress_nd(&self, frame: &[u8]) -> bool {
        if !self.nd_suppress || frame.len() < 78 {
            return false;
        }
        let ethertype = ((frame[12] as u16) << 8) | frame[13] as u16;
        ethertype == EtherType::Ipv6 as u16
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GENEVE — Generic Network Virtualization Encapsulation (RFC 8926)
// ─────────────────────────────────────────────────────────────────────────────

pub const GENEVE_UDP_PORT: u16 = 6081;
pub const GENEVE_BASE_HEADER_LEN: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct GeneveHeader {
    pub version: u8,
    pub opt_len: u8,
    pub oam: bool,
    pub critical: bool,
    pub protocol: u16,
    pub vni: u32,
}

impl GeneveHeader {
    pub fn opt_len_bytes(&self) -> usize {
        self.opt_len as usize * 4
    }
}

#[derive(Debug, Clone)]
pub struct GeneveTlvOption {
    pub class: u16,
    pub opt_type: u8,
    pub critical: bool,
    pub length: u8,
    pub data: Vec<u8>,
}

impl GeneveTlvOption {
    pub fn wire_len(&self) -> usize {
        4 + self.data.len()
    }

    pub fn encode(&self, buf: &mut Vec<u8>) {
        buf.push((self.class >> 8) as u8);
        buf.push(self.class as u8);
        let type_byte = if self.critical {
            self.opt_type | 0x80
        } else {
            self.opt_type
        };
        buf.push(type_byte);
        // Length is in 4-byte units and MUST cover the padded data we actually
        // write below (BUG-43: floor division under-reports unaligned options —
        // 5 data bytes pad to 8 on the wire but floor(5/4)=1 claims 4, desyncing
        // the remote TLV walk). Ceiling division matches the padding.
        buf.push(((self.data.len() + 3) / 4) as u8);
        buf.extend_from_slice(&self.data);
        let pad = (4 - (self.data.len() % 4)) % 4;
        for _ in 0..pad {
            buf.push(0);
        }
    }

    pub fn decode(data: &[u8]) -> Result<(Self, usize), TunnelError> {
        if data.len() < 4 {
            return Err(TunnelError::BufferTooShort);
        }
        let class = ((data[0] as u16) << 8) | data[1] as u16;
        let opt_type = data[2] & 0x7F;
        let critical = data[2] & 0x80 != 0;
        let length = data[3] & 0x1F;
        let data_len = length as usize * 4;
        if data.len() < 4 + data_len {
            return Err(TunnelError::BufferTooShort);
        }
        Ok((
            Self {
                class,
                opt_type,
                critical,
                length,
                data: data[4..4 + data_len].to_vec(),
            },
            4 + data_len,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneveInnerProtocol {
    Ethernet,
    Ipv4,
    Ipv6,
}

#[derive(Debug)]
pub struct GeneveInstance {
    pub vni: u32,
    pub local_ip: IpAddr,
    pub remote_ip: Option<IpAddr>,
    pub local_port: u16,
    pub ttl: u8,
    pub tos: u8,
    pub mtu: u16,
    pub oam: bool,
    pub options: Vec<GeneveTlvOption>,
    pub inner_proto: GeneveInnerProtocol,
    pub stats: TunnelStats,
}

impl GeneveInstance {
    pub fn new(vni: u32, local_ip: IpAddr) -> Self {
        Self {
            vni,
            local_ip,
            remote_ip: None,
            local_port: GENEVE_UDP_PORT,
            ttl: 64,
            tos: 0,
            mtu: 1400,
            oam: false,
            options: Vec::new(),
            inner_proto: GeneveInnerProtocol::Ethernet,
            stats: TunnelStats::new(),
        }
    }

    pub fn add_option(&mut self, class: u16, opt_type: u8, critical: bool, data: Vec<u8>) {
        self.options.push(GeneveTlvOption {
            class,
            opt_type,
            critical,
            length: (data.len() / 4) as u8,
            data,
        });
    }

    pub fn encapsulate(&mut self, inner: &[u8], buf: &mut Vec<u8>) -> Result<(), TunnelError> {
        if inner.len() > self.mtu as usize {
            return Err(TunnelError::MtuExceeded);
        }
        let opt_total: usize = self.options.iter().map(|o| o.wire_len()).sum();
        let opt_len_field = (opt_total / 4) as u8;
        let has_critical = self.options.iter().any(|o| o.critical);
        let proto = match self.inner_proto {
            GeneveInnerProtocol::Ethernet => EtherType::TransparentEthBridge as u16,
            GeneveInnerProtocol::Ipv4 => EtherType::Ipv4 as u16,
            GeneveInnerProtocol::Ipv6 => EtherType::Ipv6 as u16,
        };
        let byte0 = (0u8 << 6) | (opt_len_field & 0x3F);
        let mut byte1 = 0u8;
        if self.oam {
            byte1 |= 0x80;
        }
        if has_critical {
            byte1 |= 0x40;
        }
        buf.push(byte0);
        buf.push(byte1);
        buf.push((proto >> 8) as u8);
        buf.push(proto as u8);
        let vni_bytes = [
            ((self.vni >> 16) & 0xFF) as u8,
            ((self.vni >> 8) & 0xFF) as u8,
            (self.vni & 0xFF) as u8,
            0,
        ];
        buf.extend_from_slice(&vni_bytes);
        for opt in &self.options {
            opt.encode(buf);
        }
        buf.extend_from_slice(inner);
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += buf.len() as u64;
        Ok(())
    }

    pub fn decapsulate(&mut self, packet: &[u8]) -> Result<(u32, Vec<u8>), TunnelError> {
        if packet.len() < GENEVE_BASE_HEADER_LEN {
            return Err(TunnelError::BufferTooShort);
        }
        let version = (packet[0] >> 6) & 0x03;
        if version != 0 {
            return Err(TunnelError::UnsupportedVersion);
        }
        let opt_len = (packet[0] & 0x3F) as usize * 4;
        let vni = ((packet[4] as u32) << 16) | ((packet[5] as u32) << 8) | packet[6] as u32;
        if vni != self.vni {
            return Err(TunnelError::VniNotFound);
        }
        let header_total = GENEVE_BASE_HEADER_LEN + opt_len;
        if packet.len() < header_total {
            return Err(TunnelError::BufferTooShort);
        }
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.len() as u64;
        Ok((vni, packet[header_total..].to_vec()))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IP-in-IP — IPIP, SIT, IP6IP6, IPv4-in-IPv6
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpipMode {
    Ipv4InIpv4,
    Ipv6InIpv4Sit,
    Ipv6InIpv6,
    Ipv4InIpv6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitSubMode {
    SixToFour,
    SixRd,
    Isatap,
    Plain,
}

#[derive(Debug)]
pub struct IpipTunnel {
    pub id: u32,
    pub name: String,
    pub mode: IpipMode,
    pub sit_sub: Option<SitSubMode>,
    pub local: IpAddr,
    pub remote: IpAddr,
    pub ttl: u8,
    pub tos: u8,
    pub mtu: u16,
    pub pmtu_discovery: bool,
    pub dont_fragment: bool,
    pub ecn_copy: bool,
    pub sixrd_prefix: Option<Ipv6Addr>,
    pub sixrd_prefix_len: u8,
    pub relay_prefix: Option<Ipv4Addr>,
    pub relay_prefix_len: u8,
    pub stats: TunnelStats,
}

impl IpipTunnel {
    pub fn new_ipip(id: u32, name: String, local: Ipv4Addr, remote: Ipv4Addr) -> Self {
        Self {
            id,
            name,
            mode: IpipMode::Ipv4InIpv4,
            sit_sub: None,
            local: IpAddr::V4(local),
            remote: IpAddr::V4(remote),
            ttl: 64,
            tos: 0,
            mtu: 1480,
            pmtu_discovery: true,
            dont_fragment: true,
            ecn_copy: true,
            sixrd_prefix: None,
            sixrd_prefix_len: 0,
            relay_prefix: None,
            relay_prefix_len: 0,
            stats: TunnelStats::new(),
        }
    }

    pub fn new_sit(
        id: u32,
        name: String,
        local: Ipv4Addr,
        remote: Ipv4Addr,
        sub: SitSubMode,
    ) -> Self {
        Self {
            id,
            name,
            mode: IpipMode::Ipv6InIpv4Sit,
            sit_sub: Some(sub),
            local: IpAddr::V4(local),
            remote: IpAddr::V4(remote),
            ttl: 64,
            tos: 0,
            mtu: 1480,
            pmtu_discovery: true,
            dont_fragment: true,
            ecn_copy: true,
            sixrd_prefix: None,
            sixrd_prefix_len: 0,
            relay_prefix: None,
            relay_prefix_len: 0,
            stats: TunnelStats::new(),
        }
    }

    pub fn new_ip6ip6(id: u32, name: String, local: Ipv6Addr, remote: Ipv6Addr) -> Self {
        Self {
            id,
            name,
            mode: IpipMode::Ipv6InIpv6,
            sit_sub: None,
            local: IpAddr::V6(local),
            remote: IpAddr::V6(remote),
            ttl: 64,
            tos: 0,
            mtu: 1440,
            pmtu_discovery: true,
            dont_fragment: true,
            ecn_copy: true,
            sixrd_prefix: None,
            sixrd_prefix_len: 0,
            relay_prefix: None,
            relay_prefix_len: 0,
            stats: TunnelStats::new(),
        }
    }

    pub fn new_ipv4_in_ipv6(id: u32, name: String, local: Ipv6Addr, remote: Ipv6Addr) -> Self {
        Self {
            id,
            name,
            mode: IpipMode::Ipv4InIpv6,
            sit_sub: None,
            local: IpAddr::V6(local),
            remote: IpAddr::V6(remote),
            ttl: 64,
            tos: 0,
            mtu: 1440,
            pmtu_discovery: true,
            dont_fragment: true,
            ecn_copy: true,
            sixrd_prefix: None,
            sixrd_prefix_len: 0,
            relay_prefix: None,
            relay_prefix_len: 0,
            stats: TunnelStats::new(),
        }
    }

    pub fn configure_6rd(
        &mut self,
        prefix: Ipv6Addr,
        prefix_len: u8,
        relay: Ipv4Addr,
        relay_len: u8,
    ) {
        self.sixrd_prefix = Some(prefix);
        self.sixrd_prefix_len = prefix_len;
        self.relay_prefix = Some(relay);
        self.relay_prefix_len = relay_len;
    }

    pub fn encapsulate(&mut self, inner: &[u8], buf: &mut Vec<u8>) -> Result<(), TunnelError> {
        if inner.len() > self.mtu as usize {
            if self.dont_fragment {
                return Err(TunnelError::MtuExceeded);
            }
        }
        match self.mode {
            IpipMode::Ipv4InIpv4 => {
                self.build_ipv4_outer(IpProtocol::Ipv4InIp as u8, inner, buf);
            }
            IpipMode::Ipv6InIpv4Sit => {
                self.build_ipv4_outer(IpProtocol::Ipv6InIp as u8, inner, buf);
            }
            IpipMode::Ipv6InIpv6 => {
                self.build_ipv6_outer(IpProtocol::Ipv6InIp as u8, inner, buf);
            }
            IpipMode::Ipv4InIpv6 => {
                self.build_ipv6_outer(IpProtocol::Ipv4InIp as u8, inner, buf);
            }
        }
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += buf.len() as u64;
        Ok(())
    }

    pub fn decapsulate(&mut self, packet: &[u8]) -> Result<Vec<u8>, TunnelError> {
        let header_len = match self.mode {
            IpipMode::Ipv4InIpv4 | IpipMode::Ipv6InIpv4Sit => {
                if packet.len() < 20 {
                    return Err(TunnelError::BufferTooShort);
                }
                let ihl = (packet[0] & 0x0F) as usize * 4;
                ihl
            }
            IpipMode::Ipv6InIpv6 | IpipMode::Ipv4InIpv6 => {
                if packet.len() < 40 {
                    return Err(TunnelError::BufferTooShort);
                }
                40
            }
        };
        if packet.len() <= header_len {
            return Err(TunnelError::BufferTooShort);
        }
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.len() as u64;
        Ok(packet[header_len..].to_vec())
    }

    fn build_ipv4_outer(&self, proto: u8, inner: &[u8], buf: &mut Vec<u8>) {
        let total_len = (20 + inner.len()) as u16;
        buf.push(0x45);
        buf.push(self.tos);
        buf.extend_from_slice(&total_len.to_be_bytes());
        buf.extend_from_slice(&[0, 0]);
        let flags_frag: u16 = if self.dont_fragment { 0x4000 } else { 0 };
        buf.extend_from_slice(&flags_frag.to_be_bytes());
        buf.push(self.ttl);
        buf.push(proto);
        buf.extend_from_slice(&[0, 0]);
        if let IpAddr::V4(addr) = self.local {
            buf.extend_from_slice(&addr.0);
        }
        if let IpAddr::V4(addr) = self.remote {
            buf.extend_from_slice(&addr.0);
        }
        buf.extend_from_slice(inner);
    }

    fn build_ipv6_outer(&self, next_header: u8, inner: &[u8], buf: &mut Vec<u8>) {
        let payload_len = inner.len() as u16;
        let ver_tc_fl: u32 = (6 << 28) | ((self.tos as u32) << 20);
        buf.extend_from_slice(&ver_tc_fl.to_be_bytes());
        buf.extend_from_slice(&payload_len.to_be_bytes());
        buf.push(next_header);
        buf.push(self.ttl);
        if let IpAddr::V6(addr) = self.local {
            buf.extend_from_slice(&addr.0);
        }
        if let IpAddr::V6(addr) = self.remote {
            buf.extend_from_slice(&addr.0);
        }
        buf.extend_from_slice(inner);
    }

    fn ecn_decapsulate(&self, outer_ecn: u8, inner_ecn: u8) -> u8 {
        if !self.ecn_copy {
            return inner_ecn;
        }
        if outer_ecn == 0x03 && inner_ecn != 0 {
            0x03
        } else {
            inner_ecn
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WireGuard kernel integration stubs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgMessageType {
    HandshakeInit = 1,
    HandshakeResponse = 2,
    CookieReply = 3,
    TransportData = 4,
}

#[derive(Debug, Clone)]
pub struct WgAllowedIp {
    pub addr: IpAddr,
    pub cidr: u8,
}

impl WgAllowedIp {
    pub fn contains(&self, ip: &IpAddr) -> bool {
        match (ip, &self.addr) {
            (IpAddr::V4(a), IpAddr::V4(b)) => {
                let mask = if self.cidr >= 32 {
                    0xFFFF_FFFFu32
                } else {
                    !((1u32 << (32 - self.cidr)) - 1)
                };
                let a_u32 = u32::from_be_bytes(a.0);
                let b_u32 = u32::from_be_bytes(b.0);
                (a_u32 & mask) == (b_u32 & mask)
            }
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                let full_bytes = (self.cidr / 8) as usize;
                let remaining_bits = self.cidr % 8;
                if a.0[..full_bytes] != b.0[..full_bytes] {
                    return false;
                }
                if remaining_bits > 0 && full_bytes < 16 {
                    let mask = !((1u8 << (8 - remaining_bits)) - 1);
                    return (a.0[full_bytes] & mask) == (b.0[full_bytes] & mask);
                }
                true
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgHandshakeState {
    None,
    InitSent,
    InitReceived,
    Established,
    Expired,
}

#[derive(Debug)]
pub struct WgPeer {
    pub public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    pub endpoint: Option<(IpAddr, u16)>,
    pub allowed_ips: Vec<WgAllowedIp>,
    pub persistent_keepalive: u16,
    pub last_handshake: u64,
    pub handshake_state: WgHandshakeState,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub sender_index: u32,
    pub receiver_index: u32,
    pub tx_key: [u8; 32],
    pub rx_key: [u8; 32],
    pub tx_nonce: u64,
    pub rx_nonce: u64,
    pub ephemeral_private: [u8; 32],
    pub chaining_key: [u8; 32],
    pub handshake_hash: [u8; 32],
}

#[derive(Debug)]
pub struct WgDevice {
    pub name: String,
    pub private_key: [u8; 32],
    pub listen_port: u16,
    pub fwmark: u32,
    pub peers: Vec<WgPeer>,
    pub mtu: u16,
    pub stats: TunnelStats,
}

impl WgDevice {
    pub fn new(name: String, private_key: [u8; 32], listen_port: u16) -> Self {
        Self {
            name,
            private_key,
            listen_port,
            fwmark: 0,
            peers: Vec::new(),
            mtu: 1420,
            stats: TunnelStats::new(),
        }
    }

    pub fn add_peer(&mut self, peer: WgPeer) {
        self.peers.push(peer);
    }

    pub fn remove_peer(&mut self, public_key: &[u8; 32]) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| &p.public_key != public_key);
        self.peers.len() < before
    }

    pub fn lookup_peer_by_allowed_ip(&self, ip: &IpAddr) -> Option<usize> {
        for (i, peer) in self.peers.iter().enumerate() {
            for aip in &peer.allowed_ips {
                if aip.contains(ip) {
                    return Some(i);
                }
            }
        }
        None
    }

    pub fn needs_handshake(&self, peer_idx: usize) -> bool {
        if peer_idx >= self.peers.len() {
            return false;
        }
        matches!(
            self.peers[peer_idx].handshake_state,
            WgHandshakeState::None | WgHandshakeState::Expired
        )
    }

    pub fn encrypt_transport(
        &mut self,
        peer_idx: usize,
        plaintext: &[u8],
        buf: &mut Vec<u8>,
    ) -> Result<(), TunnelError> {
        if peer_idx >= self.peers.len() {
            return Err(TunnelError::DeviceNotFound);
        }
        if self.needs_handshake(peer_idx) {
            return Err(TunnelError::HandshakeRequired);
        }
        let peer = &mut self.peers[peer_idx];
        let nonce_counter = peer.tx_nonce;
        peer.tx_nonce += 1;

        let nonce = WgCrypto::wg_nonce(nonce_counter);
        let aead = crate::crypto::ChaCha20Poly1305::new(&peer.tx_key);
        let mut ciphertext = vec![0u8; plaintext.len()];
        let mut tag = [0u8; 16];
        aead.encrypt(&nonce, &[], plaintext, &mut ciphertext, &mut tag)
            .map_err(|_| TunnelError::EncryptionFailed)?;

        buf.push(WgMessageType::TransportData as u8);
        buf.extend_from_slice(&[0, 0, 0]);
        buf.extend_from_slice(&peer.receiver_index.to_le_bytes());
        buf.extend_from_slice(&nonce_counter.to_le_bytes());
        buf.extend_from_slice(&ciphertext);
        buf.extend_from_slice(&tag);

        self.peers[peer_idx].tx_bytes += plaintext.len() as u64;
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += buf.len() as u64;
        Ok(())
    }

    pub fn decrypt_transport(&mut self, packet: &[u8]) -> Result<(usize, Vec<u8>), TunnelError> {
        if packet.len() < 32 {
            return Err(TunnelError::BufferTooShort);
        }
        let msg_type = packet[0];
        if msg_type != WgMessageType::TransportData as u8 {
            return Err(TunnelError::InvalidHeader);
        }
        let receiver_index = u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]);
        let nonce_counter = u64::from_le_bytes([
            packet[8], packet[9], packet[10], packet[11], packet[12], packet[13], packet[14],
            packet[15],
        ]);
        let peer_idx = self
            .peers
            .iter()
            .position(|p| p.sender_index == receiver_index)
            .ok_or(TunnelError::DeviceNotFound)?;

        if packet.len() < 32 {
            return Err(TunnelError::BufferTooShort);
        }
        let ct_end = packet.len() - 16;
        let ciphertext = &packet[16..ct_end];
        let tag: [u8; 16] = packet[ct_end..]
            .try_into()
            .map_err(|_| TunnelError::BufferTooShort)?;

        let nonce = WgCrypto::wg_nonce(nonce_counter);
        let aead = crate::crypto::ChaCha20Poly1305::new(&self.peers[peer_idx].rx_key);
        let mut plaintext = vec![0u8; ciphertext.len()];
        aead.decrypt(&nonce, &[], ciphertext, &tag, &mut plaintext)
            .map_err(|_| TunnelError::DecryptionFailed)?;

        self.peers[peer_idx].rx_bytes += plaintext.len() as u64;
        self.peers[peer_idx].rx_nonce = nonce_counter + 1;
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.len() as u64;
        Ok((peer_idx, plaintext))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WireGuard Noise IK Handshake & Crypto-backed Transport
// ─────────────────────────────────────────────────────────────────────────────

/// WireGuard construction identifier for Noise IK
const WG_CONSTRUCTION: &[u8] = b"Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
/// WireGuard identifier string
const WG_IDENTIFIER: &[u8] = b"WireGuard v1 zx2c4 Jason@zx2c4.com";
/// WireGuard listens on UDP port 51820
pub const WG_UDP_PORT: u16 = 51820;
/// Rekey after this many seconds (2 minutes)
const WG_REKEY_AFTER_TIME: u64 = 120;
/// Reject after this many seconds (3 minutes)
const WG_REJECT_AFTER_TIME: u64 = 180;
/// Rekey after this many messages
const WG_REKEY_AFTER_MESSAGES: u64 = 0xFFFF_FFFF_FFFF_FF00;

pub struct WgCrypto;

impl WgCrypto {
    pub fn hmac_sha256(key: &[u8], input: &[u8]) -> [u8; 32] {
        let hmac = crate::crypto::HmacContext::new_sha256(key);
        let mut out = [0u8; 32];
        hmac.compute(input, &mut out);
        out
    }

    pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
        Self::hmac_sha256(salt, ikm)
    }

    pub fn hkdf_expand_2(prk: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
        let t1 = Self::hmac_sha256(prk, &[0x01]);
        let mut input = [0u8; 33];
        input[..32].copy_from_slice(&t1);
        input[32] = 0x02;
        let t2 = Self::hmac_sha256(prk, &input);
        (t1, t2)
    }

    pub fn hkdf_expand_3(prk: &[u8; 32]) -> ([u8; 32], [u8; 32], [u8; 32]) {
        let t1 = Self::hmac_sha256(prk, &[0x01]);
        let mut input2 = [0u8; 33];
        input2[..32].copy_from_slice(&t1);
        input2[32] = 0x02;
        let t2 = Self::hmac_sha256(prk, &input2);
        let mut input3 = [0u8; 33];
        input3[..32].copy_from_slice(&t2);
        input3[32] = 0x03;
        let t3 = Self::hmac_sha256(prk, &input3);
        (t1, t2, t3)
    }

    pub fn noise_hash(data1: &[u8], data2: &[u8]) -> [u8; 32] {
        use crate::crypto::HashAlgorithm;
        let mut sha = crate::crypto::Sha256Context::new();
        sha.init();
        sha.update(data1);
        sha.update(data2);
        let mut out = [0u8; 32];
        sha.finalize(&mut out);
        out
    }

    /// Build WireGuard 12-byte nonce from 8-byte counter
    pub fn wg_nonce(counter: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[4..12].copy_from_slice(&counter.to_le_bytes());
        nonce
    }

    /// MAC using HMAC-SHA256 truncated to 16 bytes (BLAKE2s stub)
    pub fn mac(key: &[u8], data: &[u8]) -> [u8; 16] {
        let full = Self::hmac_sha256(key, data);
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        out
    }

    pub fn mix_key(ck: &[u8; 32], dh_output: &[u8]) -> ([u8; 32], [u8; 32]) {
        let prk = Self::hkdf_extract(ck, dh_output);
        Self::hkdf_expand_2(&prk)
    }

    pub fn mix_hash(h: &[u8; 32], data: &[u8]) -> [u8; 32] {
        Self::noise_hash(h, data)
    }

    pub fn dh(private: &[u8; 32], public: &[u8; 32]) -> [u8; 32] {
        let ctx = crate::crypto::X25519Context::with_private_key(*private);
        let mut out = [0u8; 32];
        let _ = ctx.compute_shared_secret(public, &mut out);
        out
    }

    pub fn aead_encrypt(
        key: &[u8; 32],
        counter: u64,
        aad: &[u8],
        plaintext: &[u8],
    ) -> (Vec<u8>, [u8; 16]) {
        let nonce = Self::wg_nonce(counter);
        let aead = crate::crypto::ChaCha20Poly1305::new(key);
        let mut ciphertext = vec![0u8; plaintext.len()];
        let mut tag = [0u8; 16];
        let _ = aead.encrypt(&nonce, aad, plaintext, &mut ciphertext, &mut tag);
        (ciphertext, tag)
    }

    pub fn aead_decrypt(
        key: &[u8; 32],
        counter: u64,
        aad: &[u8],
        ciphertext: &[u8],
        tag: &[u8; 16],
    ) -> Result<Vec<u8>, TunnelError> {
        let nonce = Self::wg_nonce(counter);
        let aead = crate::crypto::ChaCha20Poly1305::new(key);
        let mut plaintext = vec![0u8; ciphertext.len()];
        aead.decrypt(&nonce, aad, ciphertext, tag, &mut plaintext)
            .map_err(|_| TunnelError::DecryptionFailed)?;
        Ok(plaintext)
    }

    fn initial_chain_key() -> [u8; 32] {
        Self::noise_hash(WG_CONSTRUCTION, &[])
    }

    fn initial_hash(chain_key: &[u8; 32]) -> [u8; 32] {
        Self::mix_hash(chain_key, WG_IDENTIFIER)
    }
}

impl WgDevice {
    /// Compute our public key from the private key via X25519 base-point multiplication.
    pub fn public_key(&self) -> [u8; 32] {
        let ctx = crate::crypto::X25519Context::with_private_key(self.private_key);
        *ctx.public_key_bytes()
    }

    /// Build a Type 1 Handshake Initiation message (Noise IK pattern, message A).
    pub fn create_handshake_init(
        &mut self,
        peer_idx: usize,
        timestamp: u64,
    ) -> Result<Vec<u8>, TunnelError> {
        if peer_idx >= self.peers.len() {
            return Err(TunnelError::DeviceNotFound);
        }

        let ck = WgCrypto::initial_chain_key();
        let h = WgCrypto::initial_hash(&ck);
        let h = WgCrypto::mix_hash(&h, &self.peers[peer_idx].public_key);

        let mut eph_ctx = crate::crypto::X25519Context::new();
        let _ = eph_ctx.generate_keypair();
        let eph_pub = *eph_ctx.public_key_bytes();

        let (ck, _) = WgCrypto::mix_key(&ck, &eph_pub);
        let h = WgCrypto::mix_hash(&h, &eph_pub);

        let dh_result = WgCrypto::dh(
            &self.peers[peer_idx].ephemeral_private,
            &self.peers[peer_idx].public_key,
        );
        let (ck, key) = WgCrypto::mix_key(&ck, &dh_result);

        let my_pub = self.public_key();
        let (enc_static, tag_s) = WgCrypto::aead_encrypt(&key, 0, &h, &my_pub);
        let mut h = WgCrypto::mix_hash(&h, &enc_static);
        h = WgCrypto::mix_hash(&h, &tag_s);

        let dh_ss = WgCrypto::dh(&self.private_key, &self.peers[peer_idx].public_key);
        let (ck, key) = WgCrypto::mix_key(&ck, &dh_ss);

        let ts_bytes = timestamp.to_le_bytes();
        let (enc_ts, tag_t) = WgCrypto::aead_encrypt(&key, 0, &h, &ts_bytes);
        let mut h = WgCrypto::mix_hash(&h, &enc_ts);
        h = WgCrypto::mix_hash(&h, &tag_t);

        self.peers[peer_idx].chaining_key = ck;
        self.peers[peer_idx].handshake_hash = h;
        self.peers[peer_idx].ephemeral_private = [0u8; 32]; // stored in eph_ctx
        self.peers[peer_idx].handshake_state = WgHandshakeState::InitSent;

        let sender_index = (peer_idx as u32).wrapping_add(1);
        self.peers[peer_idx].sender_index = sender_index;

        let mut msg = Vec::with_capacity(148);
        msg.push(WgMessageType::HandshakeInit as u8);
        msg.extend_from_slice(&[0, 0, 0]);
        msg.extend_from_slice(&sender_index.to_le_bytes());
        msg.extend_from_slice(&eph_pub);
        msg.extend_from_slice(&enc_static);
        msg.extend_from_slice(&tag_s);
        msg.extend_from_slice(&enc_ts);
        msg.extend_from_slice(&tag_t);
        let mac1 = WgCrypto::mac(&self.peers[peer_idx].public_key, &msg);
        msg.extend_from_slice(&mac1);
        msg.extend_from_slice(&[0u8; 16]); // mac2 (cookie, zeroed for now)

        self.stats.tx_packets += 1;
        self.stats.tx_bytes += msg.len() as u64;
        Ok(msg)
    }

    /// Process a Type 1 Handshake Initiation from a peer.
    pub fn process_handshake_init(&mut self, packet: &[u8]) -> Result<usize, TunnelError> {
        if packet.len() < 148 {
            return Err(TunnelError::BufferTooShort);
        }
        if packet[0] != WgMessageType::HandshakeInit as u8 {
            return Err(TunnelError::InvalidHeader);
        }
        let sender_index = u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]);
        let eph_pub: [u8; 32] = packet[8..40]
            .try_into()
            .map_err(|_| TunnelError::InvalidHeader)?;

        let ck = WgCrypto::initial_chain_key();
        let my_pub = self.public_key();
        let h = WgCrypto::initial_hash(&ck);
        let h = WgCrypto::mix_hash(&h, &my_pub);

        let (ck, _) = WgCrypto::mix_key(&ck, &eph_pub);
        let h = WgCrypto::mix_hash(&h, &eph_pub);

        let dh_result = WgCrypto::dh(&self.private_key, &eph_pub);
        let (ck, key) = WgCrypto::mix_key(&ck, &dh_result);

        let enc_static = &packet[40..72];
        let tag_s: [u8; 16] = packet[72..88]
            .try_into()
            .map_err(|_| TunnelError::InvalidHeader)?;
        let peer_static = WgCrypto::aead_decrypt(&key, 0, &h, enc_static, &tag_s)?;

        let mut peer_pub = [0u8; 32];
        if peer_static.len() >= 32 {
            peer_pub.copy_from_slice(&peer_static[..32]);
        }

        let peer_idx = self
            .peers
            .iter()
            .position(|p| p.public_key == peer_pub)
            .ok_or(TunnelError::DeviceNotFound)?;

        let mut h = WgCrypto::mix_hash(&h, enc_static);
        h = WgCrypto::mix_hash(&h, &tag_s);

        let dh_ss = WgCrypto::dh(&self.private_key, &peer_pub);
        let (ck, key) = WgCrypto::mix_key(&ck, &dh_ss);

        let enc_ts = &packet[88..96];
        let tag_t: [u8; 16] = packet[96..112]
            .try_into()
            .map_err(|_| TunnelError::InvalidHeader)?;
        let _timestamp = WgCrypto::aead_decrypt(&key, 0, &h, enc_ts, &tag_t)?;

        let mut h = WgCrypto::mix_hash(&h, enc_ts);
        h = WgCrypto::mix_hash(&h, &tag_t);

        self.peers[peer_idx].receiver_index = sender_index;
        self.peers[peer_idx].chaining_key = ck;
        self.peers[peer_idx].handshake_hash = h;
        self.peers[peer_idx].handshake_state = WgHandshakeState::InitReceived;

        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.len() as u64;
        Ok(peer_idx)
    }

    /// Build a Type 2 Handshake Response (Noise IK pattern, message B).
    pub fn create_handshake_response(&mut self, peer_idx: usize) -> Result<Vec<u8>, TunnelError> {
        if peer_idx >= self.peers.len() {
            return Err(TunnelError::DeviceNotFound);
        }
        let peer = &self.peers[peer_idx];
        if peer.handshake_state != WgHandshakeState::InitReceived {
            return Err(TunnelError::InvalidState);
        }

        let ck = peer.chaining_key;
        let h = peer.handshake_hash;

        let mut eph_ctx = crate::crypto::X25519Context::new();
        let _ = eph_ctx.generate_keypair();
        let eph_pub = *eph_ctx.public_key_bytes();

        let (ck, _) = WgCrypto::mix_key(&ck, &eph_pub);
        let h = WgCrypto::mix_hash(&h, &eph_pub);

        let dh_ee = WgCrypto::dh(&[0u8; 32], &self.peers[peer_idx].public_key);
        let (ck, _) = WgCrypto::mix_key(&ck, &dh_ee);
        let dh_se = WgCrypto::dh(&self.private_key, &self.peers[peer_idx].public_key);
        let (ck, _) = WgCrypto::mix_key(&ck, &dh_se);

        let psk = self.peers[peer_idx].preshared_key.unwrap_or([0u8; 32]);
        let (ck, temp, key) = WgCrypto::hkdf_expand_3(&WgCrypto::hkdf_extract(&ck, &psk));
        let h = WgCrypto::mix_hash(&h, &temp);

        let (enc_empty, tag) = WgCrypto::aead_encrypt(&key, 0, &h, &[]);
        let _ = enc_empty;
        let h = WgCrypto::mix_hash(&h, &tag);

        let (tx_key, rx_key) = WgCrypto::hkdf_expand_2(&ck);

        let sender_index = (peer_idx as u32).wrapping_add(0x1000);
        self.peers[peer_idx].sender_index = sender_index;
        self.peers[peer_idx].tx_key = tx_key;
        self.peers[peer_idx].rx_key = rx_key;
        self.peers[peer_idx].tx_nonce = 0;
        self.peers[peer_idx].rx_nonce = 0;
        self.peers[peer_idx].chaining_key = ck;
        self.peers[peer_idx].handshake_hash = h;
        self.peers[peer_idx].handshake_state = WgHandshakeState::Established;

        let mut msg = Vec::with_capacity(92);
        msg.push(WgMessageType::HandshakeResponse as u8);
        msg.extend_from_slice(&[0, 0, 0]);
        msg.extend_from_slice(&sender_index.to_le_bytes());
        msg.extend_from_slice(&self.peers[peer_idx].receiver_index.to_le_bytes());
        msg.extend_from_slice(&eph_pub);
        msg.extend_from_slice(&tag);
        let mac1 = WgCrypto::mac(&self.peers[peer_idx].public_key, &msg);
        msg.extend_from_slice(&mac1);
        msg.extend_from_slice(&[0u8; 16]); // mac2

        self.stats.tx_packets += 1;
        self.stats.tx_bytes += msg.len() as u64;
        Ok(msg)
    }

    /// Process a Type 2 Handshake Response.
    pub fn process_handshake_response(&mut self, packet: &[u8]) -> Result<usize, TunnelError> {
        if packet.len() < 92 {
            return Err(TunnelError::BufferTooShort);
        }
        if packet[0] != WgMessageType::HandshakeResponse as u8 {
            return Err(TunnelError::InvalidHeader);
        }

        let sender_index = u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]);
        let receiver_index = u32::from_le_bytes([packet[8], packet[9], packet[10], packet[11]]);
        let eph_pub: [u8; 32] = packet[12..44]
            .try_into()
            .map_err(|_| TunnelError::InvalidHeader)?;

        let peer_idx = self
            .peers
            .iter()
            .position(|p| p.sender_index == receiver_index)
            .ok_or(TunnelError::DeviceNotFound)?;

        let peer = &self.peers[peer_idx];
        if peer.handshake_state != WgHandshakeState::InitSent {
            return Err(TunnelError::InvalidState);
        }

        let ck = peer.chaining_key;
        let h = peer.handshake_hash;

        let (ck, _) = WgCrypto::mix_key(&ck, &eph_pub);
        let h = WgCrypto::mix_hash(&h, &eph_pub);

        let dh_ee = WgCrypto::dh(&self.peers[peer_idx].ephemeral_private, &eph_pub);
        let (ck, _) = WgCrypto::mix_key(&ck, &dh_ee);
        let dh_se = WgCrypto::dh(&self.private_key, &eph_pub);
        let (ck, _) = WgCrypto::mix_key(&ck, &dh_se);

        let psk = self.peers[peer_idx].preshared_key.unwrap_or([0u8; 32]);
        let (ck, temp, key) = WgCrypto::hkdf_expand_3(&WgCrypto::hkdf_extract(&ck, &psk));
        let h = WgCrypto::mix_hash(&h, &temp);

        let tag: [u8; 16] = packet[44..60]
            .try_into()
            .map_err(|_| TunnelError::InvalidHeader)?;
        let _empty = WgCrypto::aead_decrypt(&key, 0, &h, &[], &tag)?;
        let h = WgCrypto::mix_hash(&h, &tag);

        let (rx_key, tx_key) = WgCrypto::hkdf_expand_2(&ck);

        self.peers[peer_idx].receiver_index = sender_index;
        self.peers[peer_idx].tx_key = tx_key;
        self.peers[peer_idx].rx_key = rx_key;
        self.peers[peer_idx].tx_nonce = 0;
        self.peers[peer_idx].rx_nonce = 0;
        self.peers[peer_idx].chaining_key = ck;
        self.peers[peer_idx].handshake_hash = h;
        self.peers[peer_idx].handshake_state = WgHandshakeState::Established;

        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.len() as u64;
        Ok(peer_idx)
    }

    /// Build a Type 4 Cookie Reply for DoS mitigation.
    pub fn create_cookie_reply(&self, receiver_index: u32, mac1: &[u8; 16]) -> Vec<u8> {
        let cookie = WgCrypto::hmac_sha256(&self.private_key, mac1);
        let mut msg = Vec::with_capacity(64);
        msg.push(WgMessageType::CookieReply as u8);
        msg.extend_from_slice(&[0, 0, 0]);
        msg.extend_from_slice(&receiver_index.to_le_bytes());
        msg.extend_from_slice(&[0u8; 24]); // random nonce placeholder
        msg.extend_from_slice(&cookie[..16]);
        msg.extend_from_slice(&[0u8; 16]); // encrypted cookie tag
        msg
    }

    /// Check if handshake has expired and needs rekeying.
    pub fn needs_rekey(&self, peer_idx: usize, now: u64) -> bool {
        if peer_idx >= self.peers.len() {
            return false;
        }
        let peer = &self.peers[peer_idx];
        if peer.handshake_state != WgHandshakeState::Established {
            return true;
        }
        if now.saturating_sub(peer.last_handshake) > WG_REKEY_AFTER_TIME {
            return true;
        }
        peer.tx_nonce >= WG_REKEY_AFTER_MESSAGES
    }
}

/// A virtual WireGuard network interface that encrypts/decrypts IP packets
/// and routes them through UDP to configured peers.
#[derive(Debug)]
pub struct WireGuardInterface {
    pub device: WgDevice,
    pub listen_port: u16,
    pub rx_queue: Vec<Vec<u8>>,
    pub tx_queue: Vec<Vec<u8>>,
    pub rx_queue_capacity: usize,
}

impl WireGuardInterface {
    pub fn new(name: &str, private_key: [u8; 32], listen_port: u16) -> Self {
        Self {
            device: WgDevice::new(alloc::string::String::from(name), private_key, listen_port),
            listen_port,
            rx_queue: Vec::new(),
            tx_queue: Vec::new(),
            rx_queue_capacity: 256,
        }
    }

    /// Send an IP packet through the tunnel. Looks up the destination IP
    /// in the allowed-IPs table, encrypts via ChaCha20-Poly1305, and
    /// enqueues the encrypted UDP payload for transmission.
    pub fn send_packet(&mut self, ip_packet: &[u8]) -> Result<(), TunnelError> {
        if ip_packet.len() < 20 {
            return Err(TunnelError::BufferTooShort);
        }
        let dst_ip = Ipv4Addr([ip_packet[16], ip_packet[17], ip_packet[18], ip_packet[19]]);
        let ip_addr = IpAddr::V4(dst_ip);
        let peer_idx = self
            .device
            .lookup_peer_by_allowed_ip(&ip_addr)
            .ok_or(TunnelError::NoRoute)?;

        if self.device.needs_handshake(peer_idx) {
            return Err(TunnelError::HandshakeRequired);
        }

        let mut buf = Vec::new();
        self.device
            .encrypt_transport(peer_idx, ip_packet, &mut buf)?;

        if self.tx_queue.len() >= self.rx_queue_capacity {
            return Err(TunnelError::QueueFull);
        }
        self.tx_queue.push(buf);
        Ok(())
    }

    /// Process an incoming encrypted WireGuard packet from UDP.
    /// Returns the decrypted IP packet if successful.
    pub fn receive_packet(&mut self, udp_payload: &[u8]) -> Result<Vec<u8>, TunnelError> {
        if udp_payload.is_empty() {
            return Err(TunnelError::BufferTooShort);
        }
        match udp_payload[0] {
            1 => {
                let peer_idx = self.device.process_handshake_init(udp_payload)?;
                let response = self.device.create_handshake_response(peer_idx)?;
                self.tx_queue.push(response);
                Err(TunnelError::HandshakeRequired)
            }
            2 => {
                let _peer_idx = self.device.process_handshake_response(udp_payload)?;
                Err(TunnelError::HandshakeRequired)
            }
            4 => {
                let (_peer_idx, plaintext) = self.device.decrypt_transport(udp_payload)?;
                if self.rx_queue.len() < self.rx_queue_capacity {
                    self.rx_queue.push(plaintext.clone());
                }
                Ok(plaintext)
            }
            3 => Err(TunnelError::HandshakeRequired),
            _ => Err(TunnelError::InvalidHeader),
        }
    }

    /// Dequeue a decrypted IP packet from the receive queue.
    pub fn read_decrypted(&mut self) -> Option<Vec<u8>> {
        if self.rx_queue.is_empty() {
            None
        } else {
            Some(self.rx_queue.remove(0))
        }
    }

    /// Dequeue an encrypted packet ready to be sent as a UDP payload.
    pub fn read_encrypted(&mut self) -> Option<Vec<u8>> {
        if self.tx_queue.is_empty() {
            None
        } else {
            Some(self.tx_queue.remove(0))
        }
    }

    pub fn add_peer(&mut self, peer: WgPeer) {
        self.device.add_peer(peer);
    }

    pub fn peer_count(&self) -> usize {
        self.device.peers.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// L2TP — Layer 2 Tunneling Protocol v2/v3
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2tpVersion {
    V2,
    V3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2tpEncap {
    Udp,
    Ip,
}

#[derive(Debug, Clone)]
pub struct L2tpSession {
    pub session_id: u32,
    pub peer_session_id: u32,
    pub tunnel_id: u32,
    pub cookie: Option<Vec<u8>>,
    pub peer_cookie: Option<Vec<u8>>,
    pub is_data: bool,
    pub pseudowire_type: u16,
    pub mtu: u16,
    pub stats: TunnelStats,
}

impl L2tpSession {
    pub fn validate_cookie(&self, received: &[u8]) -> bool {
        match &self.peer_cookie {
            // Constant-time: `received` is attacker-suppliable from the network
            // and `expected` is the shared secret cookie — a `==` short-circuit
            // would leak, byte by byte via timing, how much of the cookie was
            // guessed, enabling recovery + session forgery.
            Some(expected) => crate::crypto::ct_eq(received, expected.as_slice()),
            None => true,
        }
    }
}

#[derive(Debug)]
pub struct L2tpTunnel {
    pub id: u32,
    pub version: L2tpVersion,
    pub encap: L2tpEncap,
    pub local: IpAddr,
    pub remote: IpAddr,
    pub local_port: u16,
    pub remote_port: u16,
    pub sessions: BTreeMap<u32, L2tpSession>,
    pub stats: TunnelStats,
}

impl L2tpTunnel {
    pub fn new_v2(id: u32, local: IpAddr, remote: IpAddr) -> Self {
        Self {
            id,
            version: L2tpVersion::V2,
            encap: L2tpEncap::Udp,
            local,
            remote,
            local_port: 1701,
            remote_port: 1701,
            sessions: BTreeMap::new(),
            stats: TunnelStats::new(),
        }
    }

    pub fn new_v3_udp(id: u32, local: IpAddr, remote: IpAddr) -> Self {
        Self {
            id,
            version: L2tpVersion::V3,
            encap: L2tpEncap::Udp,
            local,
            remote,
            local_port: 1701,
            remote_port: 1701,
            sessions: BTreeMap::new(),
            stats: TunnelStats::new(),
        }
    }

    pub fn new_v3_ip(id: u32, local: IpAddr, remote: IpAddr) -> Self {
        Self {
            id,
            version: L2tpVersion::V3,
            encap: L2tpEncap::Ip,
            local,
            remote,
            local_port: 0,
            remote_port: 0,
            sessions: BTreeMap::new(),
            stats: TunnelStats::new(),
        }
    }

    pub fn add_session(&mut self, session: L2tpSession) {
        self.sessions.insert(session.session_id, session);
    }

    pub fn remove_session(&mut self, session_id: u32) -> Option<L2tpSession> {
        self.sessions.remove(&session_id)
    }

    pub fn encapsulate_data(
        &mut self,
        session_id: u32,
        payload: &[u8],
        buf: &mut Vec<u8>,
    ) -> Result<(), TunnelError> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or(TunnelError::SessionNotFound)?;
        match self.version {
            L2tpVersion::V2 => {
                let flags: u16 = 0x0002;
                buf.extend_from_slice(&flags.to_be_bytes());
                let len = (12 + payload.len()) as u16;
                buf.extend_from_slice(&len.to_be_bytes());
                buf.extend_from_slice(&(self.id as u16).to_be_bytes());
                buf.extend_from_slice(&(session.session_id as u16).to_be_bytes());
            }
            L2tpVersion::V3 => {
                buf.extend_from_slice(&session.peer_session_id.to_be_bytes());
                if let Some(ref cookie) = session.cookie {
                    buf.extend_from_slice(cookie);
                }
            }
        }
        buf.extend_from_slice(payload);
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += buf.len() as u64;
        Ok(())
    }

    pub fn decapsulate_data(&mut self, packet: &[u8]) -> Result<(u32, Vec<u8>), TunnelError> {
        match self.version {
            L2tpVersion::V2 => {
                if packet.len() < 8 {
                    return Err(TunnelError::BufferTooShort);
                }
                let session_id = ((packet[4] as u32) << 8) | packet[5] as u32;
                let session = self
                    .sessions
                    .get(&session_id)
                    .ok_or(TunnelError::SessionNotFound)?;
                let _ = session;
                self.stats.rx_packets += 1;
                self.stats.rx_bytes += packet.len() as u64;
                Ok((session_id, packet[8..].to_vec()))
            }
            L2tpVersion::V3 => {
                if packet.len() < 4 {
                    return Err(TunnelError::BufferTooShort);
                }
                let session_id = u32::from_be_bytes([packet[0], packet[1], packet[2], packet[3]]);
                let session = self
                    .sessions
                    .get(&session_id)
                    .ok_or(TunnelError::SessionNotFound)?;
                let cookie_len = session.peer_cookie.as_ref().map_or(0, |c| c.len());
                if packet.len() < 4 + cookie_len {
                    return Err(TunnelError::BufferTooShort);
                }
                if cookie_len > 0 {
                    if !session.validate_cookie(&packet[4..4 + cookie_len]) {
                        return Err(TunnelError::CookieInvalid);
                    }
                }
                let offset = 4 + cookie_len;
                self.stats.rx_packets += 1;
                self.stats.rx_bytes += packet.len() as u64;
                Ok((session_id, packet[offset..].to_vec()))
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PPPoE — PPP over Ethernet (RFC 2516)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PppoeCode {
    Padi = 0x09,
    Pado = 0x07,
    Padr = 0x19,
    Pads = 0x65,
    Padt = 0xA7,
    Session = 0x00,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PppoeState {
    Initial,
    PadiSent,
    PadoReceived,
    PadrSent,
    SessionActive,
    Terminated,
}

#[derive(Debug, Clone)]
pub struct PppoeTag {
    pub tag_type: u16,
    pub value: Vec<u8>,
}

impl PppoeTag {
    pub const TAG_END_OF_LIST: u16 = 0x0000;
    pub const TAG_SERVICE_NAME: u16 = 0x0101;
    pub const TAG_AC_NAME: u16 = 0x0102;
    pub const TAG_HOST_UNIQ: u16 = 0x0103;
    pub const TAG_AC_COOKIE: u16 = 0x0104;
    pub const TAG_RELAY_SESSION_ID: u16 = 0x0110;
    pub const TAG_SERVICE_NAME_ERROR: u16 = 0x0201;
    pub const TAG_AC_SYSTEM_ERROR: u16 = 0x0202;
    pub const TAG_GENERIC_ERROR: u16 = 0x0203;
}

#[derive(Debug)]
pub struct PppoeSession {
    pub session_id: u16,
    pub state: PppoeState,
    pub peer_mac: MacAddr,
    pub local_mac: MacAddr,
    pub service_name: Vec<u8>,
    pub ac_name: Vec<u8>,
    pub ac_cookie: Option<Vec<u8>>,
    pub host_uniq: Option<Vec<u8>>,
    pub mtu: u16,
    pub mru: u16,
    pub stats: TunnelStats,
}

impl PppoeSession {
    pub fn new(local_mac: MacAddr) -> Self {
        Self {
            session_id: 0,
            state: PppoeState::Initial,
            peer_mac: MacAddr::ZERO,
            local_mac,
            service_name: Vec::new(),
            ac_name: Vec::new(),
            ac_cookie: None,
            host_uniq: None,
            mtu: 1492,
            mru: 1492,
            stats: TunnelStats::new(),
        }
    }

    pub fn build_padi(&self, service_name: &[u8], host_uniq: &[u8]) -> Vec<u8> {
        let mut pkt = Vec::new();
        pkt.push(0x11);
        pkt.push(PppoeCode::Padi as u8);
        pkt.extend_from_slice(&0u16.to_be_bytes());
        let tags_start = pkt.len();
        pkt.extend_from_slice(&PppoeTag::TAG_SERVICE_NAME.to_be_bytes());
        pkt.extend_from_slice(&(service_name.len() as u16).to_be_bytes());
        pkt.extend_from_slice(service_name);
        pkt.extend_from_slice(&PppoeTag::TAG_HOST_UNIQ.to_be_bytes());
        pkt.extend_from_slice(&(host_uniq.len() as u16).to_be_bytes());
        pkt.extend_from_slice(host_uniq);
        let payload_len = (pkt.len() - tags_start) as u16;
        pkt[2] = (payload_len >> 8) as u8;
        pkt[3] = payload_len as u8;
        pkt
    }

    pub fn build_padr(&self) -> Vec<u8> {
        let mut pkt = Vec::new();
        pkt.push(0x11);
        pkt.push(PppoeCode::Padr as u8);
        pkt.extend_from_slice(&0u16.to_be_bytes());
        let tags_start = pkt.len();
        pkt.extend_from_slice(&PppoeTag::TAG_SERVICE_NAME.to_be_bytes());
        pkt.extend_from_slice(&(self.service_name.len() as u16).to_be_bytes());
        pkt.extend_from_slice(&self.service_name);
        if let Some(ref cookie) = self.ac_cookie {
            pkt.extend_from_slice(&PppoeTag::TAG_AC_COOKIE.to_be_bytes());
            pkt.extend_from_slice(&(cookie.len() as u16).to_be_bytes());
            pkt.extend_from_slice(cookie);
        }
        if let Some(ref uniq) = self.host_uniq {
            pkt.extend_from_slice(&PppoeTag::TAG_HOST_UNIQ.to_be_bytes());
            pkt.extend_from_slice(&(uniq.len() as u16).to_be_bytes());
            pkt.extend_from_slice(uniq);
        }
        let payload_len = (pkt.len() - tags_start) as u16;
        pkt[2] = (payload_len >> 8) as u8;
        pkt[3] = payload_len as u8;
        pkt
    }

    pub fn build_padt(&self) -> Vec<u8> {
        let mut pkt = Vec::new();
        pkt.push(0x11);
        pkt.push(PppoeCode::Padt as u8);
        pkt.extend_from_slice(&self.session_id.to_be_bytes());
        pkt.extend_from_slice(&0u16.to_be_bytes());
        pkt
    }

    pub fn encapsulate_ppp(&self, protocol: u16, payload: &[u8]) -> Result<Vec<u8>, TunnelError> {
        if self.state != PppoeState::SessionActive {
            return Err(TunnelError::InvalidState);
        }
        if payload.len() > self.mtu as usize - 2 {
            return Err(TunnelError::MtuExceeded);
        }
        let mut pkt = Vec::new();
        pkt.push(0x11);
        pkt.push(PppoeCode::Session as u8);
        pkt.extend_from_slice(&self.session_id.to_be_bytes());
        let ppp_len = (2 + payload.len()) as u16;
        pkt.extend_from_slice(&ppp_len.to_be_bytes());
        pkt.extend_from_slice(&protocol.to_be_bytes());
        pkt.extend_from_slice(payload);
        Ok(pkt)
    }

    pub fn process_pado(&mut self, packet: &[u8]) -> Result<(), TunnelError> {
        if self.state != PppoeState::PadiSent {
            return Err(TunnelError::InvalidState);
        }
        if packet.len() < 6 {
            return Err(TunnelError::BufferTooShort);
        }
        self.parse_discovery_tags(&packet[6..]);
        self.state = PppoeState::PadoReceived;
        Ok(())
    }

    pub fn process_pads(&mut self, packet: &[u8]) -> Result<(), TunnelError> {
        if self.state != PppoeState::PadrSent {
            return Err(TunnelError::InvalidState);
        }
        if packet.len() < 4 {
            return Err(TunnelError::BufferTooShort);
        }
        self.session_id = ((packet[2] as u16) << 8) | packet[3] as u16;
        if self.session_id == 0 {
            return Err(TunnelError::InvalidState);
        }
        self.state = PppoeState::SessionActive;
        Ok(())
    }

    fn parse_discovery_tags(&mut self, data: &[u8]) {
        let mut offset = 0;
        while offset + 4 <= data.len() {
            let tag_type = ((data[offset] as u16) << 8) | data[offset + 1] as u16;
            let tag_len = ((data[offset + 2] as u16) << 8) | data[offset + 3] as u16;
            offset += 4;
            if offset + tag_len as usize > data.len() {
                break;
            }
            let value = &data[offset..offset + tag_len as usize];
            match tag_type {
                PppoeTag::TAG_SERVICE_NAME => {
                    self.service_name = value.to_vec();
                }
                PppoeTag::TAG_AC_NAME => {
                    self.ac_name = value.to_vec();
                }
                PppoeTag::TAG_AC_COOKIE => {
                    self.ac_cookie = Some(value.to_vec());
                }
                PppoeTag::TAG_HOST_UNIQ => {
                    self.host_uniq = Some(value.to_vec());
                }
                PppoeTag::TAG_END_OF_LIST => break,
                _ => {}
            }
            offset += tag_len as usize;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TUN/TAP — virtual network devices
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunTapMode {
    Tun,
    Tap,
}

bitflags_manual! {
    pub struct TunFlags: u16 {
        const IFF_TUN       = 0x0001;
        const IFF_TAP       = 0x0002;
        const IFF_NO_PI     = 0x1000;
        const IFF_VNET_HDR  = 0x4000;
        const IFF_MULTI_QUEUE = 0x0100;
    }
}

mod bitflags_manual {
    macro_rules! bitflags_manual {
        (pub struct $Name:ident : $T:ty { $(const $Flag:ident = $value:expr;)* }) => {
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct $Name(pub $T);
            impl $Name {
                $(pub const $Flag: Self = Self($value);)*

                pub fn contains(self, other: Self) -> bool {
                    (self.0 & other.0) == other.0
                }
                pub fn insert(&mut self, other: Self) {
                    self.0 |= other.0;
                }
                pub fn remove(&mut self, other: Self) {
                    self.0 &= !other.0;
                }
                pub fn bits(self) -> $T {
                    self.0
                }
            }
            impl core::ops::BitOr for $Name {
                type Output = Self;
                fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
            }
            impl core::ops::BitAnd for $Name {
                type Output = Self;
                fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) }
            }
        };
    }
    pub(crate) use bitflags_manual;
}
use bitflags_manual::bitflags_manual;

#[derive(Debug)]
pub struct TunTapDevice {
    pub name: String,
    pub mode: TunTapMode,
    pub flags: TunFlags,
    pub fd: i32,
    pub queues: u16,
    pub mtu: u16,
    pub persistent: bool,
    pub owner_uid: u32,
    pub group_gid: u32,
    pub hw_addr: MacAddr,
    pub rx_ring: Vec<Vec<u8>>,
    pub tx_ring: Vec<Vec<u8>>,
    pub rx_ring_capacity: usize,
    pub stats: TunnelStats,
}

impl TunTapDevice {
    pub fn new_tun(name: String) -> Self {
        Self {
            name,
            mode: TunTapMode::Tun,
            flags: TunFlags::IFF_TUN | TunFlags::IFF_NO_PI,
            fd: -1,
            queues: 1,
            mtu: 1500,
            persistent: false,
            owner_uid: 0,
            group_gid: 0,
            hw_addr: MacAddr::ZERO,
            rx_ring: Vec::new(),
            tx_ring: Vec::new(),
            rx_ring_capacity: 256,
            stats: TunnelStats::new(),
        }
    }

    pub fn new_tap(name: String) -> Self {
        Self {
            name,
            mode: TunTapMode::Tap,
            flags: TunFlags::IFF_TAP,
            fd: -1,
            queues: 1,
            mtu: 1500,
            persistent: false,
            owner_uid: 0,
            group_gid: 0,
            hw_addr: MacAddr([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]),
            rx_ring: Vec::new(),
            tx_ring: Vec::new(),
            rx_ring_capacity: 256,
            stats: TunnelStats::new(),
        }
    }

    pub fn new_multiqueue(name: String, mode: TunTapMode, queues: u16) -> Self {
        let mut dev = match mode {
            TunTapMode::Tun => Self::new_tun(name),
            TunTapMode::Tap => Self::new_tap(name),
        };
        dev.queues = queues;
        dev.flags.insert(TunFlags::IFF_MULTI_QUEUE);
        dev
    }

    pub fn set_persistent(&mut self, persistent: bool) {
        self.persistent = persistent;
    }

    pub fn set_vnet_hdr(&mut self, enable: bool) {
        if enable {
            self.flags.insert(TunFlags::IFF_VNET_HDR);
        } else {
            self.flags.remove(TunFlags::IFF_VNET_HDR);
        }
    }

    pub fn write_packet(&mut self, data: &[u8]) -> Result<(), TunnelError> {
        if self.rx_ring.len() >= self.rx_ring_capacity {
            return Err(TunnelError::QueueFull);
        }
        let mut pkt = Vec::new();
        if !self.flags.contains(TunFlags::IFF_NO_PI) {
            pkt.extend_from_slice(&[0, 0, 0x08, 0x00]);
        }
        pkt.extend_from_slice(data);
        self.rx_ring.push(pkt);
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += data.len() as u64;
        Ok(())
    }

    pub fn read_packet(&mut self) -> Option<Vec<u8>> {
        if self.tx_ring.is_empty() {
            return None;
        }
        let pkt = self.tx_ring.remove(0);
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += pkt.len() as u64;
        Some(pkt)
    }

    pub fn enqueue_tx(&mut self, data: Vec<u8>) -> Result<(), TunnelError> {
        if self.tx_ring.len() >= self.rx_ring_capacity {
            return Err(TunnelError::QueueFull);
        }
        self.tx_ring.push(data);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IPVS — IP Virtual Server (load balancing)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpvsScheduler {
    RoundRobin,
    WeightedRoundRobin,
    LeastConnection,
    WeightedLeastConnection,
    DestinationHashing,
    SourceHashing,
    ShortestExpectedDelay,
    NeverQueue,
    OverflowConnection,
    LocalityBasedLeastConnection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpvsForwardMethod {
    Nat,
    DirectReturn,
    Tunnel,
}

#[derive(Debug, Clone)]
pub struct IpvsRealServer {
    pub addr: IpAddr,
    pub port: u16,
    pub weight: i32,
    pub forward: IpvsForwardMethod,
    pub active_conns: u32,
    pub inactive_conns: u32,
    pub persistent_conns: u32,
    pub stats: IpvsStats,
}

#[derive(Debug, Clone, Default)]
pub struct IpvsStats {
    pub conns: u64,
    pub in_packets: u64,
    pub out_packets: u64,
    pub in_bytes: u64,
    pub out_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct IpvsService {
    pub protocol: u8,
    pub addr: IpAddr,
    pub port: u16,
    pub scheduler: IpvsScheduler,
    pub flags: u32,
    pub timeout: u32,
    pub netmask: u32,
    pub persistence_timeout: u32,
    pub real_servers: Vec<IpvsRealServer>,
    pub rr_index: usize,
    pub wrr_current_weight: i32,
    pub stats: IpvsStats,
}

impl IpvsService {
    pub fn new(protocol: u8, addr: IpAddr, port: u16, scheduler: IpvsScheduler) -> Self {
        Self {
            protocol,
            addr,
            port,
            scheduler,
            flags: 0,
            timeout: 0,
            netmask: 0xFFFF_FFFF,
            persistence_timeout: 0,
            real_servers: Vec::new(),
            rr_index: 0,
            wrr_current_weight: 0,
            stats: IpvsStats::default(),
        }
    }

    pub fn add_real_server(&mut self, server: IpvsRealServer) {
        self.real_servers.push(server);
    }

    pub fn remove_real_server(&mut self, addr: &IpAddr, port: u16) -> bool {
        let before = self.real_servers.len();
        self.real_servers
            .retain(|s| !matches!((&s.addr, s.port), (a, p) if a == addr && p == port));
        self.real_servers.len() < before
    }

    pub fn schedule(&mut self) -> Option<usize> {
        if self.real_servers.is_empty() {
            return None;
        }
        match self.scheduler {
            IpvsScheduler::RoundRobin => self.schedule_rr(),
            IpvsScheduler::WeightedRoundRobin => self.schedule_wrr(),
            IpvsScheduler::LeastConnection => self.schedule_lc(),
            IpvsScheduler::WeightedLeastConnection => self.schedule_wlc(),
            IpvsScheduler::DestinationHashing => self.schedule_dh(),
            IpvsScheduler::SourceHashing => self.schedule_sh(),
            IpvsScheduler::ShortestExpectedDelay => self.schedule_sed(),
            IpvsScheduler::NeverQueue => self.schedule_nq(),
            IpvsScheduler::OverflowConnection => self.schedule_ovf(),
            IpvsScheduler::LocalityBasedLeastConnection => self.schedule_lblc(),
        }
    }

    fn schedule_rr(&mut self) -> Option<usize> {
        let count = self.real_servers.len();
        for _ in 0..count {
            self.rr_index = (self.rr_index + 1) % count;
            if self.real_servers[self.rr_index].weight > 0 {
                return Some(self.rr_index);
            }
        }
        None
    }

    fn schedule_wrr(&mut self) -> Option<usize> {
        let count = self.real_servers.len();
        let max_weight = self
            .real_servers
            .iter()
            .map(|s| s.weight)
            .max()
            .unwrap_or(0);
        let gcd = self.gcd_weights();
        if max_weight == 0 {
            return None;
        }
        for _ in 0..count * max_weight as usize {
            self.rr_index = (self.rr_index + 1) % count;
            if self.rr_index == 0 {
                self.wrr_current_weight -= gcd;
                if self.wrr_current_weight <= 0 {
                    self.wrr_current_weight = max_weight;
                }
            }
            if self.real_servers[self.rr_index].weight >= self.wrr_current_weight {
                return Some(self.rr_index);
            }
        }
        None
    }

    fn schedule_lc(&self) -> Option<usize> {
        self.real_servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.weight > 0)
            .min_by_key(|(_, s)| s.active_conns)
            .map(|(i, _)| i)
    }

    fn schedule_wlc(&self) -> Option<usize> {
        self.real_servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.weight > 0)
            .min_by(|(_, a), (_, b)| {
                let a_val = (a.active_conns as i64) * (b.weight as i64);
                let b_val = (b.active_conns as i64) * (a.weight as i64);
                a_val.cmp(&b_val)
            })
            .map(|(i, _)| i)
    }

    fn schedule_dh(&self) -> Option<usize> {
        if self.real_servers.is_empty() {
            return None;
        }
        let hash = 0u32;
        Some(hash as usize % self.real_servers.len())
    }

    fn schedule_sh(&self) -> Option<usize> {
        if self.real_servers.is_empty() {
            return None;
        }
        let hash = 0u32;
        Some(hash as usize % self.real_servers.len())
    }

    fn schedule_sed(&self) -> Option<usize> {
        self.real_servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.weight > 0)
            .min_by_key(|(_, s)| ((s.active_conns + 1) as i64 * 65536) / s.weight.max(1) as i64)
            .map(|(i, _)| i)
    }

    fn schedule_nq(&self) -> Option<usize> {
        if let Some((i, _)) = self
            .real_servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.weight > 0 && s.active_conns == 0)
            .next()
        {
            return Some(i);
        }
        self.schedule_sed()
    }

    fn schedule_ovf(&self) -> Option<usize> {
        for (i, s) in self.real_servers.iter().enumerate() {
            if s.weight > 0 && s.active_conns < s.weight as u32 {
                return Some(i);
            }
        }
        self.real_servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.weight > 0)
            .min_by_key(|(_, s)| s.active_conns)
            .map(|(i, _)| i)
    }

    fn schedule_lblc(&self) -> Option<usize> {
        self.schedule_lc()
    }

    fn gcd_weights(&self) -> i32 {
        fn gcd(a: i32, b: i32) -> i32 {
            if b == 0 {
                a
            } else {
                gcd(b, a % b)
            }
        }
        self.real_servers
            .iter()
            .map(|s| s.weight.abs())
            .filter(|&w| w > 0)
            .fold(0, gcd)
            .max(1)
    }
}

#[derive(Debug, Clone)]
pub struct IpvsConnection {
    pub protocol: u8,
    pub client_addr: IpAddr,
    pub client_port: u16,
    pub virtual_addr: IpAddr,
    pub virtual_port: u16,
    pub dest_addr: IpAddr,
    pub dest_port: u16,
    pub forward: IpvsForwardMethod,
    pub state: u8,
    pub timeout: u32,
    pub flags: u32,
}

#[derive(Debug)]
pub struct IpvsSyncDaemon {
    pub state: u8,
    pub mcast_ifn: String,
    pub sync_id: u8,
    pub mcast_group: IpAddr,
    pub mcast_port: u16,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tunnel metadata — encap/decap hooks, tunnel info, tunnel key
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelType {
    Gre,
    Vxlan,
    Geneve,
    Ipip,
    Sit,
    Ip6Ip6,
    Ip4Ip6,
    WireGuard,
    L2tp,
    Pppoe,
    TunTap,
}

#[derive(Debug, Clone)]
pub struct TunnelKey {
    pub src: IpAddr,
    pub dst: IpAddr,
    pub tunnel_id: u64,
    pub tos: u8,
    pub ttl: u8,
    pub label: u32,
    pub tp_src: u16,
    pub tp_dst: u16,
}

#[derive(Debug, Clone)]
pub struct TunnelInfo {
    pub key: TunnelKey,
    pub tunnel_type: TunnelType,
    pub options: Vec<u8>,
    pub options_len: u16,
    pub mode: u8,
}

impl TunnelInfo {
    pub fn new(tunnel_type: TunnelType, key: TunnelKey) -> Self {
        Self {
            key,
            tunnel_type,
            options: Vec::new(),
            options_len: 0,
            mode: 0,
        }
    }

    pub fn set_options(&mut self, opts: Vec<u8>) {
        self.options_len = opts.len() as u16;
        self.options = opts;
    }
}

pub trait EncapHandler: Send + Sync {
    fn encapsulate(
        &self,
        info: &TunnelInfo,
        inner: &[u8],
        buf: &mut Vec<u8>,
    ) -> Result<(), TunnelError>;
    fn decapsulate(&self, packet: &[u8]) -> Result<(TunnelInfo, Vec<u8>), TunnelError>;
    fn mtu_overhead(&self) -> u16;
}

pub trait DecapHandler: Send + Sync {
    fn on_decap(&self, info: &TunnelInfo, inner: &[u8]) -> Result<(), TunnelError>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Common stats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct TunnelStats {
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_errors: u64,
    pub rx_errors: u64,
    pub tx_dropped: u64,
    pub rx_dropped: u64,
}

impl TunnelStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global tunnel subsystem
// ─────────────────────────────────────────────────────────────────────────────

pub struct TunnelSubsystem {
    pub gre_tunnels: Vec<GreTunnel>,
    pub vxlan_instances: Vec<VxlanInstance>,
    pub geneve_instances: Vec<GeneveInstance>,
    pub ipip_tunnels: Vec<IpipTunnel>,
    pub wg_devices: Vec<WgDevice>,
    pub l2tp_tunnels: Vec<L2tpTunnel>,
    pub pppoe_sessions: Vec<PppoeSession>,
    pub tuntap_devices: Vec<TunTapDevice>,
    pub ipvs_services: Vec<IpvsService>,
    pub ipvs_connections: Vec<IpvsConnection>,
    pub ipvs_sync: Option<IpvsSyncDaemon>,
    pub tunnel_infos: Vec<TunnelInfo>,
    pub next_id: u32,
    pub initialized: bool,
}

impl TunnelSubsystem {
    pub const fn new() -> Self {
        Self {
            gre_tunnels: Vec::new(),
            vxlan_instances: Vec::new(),
            geneve_instances: Vec::new(),
            ipip_tunnels: Vec::new(),
            wg_devices: Vec::new(),
            l2tp_tunnels: Vec::new(),
            pppoe_sessions: Vec::new(),
            tuntap_devices: Vec::new(),
            ipvs_services: Vec::new(),
            ipvs_connections: Vec::new(),
            ipvs_sync: None,
            tunnel_infos: Vec::new(),
            next_id: 1,
            initialized: false,
        }
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn create_gre_v0(&mut self, name: String, local: IpAddr, remote: IpAddr) -> u32 {
        let id = self.alloc_id();
        self.gre_tunnels
            .push(GreTunnel::new_v0(id, name, local, remote));
        id
    }

    pub fn create_gre_v1(
        &mut self,
        name: String,
        local: IpAddr,
        remote: IpAddr,
        call_id: u16,
    ) -> u32 {
        let id = self.alloc_id();
        self.gre_tunnels
            .push(GreTunnel::new_v1_pptp(id, name, local, remote, call_id));
        id
    }

    pub fn create_erspan(
        &mut self,
        name: String,
        local: IpAddr,
        remote: IpAddr,
        span_type: ErspanType,
        session_id: u16,
    ) -> u32 {
        let id = self.alloc_id();
        self.gre_tunnels.push(GreTunnel::new_erspan(
            id, name, local, remote, span_type, session_id,
        ));
        id
    }

    pub fn create_gre_tap(&mut self, name: String, local: IpAddr, remote: IpAddr) -> u32 {
        let id = self.alloc_id();
        self.gre_tunnels
            .push(GreTunnel::new_gre_tap(id, name, local, remote));
        id
    }

    pub fn create_vxlan(&mut self, vni: u32, local_ip: IpAddr) -> u32 {
        self.vxlan_instances.push(VxlanInstance::new(vni, local_ip));
        vni
    }

    pub fn create_vxlan_gpe(&mut self, vni: u32, local_ip: IpAddr) -> u32 {
        self.vxlan_instances
            .push(VxlanInstance::new_gpe(vni, local_ip));
        vni
    }

    pub fn create_geneve(&mut self, vni: u32, local_ip: IpAddr) -> u32 {
        self.geneve_instances
            .push(GeneveInstance::new(vni, local_ip));
        vni
    }

    pub fn create_ipip(&mut self, name: String, local: Ipv4Addr, remote: Ipv4Addr) -> u32 {
        let id = self.alloc_id();
        self.ipip_tunnels
            .push(IpipTunnel::new_ipip(id, name, local, remote));
        id
    }

    pub fn create_sit(
        &mut self,
        name: String,
        local: Ipv4Addr,
        remote: Ipv4Addr,
        sub: SitSubMode,
    ) -> u32 {
        let id = self.alloc_id();
        self.ipip_tunnels
            .push(IpipTunnel::new_sit(id, name, local, remote, sub));
        id
    }

    pub fn create_wg(&mut self, name: String, private_key: [u8; 32], listen_port: u16) -> usize {
        let dev = WgDevice::new(name, private_key, listen_port);
        self.wg_devices.push(dev);
        self.wg_devices.len() - 1
    }

    pub fn create_l2tp_v2(&mut self, local: IpAddr, remote: IpAddr) -> u32 {
        let id = self.alloc_id();
        self.l2tp_tunnels
            .push(L2tpTunnel::new_v2(id, local, remote));
        id
    }

    pub fn create_l2tp_v3_udp(&mut self, local: IpAddr, remote: IpAddr) -> u32 {
        let id = self.alloc_id();
        self.l2tp_tunnels
            .push(L2tpTunnel::new_v3_udp(id, local, remote));
        id
    }

    pub fn create_tun(&mut self, name: String) -> usize {
        let dev = TunTapDevice::new_tun(name);
        self.tuntap_devices.push(dev);
        self.tuntap_devices.len() - 1
    }

    pub fn create_tap(&mut self, name: String) -> usize {
        let dev = TunTapDevice::new_tap(name);
        self.tuntap_devices.push(dev);
        self.tuntap_devices.len() - 1
    }

    pub fn create_ipvs_service(
        &mut self,
        protocol: u8,
        addr: IpAddr,
        port: u16,
        scheduler: IpvsScheduler,
    ) -> usize {
        let svc = IpvsService::new(protocol, addr, port, scheduler);
        self.ipvs_services.push(svc);
        self.ipvs_services.len() - 1
    }

    pub fn ipvs_add_real_server(
        &mut self,
        svc_idx: usize,
        addr: IpAddr,
        port: u16,
        weight: i32,
        forward: IpvsForwardMethod,
    ) -> Result<(), TunnelError> {
        let svc = self
            .ipvs_services
            .get_mut(svc_idx)
            .ok_or(TunnelError::ServiceNotFound)?;
        svc.add_real_server(IpvsRealServer {
            addr,
            port,
            weight,
            forward,
            active_conns: 0,
            inactive_conns: 0,
            persistent_conns: 0,
            stats: IpvsStats::default(),
        });
        Ok(())
    }

    pub fn ipvs_schedule(&mut self, svc_idx: usize) -> Result<usize, TunnelError> {
        let svc = self
            .ipvs_services
            .get_mut(svc_idx)
            .ok_or(TunnelError::ServiceNotFound)?;
        svc.schedule().ok_or(TunnelError::NoRealServer)
    }

    pub fn ipvs_track_connection(&mut self, conn: IpvsConnection) {
        self.ipvs_connections.push(conn);
    }

    pub fn ipvs_setup_sync(
        &mut self,
        mcast_ifn: String,
        sync_id: u8,
        mcast_group: IpAddr,
        mcast_port: u16,
    ) {
        self.ipvs_sync = Some(IpvsSyncDaemon {
            state: 1,
            mcast_ifn,
            sync_id,
            mcast_group,
            mcast_port,
        });
    }

    pub fn total_tunnels(&self) -> usize {
        self.gre_tunnels.len()
            + self.vxlan_instances.len()
            + self.geneve_instances.len()
            + self.ipip_tunnels.len()
            + self.wg_devices.len()
            + self.l2tp_tunnels.len()
            + self.pppoe_sessions.len()
            + self.tuntap_devices.len()
    }

    pub fn total_ipvs_services(&self) -> usize {
        self.ipvs_services.len()
    }
}

pub static TUNNEL_SUBSYSTEM: Mutex<TunnelSubsystem> = Mutex::new(TunnelSubsystem::new());

pub fn init() {
    let mut ts = TUNNEL_SUBSYSTEM.lock();
    ts.initialized = true;
}
