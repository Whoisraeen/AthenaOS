//! Network connection manager — unified interface, routing, and status management.
//!
//! Coordinates DHCP clients, DNS resolvers, the routing table, WiFi scanning
//! and association, and VPN tunnel lifecycle for all network interfaces.
//! Integrates with the smoltcp stack, firewall, and traffic shaper.

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::dhcp::DhcpClient;
use crate::dns::DnsResolver;

// ── Interface Types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceType {
    Ethernet,
    Wifi,
    Loopback,
    Bridge,
    Tunnel,
    Virtual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceState {
    Up,
    Down,
    Dormant,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMethod {
    Dhcp,
    Static,
    LinkLocal,
}

// ── Interface Stats ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct InterfaceStats {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
}

// ── IP Configuration ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Ipv4Config {
    pub address: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: Option<[u8; 4]>,
    pub dns: Vec<[u8; 4]>,
    pub method: ConfigMethod,
}

impl Ipv4Config {
    pub fn new_static(address: [u8; 4], netmask: [u8; 4], gateway: Option<[u8; 4]>) -> Self {
        Self {
            address,
            netmask,
            gateway,
            dns: Vec::new(),
            method: ConfigMethod::Static,
        }
    }

    pub fn new_dhcp() -> Self {
        Self {
            address: [0; 4],
            netmask: [0; 4],
            gateway: None,
            dns: Vec::new(),
            method: ConfigMethod::Dhcp,
        }
    }

    pub fn from_dhcp_lease(lease: &crate::dhcp::DhcpLease) -> Self {
        Self {
            address: lease.client_ip,
            netmask: lease.subnet_mask,
            gateway: if lease.gateway != [0; 4] {
                Some(lease.gateway)
            } else {
                None
            },
            dns: lease.dns_servers.clone(),
            method: ConfigMethod::Dhcp,
        }
    }

    pub fn network_address(&self) -> [u8; 4] {
        [
            self.address[0] & self.netmask[0],
            self.address[1] & self.netmask[1],
            self.address[2] & self.netmask[2],
            self.address[3] & self.netmask[3],
        ]
    }

    pub fn broadcast_address(&self) -> [u8; 4] {
        [
            self.address[0] | !self.netmask[0],
            self.address[1] | !self.netmask[1],
            self.address[2] | !self.netmask[2],
            self.address[3] | !self.netmask[3],
        ]
    }

    pub fn prefix_len(&self) -> u8 {
        u32::from_be_bytes(self.netmask).count_ones() as u8
    }
}

#[derive(Debug, Clone)]
pub struct Ipv6Config {
    pub address: [u8; 16],
    pub prefix_len: u8,
    pub gateway: Option<[u8; 16]>,
}

// ── Network Interface ───────────────────────────────────────────────────────

pub struct NetworkInterface {
    pub name: String,
    pub iface_type: InterfaceType,
    pub mac_address: [u8; 6],
    pub ipv4: Option<Ipv4Config>,
    pub ipv6: Option<Ipv6Config>,
    pub state: InterfaceState,
    pub mtu: u32,
    pub speed_mbps: u32,
    pub stats: InterfaceStats,
}

impl NetworkInterface {
    pub fn new(name: String, iface_type: InterfaceType, mac: [u8; 6]) -> Self {
        Self {
            name,
            iface_type,
            mac_address: mac,
            ipv4: None,
            ipv6: None,
            state: InterfaceState::Down,
            mtu: 1500,
            speed_mbps: 0,
            stats: InterfaceStats::default(),
        }
    }

    pub fn is_up(&self) -> bool {
        self.state == InterfaceState::Up
    }

    pub fn bring_up(&mut self) {
        self.state = InterfaceState::Up;
    }

    pub fn bring_down(&mut self) {
        self.state = InterfaceState::Down;
    }

    pub fn has_ip(&self) -> bool {
        self.ipv4.as_ref().map_or(false, |c| c.address != [0; 4])
    }

    pub fn mac_string(&self) -> String {
        use core::fmt::Write;
        let mut s = String::with_capacity(17);
        for (i, b) in self.mac_address.iter().enumerate() {
            if i > 0 {
                s.push(':');
            }
            let _ = write!(s, "{:02x}", b);
        }
        s
    }
}

// ── Routing ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub destination: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub interface: String,
    pub metric: u32,
    pub flags: u32,
}

pub const ROUTE_FLAG_UP: u32 = 0x0001;
pub const ROUTE_FLAG_GATEWAY: u32 = 0x0002;
pub const ROUTE_FLAG_HOST: u32 = 0x0004;
pub const ROUTE_FLAG_DEFAULT: u32 = 0x0008;

impl RouteEntry {
    pub fn is_default(&self) -> bool {
        self.destination == [0, 0, 0, 0] && self.netmask == [0, 0, 0, 0]
    }

    pub fn matches(&self, addr: &[u8; 4]) -> bool {
        let d = u32::from_be_bytes(self.destination);
        let m = u32::from_be_bytes(self.netmask);
        let a = u32::from_be_bytes(*addr);
        (a & m) == (d & m)
    }

    fn prefix_len(&self) -> u32 {
        u32::from_be_bytes(self.netmask).count_ones()
    }
}

// ── Network Status ──────────────────────────────────────────────────────────

/// High-level network connectivity status, similar to what desktop
/// environments display in the system tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkStatus {
    /// No network interfaces are up.
    Disconnected,
    /// An interface is up but has no IP address yet (DHCP in progress).
    Connecting,
    /// IP is configured, local network reachable, but no default gateway.
    Limited,
    /// IP and gateway configured but internet connectivity unverified.
    Connected,
    /// Internet connectivity confirmed (DNS resolution works).
    FullConnectivity,
}

// ── WiFi ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiSecurity {
    Open,
    Wep,
    WpaPsk,
    Wpa2Psk,
    Wpa3Sae,
    Wpa2Enterprise,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct WifiNetwork {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub signal_strength: i8,
    pub security: WifiSecurity,
    pub frequency_mhz: u32,
    pub channel: u8,
}

#[derive(Debug, Clone)]
pub struct SavedWifiNetwork {
    pub ssid: String,
    pub security: WifiSecurity,
    pub password: Option<String>,
    pub auto_connect: bool,
    pub last_connected: u64,
    pub priority: i32,
}

pub struct WifiManager {
    pub interface: Option<String>,
    pub scan_results: Vec<WifiNetwork>,
    pub connected_network: Option<WifiNetwork>,
    pub known_networks: Vec<SavedWifiNetwork>,
    pub scanning: bool,
}

impl WifiManager {
    pub fn new() -> Self {
        Self {
            interface: None,
            scan_results: Vec::new(),
            connected_network: None,
            known_networks: Vec::new(),
            scanning: false,
        }
    }

    pub fn start_scan(&mut self) -> bool {
        if self.interface.is_none() || self.scanning {
            return false;
        }
        self.scanning = true;
        self.scan_results.clear();
        true
    }

    pub fn finish_scan(&mut self, results: Vec<WifiNetwork>) {
        self.scan_results = results;
        self.scan_results
            .sort_by(|a, b| b.signal_strength.cmp(&a.signal_strength));
        self.scanning = false;
    }

    pub fn connect(&mut self, ssid: &str, password: Option<&str>) -> WifiConnectResult {
        let network = match self.scan_results.iter().find(|n| n.ssid == ssid) {
            Some(n) => n.clone(),
            None => return WifiConnectResult::NetworkNotFound,
        };

        if network.security != WifiSecurity::Open && password.is_none() {
            return WifiConnectResult::PasswordRequired;
        }

        self.connected_network = Some(network);
        WifiConnectResult::Connected
    }

    pub fn disconnect(&mut self) -> bool {
        if self.connected_network.is_some() {
            self.connected_network = None;
            return true;
        }
        false
    }

    pub fn save_network(&mut self, ssid: String, security: WifiSecurity, password: Option<String>) {
        if let Some(existing) = self.known_networks.iter_mut().find(|n| n.ssid == ssid) {
            existing.password = password;
            existing.security = security;
            return;
        }

        self.known_networks.push(SavedWifiNetwork {
            ssid,
            security,
            password,
            auto_connect: true,
            last_connected: 0,
            priority: 0,
        });
    }

    pub fn forget_network(&mut self, ssid: &str) -> bool {
        let before = self.known_networks.len();
        self.known_networks.retain(|n| n.ssid != ssid);
        self.known_networks.len() < before
    }

    pub fn is_connected(&self) -> bool {
        self.connected_network.is_some()
    }

    pub fn signal_strength(&self) -> Option<i8> {
        self.connected_network.as_ref().map(|n| n.signal_strength)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiConnectResult {
    Connected,
    NetworkNotFound,
    PasswordRequired,
    AuthFailed,
    Timeout,
    Failed,
}

// ── VPN ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnType {
    WireGuard,
    OpenVpn,
    IpSec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Failed,
}

#[derive(Debug, Clone)]
pub struct VpnConfig {
    pub server_addr: [u8; 4],
    pub server_port: u16,
    pub local_addr: Option<[u8; 4]>,
    pub dns: Vec<[u8; 4]>,
    pub mtu: u32,
    pub keep_alive_secs: u32,
}

#[derive(Debug, Clone, Default)]
pub struct VpnStats {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub packets_in: u64,
    pub packets_out: u64,
    pub connected_since: u64,
    pub handshakes: u64,
}

pub struct VpnConnection {
    pub name: String,
    pub vpn_type: VpnType,
    pub state: VpnState,
    pub config: VpnConfig,
    pub stats: VpnStats,
}

// ── Active Connection ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionType {
    Wired,
    Wireless,
    Vpn,
    Loopback,
}

pub struct ActiveConnection {
    pub id: u64,
    pub conn_type: ConnectionType,
    pub interface_name: String,
    pub connected_at: u64,
    pub default_route: bool,
}

// ── Network Manager ─────────────────────────────────────────────────────────

pub struct NetworkManager {
    interfaces: Vec<NetworkInterface>,
    active_connections: Vec<ActiveConnection>,
    dhcp_clients: BTreeMap<String, DhcpClient>,
    dns_resolver: DnsResolver,
    routing_table: Vec<RouteEntry>,
    wifi_manager: WifiManager,
    vpn_connections: Vec<VpnConnection>,
    status: NetworkStatus,
    next_conn_id: u64,
    /// Whether we've done an internet connectivity check.
    connectivity_checked: bool,
    /// Wi-Fi software-radio state for the Quick Settings / Control Center
    /// toggle. Defaults ON; turning it OFF administratively downs wireless
    /// interfaces (the "airplane mode" affordance every switcher expects).
    wifi_radio_enabled: bool,
}

pub static NET_MANAGER: Mutex<Option<NetworkManager>> = Mutex::new(None);

impl NetworkManager {
    pub fn new() -> Self {
        Self {
            interfaces: Vec::new(),
            active_connections: Vec::new(),
            dhcp_clients: BTreeMap::new(),
            dns_resolver: DnsResolver::new(),
            routing_table: Vec::new(),
            wifi_manager: WifiManager::new(),
            vpn_connections: Vec::new(),
            status: NetworkStatus::Disconnected,
            next_conn_id: 1,
            connectivity_checked: false,
            wifi_radio_enabled: true,
        }
    }

    /// Live Wi-Fi software-radio state (Quick Settings toggle).
    pub fn wifi_radio_enabled(&self) -> bool {
        self.wifi_radio_enabled
    }

    /// Set the Wi-Fi software radio. When turned off, wireless interfaces are
    /// administratively brought down (their link goes to `Down`); turning it
    /// back on marks them up so DHCP/association can resume.
    pub fn set_wifi_radio(&mut self, enabled: bool) {
        self.wifi_radio_enabled = enabled;
        for iface in self.interfaces.iter_mut() {
            if iface.iface_type == InterfaceType::Wifi {
                if enabled {
                    iface.bring_up();
                } else {
                    iface.bring_down();
                }
            }
        }
    }

    pub fn init(&mut self) {
        // Create loopback interface
        let mut lo = NetworkInterface::new(String::from("lo"), InterfaceType::Loopback, [0; 6]);
        lo.ipv4 = Some(Ipv4Config::new_static([127, 0, 0, 1], [255, 0, 0, 0], None));
        lo.bring_up();
        self.interfaces.push(lo);

        // Loopback route
        self.routing_table.push(RouteEntry {
            destination: [127, 0, 0, 0],
            netmask: [255, 0, 0, 0],
            gateway: [0; 4],
            interface: String::from("lo"),
            metric: 0,
            flags: ROUTE_FLAG_UP,
        });

        // Set up primary ethernet from NetDriverManager or virtio-net
        let primary_mac = {
            let guard = crate::net_drivers::NET_DRIVERS.lock();
            guard
                .as_ref()
                .and_then(|mgr| mgr.default_driver())
                .map(|drv| drv.mac_address())
        }
        .or_else(|| crate::virtio_net::VIRTIO_NET.get().map(|net| net.mac()));

        if let Some(mac) = primary_mac {
            let mut eth0 =
                NetworkInterface::new(String::from("eth0"), InterfaceType::Ethernet, mac);
            eth0.ipv4 = Some(Ipv4Config::new_static(
                [10, 0, 2, 15],
                [255, 255, 255, 0],
                Some([10, 0, 2, 2]),
            ));
            eth0.speed_mbps = 1000;
            eth0.bring_up();
            self.interfaces.push(eth0);

            // Default route
            self.routing_table.push(RouteEntry {
                destination: [0; 4],
                netmask: [0; 4],
                gateway: [10, 0, 2, 2],
                interface: String::from("eth0"),
                metric: 100,
                flags: ROUTE_FLAG_UP | ROUTE_FLAG_GATEWAY | ROUTE_FLAG_DEFAULT,
            });

            // Local subnet
            self.routing_table.push(RouteEntry {
                destination: [10, 0, 2, 0],
                netmask: [255, 255, 255, 0],
                gateway: [0; 4],
                interface: String::from("eth0"),
                metric: 100,
                flags: ROUTE_FLAG_UP,
            });
        }

        // Default DNS
        self.dns_resolver.add_server([8, 8, 8, 8]);
        self.dns_resolver.add_server([1, 1, 1, 1]);

        self.update_status();
    }

    // ── Status detection ────────────────────────────────────────────────

    /// Recompute the high-level network status from interface and route state.
    pub fn update_status(&mut self) {
        let has_up_iface = self
            .interfaces
            .iter()
            .any(|i| i.is_up() && i.iface_type != InterfaceType::Loopback);

        if !has_up_iface {
            self.status = NetworkStatus::Disconnected;
            return;
        }

        let has_ip = self
            .interfaces
            .iter()
            .any(|i| i.is_up() && i.iface_type != InterfaceType::Loopback && i.has_ip());

        if !has_ip {
            self.status = NetworkStatus::Connecting;
            return;
        }

        let has_default_route = self.routing_table.iter().any(|r| r.is_default());

        if !has_default_route {
            self.status = NetworkStatus::Limited;
            return;
        }

        self.status = NetworkStatus::Connected;
    }

    /// Mark that internet connectivity has been confirmed (e.g. DNS works).
    pub fn mark_full_connectivity(&mut self) {
        if self.status == NetworkStatus::Connected {
            self.status = NetworkStatus::FullConnectivity;
            self.connectivity_checked = true;
        }
    }

    pub fn status(&self) -> NetworkStatus {
        self.status
    }

    // ── Interface management ────────────────────────────────────────────

    pub fn list_interfaces(&self) -> &[NetworkInterface] {
        &self.interfaces
    }

    pub fn get_interface(&self, name: &str) -> Option<&NetworkInterface> {
        self.interfaces.iter().find(|i| i.name == name)
    }

    pub fn get_interface_mut(&mut self, name: &str) -> Option<&mut NetworkInterface> {
        self.interfaces.iter_mut().find(|i| i.name == name)
    }

    /// Bring an interface up.
    pub fn interface_up(&mut self, name: &str) -> bool {
        if let Some(iface) = self.interfaces.iter_mut().find(|i| i.name == name) {
            iface.bring_up();
            self.update_status();
            return true;
        }
        false
    }

    /// Bring an interface down, removing its routes.
    pub fn interface_down(&mut self, name: &str) -> bool {
        if let Some(iface) = self.interfaces.iter_mut().find(|i| i.name == name) {
            iface.bring_down();
            iface.ipv4 = None;
            self.routing_table.retain(|r| r.interface != name);
            self.update_status();
            return true;
        }
        false
    }

    // ── Static IP configuration ─────────────────────────────────────────

    /// Configure an interface with a static IP. Updates the smoltcp stack,
    /// routing table, and DNS resolver accordingly.
    pub fn configure_static(
        &mut self,
        name: &str,
        address: [u8; 4],
        netmask: [u8; 4],
        gateway: Option<[u8; 4]>,
        dns: Vec<[u8; 4]>,
    ) -> bool {
        let iface = match self.interfaces.iter_mut().find(|i| i.name == name) {
            Some(i) => i,
            None => return false,
        };

        let mut config = Ipv4Config::new_static(address, netmask, gateway);
        config.dns = dns.clone();
        let net_addr = config.network_address();

        iface.ipv4 = Some(config);

        // Update routing table
        self.routing_table.retain(|r| r.interface != name);

        self.routing_table.push(RouteEntry {
            destination: net_addr,
            netmask,
            gateway: [0; 4],
            interface: String::from(name),
            metric: 100,
            flags: ROUTE_FLAG_UP,
        });

        if let Some(gw) = gateway {
            self.routing_table.push(RouteEntry {
                destination: [0; 4],
                netmask: [0; 4],
                gateway: gw,
                interface: String::from(name),
                metric: 100,
                flags: ROUTE_FLAG_UP | ROUTE_FLAG_GATEWAY | ROUTE_FLAG_DEFAULT,
            });
        }

        // Apply to smoltcp
        self.apply_to_smoltcp(address, netmask, gateway);

        // Update DNS if servers provided
        if !dns.is_empty() {
            let mut dns_guard = crate::dns::DNS_RESOLVER.lock();
            if let Some(ref mut resolver) = *dns_guard {
                resolver.set_servers(dns);
            }
        }

        self.update_status();
        true
    }

    /// Apply IP configuration to the smoltcp network stack.
    fn apply_to_smoltcp(&self, address: [u8; 4], netmask: [u8; 4], gateway: Option<[u8; 4]>) {
        use smoltcp::wire::{IpCidr, Ipv4Address};

        let prefix = u32::from_be_bytes(netmask).count_ones() as u8;
        let ip = Ipv4Address::new(address[0], address[1], address[2], address[3]);

        let mut stack_guard = crate::net::NET_STACK.lock();
        if let Some(ref mut stack) = *stack_guard {
            stack.iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::new(ip.into(), prefix));
            });

            if let Some(gw) = gateway {
                let gw_addr = Ipv4Address::new(gw[0], gw[1], gw[2], gw[3]);
                stack
                    .iface
                    .routes_mut()
                    .add_default_ipv4_route(gw_addr)
                    .ok();
            }
        }
    }

    /// Apply DHCP lease results to the interface, routing table, and DNS.
    pub fn apply_dhcp_lease(&mut self, iface_name: &str, lease: &crate::dhcp::DhcpLease) {
        let config = Ipv4Config::from_dhcp_lease(lease);
        let net_addr = config.network_address();

        if let Some(iface) = self.interfaces.iter_mut().find(|i| i.name == iface_name) {
            iface.ipv4 = Some(config);
        }

        // Update routes
        self.routing_table.retain(|r| r.interface != iface_name);

        self.routing_table.push(RouteEntry {
            destination: net_addr,
            netmask: lease.subnet_mask,
            gateway: [0; 4],
            interface: String::from(iface_name),
            metric: 100,
            flags: ROUTE_FLAG_UP,
        });

        if lease.gateway != [0; 4] {
            self.routing_table.push(RouteEntry {
                destination: [0; 4],
                netmask: [0; 4],
                gateway: lease.gateway,
                interface: String::from(iface_name),
                metric: 100,
                flags: ROUTE_FLAG_UP | ROUTE_FLAG_GATEWAY | ROUTE_FLAG_DEFAULT,
            });
        }

        self.update_status();

        crate::serial_println!(
            "[netmgr] Interface {} configured via DHCP: {}.{}.{}.{}",
            iface_name,
            lease.client_ip[0],
            lease.client_ip[1],
            lease.client_ip[2],
            lease.client_ip[3],
        );
    }

    // ── Legacy configure_interface ──────────────────────────────────────

    pub fn configure_interface(&mut self, name: &str, config: Ipv4Config) -> bool {
        if let Some(iface) = self.interfaces.iter_mut().find(|i| i.name == name) {
            if let Some(gw) = config.gateway {
                let net_addr = config.network_address();
                self.routing_table.retain(|r| r.interface != name);

                self.routing_table.push(RouteEntry {
                    destination: net_addr,
                    netmask: config.netmask,
                    gateway: [0; 4],
                    interface: String::from(name),
                    metric: 100,
                    flags: ROUTE_FLAG_UP,
                });

                self.routing_table.push(RouteEntry {
                    destination: [0; 4],
                    netmask: [0; 4],
                    gateway: gw,
                    interface: String::from(name),
                    metric: 100,
                    flags: ROUTE_FLAG_UP | ROUTE_FLAG_GATEWAY | ROUTE_FLAG_DEFAULT,
                });
            }

            iface.ipv4 = Some(config);
            self.update_status();
            return true;
        }
        false
    }

    // ── WiFi ────────────────────────────────────────────────────────────

    pub fn connect_wifi(&mut self, ssid: &str, password: Option<&str>) -> WifiConnectResult {
        self.wifi_manager.connect(ssid, password)
    }

    pub fn disconnect_wifi(&mut self) -> bool {
        self.wifi_manager.disconnect()
    }

    pub fn scan_wifi(&mut self) -> bool {
        self.wifi_manager.start_scan()
    }

    pub fn wifi_scan_results(&self) -> &[WifiNetwork] {
        &self.wifi_manager.scan_results
    }

    // ── Routing ─────────────────────────────────────────────────────────

    pub fn add_route(&mut self, route: RouteEntry) {
        let pos = self
            .routing_table
            .iter()
            .position(|r| r.metric > route.metric)
            .unwrap_or(self.routing_table.len());
        self.routing_table.insert(pos, route);
    }

    pub fn remove_route(&mut self, destination: [u8; 4], netmask: [u8; 4]) -> bool {
        let before = self.routing_table.len();
        self.routing_table
            .retain(|r| r.destination != destination || r.netmask != netmask);
        self.routing_table.len() < before
    }

    /// Longest-prefix-match route lookup with metric tie-breaking.
    pub fn lookup_route(&self, addr: &[u8; 4]) -> Option<&RouteEntry> {
        self.routing_table
            .iter()
            .filter(|r| r.matches(addr))
            .max_by(|a, b| {
                a.prefix_len()
                    .cmp(&b.prefix_len())
                    .then(b.metric.cmp(&a.metric))
            })
    }

    pub fn routing_table(&self) -> &[RouteEntry] {
        &self.routing_table
    }

    // ── VPN ─────────────────────────────────────────────────────────────

    pub fn start_vpn(&mut self, name: String, vpn_type: VpnType, config: VpnConfig) -> usize {
        let conn = VpnConnection {
            name,
            vpn_type,
            state: VpnState::Connecting,
            config,
            stats: VpnStats::default(),
        };
        self.vpn_connections.push(conn);
        self.vpn_connections.len() - 1
    }

    pub fn stop_vpn(&mut self, name: &str) -> bool {
        if let Some(conn) = self.vpn_connections.iter_mut().find(|c| c.name == name) {
            conn.state = VpnState::Disconnecting;
            conn.state = VpnState::Disconnected;
            return true;
        }
        false
    }

    pub fn vpn_status(&self, name: &str) -> Option<VpnState> {
        self.vpn_connections
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.state)
    }

    // ── DNS ─────────────────────────────────────────────────────────────

    pub fn resolve_dns(&mut self, name: &str, now: u64) -> crate::dns::ResolveResult {
        self.dns_resolver.resolve(name, now)
    }

    // ── Queries ─────────────────────────────────────────────────────────

    pub fn get_active_connections(&self) -> &[ActiveConnection] {
        &self.active_connections
    }

    pub fn primary_ip(&self) -> Option<[u8; 4]> {
        self.interfaces
            .iter()
            .filter(|i| i.is_up() && i.iface_type != InterfaceType::Loopback)
            .find_map(|i| i.ipv4.as_ref().map(|c| c.address))
    }

    pub fn primary_gateway(&self) -> Option<[u8; 4]> {
        self.routing_table
            .iter()
            .find(|r| r.is_default())
            .map(|r| r.gateway)
    }

    pub fn interface_count(&self) -> usize {
        self.interfaces.len()
    }

    pub fn update_stats(&mut self, iface_name: &str, rx_bytes: u64, tx_bytes: u64) {
        if let Some(iface) = self.interfaces.iter_mut().find(|i| i.name == iface_name) {
            iface.stats.rx_bytes += rx_bytes;
            iface.stats.tx_bytes += tx_bytes;
            iface.stats.rx_packets += 1;
            iface.stats.tx_packets += 1;
        }
    }

    /// Summary string for boot log.
    pub fn summary(&self) -> (usize, usize, NetworkStatus) {
        (self.interfaces.len(), self.routing_table.len(), self.status)
    }
}

/// Initialize the network manager.
pub fn init() {
    let mut mgr = NetworkManager::new();
    mgr.init();

    let (iface_count, route_count, status) = mgr.summary();

    *NET_MANAGER.lock() = Some(mgr);

    crate::serial_println!(
        "[ OK ] Network manager initialized ({} interfaces, {} routes, status={:?})",
        iface_count,
        route_count,
        status,
    );
}

/// Quick Settings / Control Center accessor: `(wifi_enabled, primary_ip)` for
/// the notification center's Wi-Fi/network toggle (Concept §Unified Settings).
/// `wifi_enabled` reflects the live software-radio state; `primary_ip` is the
/// first non-loopback IPv4 if one is assigned. Reads a snapshot under the lock
/// and drops it immediately (never held across the panel render).
///
/// LOCK DISCIPLINE (athena-reviewer 2026-06-17): this is reached from the
/// notification-center click path in keyboard/mouse IRQ context (RFLAGS.IF=0).
/// `NET_MANAGER` is a plain `spin::Mutex` also intended for a future preemptible
/// net-poll thread (`apply_dhcp_lease`). To pre-empt the single-CPU IF=0
/// self-deadlock that `lock_compositor`/`lock_audio` were built to kill, this
/// brief snapshot is taken with interrupts disabled — so no IRQ can land mid-hold
/// and a future preemptible holder can never strand this IF=0 waiter.
#[must_use]
pub fn quick_net_status() -> (bool, Option<[u8; 4]>) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let guard = NET_MANAGER.lock();
        match guard.as_ref() {
            Some(mgr) => (mgr.wifi_radio_enabled(), mgr.primary_ip()),
            None => (false, None),
        }
    })
}

/// Quick Settings mutator: enable/disable the Wi-Fi software radio. Drives the
/// real `NetworkManager` radio flag (interfaces are administratively brought
/// down when off), so the toggle changes networking state, not just a label.
/// Interrupts-disabled hold — see [`quick_net_status`] for the IF=0 rationale.
pub fn quick_set_wifi(enabled: bool) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut guard = NET_MANAGER.lock();
        if let Some(mgr) = guard.as_mut() {
            mgr.set_wifi_radio(enabled);
        }
    });
}

/// Apply a DHCP lease to the network manager (called after DHCP Bound event).
pub fn apply_dhcp_lease(iface_name: &str, lease: &crate::dhcp::DhcpLease) {
    let mut guard = NET_MANAGER.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.apply_dhcp_lease(iface_name, lease);
    }
}

/// Get the current network status.
pub fn network_status() -> NetworkStatus {
    let guard = NET_MANAGER.lock();
    guard
        .as_ref()
        .map_or(NetworkStatus::Disconnected, |m| m.status())
}
