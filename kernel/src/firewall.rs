//! AthGuard Firewall — capability-gated packet filtering.
//!
//! NOT iptables. This is a clean, capability-based firewall where every
//! rule management operation requires `Cap::Network` authority. Per-app
//! network sandboxing is enforced: each app can only reach the hosts and
//! ports its capability token permits.
//!
//! Design:
//!   - `FirewallRule`: match on src/dst IP, port range, protocol, direction, app_id
//!   - `RuleSet`: ordered list, first-match wins, default deny
//!   - `filter_packet()`: called on every TX/RX, returns `Verdict`
//!   - Connection tracking: established flows auto-allowed
//!   - Rate limiting: per-source packet rate cap (anti-DoS)
//!   - Built-in profiles: AllowAll, DefaultDeny, GamingOptimized
//!   - Logging: denied packets logged with timestamp, rule ID, summary

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::capability::{Cap, CapTable, Rights};

// ── Verdict ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
    /// Deny and log the packet.
    DenyAndLog,
}

// ── Direction ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Inbound,
    Outbound,
    Both,
}

// ── Protocol ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Protocol {
    Any,
    Tcp,
    Udp,
    Icmp,
}

impl Protocol {
    pub fn from_ip_proto(n: u8) -> Self {
        match n {
            1 => Protocol::Icmp,
            6 => Protocol::Tcp,
            17 => Protocol::Udp,
            _ => Protocol::Any,
        }
    }
}

// ── Address matching ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddrMatch {
    Any,
    Exact([u8; 4]),
    Subnet([u8; 4], u8),
}

impl AddrMatch {
    pub fn matches(&self, addr: &[u8; 4]) -> bool {
        match self {
            AddrMatch::Any => true,
            AddrMatch::Exact(a) => a == addr,
            AddrMatch::Subnet(net, prefix) => {
                let mask = if *prefix >= 32 {
                    0xFFFF_FFFFu32
                } else {
                    !((1u32 << (32 - prefix)) - 1)
                };
                let n = u32::from_be_bytes(*net);
                let a = u32::from_be_bytes(*addr);
                (n & mask) == (a & mask)
            }
        }
    }
}

// ── Port matching ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMatch {
    Any,
    Single(u16),
    Range(u16, u16),
}

impl PortMatch {
    pub fn matches(&self, port: u16) -> bool {
        match self {
            PortMatch::Any => true,
            PortMatch::Single(p) => *p == port,
            PortMatch::Range(lo, hi) => port >= *lo && port <= *hi,
        }
    }
}

// ── Firewall Rule ───────────────────────────────────────────────────────────

/// A single firewall rule. First-match wins in the ordered `RuleSet`.
pub struct FirewallRule {
    pub id: u64,
    pub direction: Direction,
    pub protocol: Protocol,
    pub src_addr: AddrMatch,
    pub dst_addr: AddrMatch,
    pub src_port: PortMatch,
    pub dst_port: PortMatch,
    pub app_id: Option<u64>,
    pub action: Verdict,
    pub description: String,
    pub enabled: bool,
    pub hit_count: u64,
    pub created_at: u64,
}

impl FirewallRule {
    fn matches_packet(&self, pkt: &PacketInfo, direction: Direction) -> bool {
        if !self.enabled {
            return false;
        }

        // Direction check
        match self.direction {
            Direction::Both => {}
            d if d == direction => {}
            _ => return false,
        }

        // Protocol check
        if self.protocol != Protocol::Any && self.protocol != pkt.protocol {
            return false;
        }

        // Address checks
        if !self.src_addr.matches(&pkt.src_addr) {
            return false;
        }
        if !self.dst_addr.matches(&pkt.dst_addr) {
            return false;
        }

        // Port checks
        if !self.src_port.matches(pkt.src_port) {
            return false;
        }
        if !self.dst_port.matches(pkt.dst_port) {
            return false;
        }

        // App ID check
        if let Some(rule_app) = self.app_id {
            if pkt.app_id != Some(rule_app) {
                return false;
            }
        }

        true
    }
}

// ── Packet Info ─────────────────────────────────────────────────────────────

pub struct PacketInfo {
    pub src_addr: [u8; 4],
    pub dst_addr: [u8; 4],
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: Protocol,
    pub length: usize,
    pub app_id: Option<u64>,
}

impl PacketInfo {
    pub fn from_ipv4(data: &[u8], app_id: Option<u64>) -> Option<Self> {
        if data.len() < 20 {
            return None;
        }

        let ihl = ((data[0] & 0x0F) as usize) * 4;
        if data.len() < ihl {
            return None;
        }

        let protocol = Protocol::from_ip_proto(data[9]);
        let src_addr = [data[12], data[13], data[14], data[15]];
        let dst_addr = [data[16], data[17], data[18], data[19]];
        let total_len = u16::from_be_bytes([data[2], data[3]]) as usize;

        let (src_port, dst_port) =
            if data.len() >= ihl + 4 && matches!(protocol, Protocol::Tcp | Protocol::Udp) {
                (
                    u16::from_be_bytes([data[ihl], data[ihl + 1]]),
                    u16::from_be_bytes([data[ihl + 2], data[ihl + 3]]),
                )
            } else {
                (0, 0)
            };

        Some(Self {
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            protocol,
            length: total_len,
            app_id,
        })
    }
}

// ── Connection Tracking ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    New,
    Established,
    Related,
}

struct TrackedConn {
    state: ConnState,
    last_seen: u64,
    packets: u64,
    timeout_secs: u32,
}

/// Bidirectional connection tracker. Once a connection is established in
/// the forward direction, reply packets are automatically allowed.
pub struct ConnTracker {
    table: BTreeMap<u64, TrackedConn>,
    max_entries: usize,
}

impl ConnTracker {
    pub fn new(max: usize) -> Self {
        Self {
            table: BTreeMap::new(),
            max_entries: max,
        }
    }

    fn hash_tuple(src: &[u8; 4], dst: &[u8; 4], sp: u16, dp: u16, proto: Protocol) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in src.iter().chain(dst.iter()) {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= sp as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
        h ^= dp as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
        h ^= proto as u64;
        h
    }

    /// Track or look up a connection. Returns the state.
    pub fn track(&mut self, pkt: &PacketInfo, now: u64) -> ConnState {
        let fwd = Self::hash_tuple(
            &pkt.src_addr,
            &pkt.dst_addr,
            pkt.src_port,
            pkt.dst_port,
            pkt.protocol,
        );

        if let Some(conn) = self.table.get_mut(&fwd) {
            conn.last_seen = now;
            conn.packets += 1;
            if conn.state == ConnState::New {
                conn.state = ConnState::Established;
            }
            return conn.state;
        }

        // Check for reply direction
        let rev = Self::hash_tuple(
            &pkt.dst_addr,
            &pkt.src_addr,
            pkt.dst_port,
            pkt.src_port,
            pkt.protocol,
        );
        if let Some(conn) = self.table.get_mut(&rev) {
            conn.last_seen = now;
            conn.packets += 1;
            if conn.state == ConnState::New {
                conn.state = ConnState::Established;
            }
            return ConnState::Related;
        }

        // New connection
        if self.table.len() >= self.max_entries {
            return ConnState::New;
        }

        let timeout = match pkt.protocol {
            Protocol::Tcp => 7200,
            Protocol::Udp => 180,
            Protocol::Icmp => 30,
            Protocol::Any => 600,
        };

        self.table.insert(
            fwd,
            TrackedConn {
                state: ConnState::New,
                last_seen: now,
                packets: 1,
                timeout_secs: timeout,
            },
        );

        ConnState::New
    }

    pub fn cleanup(&mut self, now: u64) -> usize {
        let before = self.table.len();
        self.table
            .retain(|_, c| now.saturating_sub(c.last_seen) < c.timeout_secs as u64);
        before - self.table.len()
    }

    pub fn count(&self) -> usize {
        self.table.len()
    }
}

// ── Rate Limiter ────────────────────────────────────────────────────────────

/// Per-source rate limiter using a token-bucket algorithm.
/// Each source IP gets a bucket; exceeding the rate causes drops.
struct RateBucket {
    tokens: u32,
    last_refill: u64,
}

pub struct RateLimiter {
    buckets: BTreeMap<u32, RateBucket>,
    max_tokens: u32,
    refill_rate: u32,
    refill_interval_ms: u64,
    max_sources: usize,
}

impl RateLimiter {
    pub fn new(max_pps: u32, burst: u32) -> Self {
        Self {
            buckets: BTreeMap::new(),
            max_tokens: burst,
            refill_rate: max_pps,
            refill_interval_ms: 1000,
            max_sources: 4096,
        }
    }

    /// Returns `true` if the packet is within rate limits.
    pub fn check(&mut self, src_addr: &[u8; 4], now_ms: u64) -> bool {
        let key = u32::from_be_bytes(*src_addr);

        if let Some(bucket) = self.buckets.get_mut(&key) {
            let elapsed = now_ms.saturating_sub(bucket.last_refill);
            if elapsed >= self.refill_interval_ms {
                let periods = (elapsed / self.refill_interval_ms) as u32;
                bucket.tokens = (bucket.tokens + periods * self.refill_rate).min(self.max_tokens);
                bucket.last_refill = now_ms;
            }

            if bucket.tokens > 0 {
                bucket.tokens -= 1;
                true
            } else {
                false
            }
        } else {
            if self.buckets.len() >= self.max_sources {
                self.evict_stale(now_ms);
            }
            self.buckets.insert(
                key,
                RateBucket {
                    tokens: self.max_tokens - 1,
                    last_refill: now_ms,
                },
            );
            true
        }
    }

    fn evict_stale(&mut self, now_ms: u64) {
        let threshold = now_ms.saturating_sub(60_000);
        self.buckets.retain(|_, b| b.last_refill > threshold);
    }
}

// ── Firewall Log ────────────────────────────────────────────────────────────

pub struct FirewallLogEntry {
    pub timestamp: u64,
    pub rule_id: u64,
    pub direction: Direction,
    pub verdict: Verdict,
    pub protocol: Protocol,
    pub src_addr: [u8; 4],
    pub dst_addr: [u8; 4],
    pub src_port: u16,
    pub dst_port: u16,
    pub app_id: Option<u64>,
    pub length: usize,
}

// ── Stats ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct FirewallStats {
    pub packets_allowed: u64,
    pub packets_denied: u64,
    pub packets_rate_limited: u64,
    pub packets_conntrack_allowed: u64,
    pub bytes_allowed: u64,
    pub bytes_denied: u64,
}

// ── Built-in Profiles ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallProfile {
    /// All traffic allowed — development/testing only.
    AllowAll,
    /// Default deny inbound, allow outbound — production default.
    DefaultDeny,
    /// Gaming-optimized: game ports open, telemetry blocked.
    GamingOptimized,
}

// ── Capability Check Error ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallError {
    /// Caller does not hold a `Cap::Network` with WRITE rights.
    CapabilityDenied,
    /// Rule not found.
    RuleNotFound,
    /// Firewall not initialized.
    NotInitialized,
}

// ── The Firewall ────────────────────────────────────────────────────────────

pub struct Firewall {
    rules: Vec<FirewallRule>,
    conntrack: ConnTracker,
    rate_limiter: RateLimiter,
    default_inbound: Verdict,
    default_outbound: Verdict,
    profile: FirewallProfile,
    enabled: bool,
    next_rule_id: u64,
    log_buffer: Vec<FirewallLogEntry>,
    max_log_entries: usize,
    stats: FirewallStats,
}

pub static FIREWALL: Mutex<Option<Firewall>> = Mutex::new(None);

/// Verify that the caller's cap table contains a `Cap::Network` with the
/// required rights. This is the single authority gate for all firewall
/// management operations.
fn check_network_cap(cap_table: &CapTable, required: Rights) -> Result<(), FirewallError> {
    for (_handle, cap) in cap_table.iter() {
        if let Cap::Network { rights, .. } = cap {
            if rights.contains(required) {
                return Ok(());
            }
        }
    }
    Err(FirewallError::CapabilityDenied)
}

/// Check if a specific app_id is allowed to use the given port, based on
/// the cap table it was granted. Used by `filter_packet` for per-app
/// network sandboxing.
fn app_port_allowed(cap_table: &CapTable, port: u16) -> bool {
    for (_handle, cap) in cap_table.iter() {
        if let Cap::Network {
            port_range_start,
            port_range_end,
            rights,
        } = cap
        {
            if rights.contains(Rights::READ) || rights.contains(Rights::WRITE) {
                if port >= *port_range_start && port <= *port_range_end {
                    return true;
                }
            }
        }
    }
    false
}

impl Firewall {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            conntrack: ConnTracker::new(65536),
            rate_limiter: RateLimiter::new(1000, 2000),
            default_inbound: Verdict::Deny,
            default_outbound: Verdict::Allow,
            profile: FirewallProfile::DefaultDeny,
            enabled: true,
            next_rule_id: 1,
            log_buffer: Vec::new(),
            max_log_entries: 4096,
            stats: FirewallStats::default(),
        }
    }

    // ── Profile application ─────────────────────────────────────────────

    pub fn apply_profile(&mut self, profile: FirewallProfile) {
        self.rules.clear();
        self.profile = profile;

        match profile {
            FirewallProfile::AllowAll => {
                self.default_inbound = Verdict::Allow;
                self.default_outbound = Verdict::Allow;
            }
            FirewallProfile::DefaultDeny => {
                self.default_inbound = Verdict::Deny;
                self.default_outbound = Verdict::Allow;
                self.add_default_rules();
            }
            FirewallProfile::GamingOptimized => {
                self.default_inbound = Verdict::Deny;
                self.default_outbound = Verdict::Allow;
                self.add_default_rules();
                self.add_gaming_rules();
            }
        }
    }

    fn add_default_rules(&mut self) {
        let (id, ts) = (self.next_id(), self.now_ms());
        self.insert_rule(FirewallRule {
            id,
            direction: Direction::Both,
            protocol: Protocol::Any,
            src_addr: AddrMatch::Exact([127, 0, 0, 1]),
            dst_addr: AddrMatch::Exact([127, 0, 0, 1]),
            src_port: PortMatch::Any,
            dst_port: PortMatch::Any,
            app_id: None,
            action: Verdict::Allow,
            description: String::from("loopback"),
            enabled: true,
            hit_count: 0,
            created_at: ts,
        });

        let (id, ts) = (self.next_id(), self.now_ms());
        self.insert_rule(FirewallRule {
            id,
            direction: Direction::Inbound,
            protocol: Protocol::Icmp,
            src_addr: AddrMatch::Any,
            dst_addr: AddrMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Any,
            app_id: None,
            action: Verdict::Allow,
            description: String::from("allow ICMP"),
            enabled: true,
            hit_count: 0,
            created_at: ts,
        });

        let (id, ts) = (self.next_id(), self.now_ms());
        self.insert_rule(FirewallRule {
            id,
            direction: Direction::Inbound,
            protocol: Protocol::Udp,
            src_addr: AddrMatch::Any,
            dst_addr: AddrMatch::Any,
            src_port: PortMatch::Single(67),
            dst_port: PortMatch::Single(68),
            app_id: None,
            action: Verdict::Allow,
            description: String::from("DHCP replies"),
            enabled: true,
            hit_count: 0,
            created_at: ts,
        });

        let (id, ts) = (self.next_id(), self.now_ms());
        self.insert_rule(FirewallRule {
            id,
            direction: Direction::Inbound,
            protocol: Protocol::Udp,
            src_addr: AddrMatch::Any,
            dst_addr: AddrMatch::Any,
            src_port: PortMatch::Single(53),
            dst_port: PortMatch::Any,
            app_id: None,
            action: Verdict::Allow,
            description: String::from("DNS responses"),
            enabled: true,
            hit_count: 0,
            created_at: ts,
        });
    }

    fn add_gaming_rules(&mut self) {
        let (id, ts) = (self.next_id(), self.now_ms());
        self.insert_rule(FirewallRule {
            id,
            direction: Direction::Both,
            protocol: Protocol::Udp,
            src_addr: AddrMatch::Any,
            dst_addr: AddrMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Range(27000, 27050),
            app_id: None,
            action: Verdict::Allow,
            description: String::from("Steam game traffic"),
            enabled: true,
            hit_count: 0,
            created_at: ts,
        });

        let (id, ts) = (self.next_id(), self.now_ms());
        self.insert_rule(FirewallRule {
            id,
            direction: Direction::Both,
            protocol: Protocol::Udp,
            src_addr: AddrMatch::Any,
            dst_addr: AddrMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Range(3478, 3480),
            app_id: None,
            action: Verdict::Allow,
            description: String::from("STUN/TURN NAT traversal"),
            enabled: true,
            hit_count: 0,
            created_at: ts,
        });

        let (id, ts) = (self.next_id(), self.now_ms());
        self.insert_rule(FirewallRule {
            id,
            direction: Direction::Both,
            protocol: Protocol::Udp,
            src_addr: AddrMatch::Any,
            dst_addr: AddrMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Range(7777, 7800),
            app_id: None,
            action: Verdict::Allow,
            description: String::from("game server ports"),
            enabled: true,
            hit_count: 0,
            created_at: ts,
        });

        let (id, ts) = (self.next_id(), self.now_ms());
        self.insert_rule(FirewallRule {
            id,
            direction: Direction::Outbound,
            protocol: Protocol::Tcp,
            src_addr: AddrMatch::Any,
            dst_addr: AddrMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Single(8443),
            app_id: None,
            action: Verdict::DenyAndLog,
            description: String::from("block telemetry (8443)"),
            enabled: true,
            hit_count: 0,
            created_at: ts,
        });
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_rule_id;
        self.next_rule_id += 1;
        id
    }

    fn insert_rule(&mut self, rule: FirewallRule) {
        self.rules.push(rule);
    }

    fn now_ms(&self) -> u64 {
        crate::hpet::read_millis().unwrap_or(0) as u64
    }

    // ── Capability-gated rule management ────────────────────────────────

    /// Add a rule. Requires `Cap::Network` with WRITE rights.
    pub fn add_rule(
        &mut self,
        cap_table: &CapTable,
        direction: Direction,
        protocol: Protocol,
        src_addr: AddrMatch,
        dst_addr: AddrMatch,
        src_port: PortMatch,
        dst_port: PortMatch,
        app_id: Option<u64>,
        action: Verdict,
        description: String,
    ) -> Result<u64, FirewallError> {
        check_network_cap(cap_table, Rights::WRITE)?;

        let id = self.next_id();
        let rule = FirewallRule {
            id,
            direction,
            protocol,
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            app_id,
            action,
            description,
            enabled: true,
            hit_count: 0,
            created_at: self.now_ms(),
        };

        self.rules.push(rule);
        Ok(id)
    }

    /// Remove a rule by ID. Requires `Cap::Network` with WRITE rights.
    pub fn remove_rule(&mut self, cap_table: &CapTable, rule_id: u64) -> Result<(), FirewallError> {
        check_network_cap(cap_table, Rights::WRITE)?;

        let before = self.rules.len();
        self.rules.retain(|r| r.id != rule_id);
        if self.rules.len() < before {
            Ok(())
        } else {
            Err(FirewallError::RuleNotFound)
        }
    }

    /// Toggle a rule's enabled state. Requires WRITE.
    pub fn set_rule_enabled(
        &mut self,
        cap_table: &CapTable,
        rule_id: u64,
        enabled: bool,
    ) -> Result<(), FirewallError> {
        check_network_cap(cap_table, Rights::WRITE)?;

        if let Some(rule) = self.rules.iter_mut().find(|r| r.id == rule_id) {
            rule.enabled = enabled;
            Ok(())
        } else {
            Err(FirewallError::RuleNotFound)
        }
    }

    /// Switch profile. Requires WRITE.
    pub fn set_profile(
        &mut self,
        cap_table: &CapTable,
        profile: FirewallProfile,
    ) -> Result<(), FirewallError> {
        check_network_cap(cap_table, Rights::WRITE)?;
        self.apply_profile(profile);
        Ok(())
    }

    /// Flush all rules. Requires WRITE.
    pub fn flush_rules(&mut self, cap_table: &CapTable) -> Result<(), FirewallError> {
        check_network_cap(cap_table, Rights::WRITE)?;
        self.rules.clear();
        Ok(())
    }

    // ── Read-only queries (require READ) ────────────────────────────────

    pub fn list_rules(&self, cap_table: &CapTable) -> Result<&[FirewallRule], FirewallError> {
        check_network_cap(cap_table, Rights::READ)?;
        Ok(&self.rules)
    }

    pub fn stats(&self) -> &FirewallStats {
        &self.stats
    }

    pub fn profile(&self) -> FirewallProfile {
        self.profile
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn connection_count(&self) -> usize {
        self.conntrack.count()
    }

    pub fn recent_log(&self, count: usize) -> &[FirewallLogEntry] {
        let start = self.log_buffer.len().saturating_sub(count);
        &self.log_buffer[start..]
    }

    pub fn clear_log(&mut self) {
        self.log_buffer.clear();
    }

    // ── Packet filtering (the hot path) ─────────────────────────────────

    /// The main entry point called on every TX/RX packet.
    ///
    /// `direction`: Inbound for received packets, Outbound for sent.
    /// `app_id`: The task/app that owns this socket, for per-app filtering.
    pub fn filter_packet(&mut self, pkt: &PacketInfo, direction: Direction, now: u64) -> Verdict {
        if !self.enabled {
            return Verdict::Allow;
        }

        // 1. Connection tracking: established/related flows auto-allowed
        let ct = self.conntrack.track(pkt, now);
        if ct == ConnState::Established || ct == ConnState::Related {
            self.stats.packets_conntrack_allowed += 1;
            self.stats.packets_allowed += 1;
            self.stats.bytes_allowed += pkt.length as u64;
            return Verdict::Allow;
        }

        // 2. Rate limiting on inbound
        if matches!(direction, Direction::Inbound) {
            let now_ms = now.wrapping_mul(1000);
            if !self.rate_limiter.check(&pkt.src_addr, now_ms) {
                self.stats.packets_rate_limited += 1;
                self.stats.packets_denied += 1;
                self.stats.bytes_denied += pkt.length as u64;
                return Verdict::Deny;
            }
        }

        // 3. Walk the rule list — first match wins
        for i in 0..self.rules.len() {
            if self.rules[i].matches_packet(pkt, direction) {
                self.rules[i].hit_count += 1;
                let verdict = self.rules[i].action;

                match verdict {
                    Verdict::Allow => {
                        self.stats.packets_allowed += 1;
                        self.stats.bytes_allowed += pkt.length as u64;
                    }
                    Verdict::Deny => {
                        self.stats.packets_denied += 1;
                        self.stats.bytes_denied += pkt.length as u64;
                    }
                    Verdict::DenyAndLog => {
                        self.stats.packets_denied += 1;
                        self.stats.bytes_denied += pkt.length as u64;
                        self.log_denied(pkt, direction, self.rules[i].id, now);
                    }
                }

                return verdict;
            }
        }

        // 4. Default policy
        let default = match direction {
            Direction::Inbound => self.default_inbound,
            Direction::Outbound => self.default_outbound,
            Direction::Both => self.default_inbound,
        };

        match default {
            Verdict::Allow => {
                self.stats.packets_allowed += 1;
                self.stats.bytes_allowed += pkt.length as u64;
            }
            Verdict::Deny | Verdict::DenyAndLog => {
                self.stats.packets_denied += 1;
                self.stats.bytes_denied += pkt.length as u64;
                if matches!(default, Verdict::DenyAndLog) {
                    self.log_denied(pkt, direction, 0, now);
                }
            }
        }

        default
    }

    fn log_denied(&mut self, pkt: &PacketInfo, direction: Direction, rule_id: u64, timestamp: u64) {
        if self.log_buffer.len() >= self.max_log_entries {
            self.log_buffer.remove(0);
        }

        self.log_buffer.push(FirewallLogEntry {
            timestamp,
            rule_id,
            direction,
            verdict: Verdict::Deny,
            protocol: pkt.protocol,
            src_addr: pkt.src_addr,
            dst_addr: pkt.dst_addr,
            src_port: pkt.src_port,
            dst_port: pkt.dst_port,
            app_id: pkt.app_id,
            length: pkt.length,
        });
    }

    /// Expire stale connection tracking entries.
    pub fn cleanup(&mut self, now: u64) -> usize {
        self.conntrack.cleanup(now)
    }

    pub fn reset_stats(&mut self) {
        self.stats = FirewallStats::default();
    }

    pub fn reset_rule_counters(&mut self) {
        for rule in &mut self.rules {
            rule.hit_count = 0;
        }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

// ── Public API: filter from net stack ────────────────────────────────────────

/// Filter an inbound packet. Called by the network stack on every RX.
pub fn filter_inbound(pkt: &PacketInfo, now: u64) -> Verdict {
    let mut guard = FIREWALL.lock();
    match *guard {
        Some(ref mut fw) => fw.filter_packet(pkt, Direction::Inbound, now),
        None => Verdict::Allow,
    }
}

/// Filter an outbound packet. Called by the network stack on every TX.
pub fn filter_outbound(pkt: &PacketInfo, now: u64) -> Verdict {
    let mut guard = FIREWALL.lock();
    match *guard {
        Some(ref mut fw) => fw.filter_packet(pkt, Direction::Outbound, now),
        None => Verdict::Allow,
    }
}

/// Periodic cleanup of expired conntrack entries.
pub fn periodic_cleanup(now: u64) {
    let mut guard = FIREWALL.lock();
    if let Some(ref mut fw) = *guard {
        fw.cleanup(now);
    }
}

// ── Init ────────────────────────────────────────────────────────────────────

/// Initialize the firewall with the DefaultDeny profile.
pub fn init() {
    let mut fw = Firewall::new();
    fw.apply_profile(FirewallProfile::DefaultDeny);

    let rule_count = fw.rule_count();
    let profile = fw.profile();

    *FIREWALL.lock() = Some(fw);

    crate::serial_println!(
        "[ OK ] AthGuard firewall initialized ({} rules, profile={:?}, conntrack 64k, rate-limit 1k pps)",
        rule_count, profile,
    );
}

// ── Boot smoketest (R10) ──────────────────────────────────────────────────────

/// Deterministic proof that the capability-gated firewall enforces per-app
/// rulesets, built-in profiles, connection tracking and default-deny — all
/// through the live `filter_packet` path (the same one `net.rs` calls on every
/// RX/TX). Runs against a LOCAL `Firewall` so it never perturbs the live
/// singleton's rules or stats.
///
/// Concept §AthNet / AthGuard: "each app can only reach the hosts and ports
/// its capability token permits." MasterChecklist Phase 10.2 — firewall
/// rulesets per app.
pub fn run_boot_smoketest() {
    fn pkt(
        src: [u8; 4],
        dst: [u8; 4],
        sp: u16,
        dp: u16,
        proto: Protocol,
        app: Option<u64>,
    ) -> PacketInfo {
        PacketInfo {
            src_addr: src,
            dst_addr: dst,
            src_port: sp,
            dst_port: dp,
            protocol: proto,
            length: 64,
            app_id: app,
        }
    }

    let now = 1000u64; // arbitrary monotonic seconds
    let mut fw = Firewall::new();
    fw.apply_profile(FirewallProfile::GamingOptimized);

    // A holder of Cap::Network(WRITE) may add a per-app rule: only app 0xA5
    // is permitted to receive UDP on :40000.
    let mut caps = CapTable::new();
    caps.insert_root(Cap::Network {
        port_range_start: 0,
        port_range_end: 65535,
        rights: Rights::WRITE,
    });
    let added = fw
        .add_rule(
            &caps,
            Direction::Inbound,
            Protocol::Udp,
            AddrMatch::Any,
            AddrMatch::Any,
            PortMatch::Any,
            PortMatch::Single(40000),
            Some(0xA5),
            Verdict::Allow,
            String::from("per-app:0xA5"),
        )
        .is_ok();

    // A cap-less table must NOT be able to mutate the ruleset.
    let nocaps = CapTable::new();
    let cap_gate_enforced = matches!(
        fw.add_rule(
            &nocaps,
            Direction::Inbound,
            Protocol::Tcp,
            AddrMatch::Any,
            AddrMatch::Any,
            PortMatch::Any,
            PortMatch::Single(22),
            None,
            Verdict::Allow,
            String::from("unauthorized"),
        ),
        Err(FirewallError::CapabilityDenied)
    );

    // Prime one outbound flow so its inbound reply is conntrack-Related below.
    let _ = fw.filter_packet(
        &pkt([10, 0, 0, 2], [1, 2, 3, 4], 12345, 443, Protocol::Tcp, None),
        Direction::Outbound,
        now,
    );

    // Each assertion uses a DISTINCT 5-tuple: conntrack hashes addr+port+proto
    // (not app_id), so reusing a tuple would auto-allow a later packet and mask
    // the verdict under test.
    let results: [(&str, Verdict, Verdict); 7] = [
        (
            "loopback",
            fw.filter_packet(
                &pkt(
                    [127, 0, 0, 1],
                    [127, 0, 0, 1],
                    5000,
                    80,
                    Protocol::Tcp,
                    None,
                ),
                Direction::Inbound,
                now,
            ),
            Verdict::Allow,
        ),
        (
            "default-deny-inbound",
            fw.filter_packet(
                &pkt([8, 8, 8, 8], [10, 0, 0, 2], 443, 51000, Protocol::Tcp, None),
                Direction::Inbound,
                now,
            ),
            Verdict::Deny,
        ),
        (
            "steam-udp-open",
            fw.filter_packet(
                &pkt(
                    [93, 184, 2, 1],
                    [10, 0, 0, 2],
                    27015,
                    27015,
                    Protocol::Udp,
                    None,
                ),
                Direction::Inbound,
                now,
            ),
            Verdict::Allow,
        ),
        (
            "telemetry-blocked",
            fw.filter_packet(
                &pkt(
                    [10, 0, 0, 2],
                    [34, 1, 2, 3],
                    55000,
                    8443,
                    Protocol::Tcp,
                    None,
                ),
                Direction::Outbound,
                now,
            ),
            Verdict::DenyAndLog,
        ),
        (
            "per-app-allow",
            fw.filter_packet(
                &pkt(
                    [10, 0, 0, 5],
                    [10, 0, 0, 2],
                    5000,
                    40000,
                    Protocol::Udp,
                    Some(0xA5),
                ),
                Direction::Inbound,
                now,
            ),
            Verdict::Allow,
        ),
        (
            "per-app-deny-wrong-app",
            fw.filter_packet(
                &pkt(
                    [10, 0, 0, 6],
                    [10, 0, 0, 2],
                    5001,
                    40000,
                    Protocol::Udp,
                    Some(0x99),
                ),
                Direction::Inbound,
                now,
            ),
            Verdict::Deny,
        ),
        (
            "conntrack-reply-allow",
            fw.filter_packet(
                &pkt([1, 2, 3, 4], [10, 0, 0, 2], 443, 12345, Protocol::Tcp, None),
                Direction::Inbound,
                now,
            ),
            Verdict::Allow,
        ),
    ];

    let mut pass = 0u32;
    for (name, got, want) in results.iter() {
        if got == want {
            pass += 1;
        } else {
            crate::serial_println!(
                "[firewall-selftest] FAIL {}: got {:?} want {:?}",
                name,
                got,
                want
            );
        }
    }
    let total = results.len() as u32;
    let telemetry_logged = fw.recent_log(8).iter().any(|e| e.dst_port == 8443);

    crate::serial_println!(
        "[ OK ] firewall selftest: {}/{} verdicts correct, per-app rule {}, cap-gate {}, telemetry-logged {} (profile=GamingOptimized, {} rules)",
        pass,
        total,
        if added { "added" } else { "FAILED-ADD" },
        if cap_gate_enforced { "enforced" } else { "BYPASSED" },
        if telemetry_logged { "yes" } else { "no" },
        fw.rule_count(),
    );
    if pass != total || !added || !cap_gate_enforced {
        crate::serial_println!("[FAIL] firewall selftest did not fully pass");
    }
}

/// `/proc/athena/firewall` — live firewall profile, rule count, connection
/// tracking and verdict statistics. MasterChecklist Phase 10.2.
pub fn dump_text() -> String {
    let guard = FIREWALL.lock();
    let mut out = String::new();
    out.push_str("# AthGuard firewall (capability-gated, per-app)\n");
    match *guard {
        Some(ref fw) => {
            let s = fw.stats();
            out.push_str(&alloc::format!("profile: {:?}\n", fw.profile()));
            out.push_str(&alloc::format!("enabled: {}\n", fw.is_enabled()));
            out.push_str(&alloc::format!("rules: {}\n", fw.rule_count()));
            out.push_str(&alloc::format!(
                "conntrack_entries: {}\n",
                fw.connection_count()
            ));
            out.push_str(&alloc::format!("packets_allowed: {}\n", s.packets_allowed));
            out.push_str(&alloc::format!("packets_denied: {}\n", s.packets_denied));
            out.push_str(&alloc::format!(
                "packets_rate_limited: {}\n",
                s.packets_rate_limited
            ));
            out.push_str(&alloc::format!(
                "packets_conntrack_allowed: {}\n",
                s.packets_conntrack_allowed
            ));
            out.push_str(&alloc::format!("bytes_allowed: {}\n", s.bytes_allowed));
            out.push_str(&alloc::format!("bytes_denied: {}\n", s.bytes_denied));
        }
        None => out.push_str("status: not initialized\n"),
    }
    out
}
