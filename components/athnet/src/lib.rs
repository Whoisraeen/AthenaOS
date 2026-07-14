//! AthNet — userspace networking above L3.
//!
//! Userspace networking with built-in WireGuard, QUIC priority, and gaming traffic shaping.
//! See `docs/components/athnet.md` for the design.
// no_std for real builds; std under `cargo test` so the host KAT harness
// (tls_crypto round-trip tests) can link.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

pub mod cookies;
pub mod discovery;
pub mod http1;
pub mod https;
pub mod socket_transport;
#[cfg(feature = "tls13")]
pub mod tls_crypto;
pub mod websocket;

pub use socket_transport::{http_get, http_get_with, RaeSocketTransport, SocketSyscalls};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    ConnectionRefused,
    ConnectionReset,
    TimedOut,
    AddrInUse,
    AddrNotAvailable,
    NotConnected,
    AlreadyConnected,
    InvalidInput,
    DnsResolutionFailed,
    PoolExhausted,
    WouldBlock,
    Other(String),
}

pub type Result<T> = core::result::Result<T, NetError>;

// ---------------------------------------------------------------------------
// Address types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv4Addr {
    pub octets: [u8; 4],
}

impl Ipv4Addr {
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self {
            octets: [a, b, c, d],
        }
    }

    pub const LOCALHOST: Self = Self::new(127, 0, 0, 1);
    pub const UNSPECIFIED: Self = Self::new(0, 0, 0, 0);
    pub const BROADCAST: Self = Self::new(255, 255, 255, 255);

    pub const fn octets(&self) -> [u8; 4] {
        self.octets
    }

    pub fn is_loopback(&self) -> bool {
        self.octets[0] == 127
    }

    pub fn is_private(&self) -> bool {
        matches!(
            (self.octets[0], self.octets[1]),
            (10, _) | (172, 16..=31) | (192, 168)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SocketAddr {
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl SocketAddr {
    pub const fn new(ip: Ipv4Addr, port: u16) -> Self {
        Self { ip, port }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SocketType {
    Tcp,
    Udp,
    Raw,
}

// ---------------------------------------------------------------------------
// Socket trait
// ---------------------------------------------------------------------------

pub trait Socket {
    fn bind(&mut self, addr: SocketAddr) -> Result<()>;
    fn connect(&mut self, addr: SocketAddr) -> Result<()>;
    fn send(&mut self, buf: &[u8]) -> Result<usize>;
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize>;
    fn close(&mut self) -> Result<()>;
    fn local_addr(&self) -> Option<SocketAddr>;
    fn remote_addr(&self) -> Option<SocketAddr>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketState {
    Unbound,
    Bound,
    Connected,
    Listening,
    Closed,
}

#[derive(Debug, Clone)]
pub struct RawSocket {
    pub socket_type: SocketType,
    pub state: SocketState,
    pub local: Option<SocketAddr>,
    pub remote: Option<SocketAddr>,
    pub recv_buffer: Vec<u8>,
    pub send_buffer: Vec<u8>,
}

impl RawSocket {
    pub fn new(socket_type: SocketType) -> Self {
        Self {
            socket_type,
            state: SocketState::Unbound,
            local: None,
            remote: None,
            recv_buffer: Vec::new(),
            send_buffer: Vec::new(),
        }
    }
}

impl Socket for RawSocket {
    fn bind(&mut self, addr: SocketAddr) -> Result<()> {
        if self.state != SocketState::Unbound {
            return Err(NetError::AddrInUse);
        }
        self.local = Some(addr);
        self.state = SocketState::Bound;
        Ok(())
    }

    fn connect(&mut self, addr: SocketAddr) -> Result<()> {
        if self.state == SocketState::Connected {
            return Err(NetError::AlreadyConnected);
        }
        self.remote = Some(addr);
        self.state = SocketState::Connected;
        Ok(())
    }

    fn send(&mut self, buf: &[u8]) -> Result<usize> {
        if self.state != SocketState::Connected {
            return Err(NetError::NotConnected);
        }
        self.send_buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.state != SocketState::Connected {
            return Err(NetError::NotConnected);
        }
        if self.recv_buffer.is_empty() {
            return Err(NetError::WouldBlock);
        }
        let len = buf.len().min(self.recv_buffer.len());
        buf[..len].copy_from_slice(&self.recv_buffer[..len]);
        self.recv_buffer.drain(..len);
        Ok(len)
    }

    fn close(&mut self) -> Result<()> {
        self.state = SocketState::Closed;
        self.recv_buffer.clear();
        self.send_buffer.clear();
        Ok(())
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote
    }
}

// ---------------------------------------------------------------------------
// DNS resolver
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsRecordType {
    A,
    AAAA,
    CNAME,
    MX,
    TXT,
    NS,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsQuery {
    pub hostname: String,
    pub record_type: DnsRecordType,
    pub id: u16,
}

impl DnsQuery {
    pub fn a_record(hostname: String, id: u16) -> Self {
        Self {
            hostname,
            record_type: DnsRecordType::A,
            id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsRecord {
    pub name: String,
    pub record_type: DnsRecordType,
    pub ttl: u32,
    pub data: DnsRecordData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsRecordData {
    A(Ipv4Addr),
    CName(String),
    Other(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsResponse {
    pub id: u16,
    pub authoritative: bool,
    pub records: Vec<DnsRecord>,
}

#[derive(Debug, Clone)]
struct DnsCacheEntry {
    addresses: Vec<Ipv4Addr>,
    ttl: u32,
    inserted_at: u64,
}

#[derive(Debug, Clone)]
pub struct DnsCache {
    entries: Vec<(String, DnsCacheEntry)>,
    max_entries: usize,
}

impl DnsCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    pub fn insert(&mut self, hostname: String, addresses: Vec<Ipv4Addr>, ttl: u32, now: u64) {
        if let Some(pos) = self.entries.iter().position(|(h, _)| *h == hostname) {
            self.entries.remove(pos);
        }
        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }
        self.entries.push((
            hostname,
            DnsCacheEntry {
                addresses,
                ttl,
                inserted_at: now,
            },
        ));
    }

    pub fn lookup(&self, hostname: &str, now: u64) -> Option<Vec<Ipv4Addr>> {
        self.entries.iter().find_map(|(h, entry)| {
            if h == hostname && now.saturating_sub(entry.inserted_at) < entry.ttl as u64 {
                Some(entry.addresses.clone())
            } else {
                None
            }
        })
    }

    pub fn evict_expired(&mut self, now: u64) {
        self.entries
            .retain(|(_, entry)| now.saturating_sub(entry.inserted_at) < entry.ttl as u64);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct DnsResolver {
    pub servers: Vec<Ipv4Addr>,
    pub cache: DnsCache,
    pub timeout_ms: u64,
}

impl DnsResolver {
    pub fn new(servers: Vec<Ipv4Addr>) -> Self {
        Self {
            servers,
            cache: DnsCache::new(256),
            timeout_ms: 5000,
        }
    }

    pub fn resolve(&mut self, hostname: &str, now: u64) -> Result<Vec<Ipv4Addr>> {
        if let Some(cached) = self.cache.lookup(hostname, now) {
            return Ok(cached);
        }

        if self.servers.is_empty() {
            return Err(NetError::DnsResolutionFailed);
        }

        // In a real implementation this would send a UDP packet to the DNS server.
        // For now, return an error indicating no network path is available.
        Err(NetError::DnsResolutionFailed)
    }

    pub fn resolve_with_response(&mut self, response: DnsResponse, now: u64) -> Vec<Ipv4Addr> {
        let mut addrs = Vec::new();
        let mut hostname = String::new();
        let mut ttl = 300u32;

        for record in &response.records {
            if let DnsRecordData::A(addr) = &record.data {
                addrs.push(*addr);
                hostname = record.name.clone();
                ttl = record.ttl;
            }
        }

        if !addrs.is_empty() && !hostname.is_empty() {
            self.cache.insert(hostname, addrs.clone(), ttl, now);
        }

        addrs
    }
}

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    GET,
    POST,
    PUT,
    DELETE,
    HEAD,
    PATCH,
    OPTIONS,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GET => "GET",
            Self::POST => "POST",
            Self::PUT => "PUT",
            Self::DELETE => "DELETE",
            Self::HEAD => "HEAD",
            Self::PATCH => "PATCH",
            Self::OPTIONS => "OPTIONS",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

impl HttpHeader {
    pub fn new(name: String, value: String) -> Self {
        Self { name, value }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn new(method: HttpMethod, url: String) -> Self {
        Self {
            method,
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn with_header(mut self, name: String, value: String) -> Self {
        self.headers.push(HttpHeader::new(name, value));
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    pub fn get(url: String) -> Self {
        Self::new(HttpMethod::GET, url)
    }

    pub fn post(url: String, body: Vec<u8>) -> Self {
        Self::new(HttpMethod::POST, url).with_body(body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub headers: Vec<HttpHeader>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status_code: u16) -> Self {
        Self {
            status_code,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status_code)
    }

    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status_code)
    }

    pub fn is_client_error(&self) -> bool {
        (400..500).contains(&self.status_code)
    }

    pub fn is_server_error(&self) -> bool {
        (500..600).contains(&self.status_code)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct HttpClient {
    pub default_headers: Vec<HttpHeader>,
    pub timeout_ms: u64,
    pub max_redirects: u8,
}

impl HttpClient {
    pub fn new() -> Self {
        Self {
            default_headers: Vec::new(),
            timeout_ms: 30_000,
            max_redirects: 10,
        }
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    pub fn with_default_header(mut self, name: String, value: String) -> Self {
        self.default_headers.push(HttpHeader::new(name, value));
        self
    }

    pub fn request(&self, req: &HttpRequest) -> Result<HttpResponse> {
        // In a real implementation this would serialize the HTTP request,
        // open a TCP connection, send, and parse the response.
        let _ = req;
        Err(NetError::NotConnected)
    }

    pub fn serialize_request(&self, req: &HttpRequest) -> Vec<u8> {
        let mut buf = Vec::new();

        // Request line
        buf.extend_from_slice(req.method.as_str().as_bytes());
        buf.push(b' ');
        buf.extend_from_slice(req.url.as_bytes());
        buf.extend_from_slice(b" HTTP/1.1\r\n");

        // Default headers
        for header in &self.default_headers {
            buf.extend_from_slice(header.name.as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(header.value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        // Request headers
        for header in &req.headers {
            buf.extend_from_slice(header.name.as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(header.value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        // Body
        if let Some(body) = &req.body {
            let len_str = alloc::format!("{}", body.len());
            buf.extend_from_slice(b"Content-Length: ");
            buf.extend_from_slice(len_str.as_bytes());
            buf.extend_from_slice(b"\r\n\r\n");
            buf.extend_from_slice(body);
        } else {
            buf.extend_from_slice(b"\r\n");
        }

        buf
    }
}

// ---------------------------------------------------------------------------
// Traffic shaping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrafficClass {
    Gaming,
    Interactive,
    Bulk,
    Background,
}

impl TrafficClass {
    pub fn priority(&self) -> u8 {
        match self {
            Self::Gaming => 0,
            Self::Interactive => 1,
            Self::Bulk => 2,
            Self::Background => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QosPolicy {
    pub class: TrafficClass,
    pub bandwidth_limit_kbps: u32,
    pub priority_weight: u16,
    pub burst_bytes: u32,
    pub latency_target_ms: u16,
}

impl QosPolicy {
    pub fn new(class: TrafficClass) -> Self {
        let (bandwidth, weight, burst, latency) = match class {
            TrafficClass::Gaming => (0, 100, 4096, 5),
            TrafficClass::Interactive => (0, 80, 8192, 20),
            TrafficClass::Bulk => (0, 40, 65536, 200),
            TrafficClass::Background => (0, 10, 16384, 1000),
        };
        Self {
            class,
            bandwidth_limit_kbps: bandwidth,
            priority_weight: weight,
            burst_bytes: burst,
            latency_target_ms: latency,
        }
    }

    pub fn with_bandwidth_limit(mut self, kbps: u32) -> Self {
        self.bandwidth_limit_kbps = kbps;
        self
    }

    pub fn with_priority_weight(mut self, weight: u16) -> Self {
        self.priority_weight = weight;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ShaperStats {
    pub packets_sent: u64,
    pub packets_dropped: u64,
    pub bytes_sent: u64,
    pub bytes_queued: u64,
}

#[derive(Debug, Clone)]
pub struct TrafficShaper {
    pub policies: Vec<QosPolicy>,
    /// Per-class FIFO queues (index = `TrafficClass::priority`). `VecDeque` for
    /// O(1) front-removal — the gaming hot path must not pay an O(n) shift +
    /// packet clone per dequeue ("fast is a feature").
    pub queues: [VecDeque<Vec<u8>>; 4],
    pub stats: ShaperStats,
    pub enabled: bool,
}

impl TrafficShaper {
    pub fn new() -> Self {
        Self {
            policies: alloc::vec![
                QosPolicy::new(TrafficClass::Gaming),
                QosPolicy::new(TrafficClass::Interactive),
                QosPolicy::new(TrafficClass::Bulk),
                QosPolicy::new(TrafficClass::Background),
            ],
            queues: [
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
            ],
            stats: ShaperStats {
                packets_sent: 0,
                packets_dropped: 0,
                bytes_sent: 0,
                bytes_queued: 0,
            },
            enabled: true,
        }
    }

    pub fn classify(&self, dst_port: u16) -> TrafficClass {
        match dst_port {
            // Common game server ports
            27000..=27100 | 7777..=7800 | 3074 => TrafficClass::Gaming,
            // Interactive: SSH, HTTP/S, DNS
            22 | 80 | 443 | 53 | 8080 => TrafficClass::Interactive,
            // Bulk: FTP, large transfers
            20 | 21 | 8000..=8999 => TrafficClass::Bulk,
            _ => TrafficClass::Background,
        }
    }

    pub fn enqueue(&mut self, packet: Vec<u8>, class: TrafficClass) {
        let idx = class.priority() as usize;
        self.stats.bytes_queued += packet.len() as u64;
        self.queues[idx].push_back(packet);
    }

    pub fn dequeue(&mut self) -> Option<Vec<u8>> {
        if !self.enabled {
            return None;
        }
        for queue in &mut self.queues {
            if let Some(packet) = queue.pop_front() {
                self.stats.packets_sent += 1;
                self.stats.bytes_sent += packet.len() as u64;
                self.stats.bytes_queued =
                    self.stats.bytes_queued.saturating_sub(packet.len() as u64);
                return Some(packet);
            }
        }
        None
    }

    pub fn set_policy(&mut self, policy: QosPolicy) {
        if let Some(existing) = self.policies.iter_mut().find(|p| p.class == policy.class) {
            *existing = policy;
        } else {
            self.policies.push(policy);
        }
    }

    pub fn pending_packets(&self) -> usize {
        self.queues.iter().map(|q| q.len()).sum()
    }
}

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305,
};

// ---------------------------------------------------------------------------
// WireGuard types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireGuardConfig {
    pub private_key: [u8; 32],
    pub public_key: [u8; 32],
    pub endpoint: SocketAddr,
    pub allowed_ips: Vec<(Ipv4Addr, u8)>,
    pub keepalive_secs: u16,
}

impl WireGuardConfig {
    pub fn new(private_key: [u8; 32], public_key: [u8; 32], endpoint: SocketAddr) -> Self {
        Self {
            private_key,
            public_key,
            endpoint,
            allowed_ips: Vec::new(),
            keepalive_secs: 25,
        }
    }

    pub fn with_allowed_ip(mut self, ip: Ipv4Addr, prefix_len: u8) -> Self {
        self.allowed_ips.push((ip, prefix_len));
        self
    }

    pub fn with_keepalive(mut self, secs: u16) -> Self {
        self.keepalive_secs = secs;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    Down,
    Handshaking,
    Established,
}

#[derive(Debug, Clone)]
pub struct WireGuardTunnel {
    pub config: WireGuardConfig,
    pub state: TunnelState,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_counter: u64,
    pub rx_counter: u64,
}

impl WireGuardTunnel {
    pub fn new(config: WireGuardConfig) -> Self {
        Self {
            config,
            state: TunnelState::Down,
            tx_bytes: 0,
            rx_bytes: 0,
            tx_counter: 0,
            rx_counter: 0,
        }
    }

    pub fn handshake(&mut self) -> Result<()> {
        // Real implementation would perform Noise_IK handshake
        // For now, move state to handshaking
        self.state = TunnelState::Handshaking;
        Ok(())
    }

    pub fn establish(&mut self) -> Result<()> {
        self.state = TunnelState::Established;
        Ok(())
    }

    pub fn encrypt_packet(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        if self.state != TunnelState::Established {
            return Err(NetError::NotConnected);
        }

        let cipher = ChaCha20Poly1305::new(self.config.private_key.as_slice().into());
        let mut n = [0u8; 12];
        n[4..12].copy_from_slice(&self.tx_counter.to_le_bytes());

        let payload = Payload {
            msg: plaintext,
            aad: &[],
        };

        match cipher.encrypt(&n.into(), payload) {
            Ok(ciphertext) => {
                self.tx_counter += 1;
                self.tx_bytes += plaintext.len() as u64;
                Ok(ciphertext)
            }
            Err(_) => Err(NetError::InvalidInput),
        }
    }

    pub fn decrypt_packet(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if self.state != TunnelState::Established {
            return Err(NetError::NotConnected);
        }

        let cipher = ChaCha20Poly1305::new(self.config.private_key.as_slice().into());
        let mut n = [0u8; 12];
        n[4..12].copy_from_slice(&self.rx_counter.to_le_bytes());

        let payload = Payload {
            msg: ciphertext,
            aad: &[],
        };

        match cipher.decrypt(&n.into(), payload) {
            Ok(plaintext) => {
                self.rx_counter += 1;
                self.rx_bytes += plaintext.len() as u64;
                Ok(plaintext)
            }
            Err(_) => Err(NetError::InvalidInput),
        }
    }

    pub fn is_established(&self) -> bool {
        self.state == TunnelState::Established
    }
}

// ---------------------------------------------------------------------------
// R10 Artifacts
// ---------------------------------------------------------------------------

static mut RESOLVER: Option<DnsResolver> = None;

/// Initialize the global networking services.
pub fn init() {
    unsafe {
        RESOLVER = Some(DnsResolver::new(alloc::vec![Ipv4Addr::new(8, 8, 8, 8)]));
    }
}

/// Prove behavioral correctness of the WireGuard tunnel and crypto.
pub fn run_boot_smoketest() -> bool {
    let config = WireGuardConfig::new(
        [0x01; 32],
        [0x02; 32],
        SocketAddr::new(Ipv4Addr::new(1, 2, 3, 4), 51820),
    );
    let mut tunnel = WireGuardTunnel::new(config);
    tunnel.establish().unwrap();

    let plaintext = b"AthNet Crypto Proof";
    let encrypted = tunnel.encrypt_packet(plaintext);
    if encrypted.is_err() {
        return false;
    }

    let decrypted = tunnel.decrypt_packet(&encrypted.unwrap());
    if decrypted.is_err() {
        return false;
    }

    decrypted.unwrap() == *plaintext
}

// ---------------------------------------------------------------------------
// Connection pool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PoolEntry<T> {
    connection: T,
    host: String,
    idle_since: u64,
}

#[derive(Debug, Clone)]
pub struct ConnectionPool<T> {
    entries: Vec<PoolEntry<T>>,
    pub max_per_host: usize,
    pub max_total: usize,
    pub idle_timeout_ms: u64,
}

impl<T: Clone + core::fmt::Debug> ConnectionPool<T> {
    pub fn new(max_per_host: usize, max_total: usize, idle_timeout_ms: u64) -> Self {
        Self {
            entries: Vec::new(),
            max_per_host,
            max_total,
            idle_timeout_ms,
        }
    }

    pub fn acquire(&mut self, host: &str, now: u64) -> Option<T> {
        self.evict_expired(now);
        if let Some(pos) = self.entries.iter().position(|e| e.host == host) {
            let entry = self.entries.remove(pos);
            Some(entry.connection)
        } else {
            None
        }
    }

    pub fn release(&mut self, host: String, connection: T, now: u64) -> Result<()> {
        self.evict_expired(now);

        let host_count = self.entries.iter().filter(|e| e.host == host).count();
        if host_count >= self.max_per_host {
            return Err(NetError::PoolExhausted);
        }
        if self.entries.len() >= self.max_total {
            return Err(NetError::PoolExhausted);
        }

        self.entries.push(PoolEntry {
            connection,
            host,
            idle_since: now,
        });
        Ok(())
    }

    pub fn evict_expired(&mut self, now: u64) {
        let timeout = self.idle_timeout_ms;
        self.entries
            .retain(|e| now.saturating_sub(e.idle_since) < timeout);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn connections_for_host(&self, host: &str) -> usize {
        self.entries.iter().filter(|e| e.host == host).count()
    }
}
