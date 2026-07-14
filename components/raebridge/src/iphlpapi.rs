//! iphlpapi.dll — IP Helper API: adapter enumeration, IP address management,
//! route tables, ARP/neighbor tables, TCP/UDP connection tables, interface
//! statistics, DNS configuration, and change notifications for RaeBridge.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// =========================================================================
// Error codes
// =========================================================================

pub const NO_ERROR: u32 = 0;
pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_BUFFER_OVERFLOW: u32 = 111;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_NOT_SUPPORTED: u32 = 50;
pub const ERROR_NO_DATA: u32 = 232;
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub const ERROR_INVALID_DATA: u32 = 13;

// =========================================================================
// GetAdaptersAddresses flags
// =========================================================================

pub const GAA_FLAG_SKIP_UNICAST: u32 = 0x0001;
pub const GAA_FLAG_SKIP_ANYCAST: u32 = 0x0002;
pub const GAA_FLAG_SKIP_MULTICAST: u32 = 0x0004;
pub const GAA_FLAG_SKIP_DNS_SERVER: u32 = 0x0008;
pub const GAA_FLAG_INCLUDE_PREFIX: u32 = 0x0010;
pub const GAA_FLAG_SKIP_FRIENDLY_NAME: u32 = 0x0020;
pub const GAA_FLAG_INCLUDE_WINS_SERVER_ADDRESSES: u32 = 0x0040;
pub const GAA_FLAG_INCLUDE_GATEWAYS: u32 = 0x0080;
pub const GAA_FLAG_INCLUDE_ALL_INTERFACES: u32 = 0x0100;
pub const GAA_FLAG_INCLUDE_ALL_COMPARTMENTS: u32 = 0x0200;
pub const GAA_FLAG_INCLUDE_TUNNEL_BINDINGORDER: u32 = 0x0400;

// =========================================================================
// Address family
// =========================================================================

pub const AF_UNSPEC: u16 = 0;
pub const AF_INET: u16 = 2;
pub const AF_INET6: u16 = 23;

// =========================================================================
// Interface operational status
// =========================================================================

pub const IF_OPER_STATUS_UP: u32 = 1;
pub const IF_OPER_STATUS_DOWN: u32 = 2;
pub const IF_OPER_STATUS_TESTING: u32 = 3;
pub const IF_OPER_STATUS_UNKNOWN: u32 = 4;
pub const IF_OPER_STATUS_DORMANT: u32 = 5;
pub const IF_OPER_STATUS_NOT_PRESENT: u32 = 6;
pub const IF_OPER_STATUS_LOWER_LAYER_DOWN: u32 = 7;

// =========================================================================
// Interface types
// =========================================================================

pub const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
pub const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
pub const IF_TYPE_TUNNEL: u32 = 131;
pub const IF_TYPE_IEEE80211: u32 = 71;
pub const IF_TYPE_IEEE1394: u32 = 144;

// =========================================================================
// Media connect state
// =========================================================================

pub const MEDIA_CONNECT_STATE_UNKNOWN: u32 = 0;
pub const MEDIA_CONNECT_STATE_CONNECTED: u32 = 1;
pub const MEDIA_CONNECT_STATE_DISCONNECTED: u32 = 2;

// =========================================================================
// TCP states (MIB_TCP_STATE)
// =========================================================================

pub const MIB_TCP_STATE_CLOSED: u32 = 1;
pub const MIB_TCP_STATE_LISTEN: u32 = 2;
pub const MIB_TCP_STATE_SYN_SENT: u32 = 3;
pub const MIB_TCP_STATE_SYN_RCVD: u32 = 4;
pub const MIB_TCP_STATE_ESTAB: u32 = 5;
pub const MIB_TCP_STATE_FIN_WAIT1: u32 = 6;
pub const MIB_TCP_STATE_FIN_WAIT2: u32 = 7;
pub const MIB_TCP_STATE_CLOSE_WAIT: u32 = 8;
pub const MIB_TCP_STATE_CLOSING: u32 = 9;
pub const MIB_TCP_STATE_LAST_ACK: u32 = 10;
pub const MIB_TCP_STATE_TIME_WAIT: u32 = 11;
pub const MIB_TCP_STATE_DELETE_TCB: u32 = 12;

// =========================================================================
// Connection type
// =========================================================================

pub const NET_IF_CONNECTION_DEDICATED: u32 = 1;
pub const NET_IF_CONNECTION_PASSIVE: u32 = 2;
pub const NET_IF_CONNECTION_DEMAND: u32 = 3;

// =========================================================================
// Tunnel type
// =========================================================================

pub const TUNNEL_TYPE_NONE: u32 = 0;
pub const TUNNEL_TYPE_OTHER: u32 = 1;
pub const TUNNEL_TYPE_DIRECT: u32 = 2;
pub const TUNNEL_TYPE_6TO4: u32 = 11;
pub const TUNNEL_TYPE_ISATAP: u32 = 13;
pub const TUNNEL_TYPE_TEREDO: u32 = 14;

// =========================================================================
// Node type
// =========================================================================

pub const BROADCAST_NODETYPE: u32 = 0x0001;
pub const PEER_TO_PEER_NODETYPE: u32 = 0x0002;
pub const MIXED_NODETYPE: u32 = 0x0004;
pub const HYBRID_NODETYPE: u32 = 0x0008;

// =========================================================================
// DNS record types
// =========================================================================

pub const DNS_TYPE_A: u16 = 1;
pub const DNS_TYPE_NS: u16 = 2;
pub const DNS_TYPE_CNAME: u16 = 5;
pub const DNS_TYPE_SOA: u16 = 6;
pub const DNS_TYPE_PTR: u16 = 12;
pub const DNS_TYPE_MX: u16 = 15;
pub const DNS_TYPE_TXT: u16 = 16;
pub const DNS_TYPE_AAAA: u16 = 28;
pub const DNS_TYPE_SRV: u16 = 33;

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct IpAddress {
    pub family: u16,
    pub address: [u8; 16],
    pub prefix_length: u8,
}

impl IpAddress {
    pub fn ipv4(a: u8, b: u8, c: u8, d: u8, prefix: u8) -> Self {
        let mut addr = [0u8; 16];
        addr[0] = a;
        addr[1] = b;
        addr[2] = c;
        addr[3] = d;
        Self {
            family: AF_INET,
            address: addr,
            prefix_length: prefix,
        }
    }

    pub fn ipv6(bytes: [u8; 16], prefix: u8) -> Self {
        Self {
            family: AF_INET6,
            address: bytes,
            prefix_length: prefix,
        }
    }

    pub fn to_string_repr(&self) -> String {
        if self.family == AF_INET {
            alloc::format!(
                "{}.{}.{}.{}",
                self.address[0],
                self.address[1],
                self.address[2],
                self.address[3]
            )
        } else {
            let mut s = String::new();
            for i in 0..8 {
                if i > 0 {
                    s.push(':');
                }
                let hi = self.address[i * 2];
                let lo = self.address[i * 2 + 1];
                s.push_str(&alloc::format!("{:x}{:02x}", hi, lo));
            }
            s
        }
    }
}

#[derive(Debug, Clone)]
pub struct IpAdapterAddresses {
    pub adapter_name: String,
    pub dns_suffix: String,
    pub description: String,
    pub friendly_name: String,
    pub physical_address: [u8; 6],
    pub physical_address_length: u32,
    pub mtu: u32,
    pub if_type: u32,
    pub oper_status: u32,
    pub ipv6_if_index: u32,
    pub zone_indices: [u32; 16],
    pub unicast_addresses: Vec<IpAddress>,
    pub anycast_addresses: Vec<IpAddress>,
    pub multicast_addresses: Vec<IpAddress>,
    pub dns_server_addresses: Vec<IpAddress>,
    pub prefix_list: Vec<(IpAddress, u8)>,
    pub receive_link_speed: u64,
    pub transmit_link_speed: u64,
    pub wins_server_addresses: Vec<IpAddress>,
    pub gateway_addresses: Vec<IpAddress>,
    pub ipv4_metric: u32,
    pub ipv6_metric: u32,
    pub luid: u64,
    pub dhcpv4_server: Option<IpAddress>,
    pub compartment_id: u32,
    pub network_guid: [u8; 16],
    pub connection_type: u32,
    pub tunnel_type: u32,
    pub dhcpv6_server: Option<IpAddress>,
    pub dhcpv6_client_duid: Vec<u8>,
    pub dns_suffix_list: Vec<String>,
}

impl IpAdapterAddresses {
    pub fn loopback() -> Self {
        Self {
            adapter_name: String::from("lo0"),
            dns_suffix: String::new(),
            description: String::from("Software Loopback Interface"),
            friendly_name: String::from("Loopback Pseudo-Interface"),
            physical_address: [0; 6],
            physical_address_length: 0,
            mtu: 65536,
            if_type: IF_TYPE_SOFTWARE_LOOPBACK,
            oper_status: IF_OPER_STATUS_UP,
            ipv6_if_index: 1,
            zone_indices: [0; 16],
            unicast_addresses: alloc::vec![IpAddress::ipv4(127, 0, 0, 1, 8)],
            anycast_addresses: Vec::new(),
            multicast_addresses: Vec::new(),
            dns_server_addresses: Vec::new(),
            prefix_list: Vec::new(),
            receive_link_speed: 1_000_000_000,
            transmit_link_speed: 1_000_000_000,
            wins_server_addresses: Vec::new(),
            gateway_addresses: Vec::new(),
            ipv4_metric: 75,
            ipv6_metric: 75,
            luid: 1,
            dhcpv4_server: None,
            compartment_id: 1,
            network_guid: [0; 16],
            connection_type: NET_IF_CONNECTION_DEDICATED,
            tunnel_type: TUNNEL_TYPE_NONE,
            dhcpv6_server: None,
            dhcpv6_client_duid: Vec::new(),
            dns_suffix_list: Vec::new(),
        }
    }

    pub fn ethernet() -> Self {
        Self {
            adapter_name: String::from("eth0"),
            dns_suffix: String::from("localdomain"),
            description: String::from("RaeenOS Virtual Ethernet Adapter"),
            friendly_name: String::from("Ethernet"),
            physical_address: [0x00, 0x15, 0x5D, 0xAA, 0xBB, 0xCC],
            physical_address_length: 6,
            mtu: 1500,
            if_type: IF_TYPE_ETHERNET_CSMACD,
            oper_status: IF_OPER_STATUS_UP,
            ipv6_if_index: 2,
            zone_indices: [0; 16],
            unicast_addresses: alloc::vec![IpAddress::ipv4(192, 168, 1, 100, 24)],
            anycast_addresses: Vec::new(),
            multicast_addresses: Vec::new(),
            dns_server_addresses: alloc::vec![
                IpAddress::ipv4(8, 8, 8, 8, 32),
                IpAddress::ipv4(8, 8, 4, 4, 32),
            ],
            prefix_list: Vec::new(),
            receive_link_speed: 1_000_000_000,
            transmit_link_speed: 1_000_000_000,
            wins_server_addresses: Vec::new(),
            gateway_addresses: alloc::vec![IpAddress::ipv4(192, 168, 1, 1, 32)],
            ipv4_metric: 25,
            ipv6_metric: 25,
            luid: 2,
            dhcpv4_server: Some(IpAddress::ipv4(192, 168, 1, 1, 32)),
            compartment_id: 1,
            network_guid: [0; 16],
            connection_type: NET_IF_CONNECTION_DEDICATED,
            tunnel_type: TUNNEL_TYPE_NONE,
            dhcpv6_server: None,
            dhcpv6_client_duid: Vec::new(),
            dns_suffix_list: alloc::vec![String::from("localdomain")],
        }
    }
}

// =========================================================================
// Adapter enumeration
// =========================================================================

pub fn get_adapters_info() -> Vec<IpAdapterAddresses> {
    alloc::vec![
        IpAdapterAddresses::ethernet(),
        IpAdapterAddresses::loopback()
    ]
}

pub fn get_adapters_addresses(family: u16, _flags: u32) -> Vec<IpAdapterAddresses> {
    let all = alloc::vec![
        IpAdapterAddresses::ethernet(),
        IpAdapterAddresses::loopback()
    ];
    if family == AF_UNSPEC {
        all
    } else {
        all.into_iter()
            .filter(|a| a.unicast_addresses.iter().any(|u| u.family == family))
            .collect()
    }
}

// =========================================================================
// IP address management
// =========================================================================

pub fn add_ip_address(_addr: &IpAddress, _if_index: u32) -> u32 {
    NO_ERROR
}

pub fn delete_ip_address(_addr: &IpAddress, _if_index: u32) -> u32 {
    NO_ERROR
}

#[derive(Debug, Clone)]
pub struct UnicastIpAddressEntry {
    pub address: IpAddress,
    pub interface_luid: u64,
    pub interface_index: u32,
    pub prefix_origin: u32,
    pub suffix_origin: u32,
    pub valid_lifetime: u32,
    pub preferred_lifetime: u32,
    pub on_link_prefix_length: u8,
    pub dad_state: u32,
}

pub fn get_unicast_ip_address_table(family: u16) -> Vec<UnicastIpAddressEntry> {
    let adapters = get_adapters_addresses(family, 0);
    let mut entries = Vec::new();
    for (idx, adapter) in adapters.iter().enumerate() {
        for addr in &adapter.unicast_addresses {
            entries.push(UnicastIpAddressEntry {
                address: addr.clone(),
                interface_luid: adapter.luid,
                interface_index: idx as u32 + 1,
                prefix_origin: 2, // Manual
                suffix_origin: 2,
                valid_lifetime: 0xFFFFFFFF,
                preferred_lifetime: 0xFFFFFFFF,
                on_link_prefix_length: addr.prefix_length,
                dad_state: 4, // Preferred
            });
        }
    }
    entries
}

pub fn get_unicast_ip_address_entry(_luid: u64) -> Option<UnicastIpAddressEntry> {
    let table = get_unicast_ip_address_table(AF_UNSPEC);
    table.into_iter().next()
}

pub fn create_unicast_ip_address_entry(_entry: &UnicastIpAddressEntry) -> u32 {
    NO_ERROR
}

pub fn delete_unicast_ip_address_entry(_entry: &UnicastIpAddressEntry) -> u32 {
    NO_ERROR
}

// =========================================================================
// Route table
// =========================================================================

#[derive(Debug, Clone)]
pub struct IpForwardEntry {
    pub destination: IpAddress,
    pub prefix_length: u8,
    pub next_hop: IpAddress,
    pub interface_luid: u64,
    pub interface_index: u32,
    pub metric: u32,
    pub protocol: u32,
    pub loopback: bool,
    pub age: u32,
}

pub fn get_ip_forward_table() -> Vec<IpForwardEntry> {
    alloc::vec![
        IpForwardEntry {
            destination: IpAddress::ipv4(0, 0, 0, 0, 0),
            prefix_length: 0,
            next_hop: IpAddress::ipv4(192, 168, 1, 1, 32),
            interface_luid: 2,
            interface_index: 2,
            metric: 25,
            protocol: 3, // PROTO_IP_NETMGMT
            loopback: false,
            age: 0,
        },
        IpForwardEntry {
            destination: IpAddress::ipv4(192, 168, 1, 0, 24),
            prefix_length: 24,
            next_hop: IpAddress::ipv4(0, 0, 0, 0, 0),
            interface_luid: 2,
            interface_index: 2,
            metric: 25,
            protocol: 2, // PROTO_IP_LOCAL
            loopback: false,
            age: 0,
        },
        IpForwardEntry {
            destination: IpAddress::ipv4(127, 0, 0, 0, 8),
            prefix_length: 8,
            next_hop: IpAddress::ipv4(127, 0, 0, 1, 32),
            interface_luid: 1,
            interface_index: 1,
            metric: 75,
            protocol: 2,
            loopback: true,
            age: 0,
        },
    ]
}

pub fn get_ip_forward_table2(family: u16) -> Vec<IpForwardEntry> {
    let all = get_ip_forward_table();
    if family == AF_UNSPEC {
        all
    } else {
        all.into_iter()
            .filter(|r| r.destination.family == family)
            .collect()
    }
}

pub fn create_ip_forward_entry2(_entry: &IpForwardEntry) -> u32 {
    NO_ERROR
}
pub fn delete_ip_forward_entry2(_entry: &IpForwardEntry) -> u32 {
    NO_ERROR
}
pub fn set_ip_forward_entry2(_entry: &IpForwardEntry) -> u32 {
    NO_ERROR
}

pub fn get_best_route2(
    _interface_luid: Option<u64>,
    _interface_index: u32,
    _source: Option<&IpAddress>,
    destination: &IpAddress,
    _sort_options: u32,
    best_route: &mut IpForwardEntry,
    best_source: &mut IpAddress,
) -> u32 {
    let routes = get_ip_forward_table();
    if let Some(r) = routes
        .into_iter()
        .find(|r| r.destination.family == destination.family)
    {
        *best_route = r;
    }
    *best_source = IpAddress::ipv4(192, 168, 1, 100, 24);
    NO_ERROR
}

// =========================================================================
// ARP / Neighbor table
// =========================================================================

#[derive(Debug, Clone)]
pub struct IpNetEntry {
    pub address: IpAddress,
    pub physical_address: [u8; 6],
    pub interface_luid: u64,
    pub interface_index: u32,
    pub state: u32,
    pub is_router: bool,
    pub is_unreachable: bool,
}

pub const NL_NEIGHBOR_STATE_REACHABLE: u32 = 3;
pub const NL_NEIGHBOR_STATE_STALE: u32 = 4;
pub const NL_NEIGHBOR_STATE_PERMANENT: u32 = 6;

pub fn get_ip_net_table() -> Vec<IpNetEntry> {
    alloc::vec![IpNetEntry {
        address: IpAddress::ipv4(192, 168, 1, 1, 32),
        physical_address: [0x00, 0x15, 0x5D, 0x01, 0x02, 0x03],
        interface_luid: 2,
        interface_index: 2,
        state: NL_NEIGHBOR_STATE_REACHABLE,
        is_router: true,
        is_unreachable: false,
    }]
}

pub fn get_ip_net_table2(family: u16) -> Vec<IpNetEntry> {
    let all = get_ip_net_table();
    if family == AF_UNSPEC {
        all
    } else {
        all.into_iter()
            .filter(|e| e.address.family == family)
            .collect()
    }
}

pub fn create_ip_net_entry2(_entry: &IpNetEntry) -> u32 {
    NO_ERROR
}
pub fn delete_ip_net_entry2(_entry: &IpNetEntry) -> u32 {
    NO_ERROR
}
pub fn flush_ip_net_table2(_family: u16, _if_index: u32) -> u32 {
    NO_ERROR
}

pub fn resolve_ip_net_entry2(
    _address: &IpAddress,
    _source: Option<&IpAddress>,
    phys_addr: &mut [u8; 6],
) -> u32 {
    *phys_addr = [0x00, 0x15, 0x5D, 0xFF, 0xFE, 0x01];
    NO_ERROR
}

// =========================================================================
// TCP table
// =========================================================================

#[derive(Debug, Clone)]
pub struct TcpRow {
    pub state: u32,
    pub local_addr: IpAddress,
    pub local_port: u16,
    pub remote_addr: IpAddress,
    pub remote_port: u16,
    pub owning_pid: u32,
    pub create_timestamp: u64,
}

pub fn get_tcp_table2() -> Vec<TcpRow> {
    Vec::new()
}

pub fn get_extended_tcp_table(_family: u16) -> Vec<TcpRow> {
    Vec::new()
}

pub fn set_tcp_entry(
    local_addr: &IpAddress,
    local_port: u16,
    remote_addr: &IpAddress,
    remote_port: u16,
) -> u32 {
    let _ = (local_addr, local_port, remote_addr, remote_port);
    NO_ERROR
}

pub fn get_tcp6_table2() -> Vec<TcpRow> {
    Vec::new()
}

// =========================================================================
// UDP table
// =========================================================================

#[derive(Debug, Clone)]
pub struct UdpRow {
    pub local_addr: IpAddress,
    pub local_port: u16,
    pub owning_pid: u32,
}

pub fn get_extended_udp_table(_family: u16) -> Vec<UdpRow> {
    Vec::new()
}

pub fn get_udp6_table() -> Vec<UdpRow> {
    Vec::new()
}

// =========================================================================
// Statistics
// =========================================================================

#[derive(Debug, Clone, Default)]
pub struct TcpStatistics {
    pub rto_algorithm: u32,
    pub rto_min: u32,
    pub rto_max: u32,
    pub max_conn: u32,
    pub active_opens: u32,
    pub passive_opens: u32,
    pub attempt_fails: u32,
    pub estab_resets: u32,
    pub curr_estab: u32,
    pub in_segs: u64,
    pub out_segs: u64,
    pub retrans_segs: u64,
    pub in_errs: u32,
    pub out_rsts: u32,
    pub num_conns: u32,
}

#[derive(Debug, Clone, Default)]
pub struct UdpStatistics {
    pub in_datagrams: u64,
    pub no_ports: u32,
    pub in_errors: u32,
    pub out_datagrams: u64,
    pub num_addrs: u32,
}

#[derive(Debug, Clone, Default)]
pub struct IpStatistics {
    pub forwarding: u32,
    pub default_ttl: u32,
    pub in_receives: u64,
    pub in_hdr_errors: u64,
    pub in_addr_errors: u64,
    pub forw_datagrams: u64,
    pub in_unknown_protos: u64,
    pub in_discards: u64,
    pub in_delivers: u64,
    pub out_requests: u64,
    pub out_discards: u64,
    pub out_no_routes: u64,
    pub reasm_timeout: u32,
    pub reasm_reqds: u64,
    pub reasm_oks: u64,
    pub reasm_fails: u64,
    pub frag_oks: u64,
    pub frag_fails: u64,
    pub frag_creates: u64,
    pub num_if: u32,
    pub num_addr: u32,
    pub num_routes: u32,
}

#[derive(Debug, Clone, Default)]
pub struct IcmpStatistics {
    pub msgs_in: u64,
    pub errors_in: u64,
    pub dest_unreachs_in: u64,
    pub time_excds_in: u64,
    pub echo_reps_in: u64,
    pub echos_in: u64,
    pub msgs_out: u64,
    pub errors_out: u64,
    pub dest_unreachs_out: u64,
    pub echos_out: u64,
    pub echo_reps_out: u64,
}

pub fn get_tcp_statistics_ex2(family: u16) -> TcpStatistics {
    let _ = family;
    TcpStatistics {
        rto_algorithm: 4,
        rto_min: 300,
        rto_max: 120_000,
        max_conn: 0xFFFFFFFE,
        ..Default::default()
    }
}

pub fn get_udp_statistics_ex2(family: u16) -> UdpStatistics {
    let _ = family;
    UdpStatistics::default()
}

pub fn get_ip_statistics_ex(family: u16) -> IpStatistics {
    let _ = family;
    IpStatistics {
        forwarding: 2,
        default_ttl: 128,
        num_if: 2,
        num_addr: 2,
        num_routes: 3,
        ..Default::default()
    }
}

pub fn get_icmp_statistics_ex(_family: u16) -> IcmpStatistics {
    IcmpStatistics::default()
}

// =========================================================================
// Interface table
// =========================================================================

#[derive(Debug, Clone)]
pub struct MibIfRow2 {
    pub interface_luid: u64,
    pub interface_index: u32,
    pub interface_guid: [u8; 16],
    pub alias: String,
    pub description: String,
    pub phys_address: [u8; 6],
    pub phys_address_length: u32,
    pub if_type: u32,
    pub mtu: u32,
    pub oper_status: u32,
    pub admin_status: u32,
    pub media_connect_state: u32,
    pub speed: u64,
    pub in_octets: u64,
    pub in_unicast_pkts: u64,
    pub in_multicast_pkts: u64,
    pub in_broadcast_pkts: u64,
    pub in_errors: u64,
    pub in_discards: u64,
    pub out_octets: u64,
    pub out_unicast_pkts: u64,
    pub out_multicast_pkts: u64,
    pub out_broadcast_pkts: u64,
    pub out_errors: u64,
    pub out_discards: u64,
}

pub fn get_if_table2() -> Vec<MibIfRow2> {
    alloc::vec![
        MibIfRow2 {
            interface_luid: 1,
            interface_index: 1,
            interface_guid: [0; 16],
            alias: String::from("Loopback Pseudo-Interface"),
            description: String::from("Software Loopback Interface"),
            phys_address: [0; 6],
            phys_address_length: 0,
            if_type: IF_TYPE_SOFTWARE_LOOPBACK,
            mtu: 65536,
            oper_status: IF_OPER_STATUS_UP,
            admin_status: IF_OPER_STATUS_UP,
            media_connect_state: MEDIA_CONNECT_STATE_CONNECTED,
            speed: 1_073_741_824,
            in_octets: 0,
            in_unicast_pkts: 0,
            in_multicast_pkts: 0,
            in_broadcast_pkts: 0,
            in_errors: 0,
            in_discards: 0,
            out_octets: 0,
            out_unicast_pkts: 0,
            out_multicast_pkts: 0,
            out_broadcast_pkts: 0,
            out_errors: 0,
            out_discards: 0,
        },
        MibIfRow2 {
            interface_luid: 2,
            interface_index: 2,
            interface_guid: [0; 16],
            alias: String::from("Ethernet"),
            description: String::from("RaeenOS Virtual Ethernet Adapter"),
            phys_address: [0x00, 0x15, 0x5D, 0xAA, 0xBB, 0xCC],
            phys_address_length: 6,
            if_type: IF_TYPE_ETHERNET_CSMACD,
            mtu: 1500,
            oper_status: IF_OPER_STATUS_UP,
            admin_status: IF_OPER_STATUS_UP,
            media_connect_state: MEDIA_CONNECT_STATE_CONNECTED,
            speed: 1_000_000_000,
            in_octets: 0,
            in_unicast_pkts: 0,
            in_multicast_pkts: 0,
            in_broadcast_pkts: 0,
            in_errors: 0,
            in_discards: 0,
            out_octets: 0,
            out_unicast_pkts: 0,
            out_multicast_pkts: 0,
            out_broadcast_pkts: 0,
            out_errors: 0,
            out_discards: 0,
        },
    ]
}

pub fn get_if_entry2(interface_index: u32) -> Option<MibIfRow2> {
    get_if_table2()
        .into_iter()
        .find(|r| r.interface_index == interface_index)
}

// =========================================================================
// DNS
// =========================================================================

#[derive(Debug, Clone)]
pub struct NetworkParams {
    pub host_name: String,
    pub domain_name: String,
    pub dns_server_list: Vec<IpAddress>,
    pub node_type: u32,
    pub scope_id: String,
}

pub fn get_network_params() -> NetworkParams {
    NetworkParams {
        host_name: String::from("RAEENOS"),
        domain_name: String::from("localdomain"),
        dns_server_list: alloc::vec![
            IpAddress::ipv4(8, 8, 8, 8, 32),
            IpAddress::ipv4(8, 8, 4, 4, 32),
        ],
        node_type: HYBRID_NODETYPE,
        scope_id: String::new(),
    }
}

#[derive(Debug, Clone)]
pub struct DnsRecord {
    pub name: String,
    pub record_type: u16,
    pub ttl: u32,
    pub data: DnsRecordData,
}

#[derive(Debug, Clone)]
pub enum DnsRecordData {
    A([u8; 4]),
    Aaaa([u8; 16]),
    Cname(String),
    Mx {
        preference: u16,
        exchange: String,
    },
    Txt(String),
    Ptr(String),
    Srv {
        priority: u16,
        weight: u16,
        port: u16,
        target: String,
    },
    Ns(String),
    Unknown,
}

pub fn dns_query_w(_name: &str, record_type: u16) -> Option<Vec<DnsRecord>> {
    let _ = record_type;
    None
}

pub fn dns_record_list_free(_records: Vec<DnsRecord>) {
    // no-op, memory is managed by Rust
}

pub fn dns_modify_records_in_set(_add_records: &[DnsRecord], _delete_records: &[DnsRecord]) -> u32 {
    NO_ERROR
}

// =========================================================================
// Change notifications
// =========================================================================

static NEXT_NOTIFY: AtomicU64 = AtomicU64::new(0x80000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyHandle(pub u64);

pub fn notify_addr_change() -> NotifyHandle {
    NotifyHandle(NEXT_NOTIFY.fetch_add(1, Ordering::SeqCst))
}

pub fn notify_route_change() -> NotifyHandle {
    NotifyHandle(NEXT_NOTIFY.fetch_add(1, Ordering::SeqCst))
}

pub fn notify_ip_interface_change(_family: u16, _callback: u64) -> NotifyHandle {
    NotifyHandle(NEXT_NOTIFY.fetch_add(1, Ordering::SeqCst))
}

pub fn notify_unicast_ip_address_change(_family: u16, _callback: u64) -> NotifyHandle {
    NotifyHandle(NEXT_NOTIFY.fetch_add(1, Ordering::SeqCst))
}

pub fn cancel_mib_change_notify2(_handle: NotifyHandle) -> u32 {
    NO_ERROR
}

// =========================================================================
// Global IP_HELPER runtime
// =========================================================================

static IP_HELPER_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct IpHelperRuntime {
    pub adapters: Vec<IpAdapterAddresses>,
    pub routes: Vec<IpForwardEntry>,
    pub neighbors: Vec<IpNetEntry>,
    pub notify_handles: Vec<NotifyHandle>,
}

impl IpHelperRuntime {
    fn new() -> Self {
        Self {
            adapters: get_adapters_info(),
            routes: get_ip_forward_table(),
            neighbors: get_ip_net_table(),
            notify_handles: Vec::new(),
        }
    }
}

static mut IP_HELPER_INNER: Option<IpHelperRuntime> = None;

pub fn init() {
    if IP_HELPER_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            IP_HELPER_INNER = Some(IpHelperRuntime::new());
        }
    }
}

pub fn runtime() -> Option<&'static IpHelperRuntime> {
    if IP_HELPER_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { IP_HELPER_INNER.as_ref() }
    } else {
        None
    }
}

pub fn runtime_mut() -> Option<&'static mut IpHelperRuntime> {
    if IP_HELPER_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { IP_HELPER_INNER.as_mut() }
    } else {
        None
    }
}
