//! DHCP client — RFC 2131 implementation for automatic IP configuration.
//!
//! State machine: Init → Selecting → Requesting → Bound → Renewing → Rebinding.
//! Builds Discover/Request packets, parses server Offer/Ack responses,
//! manages lease timers, and supports release and rebind flows.
//!
//! `tick()` drives the state machine from the network poll loop.
//! `dhcp_configure()` applies the obtained lease to the smoltcp interface
//! and DNS resolver automatically.

#![allow(dead_code)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ── DHCP State Machine ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpState {
    Init,
    Selecting,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
    Released,
}

// ── Lease ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DhcpLease {
    pub client_ip: [u8; 4],
    pub server_ip: [u8; 4],
    pub gateway: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub dns_servers: Vec<[u8; 4]>,
    pub domain_name: Option<String>,
    pub lease_time_secs: u32,
    pub renewal_time_secs: u32,
    pub rebind_time_secs: u32,
    pub obtained_at: u64,
}

impl DhcpLease {
    pub fn is_expired(&self, now: u64) -> bool {
        now.saturating_sub(self.obtained_at) >= self.lease_time_secs as u64
    }

    pub fn needs_renewal(&self, now: u64) -> bool {
        now.saturating_sub(self.obtained_at) >= self.renewal_time_secs as u64
    }

    pub fn needs_rebind(&self, now: u64) -> bool {
        now.saturating_sub(self.obtained_at) >= self.rebind_time_secs as u64
    }

    pub fn time_remaining(&self, now: u64) -> u64 {
        let elapsed = now.saturating_sub(self.obtained_at);
        (self.lease_time_secs as u64).saturating_sub(elapsed)
    }

    pub fn time_to_renewal(&self, now: u64) -> u64 {
        let elapsed = now.saturating_sub(self.obtained_at);
        (self.renewal_time_secs as u64).saturating_sub(elapsed)
    }

    pub fn prefix_len(&self) -> u8 {
        u32::from_be_bytes(self.subnet_mask).count_ones() as u8
    }
}

// ── DHCP Options ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DhcpOption {
    Pad = 0,
    SubnetMask = 1,
    Router = 3,
    DnsServer = 6,
    HostName = 12,
    DomainName = 15,
    BroadcastAddr = 28,
    RequestedIp = 50,
    LeaseTime = 51,
    MessageType = 53,
    ServerId = 54,
    ParamRequest = 55,
    RenewalTime = 58,
    RebindTime = 59,
    ClientId = 61,
    DomainSearch = 119,
    End = 255,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DhcpMessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

impl DhcpMessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Discover),
            2 => Some(Self::Offer),
            3 => Some(Self::Request),
            4 => Some(Self::Decline),
            5 => Some(Self::Ack),
            6 => Some(Self::Nak),
            7 => Some(Self::Release),
            8 => Some(Self::Inform),
            _ => None,
        }
    }
}

// ── DHCP Packet ─────────────────────────────────────────────────────────────

const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
const BOOTP_REQUEST: u8 = 1;
const BOOTP_REPLY: u8 = 2;
const HTYPE_ETHERNET: u8 = 1;

pub struct DhcpPacket {
    pub op: u8,
    pub htype: u8,
    pub hlen: u8,
    pub hops: u8,
    pub xid: u32,
    pub secs: u16,
    pub flags: u16,
    pub ciaddr: [u8; 4],
    pub yiaddr: [u8; 4],
    pub siaddr: [u8; 4],
    pub giaddr: [u8; 4],
    pub chaddr: [u8; 16],
    pub sname: [u8; 64],
    pub file: [u8; 128],
    pub options: Vec<(u8, Vec<u8>)>,
}

impl DhcpPacket {
    pub fn new_request(xid: u32, mac: [u8; 6]) -> Self {
        let mut chaddr = [0u8; 16];
        chaddr[..6].copy_from_slice(&mac);

        Self {
            op: BOOTP_REQUEST,
            htype: HTYPE_ETHERNET,
            hlen: 6,
            hops: 0,
            xid,
            secs: 0,
            flags: 0x8000, // broadcast flag
            ciaddr: [0; 4],
            yiaddr: [0; 4],
            siaddr: [0; 4],
            giaddr: [0; 4],
            chaddr,
            sname: [0; 64],
            file: [0; 128],
            options: Vec::new(),
        }
    }

    pub fn add_option(&mut self, code: u8, data: Vec<u8>) {
        self.options.push((code, data));
    }

    pub fn add_message_type(&mut self, msg_type: DhcpMessageType) {
        self.add_option(DhcpOption::MessageType as u8, alloc::vec![msg_type as u8]);
    }

    pub fn get_option(&self, code: u8) -> Option<&[u8]> {
        self.options
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, d)| d.as_slice())
    }

    pub fn get_message_type(&self) -> Option<DhcpMessageType> {
        self.get_option(DhcpOption::MessageType as u8)
            .and_then(|d| d.first().copied())
            .and_then(DhcpMessageType::from_u8)
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(576);

        buf.push(self.op);
        buf.push(self.htype);
        buf.push(self.hlen);
        buf.push(self.hops);
        buf.extend_from_slice(&self.xid.to_be_bytes());
        buf.extend_from_slice(&self.secs.to_be_bytes());
        buf.extend_from_slice(&self.flags.to_be_bytes());
        buf.extend_from_slice(&self.ciaddr);
        buf.extend_from_slice(&self.yiaddr);
        buf.extend_from_slice(&self.siaddr);
        buf.extend_from_slice(&self.giaddr);
        buf.extend_from_slice(&self.chaddr);
        buf.extend_from_slice(&self.sname);
        buf.extend_from_slice(&self.file);
        buf.extend_from_slice(&DHCP_MAGIC_COOKIE);

        for (code, data) in &self.options {
            buf.push(*code);
            if *code != DhcpOption::Pad as u8 && *code != DhcpOption::End as u8 {
                buf.push(data.len() as u8);
                buf.extend_from_slice(data);
            }
        }
        buf.push(DhcpOption::End as u8);

        while buf.len() < 300 {
            buf.push(0);
        }

        buf
    }

    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 240 {
            return None;
        }

        let op = data[0];
        let htype = data[1];
        let hlen = data[2];
        let hops = data[3];
        let xid = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let secs = u16::from_be_bytes([data[8], data[9]]);
        let flags = u16::from_be_bytes([data[10], data[11]]);

        let ciaddr = [data[12], data[13], data[14], data[15]];
        let yiaddr = [data[16], data[17], data[18], data[19]];
        let siaddr = [data[20], data[21], data[22], data[23]];
        let giaddr = [data[24], data[25], data[26], data[27]];

        let mut chaddr = [0u8; 16];
        chaddr.copy_from_slice(&data[28..44]);
        let mut sname = [0u8; 64];
        sname.copy_from_slice(&data[44..108]);
        let mut file = [0u8; 128];
        file.copy_from_slice(&data[108..236]);

        if data[236..240] != DHCP_MAGIC_COOKIE {
            return None;
        }

        let options = Self::parse_options(&data[240..]);

        Some(Self {
            op,
            htype,
            hlen,
            hops,
            xid,
            secs,
            flags,
            ciaddr,
            yiaddr,
            siaddr,
            giaddr,
            chaddr,
            sname,
            file,
            options,
        })
    }

    fn parse_options(data: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut opts = Vec::new();
        let mut i = 0;

        while i < data.len() {
            let code = data[i];
            if code == DhcpOption::End as u8 {
                break;
            }
            if code == DhcpOption::Pad as u8 {
                i += 1;
                continue;
            }

            i += 1;
            if i >= data.len() {
                break;
            }
            let len = data[i] as usize;
            i += 1;

            if i + len > data.len() {
                break;
            }

            opts.push((code, data[i..i + len].to_vec()));
            i += len;
        }

        opts
    }
}

// ── DHCP Client ─────────────────────────────────────────────────────────────

pub struct DhcpClient {
    pub state: DhcpState,
    pub interface: String,
    pub mac_address: [u8; 6],
    pub transaction_id: u32,
    pub current_lease: Option<DhcpLease>,
    pub offered_lease: Option<DhcpLease>,
    pub requested_options: Vec<u8>,
    pub hostname: Option<String>,
    pub retries: u32,
    pub max_retries: u32,
    xid_counter: u32,
    /// Timestamp (seconds) of the last packet we sent. Used for retry timing.
    last_sent: u64,
    /// Retry backoff in seconds.
    retry_interval: u64,
    /// False until the current Bound lease has been pushed to smoltcp/DNS via
    /// `dhcp_configure`. The ACK→Bound transition happens in `handle_packet` on
    /// the rx path (inside `iface.poll()`, holding NET_STACK), which can't call
    /// `dhcp_configure` itself — so `dhcp::tick` (running after NET_STACK is free)
    /// applies it the first time it sees a Bound, un-applied lease. Reset to
    /// false on every ACK so a renewal re-installs.
    lease_applied: bool,
}

pub static DHCP_CLIENT: Mutex<Option<DhcpClient>> = Mutex::new(None);

impl DhcpClient {
    pub fn new(interface: String, mac: [u8; 6]) -> Self {
        let seed = u32::from_be_bytes([mac[0], mac[1], mac[2], mac[3]])
            ^ u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]);

        Self {
            state: DhcpState::Init,
            interface,
            mac_address: mac,
            transaction_id: seed,
            current_lease: None,
            offered_lease: None,
            requested_options: alloc::vec![
                DhcpOption::SubnetMask as u8,
                DhcpOption::Router as u8,
                DhcpOption::DnsServer as u8,
                DhcpOption::DomainName as u8,
                DhcpOption::LeaseTime as u8,
                DhcpOption::RenewalTime as u8,
                DhcpOption::RebindTime as u8,
                DhcpOption::BroadcastAddr as u8,
            ],
            hostname: Some(String::from("athenaos")),
            retries: 0,
            max_retries: 5,
            xid_counter: seed,
            last_sent: 0,
            retry_interval: 4,
            lease_applied: false,
        }
    }

    fn next_xid(&mut self) -> u32 {
        self.xid_counter = self
            .xid_counter
            .wrapping_mul(1103515245)
            .wrapping_add(12345);
        self.xid_counter
    }

    pub fn start(&mut self) -> Option<Vec<u8>> {
        self.state = DhcpState::Init;
        self.retries = 0;
        self.retry_interval = 4;
        self.discover()
    }

    pub fn discover(&mut self) -> Option<Vec<u8>> {
        self.transaction_id = self.next_xid();
        self.state = DhcpState::Selecting;

        let pkt = self.build_discover_packet();
        Some(pkt.serialize())
    }

    pub fn request(&mut self) -> Option<Vec<u8>> {
        let lease = self.offered_lease.as_ref()?;
        self.state = DhcpState::Requesting;

        let pkt = self.build_request_packet(lease.client_ip, lease.server_ip);
        Some(pkt.serialize())
    }

    pub fn renew(&mut self) -> Option<Vec<u8>> {
        let lease = self.current_lease.as_ref()?;
        self.state = DhcpState::Renewing;

        let mut pkt = DhcpPacket::new_request(self.transaction_id, self.mac_address);
        pkt.add_message_type(DhcpMessageType::Request);
        pkt.ciaddr = lease.client_ip;
        pkt.add_option(DhcpOption::RequestedIp as u8, lease.client_ip.to_vec());

        Some(pkt.serialize())
    }

    pub fn rebind(&mut self) -> Option<Vec<u8>> {
        self.state = DhcpState::Rebinding;
        self.renew()
    }

    pub fn release(&mut self) -> Option<Vec<u8>> {
        let lease = self.current_lease.as_ref()?;
        let mut pkt = DhcpPacket::new_request(self.transaction_id, self.mac_address);
        pkt.add_message_type(DhcpMessageType::Release);
        pkt.ciaddr = lease.client_ip;
        pkt.add_option(DhcpOption::ServerId as u8, lease.server_ip.to_vec());

        self.state = DhcpState::Released;
        self.current_lease = None;

        Some(pkt.serialize())
    }

    pub fn handle_packet(&mut self, data: &[u8], now: u64) -> DhcpEvent {
        let pkt = match DhcpPacket::parse(data) {
            Some(p) => p,
            None => return DhcpEvent::InvalidPacket,
        };

        if pkt.op != BOOTP_REPLY || pkt.xid != self.transaction_id {
            return DhcpEvent::Ignored;
        }

        let msg_type = match pkt.get_message_type() {
            Some(t) => t,
            None => return DhcpEvent::InvalidPacket,
        };

        match (self.state, msg_type) {
            (DhcpState::Selecting, DhcpMessageType::Offer) => {
                let lease = self.extract_lease(&pkt, now);
                self.offered_lease = Some(lease);
                DhcpEvent::OfferReceived
            }
            (
                DhcpState::Requesting | DhcpState::Renewing | DhcpState::Rebinding,
                DhcpMessageType::Ack,
            ) => {
                let lease = self.extract_lease(&pkt, now);
                self.current_lease = Some(lease);
                self.offered_lease = None;
                self.state = DhcpState::Bound;
                self.retries = 0;
                self.retry_interval = 4;
                // New lease — mark un-applied so `dhcp::tick` installs it into
                // smoltcp/DNS (this rx path can't, it holds NET_STACK).
                self.lease_applied = false;
                DhcpEvent::Bound
            }
            (
                DhcpState::Requesting | DhcpState::Renewing | DhcpState::Rebinding,
                DhcpMessageType::Nak,
            ) => {
                self.current_lease = None;
                self.offered_lease = None;
                self.state = DhcpState::Init;
                DhcpEvent::Rejected
            }
            _ => DhcpEvent::Ignored,
        }
    }

    /// Drive the DHCP state machine. Called periodically (e.g. every second).
    /// Returns an optional packet to send and an event describing what happened.
    pub fn tick(&mut self, now: u64) -> (Option<Vec<u8>>, DhcpEvent) {
        match self.state {
            DhcpState::Init => {
                let pkt = self.start();
                self.last_sent = now;
                (pkt, DhcpEvent::DiscoverSent)
            }
            DhcpState::Selecting => {
                // An OFFER arrived (handle_packet stored it) — proceed to the
                // REQUEST. WITHOUT THIS the machine re-DISCOVERs forever even
                // though the server answered: the classic "stuck at Selecting".
                if self.offered_lease.is_some() {
                    if let Some(pkt) = self.request() {
                        self.last_sent = now;
                        self.retries = 0;
                        self.retry_interval = 4;
                        return (Some(pkt), DhcpEvent::RequestSent);
                    }
                }
                // Retry discover if we haven't received an offer
                if now.saturating_sub(self.last_sent) >= self.retry_interval {
                    if self.retries >= self.max_retries {
                        self.state = DhcpState::Init;
                        self.retries = 0;
                        return (None, DhcpEvent::Timeout);
                    }
                    self.retries += 1;
                    self.retry_interval = (self.retry_interval * 2).min(64);
                    let pkt = self.discover();
                    self.last_sent = now;
                    (pkt, DhcpEvent::DiscoverSent)
                } else {
                    (None, DhcpEvent::Waiting)
                }
            }
            DhcpState::Requesting => {
                if now.saturating_sub(self.last_sent) >= self.retry_interval {
                    if self.retries >= self.max_retries {
                        self.state = DhcpState::Init;
                        self.retries = 0;
                        return (None, DhcpEvent::Timeout);
                    }
                    self.retries += 1;
                    let pkt = self.request();
                    self.last_sent = now;
                    (pkt, DhcpEvent::RequestSent)
                } else {
                    (None, DhcpEvent::Waiting)
                }
            }
            DhcpState::Bound => {
                if let Some(ref lease) = self.current_lease {
                    if lease.is_expired(now) {
                        self.current_lease = None;
                        self.state = DhcpState::Init;
                        return (None, DhcpEvent::LeaseExpired);
                    }
                    if lease.needs_rebind(now) {
                        let pkt = self.rebind();
                        self.last_sent = now;
                        return (pkt, DhcpEvent::RebindSent);
                    }
                    if lease.needs_renewal(now) {
                        let pkt = self.renew();
                        self.last_sent = now;
                        return (pkt, DhcpEvent::RenewSent);
                    }
                }
                (None, DhcpEvent::Waiting)
            }
            DhcpState::Renewing => {
                if now.saturating_sub(self.last_sent) >= self.retry_interval {
                    if let Some(ref lease) = self.current_lease {
                        if lease.needs_rebind(now) {
                            let pkt = self.rebind();
                            self.last_sent = now;
                            return (pkt, DhcpEvent::RebindSent);
                        }
                    }
                    self.retries += 1;
                    let pkt = self.renew();
                    self.last_sent = now;
                    (pkt, DhcpEvent::RenewSent)
                } else {
                    (None, DhcpEvent::Waiting)
                }
            }
            DhcpState::Rebinding => {
                if let Some(ref lease) = self.current_lease {
                    if lease.is_expired(now) {
                        self.current_lease = None;
                        self.state = DhcpState::Init;
                        return (None, DhcpEvent::LeaseExpired);
                    }
                }
                if now.saturating_sub(self.last_sent) >= self.retry_interval {
                    self.retries += 1;
                    let pkt = self.rebind();
                    self.last_sent = now;
                    (pkt, DhcpEvent::RebindSent)
                } else {
                    (None, DhcpEvent::Waiting)
                }
            }
            DhcpState::Released => (None, DhcpEvent::Waiting),
        }
    }

    fn extract_lease(&self, pkt: &DhcpPacket, now: u64) -> DhcpLease {
        let client_ip = pkt.yiaddr;
        let server_ip = pkt
            .get_option(DhcpOption::ServerId as u8)
            .and_then(|d| {
                if d.len() >= 4 {
                    Some([d[0], d[1], d[2], d[3]])
                } else {
                    None
                }
            })
            .unwrap_or(pkt.siaddr);

        let subnet_mask = pkt
            .get_option(DhcpOption::SubnetMask as u8)
            .and_then(|d| {
                if d.len() >= 4 {
                    Some([d[0], d[1], d[2], d[3]])
                } else {
                    None
                }
            })
            .unwrap_or([255, 255, 255, 0]);

        let gateway = pkt
            .get_option(DhcpOption::Router as u8)
            .and_then(|d| {
                if d.len() >= 4 {
                    Some([d[0], d[1], d[2], d[3]])
                } else {
                    None
                }
            })
            .unwrap_or([0; 4]);

        let dns_servers = pkt
            .get_option(DhcpOption::DnsServer as u8)
            .map(|d| {
                d.chunks_exact(4)
                    .map(|c| [c[0], c[1], c[2], c[3]])
                    .collect()
            })
            .unwrap_or_default();

        let domain_name = pkt
            .get_option(DhcpOption::DomainName as u8)
            .and_then(|d| core::str::from_utf8(d).ok())
            .map(|s| String::from(s));

        let lease_time = pkt
            .get_option(DhcpOption::LeaseTime as u8)
            .and_then(|d| {
                if d.len() >= 4 {
                    Some(u32::from_be_bytes([d[0], d[1], d[2], d[3]]))
                } else {
                    None
                }
            })
            .unwrap_or(86400);

        let renewal_time = pkt
            .get_option(DhcpOption::RenewalTime as u8)
            .and_then(|d| {
                if d.len() >= 4 {
                    Some(u32::from_be_bytes([d[0], d[1], d[2], d[3]]))
                } else {
                    None
                }
            })
            .unwrap_or(lease_time / 2);

        let rebind_time = pkt
            .get_option(DhcpOption::RebindTime as u8)
            .and_then(|d| {
                if d.len() >= 4 {
                    Some(u32::from_be_bytes([d[0], d[1], d[2], d[3]]))
                } else {
                    None
                }
            })
            .unwrap_or(((lease_time as u64) * 7 / 8) as u32);

        DhcpLease {
            client_ip,
            server_ip,
            gateway,
            subnet_mask,
            dns_servers,
            domain_name,
            lease_time_secs: lease_time,
            renewal_time_secs: renewal_time,
            rebind_time_secs: rebind_time,
            obtained_at: now,
        }
    }

    fn build_discover_packet(&self) -> DhcpPacket {
        let mut pkt = DhcpPacket::new_request(self.transaction_id, self.mac_address);
        pkt.add_message_type(DhcpMessageType::Discover);

        let mut client_id = alloc::vec![HTYPE_ETHERNET];
        client_id.extend_from_slice(&self.mac_address);
        pkt.add_option(DhcpOption::ClientId as u8, client_id);

        if let Some(ref hostname) = self.hostname {
            pkt.add_option(DhcpOption::HostName as u8, hostname.as_bytes().to_vec());
        }

        pkt.add_option(
            DhcpOption::ParamRequest as u8,
            self.requested_options.clone(),
        );

        pkt
    }

    fn build_request_packet(&self, requested_ip: [u8; 4], server_ip: [u8; 4]) -> DhcpPacket {
        let mut pkt = DhcpPacket::new_request(self.transaction_id, self.mac_address);
        pkt.add_message_type(DhcpMessageType::Request);

        pkt.add_option(DhcpOption::RequestedIp as u8, requested_ip.to_vec());
        pkt.add_option(DhcpOption::ServerId as u8, server_ip.to_vec());

        let mut client_id = alloc::vec![HTYPE_ETHERNET];
        client_id.extend_from_slice(&self.mac_address);
        pkt.add_option(DhcpOption::ClientId as u8, client_id);

        if let Some(ref hostname) = self.hostname {
            pkt.add_option(DhcpOption::HostName as u8, hostname.as_bytes().to_vec());
        }

        pkt.add_option(
            DhcpOption::ParamRequest as u8,
            self.requested_options.clone(),
        );

        pkt
    }

    pub fn is_lease_expired(&self, now: u64) -> bool {
        self.current_lease
            .as_ref()
            .map_or(true, |l| l.is_expired(now))
    }

    pub fn time_to_renewal(&self, now: u64) -> Option<u64> {
        self.current_lease.as_ref().map(|l| l.time_to_renewal(now))
    }

    pub fn current_ip(&self) -> Option<[u8; 4]> {
        self.current_lease.as_ref().map(|l| l.client_ip)
    }

    pub fn current_gateway(&self) -> Option<[u8; 4]> {
        self.current_lease.as_ref().map(|l| l.gateway)
    }

    pub fn current_dns(&self) -> Vec<[u8; 4]> {
        self.current_lease
            .as_ref()
            .map(|l| l.dns_servers.clone())
            .unwrap_or_default()
    }

    pub fn lease_time_remaining(&self, now: u64) -> Option<u64> {
        self.current_lease.as_ref().map(|l| l.time_remaining(now))
    }
}

// ── Events ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpEvent {
    OfferReceived,
    Bound,
    Rejected,
    InvalidPacket,
    Ignored,
    Timeout,
    Waiting,
    DiscoverSent,
    RequestSent,
    RenewSent,
    RebindSent,
    LeaseExpired,
}

/// Initialize the DHCP client for the primary network interface.
pub fn init() {
    let mac = {
        let guard = crate::net_drivers::NET_DRIVERS.lock();
        guard
            .as_ref()
            .and_then(|mgr| mgr.default_driver())
            .map(|drv| drv.mac_address())
    }
    .unwrap_or_else(|| {
        crate::virtio_net::VIRTIO_NET
            .get()
            .map(|net| net.mac())
            .unwrap_or([0x52, 0x54, 0x00, 0x12, 0x34, 0x56])
    });

    let client = DhcpClient::new(String::from("eth0"), mac);
    *DHCP_CLIENT.lock() = Some(client);

    crate::serial_println!(
        "[ OK ] DHCP client initialized (MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
    );
}

// ── Auto-configuration ──────────────────────────────────────────────────────

/// Apply a DHCP lease to the network stack: configure the smoltcp interface
/// with the obtained IP, update the default gateway in the routing table,
/// and set DNS servers in the resolver.
pub fn dhcp_configure(lease: &DhcpLease) {
    use smoltcp::wire::{IpCidr, Ipv4Address};

    // 1. Configure IP on smoltcp interface
    let prefix = lease.prefix_len();
    let ip = Ipv4Address::new(
        lease.client_ip[0],
        lease.client_ip[1],
        lease.client_ip[2],
        lease.client_ip[3],
    );

    let mut stack_guard = crate::net::NET_STACK.lock();
    if let Some(ref mut stack) = *stack_guard {
        stack.iface.update_ip_addrs(|addrs| {
            addrs.clear();
            let _ = addrs.push(IpCidr::new(ip.into(), prefix));
        });

        // Set default gateway
        if lease.gateway != [0, 0, 0, 0] {
            let gw = Ipv4Address::new(
                lease.gateway[0],
                lease.gateway[1],
                lease.gateway[2],
                lease.gateway[3],
            );
            stack.iface.routes_mut().add_default_ipv4_route(gw).ok();
        }
    }
    drop(stack_guard);

    // 2. Configure DNS servers from the lease
    if !lease.dns_servers.is_empty() {
        let mut dns_guard = crate::dns::DNS_RESOLVER.lock();
        if let Some(ref mut resolver) = *dns_guard {
            resolver.set_servers(lease.dns_servers.clone());
        }
    }

    crate::serial_println!(
        "[DHCP] Configured: IP {}.{}.{}.{}/{}, GW {}.{}.{}.{}, DNS servers: {}",
        lease.client_ip[0],
        lease.client_ip[1],
        lease.client_ip[2],
        lease.client_ip[3],
        prefix,
        lease.gateway[0],
        lease.gateway[1],
        lease.gateway[2],
        lease.gateway[3],
        lease.dns_servers.len(),
    );
}

/// Receive an Ethernet frame and forward to the DHCP state machine if
/// it's a UDP packet to port 68 (DHCP client). Returns true if the
/// frame matched and was processed. Tight bounds + strict checks so a
/// malformed frame can't slip through into the parser.
pub fn handle_eth_frame(frame: &[u8], now: u64) -> bool {
    // Minimum size: 14 (eth) + 20 (IPv4) + 8 (UDP) + 244 (DHCP) = 286B.
    // We accept anything ≥ 14 + 20 + 8 + 1 and let DhcpPacket::parse
    // validate the inner structure.
    if frame.len() < 14 + 20 + 8 + 1 {
        return false;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != 0x0800 {
        return false;
    } // IPv4 only

    let ip = &frame[14..];
    if ip.len() < 20 {
        return false;
    }
    let version_ihl = ip[0];
    if (version_ihl >> 4) != 4 {
        return false;
    } // IPv4
    let ihl_bytes = ((version_ihl & 0x0f) as usize) * 4;
    if ihl_bytes < 20 || ip.len() < ihl_bytes {
        return false;
    }
    if ip[9] != 17 {
        return false;
    } // UDP

    let udp = &ip[ihl_bytes..];
    if udp.len() < 8 {
        return false;
    }
    let _src_port = u16::from_be_bytes([udp[0], udp[1]]);
    let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
    if dst_port != 68 {
        return false;
    } // DHCP client

    let payload = &udp[8..];
    let event = {
        let mut g = DHCP_CLIENT.lock();
        match *g {
            Some(ref mut c) => c.handle_packet(payload, now),
            None => return false,
        }
    };

    // OFFER → emit REQUEST. Bound transition handled by next tick().
    if event == DhcpEvent::OfferReceived {
        let req = {
            let mut g = DHCP_CLIENT.lock();
            g.as_mut().map(|c| (c.mac_address, c.request()))
        };
        if let Some((mac, Some(data))) = req {
            let _ = send_dhcp_payload(mac, &data);
        }
    }
    crate::serial_println!("[dhcp] rx -> event={:?}", event);
    true
}

/// Wrap a raw DHCP BOOTP payload in UDP/68→67, IPv4 src 0.0.0.0/dst
/// 255.255.255.255, and an Ethernet broadcast frame. The DhcpPacket
/// serializer outputs only the BOOTP payload; without the surrounding
/// L2/L3/L4 headers, QEMU's user-mode network drops the frame as
/// malformed. This wrapper produces a wire-legal DHCPDISCOVER frame.
fn wrap_dhcp_broadcast(src_mac: [u8; 6], payload: &[u8]) -> alloc::vec::Vec<u8> {
    let udp_len = 8 + payload.len();
    let ip_total = 20 + udp_len;
    let frame_len = 14 + ip_total;
    let mut f = alloc::vec![0u8; frame_len];

    // Ethernet: dst = broadcast, src = us, ethertype = IPv4.
    f[0..6].copy_from_slice(&[0xff; 6]);
    f[6..12].copy_from_slice(&src_mac);
    f[12] = 0x08;
    f[13] = 0x00;

    // IPv4 header.
    let ip = &mut f[14..14 + 20];
    ip[0] = 0x45; // version=4, IHL=5
    ip[1] = 0x00; // DSCP/ECN
    ip[2..4].copy_from_slice(&(ip_total as u16).to_be_bytes()); // total length
    ip[4..6].copy_from_slice(&0u16.to_be_bytes()); // identification
    ip[6..8].copy_from_slice(&0u16.to_be_bytes()); // flags + frag
    ip[8] = 64; // TTL
    ip[9] = 17; // protocol = UDP
                // checksum at ip[10..12] computed below
    ip[12..16].copy_from_slice(&[0, 0, 0, 0]); // src 0.0.0.0
    ip[16..20].copy_from_slice(&[255, 255, 255, 255]); // dst 255.255.255.255
                                                       // IPv4 checksum (header only).
    let mut sum: u32 = 0;
    for i in (0..20).step_by(2) {
        sum += u16::from_be_bytes([ip[i], ip[i + 1]]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    let cksum = !(sum as u16);
    ip[10..12].copy_from_slice(&cksum.to_be_bytes());

    // UDP header.
    let udp = &mut f[34..34 + 8];
    udp[0..2].copy_from_slice(&68u16.to_be_bytes()); // src port 68
    udp[2..4].copy_from_slice(&67u16.to_be_bytes()); // dst port 67
    udp[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes()); // udp length
    udp[6..8].copy_from_slice(&0u16.to_be_bytes()); // udp checksum 0 = no check

    // DHCP payload.
    f[42..42 + payload.len()].copy_from_slice(payload);
    f
}

/// Send a DHCP BOOTP payload via the active NIC backend.
/// The payload is always wrapped into a valid L2/L3/L4 broadcast frame.
fn send_dhcp_payload(src_mac: [u8; 6], payload: &[u8]) -> bool {
    let frame = wrap_dhcp_broadcast(src_mac, payload);

    {
        let mut guard = crate::net_drivers::NET_DRIVERS.lock();
        if let Some(mgr) = guard.as_mut() {
            if let Some(drv) = mgr.default_driver_mut() {
                return drv.send(&frame).is_ok();
            }
        }
    }

    if let Some(net) = crate::virtio_net::VIRTIO_NET.get() {
        return net.tx_frame(&frame).is_ok();
    }
    false
}

/// Kick the DHCP state machine into DISCOVER. Returns true if a
/// DHCPDISCOVER was emitted on the wire.
pub fn kick_discovery(now: u64) -> bool {
    let (pkt, mac) = {
        let mut g = DHCP_CLIENT.lock();
        match *g {
            Some(ref mut c) => {
                let _ = now;
                (c.start(), c.mac_address)
            }
            None => return false,
        }
    };
    if let Some(data) = pkt {
        return send_dhcp_payload(mac, &data);
    }
    false
}

/// Snapshot of the DHCP client's current state — for /proc and the
/// boot smoketest.
pub fn current_state() -> Option<DhcpState> {
    DHCP_CLIENT.lock().as_ref().map(|c| c.state)
}

/// /proc/athena/dhcp dump.
pub fn dump_text() -> alloc::string::String {
    use alloc::string::String;
    let g = DHCP_CLIENT.lock();
    let mut out = String::new();
    match g.as_ref() {
        None => {
            out.push_str("# dhcp client not initialized\n");
            return out;
        }
        Some(c) => {
            out.push_str(&alloc::format!(
                "# AthenaOS DHCP client (iface=\"{}\")\n\
                 mac:   {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n\
                 state: {:?}\n",
                c.interface,
                c.mac_address[0],
                c.mac_address[1],
                c.mac_address[2],
                c.mac_address[3],
                c.mac_address[4],
                c.mac_address[5],
                c.state,
            ));
            if let Some(ref lease) = c.current_lease {
                let ip = lease.client_ip;
                let gw = lease.gateway;
                out.push_str(&alloc::format!(
                    "lease_ip: {}.{}.{}.{}\n\
                     gateway:  {}.{}.{}.{}\n\
                     lease_s:  {}\n",
                    ip[0],
                    ip[1],
                    ip[2],
                    ip[3],
                    gw[0],
                    gw[1],
                    gw[2],
                    gw[3],
                    lease.lease_time_secs,
                ));
            } else {
                out.push_str("lease_ip: none\n");
            }
        }
    }
    out
}

/// Periodic tick — drives the DHCP state machine and auto-configures
/// on lease acquisition. Called from the network poll loop.
pub fn tick(now: u64) {
    let result = {
        let mut guard = DHCP_CLIENT.lock();
        match *guard {
            Some(ref mut client) => {
                let (pkt, event) = client.tick(now);

                // Install the lease into smoltcp/DNS the first time we observe a
                // Bound, un-applied client. The ACK→Bound transition happens in
                // `handle_packet` on the rx path (inside iface.poll, holding
                // NET_STACK), so it can't configure itself — this runs from
                // poll_full AFTER poll() released NET_STACK, so it's safe here.
                let fresh_lease = if client.state == DhcpState::Bound && !client.lease_applied {
                    client.lease_applied = true;
                    client.current_lease.clone()
                } else {
                    None
                };

                if event == DhcpEvent::OfferReceived {
                    let req = client.request();
                    (req, event, fresh_lease)
                } else {
                    (pkt, event, fresh_lease)
                }
            }
            None => return,
        }
    };

    let (pkt, event, lease) = result;

    // Send any outbound DHCP packet via the active NIC
    if let Some(data) = pkt {
        let mac = {
            let guard = DHCP_CLIENT.lock();
            guard.as_ref().map(|c| c.mac_address).unwrap_or([0; 6])
        };
        let _ = send_dhcp_payload(mac, &data);
    }

    // Auto-configure on Bound
    if let Some(lease) = lease {
        dhcp_configure(&lease);
    }

    match event {
        DhcpEvent::Bound => {
            crate::serial_println!("[DHCP] Lease acquired");
        }
        DhcpEvent::LeaseExpired => {
            crate::serial_println!("[DHCP] Lease expired, restarting discovery");
        }
        DhcpEvent::Timeout => {
            crate::serial_println!("[DHCP] Discovery timeout, retrying");
        }
        _ => {}
    }
}

// ── Lease renewal timer ───────────────────────────────────────────────────────

/// Called periodically (e.g. from the network timer tick) to check whether
/// the DHCP lease needs renewal and trigger a unicast REQUEST if so.
///
/// MasterChecklist Phase 10: "DHCP renewal on lease expiry."
///
/// Renewal flow (RFC 2131 §4.4.5):
///   T1 (renewal time, default = 0.5 × lease time): unicast REQUEST to server.
///   T2 (rebind time, default = 0.875 × lease time): broadcast REQUEST.
///   Lease expiry: re-run full DORA discovery.
pub fn check_lease_renewal() {
    // MONOTONIC seconds since boot — the SAME clock the lease's `obtained_at`
    // and `dhcp::tick`/`handle_eth_frame` use (poll_full / net.rs receive). Using
    // rtc epoch here (~1.7e9) against an hpet-monotonic `obtained_at` (~tens of
    // seconds) made `is_expired` true the instant DHCP bound on iron → an endless
    // "lease expired — restarting DORA" loop the moment the OFFER routing fix let
    // it reach Bound. Lease timers are RELATIVE (elapsed since obtained), so
    // monotonic uptime is the correct clock and is immune to RTC adjustments.
    let now = (crate::hpet::read_millis().unwrap_or(0) as u64) / 1000;
    let mut client = DHCP_CLIENT.lock();
    let Some(client) = client.as_mut() else {
        return;
    };

    let Some(lease) = &client.current_lease else {
        return;
    };

    if lease.is_expired(now) {
        crate::serial_println!("[dhcp] lease expired — restarting DORA discovery");
        client.state = DhcpState::Selecting; // restart from first selectable state
                                             // Trigger re-discovery on next poll_dhcp() call.
        return;
    }

    if lease.needs_renewal(now) {
        crate::serial_println!("[dhcp] lease renewal needed — sending unicast REQUEST");
        // Generate a renewal REQUEST and queue it for the next poll_dhcp() call.
        // The `renew()` method on DhcpClient constructs the renewal packet.
        let _ = client.renew();
    }
}

/// Build a synthetic BOOTP REPLY (OFFER/ACK) for `xid` granting `yiaddr`, so the
/// boot smoketest can drive the client through the full handshake off the wire.
fn build_reply(xid: u32, yiaddr: [u8; 4], mtype: DhcpMessageType) -> Vec<u8> {
    let mut pkt = DhcpPacket::new_request(xid, [0u8; 6]);
    pkt.op = BOOTP_REPLY;
    pkt.yiaddr = yiaddr;
    pkt.add_message_type(mtype);
    pkt.add_option(DhcpOption::ServerId as u8, alloc::vec![192, 168, 1, 1]);
    pkt.add_option(DhcpOption::LeaseTime as u8, 3600u32.to_be_bytes().to_vec());
    pkt.serialize()
}

/// DHCP boot smoketest — drives the full handshake with synthetic server replies
/// and asserts the client reaches Bound. This is the regression fence for the
/// "stuck at Selecting" bug: an OFFER must advance the machine to REQUEST, not
/// loop re-DISCOVERing. Must be able to print FAIL.
pub fn run_boot_smoketest() {
    let mut c = DhcpClient::new(String::from("smoketest0"), [0x02, 0, 0, 0, 0, 1]);

    // 1. DISCOVER -> Selecting.
    let _ = c.start();
    let selecting = c.state == DhcpState::Selecting;
    let xid = c.transaction_id;

    // 2. Server OFFER -> stored, still Selecting.
    let offer = build_reply(xid, [192, 168, 1, 50], DhcpMessageType::Offer);
    let offer_ok =
        c.handle_packet(&offer, 1) == DhcpEvent::OfferReceived && c.offered_lease.is_some();

    // 3. tick() must now send the REQUEST and advance (the fix).
    let (req_pkt, req_ev) = c.tick(2);
    let request_ok =
        req_ev == DhcpEvent::RequestSent && c.state == DhcpState::Requesting && req_pkt.is_some();

    // 4. Server ACK -> Bound with the granted address.
    let ack = build_reply(xid, [192, 168, 1, 50], DhcpMessageType::Ack);
    let bound_ok = c.handle_packet(&ack, 3) == DhcpEvent::Bound
        && c.state == DhcpState::Bound
        && c.current_lease.as_ref().map(|l| l.client_ip) == Some([192, 168, 1, 50]);

    // 5. DoS resilience: a DHCP packet is ATTACKER-CONTROLLED (any host on the
    //    LAN can send offers). Malformed inputs must parse to None / a bounded
    //    result and MUST NOT panic (an out-of-bounds panic on the RX path is a
    //    LAN DoS). Reaching the tally proves no panic.
    let res_empty = DhcpPacket::parse(&[]).is_none();
    let res_short = DhcpPacket::parse(&[0u8; 100]).is_none(); // < 240-byte minimum
                                                              // A 240-byte packet with the right magic cookie but an option whose length
                                                              // byte claims more bytes than remain — the option loop must stop, not panic.
    let mut trunc_opt = alloc::vec![0u8; 240];
    trunc_opt[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);
    trunc_opt.push(53); // MessageType option code
    trunc_opt.push(200); // length = 200, but no bytes follow → must not panic
    let res_optlen = DhcpPacket::parse(&trunc_opt).is_some(); // parses, option dropped
    let res_pass = res_empty && res_short && res_optlen;

    let pass = selecting && offer_ok && request_ok && bound_ok && res_pass;
    crate::selftest::record_smoketest("dhcp_handshake", pass);
    crate::serial_println!(
        "[dhcp] handshake smoketest: select={} offer={} request={} bound={} resilience(empty={} short={} optlen={}) -> {}",
        selecting,
        offer_ok,
        request_ok,
        bound_ok,
        res_empty,
        res_short,
        res_optlen,
        if pass { "PASS" } else { "FAIL" }
    );

    // Live client state (informational).
    let guard = DHCP_CLIENT.lock();
    match guard.as_ref() {
        None => crate::serial_println!("[dhcp] live client: not initialized"),
        Some(live) => {
            let lease_info = live
                .current_lease
                .as_ref()
                .map(|l| {
                    alloc::format!(
                        "ip={}.{}.{}.{} lease={}s",
                        l.client_ip[0],
                        l.client_ip[1],
                        l.client_ip[2],
                        l.client_ip[3],
                        l.lease_time_secs
                    )
                })
                .unwrap_or_else(|| String::from("no-lease"));
            crate::serial_println!("[dhcp] live client: state={:?} {}", live.state, lease_info);
        }
    }
}
