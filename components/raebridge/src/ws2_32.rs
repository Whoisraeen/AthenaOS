//! ws2_32.dll — Windows Sockets 2 (Winsock) API stubs for AthBridge.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    CompatContext, HandleType, WinBool, WinHandle, WsaData, FALSE, GENERIC_ALL, INVALID_SOCKET,
    NULL_HANDLE, SOCKET_ERROR, TRUE,
};

// =========================================================================
// Winsock error codes
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
pub const WSAEAFNOSUPPORT: i32 = 10047;
pub const WSAEADDRINUSE: i32 = 10048;
pub const WSAEADDRNOTAVAIL: i32 = 10049;
pub const WSAENETDOWN: i32 = 10050;
pub const WSAENETUNREACH: i32 = 10051;
pub const WSAECONNABORTED: i32 = 10053;
pub const WSAECONNRESET: i32 = 10054;
pub const WSAENOBUFS: i32 = 10055;
pub const WSAEISCONN: i32 = 10056;
pub const WSAENOTCONN: i32 = 10057;
pub const WSAESHUTDOWN: i32 = 10058;
pub const WSAETIMEDOUT: i32 = 10060;
pub const WSAECONNREFUSED: i32 = 10061;
pub const WSANOTINITIALISED: i32 = 10093;

// Address families
pub const AF_UNSPEC: i32 = 0;
pub const AF_INET: i32 = 2;
pub const AF_INET6: i32 = 23;

// Socket types
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_RAW: i32 = 3;

// Protocols
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

// Shutdown modes
pub const SD_RECEIVE: i32 = 0;
pub const SD_SEND: i32 = 1;
pub const SD_BOTH: i32 = 2;

// Socket option levels
pub const SOL_SOCKET: i32 = 0xFFFF;
pub const SO_REUSEADDR: i32 = 0x0004;
pub const SO_KEEPALIVE: i32 = 0x0008;
pub const SO_BROADCAST: i32 = 0x0020;
pub const SO_LINGER: i32 = 0x0080;
pub const SO_RCVBUF: i32 = 0x1002;
pub const SO_SNDBUF: i32 = 0x1001;
pub const SO_RCVTIMEO: i32 = 0x1006;
pub const SO_SNDTIMEO: i32 = 0x1005;
pub const TCP_NODELAY: i32 = 0x0001;

// ioctlsocket commands
pub const FIONBIO: u32 = 0x8004667E;
pub const FIONREAD: u32 = 0x4004667F;

// =========================================================================
// Socket address structures
// =========================================================================

#[derive(Debug, Clone, Default)]
pub struct SockAddr {
    pub family: i16,
    pub port: u16,
    pub addr: [u8; 14],
}

#[derive(Debug, Clone, Default)]
pub struct SockAddrIn {
    pub family: i16,
    pub port: u16,
    pub addr: u32,
    pub zero: [u8; 8],
}

#[derive(Debug, Clone)]
pub struct AddrInfo {
    pub flags: i32,
    pub family: i32,
    pub socktype: i32,
    pub protocol: i32,
    pub addr_len: usize,
    pub canon_name: Option<String>,
    pub addr: SockAddr,
}

// =========================================================================
// Startup / cleanup
// =========================================================================

pub fn wsa_startup(ctx: &mut CompatContext, version_requested: u16, wsa_data: &mut WsaData) -> i32 {
    if ctx.wsa_initialized {
        ctx.wsa_error = 0;
        return 0;
    }

    let major = version_requested & 0xFF;
    let minor = (version_requested >> 8) & 0xFF;

    if major < 2 {
        ctx.wsa_error = WSAEINVAL;
        return WSAEINVAL;
    }

    wsa_data.version = version_requested;
    wsa_data.high_version = 0x0202; // Winsock 2.2
    wsa_data.description = String::from("AthBridge Winsock 2.2");
    wsa_data.system_status = String::from("Running");
    wsa_data.max_sockets = 1024;
    wsa_data.max_udp_dg = 65507;

    let _ = minor;
    ctx.wsa_initialized = true;
    ctx.wsa_error = 0;
    0
}

pub fn wsa_cleanup(ctx: &mut CompatContext) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    ctx.wsa_initialized = false;
    ctx.wsa_error = 0;
    0
}

pub fn wsa_get_last_error(ctx: &CompatContext) -> i32 {
    ctx.wsa_error
}

pub fn wsa_set_last_error(ctx: &mut CompatContext, error: i32) {
    ctx.wsa_error = error;
}

// =========================================================================
// Socket creation and connection
// =========================================================================

pub fn socket(ctx: &mut CompatContext, af: i32, sock_type: i32, protocol: i32) -> u64 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return INVALID_SOCKET;
    }

    if af != AF_INET && af != AF_INET6 && af != AF_UNSPEC {
        ctx.wsa_error = WSAEAFNOSUPPORT;
        return INVALID_SOCKET;
    }

    if sock_type != SOCK_STREAM && sock_type != SOCK_DGRAM && sock_type != SOCK_RAW {
        ctx.wsa_error = WSAESOCKTNOSUPPORT;
        return INVALID_SOCKET;
    }

    let _ = protocol;

    let h = ctx.handle_table.allocate(
        HandleType::IoCompletion,
        GENERIC_ALL,
        Some(String::from("socket")),
    );
    ctx.wsa_error = 0;
    h
}

pub fn bind(ctx: &mut CompatContext, sock: u64, addr: &SockAddr, _addr_len: i32) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    let _ = addr;
    ctx.wsa_error = 0;
    0
}

pub fn listen(ctx: &mut CompatContext, sock: u64, backlog: i32) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    let _ = backlog;
    ctx.wsa_error = 0;
    0
}

pub fn accept(
    ctx: &mut CompatContext,
    sock: u64,
    addr: Option<&mut SockAddr>,
    addr_len: Option<&mut i32>,
) -> u64 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return INVALID_SOCKET;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return INVALID_SOCKET;
    }

    let new_sock = ctx.handle_table.allocate(
        HandleType::IoCompletion,
        GENERIC_ALL,
        Some(String::from("accepted_socket")),
    );

    if let Some(a) = addr {
        a.family = AF_INET as i16;
        a.port = 0;
        a.addr = [127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    }
    if let Some(len) = addr_len {
        *len = 16;
    }

    ctx.wsa_error = 0;
    new_sock
}

pub fn connect(ctx: &mut CompatContext, sock: u64, addr: &SockAddr, _addr_len: i32) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    let _ = addr;
    ctx.wsa_error = 0;
    0
}

pub fn send(ctx: &mut CompatContext, sock: u64, buf: &[u8], _flags: i32) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    ctx.wsa_error = 0;
    buf.len() as i32
}

pub fn recv(ctx: &mut CompatContext, sock: u64, buf: &mut [u8], _flags: i32) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    for b in buf.iter_mut() {
        *b = 0;
    }
    ctx.wsa_error = 0;
    0 // graceful close — 0 bytes received
}

pub fn sendto(
    ctx: &mut CompatContext,
    sock: u64,
    buf: &[u8],
    _flags: i32,
    to: &SockAddr,
    _to_len: i32,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    let _ = to;
    ctx.wsa_error = 0;
    buf.len() as i32
}

pub fn recvfrom(
    ctx: &mut CompatContext,
    sock: u64,
    buf: &mut [u8],
    _flags: i32,
    from: Option<&mut SockAddr>,
    from_len: Option<&mut i32>,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    for b in buf.iter_mut() {
        *b = 0;
    }
    if let Some(addr) = from {
        addr.family = AF_INET as i16;
        addr.port = 0;
        addr.addr = [0u8; 14];
    }
    if let Some(len) = from_len {
        *len = 16;
    }
    ctx.wsa_error = 0;
    0
}

// =========================================================================
// Socket management
// =========================================================================

pub fn closesocket(ctx: &mut CompatContext, sock: u64) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.close(sock) {
        ctx.wsa_error = 0;
        0
    } else {
        ctx.wsa_error = WSAENOTSOCK;
        SOCKET_ERROR
    }
}

pub fn shutdown(ctx: &mut CompatContext, sock: u64, how: i32) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    if how != SD_RECEIVE && how != SD_SEND && how != SD_BOTH {
        ctx.wsa_error = WSAEINVAL;
        return SOCKET_ERROR;
    }
    ctx.wsa_error = 0;
    0
}

#[derive(Debug, Clone, Default)]
pub struct FdSet {
    pub count: u32,
    pub fds: Vec<u64>,
}

impl FdSet {
    pub fn new() -> Self {
        Self {
            count: 0,
            fds: Vec::new(),
        }
    }

    pub fn set(&mut self, fd: u64) {
        if !self.fds.contains(&fd) {
            self.fds.push(fd);
            self.count += 1;
        }
    }

    pub fn is_set(&self, fd: u64) -> bool {
        self.fds.contains(&fd)
    }

    pub fn clear(&mut self) {
        self.fds.clear();
        self.count = 0;
    }
}

pub fn select(
    ctx: &mut CompatContext,
    _nfds: i32,
    read_fds: Option<&mut FdSet>,
    write_fds: Option<&mut FdSet>,
    except_fds: Option<&mut FdSet>,
    _timeout: Option<&TimeVal>,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }

    let mut total = 0i32;
    if let Some(wfds) = write_fds {
        total += wfds.count as i32;
    }
    if let Some(rfds) = read_fds {
        let _ = rfds;
    }
    if let Some(efds) = except_fds {
        efds.clear();
    }
    ctx.wsa_error = 0;
    total
}

pub fn ioctlsocket(ctx: &mut CompatContext, sock: u64, cmd: u32, argp: &mut u32) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }

    match cmd {
        FIONBIO => {
            let _ = *argp; // set non-blocking mode
            ctx.wsa_error = 0;
            0
        }
        FIONREAD => {
            *argp = 0;
            ctx.wsa_error = 0;
            0
        }
        _ => {
            ctx.wsa_error = WSAEINVAL;
            SOCKET_ERROR
        }
    }
}

pub fn setsockopt(
    ctx: &mut CompatContext,
    sock: u64,
    level: i32,
    optname: i32,
    optval: &[u8],
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    let _ = (level, optname, optval);
    ctx.wsa_error = 0;
    0
}

pub fn getsockopt(
    ctx: &mut CompatContext,
    sock: u64,
    level: i32,
    optname: i32,
    optval: &mut [u8],
    optlen: &mut i32,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    let _ = (level, optname);
    for b in optval.iter_mut() {
        *b = 0;
    }
    *optlen = 4;
    ctx.wsa_error = 0;
    0
}

// =========================================================================
// Name resolution
// =========================================================================

pub fn getaddrinfo(
    ctx: &mut CompatContext,
    node: Option<&str>,
    service: Option<&str>,
    hints: Option<&AddrInfo>,
    result: &mut Vec<AddrInfo>,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return WSANOTINITIALISED;
    }

    let family = hints.map(|h| h.family).unwrap_or(AF_INET);
    let socktype = hints.map(|h| h.socktype).unwrap_or(SOCK_STREAM);
    let protocol = hints.map(|h| h.protocol).unwrap_or(IPPROTO_TCP);

    let port: u16 = match service {
        Some("http") => 80,
        Some("https") => 443,
        Some(s) => s.parse().unwrap_or(0),
        None => 0,
    };

    let addr = SockAddr {
        family: family as i16,
        port,
        addr: [127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    };

    result.push(AddrInfo {
        flags: 0,
        family,
        socktype,
        protocol,
        addr_len: 16,
        canon_name: node.map(String::from),
        addr,
    });

    ctx.wsa_error = 0;
    0
}

pub fn freeaddrinfo(_ctx: &mut CompatContext, result: &mut Vec<AddrInfo>) {
    result.clear();
}

pub fn getnameinfo(
    ctx: &mut CompatContext,
    addr: &SockAddr,
    _addr_len: i32,
    host: &mut [u8],
    _host_len: u32,
    serv: &mut [u8],
    _serv_len: u32,
    _flags: i32,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return WSANOTINITIALISED;
    }

    let _ = addr;
    let localhost = b"127.0.0.1\0";
    let port_str = b"0\0";

    let h_copy = core::cmp::min(localhost.len(), host.len());
    host[..h_copy].copy_from_slice(&localhost[..h_copy]);

    let s_copy = core::cmp::min(port_str.len(), serv.len());
    serv[..s_copy].copy_from_slice(&port_str[..s_copy]);

    ctx.wsa_error = 0;
    0
}

// =========================================================================
// Byte-order conversion (pure, no context needed)
// =========================================================================

pub fn htons(hostshort: u16) -> u16 {
    hostshort.to_be()
}

pub fn htonl(hostlong: u32) -> u32 {
    hostlong.to_be()
}

pub fn ntohs(netshort: u16) -> u16 {
    u16::from_be(netshort)
}

pub fn ntohl(netlong: u32) -> u32 {
    u32::from_be(netlong)
}

pub fn inet_addr(cp: &str) -> u32 {
    let mut parts = [0u8; 4];
    let mut idx = 0;
    let mut acc: u32 = 0;

    for b in cp.bytes() {
        if b == b'.' {
            if idx >= 3 {
                return 0xFFFFFFFF; // INADDR_NONE
            }
            parts[idx] = acc as u8;
            idx += 1;
            acc = 0;
        } else if b >= b'0' && b <= b'9' {
            acc = acc * 10 + (b - b'0') as u32;
            if acc > 255 {
                return 0xFFFFFFFF;
            }
        } else {
            return 0xFFFFFFFF;
        }
    }

    if idx != 3 {
        return 0xFFFFFFFF;
    }
    parts[3] = acc as u8;

    u32::from_le_bytes(parts)
}

pub fn inet_ntoa(addr: u32) -> String {
    let bytes = addr.to_le_bytes();
    let mut s = String::new();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push('.');
        }
        // Manual u8-to-string without std
        if b >= 100 {
            s.push((b'0' + b / 100) as char);
            s.push((b'0' + (b / 10) % 10) as char);
            s.push((b'0' + b % 10) as char);
        } else if b >= 10 {
            s.push((b'0' + b / 10) as char);
            s.push((b'0' + b % 10) as char);
        } else {
            s.push((b'0' + b) as char);
        }
    }
    s
}

// =========================================================================
// TimeVal for select()
// =========================================================================

#[derive(Debug, Clone, Copy, Default)]
pub struct TimeVal {
    pub tv_sec: i32,
    pub tv_usec: i32,
}

// =========================================================================
// gethostbyname — legacy name resolution
// =========================================================================

#[derive(Debug, Clone)]
pub struct HostEnt {
    pub name: String,
    pub aliases: Vec<String>,
    pub addr_type: i16,
    pub length: i16,
    pub addr_list: Vec<u32>,
}

impl HostEnt {
    pub fn loopback(name: &str) -> Self {
        Self {
            name: String::from(name),
            aliases: Vec::new(),
            addr_type: AF_INET as i16,
            length: 4,
            addr_list: alloc::vec![u32::from_le_bytes([127, 0, 0, 1])],
        }
    }
}

pub fn gethostbyname(ctx: &mut CompatContext, name: &str) -> Option<HostEnt> {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return None;
    }

    if name.is_empty() {
        ctx.wsa_error = WSAEINVAL;
        return None;
    }

    ctx.wsa_error = 0;

    if name == "localhost" || name == "127.0.0.1" {
        return Some(HostEnt::loopback(name));
    }

    Some(HostEnt::loopback(name))
}

pub fn gethostname(ctx: &mut CompatContext, name: &mut [u8]) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }

    let hostname = b"ATHENAOS\0";
    let copy = core::cmp::min(hostname.len(), name.len());
    name[..copy].copy_from_slice(&hostname[..copy]);
    ctx.wsa_error = 0;
    0
}

// =========================================================================
// WSASocket — extended socket creation
// =========================================================================

pub fn wsa_socket(
    ctx: &mut CompatContext,
    af: i32,
    sock_type: i32,
    protocol: i32,
    _protocol_info: u64,
    _group: u32,
    _flags: u32,
) -> u64 {
    socket(ctx, af, sock_type, protocol)
}

// =========================================================================
// WSASend / WSARecv — scatter/gather I/O
// =========================================================================

#[derive(Debug, Clone)]
pub struct WsaBuf {
    pub len: u32,
    pub buf_addr: u64,
}

pub fn wsa_send(
    ctx: &mut CompatContext,
    sock: u64,
    bufs: &[WsaBuf],
    _flags: u32,
    bytes_sent: &mut u32,
    _overlapped: u64,
    _completion: u64,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }

    let total: u32 = bufs.iter().map(|b| b.len).sum();
    *bytes_sent = total;
    ctx.wsa_error = 0;
    0
}

pub fn wsa_recv(
    ctx: &mut CompatContext,
    sock: u64,
    _bufs: &[WsaBuf],
    _flags: &mut u32,
    bytes_received: &mut u32,
    _overlapped: u64,
    _completion: u64,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }

    *bytes_received = 0;
    ctx.wsa_error = 0;
    0
}

// =========================================================================
// WSAEventSelect / WSAEnumNetworkEvents — async event notification
// =========================================================================

pub fn wsa_create_event(_ctx: &mut CompatContext) -> u64 {
    static NEXT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0xA000_0000);
    NEXT.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

pub fn wsa_close_event(_ctx: &mut CompatContext, _event: u64) -> bool {
    true
}

pub fn wsa_set_event(_ctx: &mut CompatContext, _event: u64) -> bool {
    true
}

pub fn wsa_reset_event(_ctx: &mut CompatContext, _event: u64) -> bool {
    true
}

pub fn wsa_event_select(
    ctx: &mut CompatContext,
    sock: u64,
    _event: u64,
    _network_events: i32,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    ctx.wsa_error = 0;
    0
}

#[derive(Debug, Clone, Default)]
pub struct WsaNetworkEvents {
    pub network_events: i32,
    pub error_code: [i32; 10],
}

pub fn wsa_enum_network_events(
    ctx: &mut CompatContext,
    sock: u64,
    _event: u64,
    events: &mut WsaNetworkEvents,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }

    events.network_events = 0;
    events.error_code = [0; 10];
    ctx.wsa_error = 0;
    0
}

// =========================================================================
// Network event bits for WSAEventSelect
// =========================================================================

pub const FD_READ: i32 = 0x01;
pub const FD_WRITE: i32 = 0x02;
pub const FD_OOB: i32 = 0x04;
pub const FD_ACCEPT: i32 = 0x08;
pub const FD_CONNECT: i32 = 0x10;
pub const FD_CLOSE: i32 = 0x20;
pub const FD_QOS: i32 = 0x40;
pub const FD_GROUP_QOS: i32 = 0x80;

// =========================================================================
// getpeername / getsockname — socket address query
// =========================================================================

pub fn getpeername(
    ctx: &mut CompatContext,
    sock: u64,
    addr: &mut SockAddr,
    addr_len: &mut i32,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }

    addr.family = AF_INET as i16;
    addr.port = 0;
    addr.addr = [127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    *addr_len = 16;
    ctx.wsa_error = 0;
    0
}

pub fn getsockname(
    ctx: &mut CompatContext,
    sock: u64,
    addr: &mut SockAddr,
    addr_len: &mut i32,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }

    addr.family = AF_INET as i16;
    addr.port = 0;
    addr.addr = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    *addr_len = 16;
    ctx.wsa_error = 0;
    0
}

// =========================================================================
// inet_pton / inet_ntop — modern address conversion
// =========================================================================

pub fn inet_pton(af: i32, src: &str, dst: &mut [u8]) -> i32 {
    if af != AF_INET {
        return -1;
    }
    if dst.len() < 4 {
        return -1;
    }

    let addr = inet_addr(src);
    if addr == 0xFFFFFFFF && src != "255.255.255.255" {
        return 0;
    }

    dst[..4].copy_from_slice(&addr.to_le_bytes());
    1
}

pub fn inet_ntop(af: i32, src: &[u8], dst: &mut [u8]) -> bool {
    if af != AF_INET || src.len() < 4 {
        return false;
    }

    let addr = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
    let s = inet_ntoa(addr);
    let bytes = s.as_bytes();

    if dst.len() < bytes.len() + 1 {
        return false;
    }

    dst[..bytes.len()].copy_from_slice(bytes);
    dst[bytes.len()] = 0;
    true
}

// =========================================================================
// WSAIoctl — extended socket I/O control
// =========================================================================

pub fn wsa_ioctl(
    ctx: &mut CompatContext,
    sock: u64,
    _ioctl_code: u32,
    _in_buf: &[u8],
    _out_buf: &mut [u8],
    bytes_returned: &mut u32,
    _overlapped: u64,
    _completion: u64,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }

    *bytes_returned = 0;
    ctx.wsa_error = 0;
    0
}

// =========================================================================
// WSAAddressToString / WSAStringToAddress
// =========================================================================

pub fn wsa_address_to_string_a(
    addr: &SockAddr,
    _protocol_info: u64,
    buf: &mut [u8],
    buf_len: &mut u32,
) -> i32 {
    let ip = if addr.addr.len() >= 4 {
        inet_ntoa(u32::from_le_bytes([
            addr.addr[0],
            addr.addr[1],
            addr.addr[2],
            addr.addr[3],
        ]))
    } else {
        String::from("0.0.0.0")
    };

    let port = addr.port;
    let s = if port != 0 {
        let mut result = ip;
        result.push(':');
        let mut digits = [0u8; 5];
        let mut p = port;
        let mut i = 0;
        if p == 0 {
            result.push('0');
        } else {
            while p > 0 {
                digits[i] = (p % 10) as u8 + b'0';
                p /= 10;
                i += 1;
            }
            for j in (0..i).rev() {
                result.push(digits[j] as char);
            }
        }
        result
    } else {
        ip
    };

    let needed = s.len() + 1;
    if buf.len() < needed || (*buf_len as usize) < needed {
        *buf_len = needed as u32;
        return SOCKET_ERROR;
    }

    let bytes = s.as_bytes();
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[bytes.len()] = 0;
    *buf_len = needed as u32;
    0
}

// =========================================================================
// WSAAsyncSelect / WSAAsyncGetHostByName
// =========================================================================

pub fn wsa_async_select(
    ctx: &mut CompatContext,
    sock: u64,
    _hwnd: WinHandle,
    _msg: u32,
    _event: i32,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    ctx.wsa_error = 0;
    0
}

pub fn wsa_async_get_host_by_name(
    _ctx: &mut CompatContext,
    _hwnd: WinHandle,
    _msg: u32,
    _name: &str,
    _buf: &mut [u8],
) -> u64 {
    1 // non-zero async task handle
}

pub fn wsa_cancel_async_request(_ctx: &mut CompatContext, _async_task: u64) -> i32 {
    0
}

// =========================================================================
// WSAWaitForMultipleEvents
// =========================================================================

pub fn wsa_wait_for_multiple_events(
    ctx: &mut CompatContext,
    _events: &[u64],
    _wait_all: bool,
    timeout: u32,
    _alertable: bool,
) -> u32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return WSA_WAIT_FAILED;
    }
    let _ = timeout;
    WSA_WAIT_TIMEOUT
}

const WSA_WAIT_FAILED: u32 = 0xFFFFFFFF;
const WSA_WAIT_TIMEOUT: u32 = 0x00000102;

// =========================================================================
// Additional socket options
// =========================================================================

pub fn wsa_recv_from(
    ctx: &mut CompatContext,
    sock: u64,
    buf: &mut [u8],
    _flags: u32,
    from: &mut SockAddr,
    from_len: &mut i32,
    _overlapped: u64,
    _completion: u64,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    let _ = buf;
    from.family = AF_INET as i16;
    from.port = 0;
    from.addr = [127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    *from_len = 16;
    ctx.wsa_error = WSAEWOULDBLOCK;
    SOCKET_ERROR
}

pub fn wsa_send_to(
    ctx: &mut CompatContext,
    sock: u64,
    _buf: &[u8],
    _flags: u32,
    _to: &SockAddr,
    _to_len: i32,
    bytes_sent: &mut u32,
    _overlapped: u64,
    _completion: u64,
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    *bytes_sent = _buf.len() as u32;
    ctx.wsa_error = 0;
    0
}

pub fn wsa_string_to_address_a(
    address_string: &str,
    family: i32,
    addr: &mut SockAddr,
    addr_len: &mut i32,
) -> i32 {
    if family != AF_INET {
        return SOCKET_ERROR;
    }
    let result = inet_addr(address_string);
    if result == 0xFFFFFFFF {
        return SOCKET_ERROR;
    }
    addr.family = AF_INET as i16;
    addr.port = 0;
    addr.addr = [
        (result & 0xFF) as u8,
        ((result >> 8) & 0xFF) as u8,
        ((result >> 16) & 0xFF) as u8,
        ((result >> 24) & 0xFF) as u8,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    *addr_len = 16;
    0
}

// =========================================================================
// SO_KEEPALIVE / TCP_NODELAY already handled in setsockopt;
// add WSADuplicateSocket, TransmitFile stubs
// =========================================================================

pub fn wsa_duplicate_socket(
    ctx: &mut CompatContext,
    sock: u64,
    _process_id: u32,
    _protocol_info: &mut [u8],
) -> i32 {
    if !ctx.wsa_initialized {
        ctx.wsa_error = WSANOTINITIALISED;
        return SOCKET_ERROR;
    }
    if ctx.handle_table.get(sock).is_none() {
        ctx.wsa_error = WSAENOTSOCK;
        return SOCKET_ERROR;
    }
    ctx.wsa_error = 0;
    0
}

pub fn transmit_file(
    ctx: &mut CompatContext,
    sock: u64,
    _file: WinHandle,
    _bytes_to_write: u32,
    _bytes_per_send: u32,
    _overlapped: u64,
    _transmit_buffers: u64,
    _flags: u32,
) -> WinBool {
    if ctx.handle_table.get(sock).is_none() {
        return FALSE;
    }
    TRUE
}
