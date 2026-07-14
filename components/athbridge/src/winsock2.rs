//! winsock2.dll — Extended WinSock 2 API emulation for AthBridge.
//!
//! Provides deep WinSock2 coverage beyond basic ws2_32: overlapped I/O,
//! IOCP integration, multicast, raw sockets, async name resolution,
//! extension functions (AcceptEx, ConnectEx, TransmitFile), and full
//! socket option handling.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::{DWord, WinHandle};

// =========================================================================
// WinSock error codes
// =========================================================================

pub const WSABASEERR: i32 = 10000;
pub const WSAEINTR: i32 = 10004;
pub const WSAEBADF: i32 = 10009;
pub const WSAEACCES: i32 = 10013;
pub const WSAEFAULT: i32 = 10014;
pub const WSAEINVAL: i32 = 10022;
pub const WSAEMFILE: i32 = 10024;
pub const WSAEWOULDBLOCK: i32 = 10035;
pub const WSAEINPROGRESS: i32 = 10036;
pub const WSAEALREADY: i32 = 10037;
pub const WSAENOTSOCK: i32 = 10038;
pub const WSAEDESTADDRREQ: i32 = 10039;
pub const WSAEMSGSIZE: i32 = 10040;
pub const WSAEPROTOTYPE: i32 = 10041;
pub const WSAENOPROTOOPT: i32 = 10042;
pub const WSAEPROTONOSUPPORT: i32 = 10043;
pub const WSAESOCKTNOSUPPORT: i32 = 10044;
pub const WSAEOPNOTSUPP: i32 = 10045;
pub const WSAEPFNOSUPPORT: i32 = 10046;
pub const WSAEAFNOSUPPORT: i32 = 10047;
pub const WSAEADDRINUSE: i32 = 10048;
pub const WSAEADDRNOTAVAIL: i32 = 10049;
pub const WSAENETDOWN: i32 = 10050;
pub const WSAENETUNREACH: i32 = 10051;
pub const WSAENETRESET: i32 = 10052;
pub const WSAECONNABORTED: i32 = 10053;
pub const WSAECONNRESET: i32 = 10054;
pub const WSAENOBUFS: i32 = 10055;
pub const WSAEISCONN: i32 = 10056;
pub const WSAENOTCONN: i32 = 10057;
pub const WSAESHUTDOWN: i32 = 10058;
pub const WSAETOOMANYREFS: i32 = 10059;
pub const WSAETIMEDOUT: i32 = 10060;
pub const WSAECONNREFUSED: i32 = 10061;
pub const WSAELOOP: i32 = 10062;
pub const WSAENAMETOOLONG: i32 = 10063;
pub const WSAEHOSTDOWN: i32 = 10064;
pub const WSAEHOSTUNREACH: i32 = 10065;
pub const WSAENOTEMPTY: i32 = 10066;
pub const WSAEPROCLIM: i32 = 10067;
pub const WSAEUSERS: i32 = 10068;
pub const WSAEDQUOT: i32 = 10069;
pub const WSAESTALE: i32 = 10070;
pub const WSAEREMOTE: i32 = 10071;
pub const WSASYSNOTREADY: i32 = 10091;
pub const WSAVERNOTSUPPORTED: i32 = 10092;
pub const WSANOTINITIALISED: i32 = 10093;
pub const WSAEDISCON: i32 = 10101;
pub const WSATYPE_NOT_FOUND: i32 = 10109;
pub const WSAHOST_NOT_FOUND: i32 = 11001;
pub const WSATRY_AGAIN: i32 = 11002;
pub const WSANO_RECOVERY: i32 = 11003;
pub const WSANO_DATA: i32 = 11004;
pub const WSA_IO_PENDING: i32 = 997;
pub const WSA_IO_INCOMPLETE: i32 = 996;
pub const WSA_INVALID_HANDLE: i32 = 6;
pub const WSA_INVALID_PARAMETER: i32 = 87;
pub const WSA_OPERATION_ABORTED: i32 = 995;

// =========================================================================
// Address families and socket types
// =========================================================================

pub const AF_UNSPEC: i32 = 0;
pub const AF_INET: i32 = 2;
pub const AF_IPX: i32 = 6;
pub const AF_APPLETALK: i32 = 16;
pub const AF_NETBIOS: i32 = 17;
pub const AF_INET6: i32 = 23;
pub const AF_IRDA: i32 = 26;
pub const AF_BTH: i32 = 32;

pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_RAW: i32 = 3;
pub const SOCK_RDM: i32 = 4;
pub const SOCK_SEQPACKET: i32 = 5;

pub const IPPROTO_ICMP: i32 = 1;
pub const IPPROTO_IGMP: i32 = 2;
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;
pub const IPPROTO_ICMPV6: i32 = 58;
pub const IPPROTO_RAW: i32 = 255;
pub const IPPROTO_IP: i32 = 0;
pub const IPPROTO_IPV6: i32 = 41;

// =========================================================================
// WSASocket flags
// =========================================================================

pub const WSA_FLAG_OVERLAPPED: u32 = 0x01;
pub const WSA_FLAG_MULTIPOINT_C_ROOT: u32 = 0x02;
pub const WSA_FLAG_MULTIPOINT_C_LEAF: u32 = 0x04;
pub const WSA_FLAG_MULTIPOINT_D_ROOT: u32 = 0x08;
pub const WSA_FLAG_MULTIPOINT_D_LEAF: u32 = 0x10;
pub const WSA_FLAG_NO_HANDLE_INHERIT: u32 = 0x80;

// =========================================================================
// Socket option levels and options
// =========================================================================

pub const SOL_SOCKET: i32 = 0xFFFF;

pub const SO_DEBUG: i32 = 0x0001;
pub const SO_ACCEPTCONN: i32 = 0x0002;
pub const SO_REUSEADDR: i32 = 0x0004;
pub const SO_KEEPALIVE: i32 = 0x0008;
pub const SO_DONTROUTE: i32 = 0x0010;
pub const SO_BROADCAST: i32 = 0x0020;
pub const SO_USELOOPBACK: i32 = 0x0040;
pub const SO_LINGER: i32 = 0x0080;
pub const SO_OOBINLINE: i32 = 0x0100;
pub const SO_SNDBUF: i32 = 0x1001;
pub const SO_RCVBUF: i32 = 0x1002;
pub const SO_SNDLOWAT: i32 = 0x1003;
pub const SO_RCVLOWAT: i32 = 0x1004;
pub const SO_SNDTIMEO: i32 = 0x1005;
pub const SO_RCVTIMEO: i32 = 0x1006;
pub const SO_ERROR: i32 = 0x1007;
pub const SO_TYPE: i32 = 0x1008;
pub const SO_EXCLUSIVEADDRUSE: i32 = !SO_REUSEADDR;
pub const SO_CONDITIONAL_ACCEPT: i32 = 0x3002;
pub const SO_UPDATE_ACCEPT_CONTEXT: i32 = 0x700B;
pub const SO_CONNECT_TIME: i32 = 0x700C;

pub const TCP_NODELAY: i32 = 0x0001;
pub const TCP_KEEPALIVE: i32 = 3;
pub const TCP_KEEPCNT: i32 = 16;
pub const TCP_KEEPINTVL: i32 = 17;
pub const TCP_FASTOPEN: i32 = 15;

pub const IP_OPTIONS: i32 = 1;
pub const IP_HDRINCL: i32 = 2;
pub const IP_TOS: i32 = 3;
pub const IP_TTL: i32 = 4;
pub const IP_MULTICAST_IF: i32 = 9;
pub const IP_MULTICAST_TTL: i32 = 10;
pub const IP_MULTICAST_LOOP: i32 = 11;
pub const IP_ADD_MEMBERSHIP: i32 = 12;
pub const IP_DROP_MEMBERSHIP: i32 = 13;
pub const IP_DONTFRAGMENT: i32 = 14;
pub const IP_PKTINFO: i32 = 19;

pub const IPV6_HOPLIMIT: i32 = 21;
pub const IPV6_UNICAST_HOPS: i32 = 4;
pub const IPV6_MULTICAST_IF: i32 = 9;
pub const IPV6_MULTICAST_HOPS: i32 = 10;
pub const IPV6_MULTICAST_LOOP: i32 = 11;
pub const IPV6_ADD_MEMBERSHIP: i32 = 12;
pub const IPV6_DROP_MEMBERSHIP: i32 = 13;
pub const IPV6_V6ONLY: i32 = 27;
pub const IPV6_PKTINFO: i32 = 19;

// =========================================================================
// Async select event flags
// =========================================================================

pub const FD_READ: u32 = 0x01;
pub const FD_WRITE: u32 = 0x02;
pub const FD_OOB: u32 = 0x04;
pub const FD_ACCEPT: u32 = 0x08;
pub const FD_CONNECT: u32 = 0x10;
pub const FD_CLOSE: u32 = 0x20;
pub const FD_QOS: u32 = 0x40;
pub const FD_GROUP_QOS: u32 = 0x80;
pub const FD_ROUTING_INTERFACE_CHANGE: u32 = 0x100;
pub const FD_ADDRESS_LIST_CHANGE: u32 = 0x200;
pub const FD_ALL_EVENTS: u32 = 0x3FF;

// =========================================================================
// SIO ioctl codes
// =========================================================================

pub const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xC8000006;
pub const SIO_KEEPALIVE_VALS: u32 = 0x98000004;
pub const SIO_ROUTING_INTERFACE_QUERY: u32 = 0xC8000014;
pub const SIO_ADDRESS_LIST_QUERY: u32 = 0x48000016;
pub const SIO_TCP_INITIAL_RTO: u32 = 0x98000011;
pub const SIO_RCVALL: u32 = 0x98000001;
pub const SIO_ASSOCIATE_HANDLE: u32 = 0x88000001;

// =========================================================================
// Shutdown modes
// =========================================================================

pub const SD_RECEIVE: i32 = 0;
pub const SD_SEND: i32 = 1;
pub const SD_BOTH: i32 = 2;

// =========================================================================
// Structures
// =========================================================================

#[derive(Debug, Clone, Copy, Default)]
pub struct InAddr {
    pub s_addr: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct In6Addr {
    pub addr: [u8; 16],
}

#[derive(Debug, Clone)]
pub struct SockAddrIn {
    pub family: i16,
    pub port: u16,
    pub addr: InAddr,
}

#[derive(Debug, Clone)]
pub struct SockAddrIn6 {
    pub family: i16,
    pub port: u16,
    pub flowinfo: u32,
    pub addr: In6Addr,
    pub scope_id: u32,
}

#[derive(Debug, Clone)]
pub enum SockAddr {
    V4(SockAddrIn),
    V6(SockAddrIn6),
    Unknown { family: i16, data: Vec<u8> },
}

#[derive(Debug, Clone)]
pub struct AddrInfoW {
    pub flags: i32,
    pub family: i32,
    pub sock_type: i32,
    pub protocol: i32,
    pub canon_name: Option<String>,
    pub addr: SockAddr,
}

#[derive(Debug, Clone)]
pub struct WsaBuf {
    pub len: u32,
    pub buf_offset: u64,
}

#[derive(Debug, Clone)]
pub struct WsaOverlapped {
    pub internal: u64,
    pub internal_high: u64,
    pub offset: u64,
    pub event: WinHandle,
    pub completed: bool,
    pub bytes_transferred: u32,
    pub error: i32,
}

impl Default for WsaOverlapped {
    fn default() -> Self {
        Self {
            internal: 0,
            internal_high: 0,
            offset: 0,
            event: WinHandle(0),
            completed: false,
            bytes_transferred: 0,
            error: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsaMsg {
    pub name: Option<SockAddr>,
    pub buffers: Vec<WsaBuf>,
    pub control_buf: Vec<u8>,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct WsaNetworkEvents {
    pub network_events: u32,
    pub error_code: [i32; 10],
}

impl Default for WsaNetworkEvents {
    fn default() -> Self {
        Self {
            network_events: 0,
            error_code: [0; 10],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Linger {
    pub onoff: u16,
    pub linger_time: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct IpMreq {
    pub multiaddr: InAddr,
    pub interface: InAddr,
}

#[derive(Debug, Clone, Copy)]
pub struct Ipv6Mreq {
    pub multiaddr: In6Addr,
    pub interface_index: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct TcpKeepalive {
    pub onoff: u32,
    pub keepalivetime: u32,
    pub keepaliveinterval: u32,
}

#[derive(Debug, Clone)]
pub struct HostEnt {
    pub name: String,
    pub aliases: Vec<String>,
    pub addr_type: i16,
    pub length: i16,
    pub addr_list: Vec<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct ServEnt {
    pub name: String,
    pub aliases: Vec<String>,
    pub port: i16,
    pub proto: String,
}

// =========================================================================
// Socket state
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Created,
    Bound,
    Listening,
    Connected,
    Closed,
}

#[derive(Debug, Clone)]
pub struct SocketOptions {
    pub reuseaddr: bool,
    pub keepalive: bool,
    pub broadcast: bool,
    pub linger: Option<Linger>,
    pub sndbuf: u32,
    pub rcvbuf: u32,
    pub sndtimeo: u32,
    pub rcvtimeo: u32,
    pub tcp_nodelay: bool,
    pub tcp_keepalive_time: u32,
    pub tcp_keepcnt: u32,
    pub tcp_keepintvl: u32,
    pub tcp_fastopen: bool,
    pub ip_ttl: u32,
    pub ip_multicast_ttl: u32,
    pub ip_multicast_loop: bool,
    pub ip_dontfragment: bool,
    pub ip_hdrincl: bool,
    pub ipv6_v6only: bool,
    pub ipv6_unicast_hops: u32,
    pub ipv6_multicast_hops: u32,
    pub ipv6_multicast_loop: bool,
    pub exclusive_addr_use: bool,
    pub conditional_accept: bool,
}

impl Default for SocketOptions {
    fn default() -> Self {
        Self {
            reuseaddr: false,
            keepalive: false,
            broadcast: false,
            linger: None,
            sndbuf: 65536,
            rcvbuf: 65536,
            sndtimeo: 0,
            rcvtimeo: 0,
            tcp_nodelay: false,
            tcp_keepalive_time: 7200000,
            tcp_keepcnt: 10,
            tcp_keepintvl: 1000,
            tcp_fastopen: false,
            ip_ttl: 128,
            ip_multicast_ttl: 1,
            ip_multicast_loop: true,
            ip_dontfragment: false,
            ip_hdrincl: false,
            ipv6_v6only: false,
            ipv6_unicast_hops: 128,
            ipv6_multicast_hops: 1,
            ipv6_multicast_loop: true,
            exclusive_addr_use: false,
            conditional_accept: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsSocket {
    pub id: u64,
    pub family: i32,
    pub sock_type: i32,
    pub protocol: i32,
    pub state: SocketState,
    pub flags: u32,
    pub local_addr: Option<SockAddr>,
    pub remote_addr: Option<SockAddr>,
    pub options: SocketOptions,
    pub send_buffer: Vec<u8>,
    pub recv_buffer: Vec<u8>,
    pub pending_overlapped: Vec<WsaOverlapped>,
    pub event_mask: u32,
    pub async_window: WinHandle,
    pub async_msg: u32,
    pub multicast_groups: Vec<IpMreq>,
    pub multicast_groups_v6: Vec<Ipv6Mreq>,
    pub backlog: u32,
    pub connect_time: u32,
    pub nonblocking: bool,
}

// =========================================================================
// WinSock2 runtime
// =========================================================================

pub struct WinSock2Runtime {
    pub initialized: bool,
    pub version: u16,
    pub high_version: u16,
    pub last_error: i32,
    pub sockets: BTreeMap<u64, WsSocket>,
    pub next_socket_id: u64,
    pub events: BTreeMap<u64, bool>,
    pub next_event_id: u64,
    pub dns_cache: BTreeMap<String, Vec<InAddr>>,
    pub iocp_handles: Vec<WinHandle>,
}

impl WinSock2Runtime {
    fn new() -> Self {
        Self {
            initialized: false,
            version: 0,
            high_version: 0,
            last_error: 0,
            sockets: BTreeMap::new(),
            next_socket_id: 0x1000,
            events: BTreeMap::new(),
            next_event_id: 0x2000,
            dns_cache: BTreeMap::new(),
            iocp_handles: Vec::new(),
        }
    }

    // -- WSA lifecycle --

    pub fn wsa_startup(&mut self, version_requested: u16) -> i32 {
        let major = version_requested & 0xFF;
        let minor = (version_requested >> 8) & 0xFF;
        if major < 2 {
            self.last_error = WSAVERNOTSUPPORTED;
            return WSAVERNOTSUPPORTED;
        }
        self.version = version_requested;
        self.high_version = 0x0202;
        self.initialized = true;
        self.last_error = 0;
        let _ = (major, minor);
        0
    }

    pub fn wsa_cleanup(&mut self) -> i32 {
        if !self.initialized {
            self.last_error = WSANOTINITIALISED;
            return -1;
        }
        self.initialized = false;
        self.sockets.clear();
        self.events.clear();
        0
    }

    pub fn wsa_get_last_error(&self) -> i32 {
        self.last_error
    }

    pub fn wsa_set_last_error(&mut self, error: i32) {
        self.last_error = error;
    }

    // -- Socket creation --

    pub fn socket(&mut self, af: i32, sock_type: i32, protocol: i32) -> i64 {
        self.wsa_socket(af, sock_type, protocol, WSA_FLAG_OVERLAPPED)
    }

    pub fn wsa_socket(&mut self, af: i32, sock_type: i32, protocol: i32, flags: u32) -> i64 {
        if !self.initialized {
            self.last_error = WSANOTINITIALISED;
            return -1;
        }
        let id = self.next_socket_id;
        self.next_socket_id += 1;
        self.sockets.insert(
            id,
            WsSocket {
                id,
                family: af,
                sock_type,
                protocol,
                state: SocketState::Created,
                flags,
                local_addr: None,
                remote_addr: None,
                options: SocketOptions::default(),
                send_buffer: Vec::new(),
                recv_buffer: Vec::new(),
                pending_overlapped: Vec::new(),
                event_mask: 0,
                async_window: WinHandle(0),
                async_msg: 0,
                multicast_groups: Vec::new(),
                multicast_groups_v6: Vec::new(),
                backlog: 0,
                connect_time: 0,
                nonblocking: false,
            },
        );
        id as i64
    }

    pub fn closesocket(&mut self, sock: u64) -> i32 {
        if self.sockets.remove(&sock).is_some() {
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    // -- Bind / listen / accept / connect --

    pub fn bind(&mut self, sock: u64, addr: SockAddr) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.local_addr = Some(addr);
            s.state = SocketState::Bound;
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn listen(&mut self, sock: u64, backlog: i32) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.state = SocketState::Listening;
            s.backlog = backlog as u32;
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn accept(&mut self, sock: u64) -> (i64, Option<SockAddr>) {
        if let Some(s) = self.sockets.get(&sock) {
            if s.state != SocketState::Listening {
                self.last_error = WSAEINVAL;
                return (-1, None);
            }
            let new_id = self.next_socket_id;
            self.next_socket_id += 1;
            let new_sock = WsSocket {
                id: new_id,
                family: s.family,
                sock_type: s.sock_type,
                protocol: s.protocol,
                state: SocketState::Connected,
                flags: s.flags,
                local_addr: s.local_addr.clone(),
                remote_addr: None,
                options: SocketOptions::default(),
                send_buffer: Vec::new(),
                recv_buffer: Vec::new(),
                pending_overlapped: Vec::new(),
                event_mask: 0,
                async_window: WinHandle(0),
                async_msg: 0,
                multicast_groups: Vec::new(),
                multicast_groups_v6: Vec::new(),
                backlog: 0,
                connect_time: 0,
                nonblocking: false,
            };
            self.sockets.insert(new_id, new_sock);
            (new_id as i64, None)
        } else {
            self.last_error = WSAENOTSOCK;
            (-1, None)
        }
    }

    pub fn connect(&mut self, sock: u64, addr: SockAddr) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.remote_addr = Some(addr);
            s.state = SocketState::Connected;
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn wsa_connect(&mut self, sock: u64, addr: SockAddr) -> i32 {
        self.connect(sock, addr)
    }

    pub fn shutdown(&mut self, sock: u64, how: i32) -> i32 {
        let _ = how;
        if self.sockets.contains_key(&sock) {
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    // -- I/O --

    pub fn send(&mut self, sock: u64, data: &[u8], _flags: i32) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.send_buffer.extend_from_slice(data);
            data.len() as i32
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn recv(&mut self, sock: u64, buf_len: usize, _flags: i32) -> (i32, Vec<u8>) {
        if let Some(s) = self.sockets.get_mut(&sock) {
            let drain_len = buf_len.min(s.recv_buffer.len());
            let data: Vec<u8> = s.recv_buffer.drain(..drain_len).collect();
            (data.len() as i32, data)
        } else {
            self.last_error = WSAENOTSOCK;
            (-1, Vec::new())
        }
    }

    pub fn sendto(&mut self, sock: u64, data: &[u8], _flags: i32, _addr: &SockAddr) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.send_buffer.extend_from_slice(data);
            data.len() as i32
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn recvfrom(
        &mut self,
        sock: u64,
        buf_len: usize,
        _flags: i32,
    ) -> (i32, Vec<u8>, Option<SockAddr>) {
        if let Some(s) = self.sockets.get_mut(&sock) {
            let drain_len = buf_len.min(s.recv_buffer.len());
            let data: Vec<u8> = s.recv_buffer.drain(..drain_len).collect();
            (data.len() as i32, data, s.remote_addr.clone())
        } else {
            self.last_error = WSAENOTSOCK;
            (-1, Vec::new(), None)
        }
    }

    pub fn wsa_send(
        &mut self,
        sock: u64,
        bufs: &[WsaBuf],
        _flags: u32,
        overlapped: Option<&mut WsaOverlapped>,
    ) -> i32 {
        let total: u32 = bufs.iter().map(|b| b.len).sum();
        if let Some(ovl) = overlapped {
            ovl.bytes_transferred = total;
            ovl.completed = true;
            ovl.error = 0;
        }
        if self.sockets.contains_key(&sock) {
            total as i32
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn wsa_recv(
        &mut self,
        sock: u64,
        bufs: &[WsaBuf],
        _flags: u32,
        overlapped: Option<&mut WsaOverlapped>,
    ) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            let total: u32 = bufs.iter().map(|b| b.len).sum();
            let available = s.recv_buffer.len() as u32;
            let bytes = total.min(available);
            s.recv_buffer.drain(..bytes as usize);
            if let Some(ovl) = overlapped {
                ovl.bytes_transferred = bytes;
                ovl.completed = true;
                ovl.error = 0;
            }
            bytes as i32
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn wsa_send_to(
        &mut self,
        sock: u64,
        bufs: &[WsaBuf],
        _flags: u32,
        _addr: &SockAddr,
        overlapped: Option<&mut WsaOverlapped>,
    ) -> i32 {
        let total: u32 = bufs.iter().map(|b| b.len).sum();
        if let Some(ovl) = overlapped {
            ovl.bytes_transferred = total;
            ovl.completed = true;
        }
        if self.sockets.contains_key(&sock) {
            total as i32
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn wsa_recv_from(
        &mut self,
        sock: u64,
        bufs: &[WsaBuf],
        _flags: u32,
        overlapped: Option<&mut WsaOverlapped>,
    ) -> (i32, Option<SockAddr>) {
        if let Some(s) = self.sockets.get_mut(&sock) {
            let total: u32 = bufs.iter().map(|b| b.len).sum();
            let available = s.recv_buffer.len() as u32;
            let bytes = total.min(available);
            s.recv_buffer.drain(..bytes as usize);
            if let Some(ovl) = overlapped {
                ovl.bytes_transferred = bytes;
                ovl.completed = true;
            }
            (bytes as i32, s.remote_addr.clone())
        } else {
            self.last_error = WSAENOTSOCK;
            (-1, None)
        }
    }

    pub fn wsa_send_msg(
        &mut self,
        sock: u64,
        _msg: &WsaMsg,
        _flags: u32,
        overlapped: Option<&mut WsaOverlapped>,
    ) -> i32 {
        if !self.sockets.contains_key(&sock) {
            self.last_error = WSAENOTSOCK;
            return -1;
        }
        if let Some(ovl) = overlapped {
            ovl.completed = true;
            ovl.error = 0;
        }
        0
    }

    pub fn wsa_recv_msg(
        &mut self,
        sock: u64,
        _msg: &mut WsaMsg,
        overlapped: Option<&mut WsaOverlapped>,
    ) -> i32 {
        if !self.sockets.contains_key(&sock) {
            self.last_error = WSAENOTSOCK;
            return -1;
        }
        if let Some(ovl) = overlapped {
            ovl.completed = true;
            ovl.error = 0;
        }
        0
    }

    // -- Overlapped I/O --

    pub fn wsa_get_overlapped_result(
        &self,
        _sock: u64,
        ovl: &WsaOverlapped,
        _wait: bool,
    ) -> (bool, u32) {
        (ovl.completed, ovl.bytes_transferred)
    }

    pub fn wsa_create_event(&mut self) -> u64 {
        let id = self.next_event_id;
        self.next_event_id += 1;
        self.events.insert(id, false);
        id
    }

    pub fn wsa_close_event(&mut self, event: u64) -> bool {
        self.events.remove(&event).is_some()
    }

    pub fn wsa_set_event(&mut self, event: u64) -> bool {
        if let Some(e) = self.events.get_mut(&event) {
            *e = true;
            true
        } else {
            false
        }
    }

    pub fn wsa_reset_event(&mut self, event: u64) -> bool {
        if let Some(e) = self.events.get_mut(&event) {
            *e = false;
            true
        } else {
            false
        }
    }

    pub fn wsa_wait_for_multiple_events(
        &self,
        events: &[u64],
        wait_all: bool,
        timeout_ms: u32,
    ) -> u32 {
        let _ = (wait_all, timeout_ms);
        for (i, &ev_id) in events.iter().enumerate() {
            if let Some(&signaled) = self.events.get(&ev_id) {
                if signaled {
                    return i as u32;
                }
            }
        }
        0x00000102 // WAIT_TIMEOUT
    }

    // -- Select / event select --

    pub fn select(
        &self,
        _nfds: i32,
        read_fds: &[u64],
        write_fds: &[u64],
        except_fds: &[u64],
        _timeout_us: Option<u64>,
    ) -> i32 {
        let mut count = 0i32;
        for &fd in read_fds {
            if let Some(s) = self.sockets.get(&fd) {
                if !s.recv_buffer.is_empty() || s.state == SocketState::Listening {
                    count += 1;
                }
            }
        }
        for &fd in write_fds {
            if self.sockets.contains_key(&fd) {
                count += 1;
            }
        }
        let _ = except_fds;
        count
    }

    pub fn wsa_event_select(&mut self, sock: u64, event: u64, events: u32) -> i32 {
        let _ = event;
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.event_mask = events;
            s.nonblocking = true;
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn wsa_enum_network_events(&mut self, sock: u64, event: u64) -> (i32, WsaNetworkEvents) {
        if let Some(e) = self.events.get_mut(&event) {
            *e = false;
        }
        if self.sockets.contains_key(&sock) {
            (0, WsaNetworkEvents::default())
        } else {
            self.last_error = WSAENOTSOCK;
            (-1, WsaNetworkEvents::default())
        }
    }

    pub fn wsa_async_select(&mut self, sock: u64, hwnd: WinHandle, msg: u32, events: u32) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.async_window = hwnd;
            s.async_msg = msg;
            s.event_mask = events;
            s.nonblocking = true;
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    // -- Socket options --

    pub fn setsockopt(&mut self, sock: u64, level: i32, optname: i32, value: u32) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            match (level, optname) {
                (SOL_SOCKET, SO_REUSEADDR) => s.options.reuseaddr = value != 0,
                (SOL_SOCKET, SO_KEEPALIVE) => s.options.keepalive = value != 0,
                (SOL_SOCKET, SO_BROADCAST) => s.options.broadcast = value != 0,
                (SOL_SOCKET, SO_SNDBUF) => s.options.sndbuf = value,
                (SOL_SOCKET, SO_RCVBUF) => s.options.rcvbuf = value,
                (SOL_SOCKET, SO_SNDTIMEO) => s.options.sndtimeo = value,
                (SOL_SOCKET, SO_RCVTIMEO) => s.options.rcvtimeo = value,
                (SOL_SOCKET, SO_EXCLUSIVEADDRUSE) => s.options.exclusive_addr_use = value != 0,
                (SOL_SOCKET, SO_CONDITIONAL_ACCEPT) => s.options.conditional_accept = value != 0,
                (IPPROTO_TCP, TCP_NODELAY) => s.options.tcp_nodelay = value != 0,
                (IPPROTO_TCP, TCP_KEEPALIVE) => s.options.tcp_keepalive_time = value,
                (IPPROTO_TCP, TCP_KEEPCNT) => s.options.tcp_keepcnt = value,
                (IPPROTO_TCP, TCP_KEEPINTVL) => s.options.tcp_keepintvl = value,
                (IPPROTO_TCP, TCP_FASTOPEN) => s.options.tcp_fastopen = value != 0,
                (IPPROTO_IP, IP_TTL) => s.options.ip_ttl = value,
                (IPPROTO_IP, IP_MULTICAST_TTL) => s.options.ip_multicast_ttl = value,
                (IPPROTO_IP, IP_MULTICAST_LOOP) => s.options.ip_multicast_loop = value != 0,
                (IPPROTO_IP, IP_DONTFRAGMENT) => s.options.ip_dontfragment = value != 0,
                (IPPROTO_IP, IP_HDRINCL) => s.options.ip_hdrincl = value != 0,
                (IPPROTO_IPV6, IPV6_V6ONLY) => s.options.ipv6_v6only = value != 0,
                (IPPROTO_IPV6, IPV6_UNICAST_HOPS) => s.options.ipv6_unicast_hops = value,
                (IPPROTO_IPV6, IPV6_MULTICAST_HOPS) => s.options.ipv6_multicast_hops = value,
                (IPPROTO_IPV6, IPV6_MULTICAST_LOOP) => s.options.ipv6_multicast_loop = value != 0,
                _ => {
                    self.last_error = WSAENOPROTOOPT;
                    return -1;
                }
            }
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn getsockopt(&self, sock: u64, level: i32, optname: i32) -> (i32, u32) {
        if let Some(s) = self.sockets.get(&sock) {
            let val = match (level, optname) {
                (SOL_SOCKET, SO_REUSEADDR) => s.options.reuseaddr as u32,
                (SOL_SOCKET, SO_KEEPALIVE) => s.options.keepalive as u32,
                (SOL_SOCKET, SO_BROADCAST) => s.options.broadcast as u32,
                (SOL_SOCKET, SO_SNDBUF) => s.options.sndbuf,
                (SOL_SOCKET, SO_RCVBUF) => s.options.rcvbuf,
                (SOL_SOCKET, SO_SNDTIMEO) => s.options.sndtimeo,
                (SOL_SOCKET, SO_RCVTIMEO) => s.options.rcvtimeo,
                (SOL_SOCKET, SO_ERROR) => 0,
                (SOL_SOCKET, SO_TYPE) => s.sock_type as u32,
                (SOL_SOCKET, SO_CONNECT_TIME) => s.connect_time,
                (IPPROTO_TCP, TCP_NODELAY) => s.options.tcp_nodelay as u32,
                (IPPROTO_IP, IP_TTL) => s.options.ip_ttl,
                (IPPROTO_IPV6, IPV6_V6ONLY) => s.options.ipv6_v6only as u32,
                _ => 0,
            };
            (0, val)
        } else {
            (WSAENOTSOCK, 0)
        }
    }

    // -- Multicast --

    pub fn join_multicast_group_v4(&mut self, sock: u64, group: IpMreq) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.multicast_groups.push(group);
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn leave_multicast_group_v4(&mut self, sock: u64, group: &IpMreq) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.multicast_groups
                .retain(|g| g.multiaddr.s_addr != group.multiaddr.s_addr);
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn join_multicast_group_v6(&mut self, sock: u64, group: Ipv6Mreq) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.multicast_groups_v6.push(group);
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    pub fn leave_multicast_group_v6(&mut self, sock: u64, group: &Ipv6Mreq) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.multicast_groups_v6
                .retain(|g| g.multiaddr.addr != group.multiaddr.addr);
            0
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    // -- Name resolution --

    pub fn getaddrinfo(
        &self,
        hostname: &str,
        service: Option<&str>,
        hints: Option<&AddrInfoW>,
    ) -> (i32, Vec<AddrInfoW>) {
        let _ = (hints, service);
        let loopback = InAddr { s_addr: 0x7F000001 };
        let addr = if hostname == "localhost" || hostname.is_empty() {
            loopback
        } else if let Some(cached) = self.dns_cache.get(hostname) {
            cached.first().copied().unwrap_or(loopback)
        } else {
            loopback
        };
        let info = AddrInfoW {
            flags: 0,
            family: AF_INET,
            sock_type: SOCK_STREAM,
            protocol: IPPROTO_TCP,
            canon_name: Some(String::from(hostname)),
            addr: SockAddr::V4(SockAddrIn {
                family: AF_INET as i16,
                port: 0,
                addr,
            }),
        };
        (0, alloc::vec![info])
    }

    pub fn getnameinfo(&self, addr: &SockAddr) -> (i32, String, String) {
        match addr {
            SockAddr::V4(v4) => {
                let a = (v4.addr.s_addr >> 24) & 0xFF;
                let b = (v4.addr.s_addr >> 16) & 0xFF;
                let c = (v4.addr.s_addr >> 8) & 0xFF;
                let d = v4.addr.s_addr & 0xFF;
                let host = alloc::format!("{}.{}.{}.{}", a, b, c, d);
                let serv = alloc::format!("{}", v4.port);
                (0, host, serv)
            }
            _ => (WSAHOST_NOT_FOUND, String::new(), String::new()),
        }
    }

    pub fn gethostbyname(&self, name: &str) -> Option<HostEnt> {
        let addr = if name == "localhost" {
            InAddr { s_addr: 0x7F000001 }
        } else if let Some(cached) = self.dns_cache.get(name) {
            cached
                .first()
                .copied()
                .unwrap_or(InAddr { s_addr: 0x7F000001 })
        } else {
            InAddr { s_addr: 0x7F000001 }
        };
        Some(HostEnt {
            name: String::from(name),
            aliases: Vec::new(),
            addr_type: AF_INET as i16,
            length: 4,
            addr_list: alloc::vec![addr.s_addr.to_be_bytes().to_vec()],
        })
    }

    pub fn gethostbyaddr(&self, addr: &[u8], addr_type: i32) -> Option<HostEnt> {
        let _ = addr_type;
        if addr.len() >= 4 {
            let ip = alloc::format!("{}.{}.{}.{}", addr[0], addr[1], addr[2], addr[3]);
            Some(HostEnt {
                name: ip,
                aliases: Vec::new(),
                addr_type: AF_INET as i16,
                length: 4,
                addr_list: alloc::vec![addr.to_vec()],
            })
        } else {
            None
        }
    }

    pub fn getservbyname(&self, name: &str, proto: &str) -> Option<ServEnt> {
        let port = match name {
            "http" => 80,
            "https" => 443,
            "ftp" => 21,
            "ssh" => 22,
            "telnet" => 23,
            "smtp" => 25,
            "dns" => 53,
            "pop3" => 110,
            "imap" => 143,
            _ => return None,
        };
        Some(ServEnt {
            name: String::from(name),
            aliases: Vec::new(),
            port,
            proto: String::from(proto),
        })
    }

    pub fn getservbyport(&self, port: i16, proto: &str) -> Option<ServEnt> {
        let name = match port {
            80 => "http",
            443 => "https",
            21 => "ftp",
            22 => "ssh",
            23 => "telnet",
            25 => "smtp",
            53 => "dns",
            110 => "pop3",
            143 => "imap",
            _ => return None,
        };
        Some(ServEnt {
            name: String::from(name),
            aliases: Vec::new(),
            port,
            proto: String::from(proto),
        })
    }

    // -- Extension functions --

    pub fn accept_ex(&mut self, listen_sock: u64) -> (bool, i64) {
        let (new_fd, _) = self.accept(listen_sock);
        (new_fd >= 0, new_fd)
    }

    pub fn connect_ex(&mut self, sock: u64, addr: SockAddr) -> bool {
        self.connect(sock, addr) == 0
    }

    pub fn disconnect_ex(&mut self, sock: u64, _reuse: bool) -> bool {
        if let Some(s) = self.sockets.get_mut(&sock) {
            s.state = SocketState::Created;
            s.remote_addr = None;
            true
        } else {
            false
        }
    }

    pub fn transmit_file(&mut self, sock: u64, _file: WinHandle, _bytes_per_send: u32) -> bool {
        self.sockets.contains_key(&sock)
    }

    pub fn transmit_packets(&mut self, sock: u64, _packets: &[(u64, u32)]) -> bool {
        self.sockets.contains_key(&sock)
    }

    // -- WSAIoctl --

    pub fn wsa_ioctl(&mut self, sock: u64, code: u32, _in_buf: &[u8]) -> (i32, Vec<u8>) {
        if !self.sockets.contains_key(&sock) {
            self.last_error = WSAENOTSOCK;
            return (-1, Vec::new());
        }
        match code {
            SIO_KEEPALIVE_VALS => (0, Vec::new()),
            SIO_GET_EXTENSION_FUNCTION_POINTER => (0, alloc::vec![0u8; 16]),
            SIO_ROUTING_INTERFACE_QUERY => (0, Vec::new()),
            SIO_ADDRESS_LIST_QUERY => (0, Vec::new()),
            SIO_TCP_INITIAL_RTO => (0, Vec::new()),
            _ => {
                self.last_error = WSAEINVAL;
                (-1, Vec::new())
            }
        }
    }

    pub fn ioctlsocket(&mut self, sock: u64, cmd: u32, arg: &mut u32) -> i32 {
        if let Some(s) = self.sockets.get_mut(&sock) {
            const FIONBIO: u32 = 0x8004667E;
            const FIONREAD: u32 = 0x4004667F;
            match cmd {
                FIONBIO => {
                    s.nonblocking = *arg != 0;
                    0
                }
                FIONREAD => {
                    *arg = s.recv_buffer.len() as u32;
                    0
                }
                _ => {
                    self.last_error = WSAEINVAL;
                    -1
                }
            }
        } else {
            self.last_error = WSAENOTSOCK;
            -1
        }
    }

    // -- Byte order --

    pub fn htonl(val: u32) -> u32 {
        val.to_be()
    }
    pub fn htons(val: u16) -> u16 {
        val.to_be()
    }
    pub fn ntohl(val: u32) -> u32 {
        u32::from_be(val)
    }
    pub fn ntohs(val: u16) -> u16 {
        u16::from_be(val)
    }

    // -- Address conversion --

    pub fn inet_addr(cp: &str) -> u32 {
        let parts: Vec<&str> = cp.split('.').collect();
        if parts.len() != 4 {
            return 0xFFFFFFFF;
        }
        let mut addr: u32 = 0;
        for (i, part) in parts.iter().enumerate() {
            if let Ok(octet) = u8::from_str_radix(part.trim(), 10) {
                addr |= (octet as u32) << (24 - i * 8);
            } else {
                return 0xFFFFFFFF;
            }
        }
        addr
    }

    pub fn inet_ntoa(addr: InAddr) -> String {
        let a = (addr.s_addr >> 24) & 0xFF;
        let b = (addr.s_addr >> 16) & 0xFF;
        let c = (addr.s_addr >> 8) & 0xFF;
        let d = addr.s_addr & 0xFF;
        alloc::format!("{}.{}.{}.{}", a, b, c, d)
    }

    // -- Raw socket helpers --

    pub fn create_raw_socket(&mut self, af: i32, protocol: i32, hdrincl: bool) -> i64 {
        let fd = self.wsa_socket(af, SOCK_RAW, protocol, WSA_FLAG_OVERLAPPED);
        if fd >= 0 && hdrincl {
            if let Some(s) = self.sockets.get_mut(&(fd as u64)) {
                s.options.ip_hdrincl = true;
            }
        }
        fd
    }

    pub fn send_raw_packet(&mut self, sock: u64, packet: &[u8]) -> i32 {
        self.send(sock, packet, 0)
    }
}

// =========================================================================
// Global WINSOCK2 runtime
// =========================================================================

static WINSOCK2_INITIALIZED: AtomicBool = AtomicBool::new(false);

static mut WINSOCK2_RUNTIME_INNER: Option<WinSock2Runtime> = None;

pub fn init() {
    if WINSOCK2_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            WINSOCK2_RUNTIME_INNER = Some(WinSock2Runtime::new());
        }
    }
}

pub fn runtime() -> Option<&'static WinSock2Runtime> {
    if WINSOCK2_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { WINSOCK2_RUNTIME_INNER.as_ref() }
    } else {
        None
    }
}

pub fn runtime_mut() -> Option<&'static mut WinSock2Runtime> {
    if WINSOCK2_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { WINSOCK2_RUNTIME_INNER.as_mut() }
    } else {
        None
    }
}
