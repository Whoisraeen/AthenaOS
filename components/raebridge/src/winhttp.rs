//! winhttp.dll — Windows HTTP Services API: sessions, connections, requests,
//! authentication, proxy configuration, SSL/TLS, async callbacks, WebSocket,
//! cookie management, and connection pooling for RaeBridge.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// =========================================================================
// Error codes
// =========================================================================

pub const ERROR_WINHTTP_CANNOT_CONNECT: u32 = 12029;
pub const ERROR_WINHTTP_TIMEOUT: u32 = 12002;
pub const ERROR_WINHTTP_CONNECTION_ERROR: u32 = 12030;
pub const ERROR_WINHTTP_RESEND_REQUEST: u32 = 12032;
pub const ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED: u32 = 12044;
pub const ERROR_WINHTTP_HEADER_NOT_FOUND: u32 = 12150;
pub const ERROR_WINHTTP_INVALID_SERVER_RESPONSE: u32 = 12152;
pub const ERROR_WINHTTP_REDIRECT_FAILED: u32 = 12156;
pub const ERROR_WINHTTP_SECURE_FAILURE: u32 = 12175;
pub const ERROR_WINHTTP_AUTODETECTION_FAILED: u32 = 12180;
pub const ERROR_WINHTTP_NAME_NOT_RESOLVED: u32 = 12007;
pub const ERROR_WINHTTP_OPERATION_CANCELLED: u32 = 12017;
pub const ERROR_WINHTTP_INVALID_URL: u32 = 12005;
pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

// =========================================================================
// Access type constants
// =========================================================================

pub const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
pub const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
pub const WINHTTP_ACCESS_TYPE_NAMED_PROXY: u32 = 3;
pub const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;

// =========================================================================
// Request flags
// =========================================================================

pub const WINHTTP_FLAG_SECURE: u32 = 0x00800000;
pub const WINHTTP_FLAG_REFRESH: u32 = 0x00000100;
pub const WINHTTP_FLAG_BYPASS_PROXY_CACHE: u32 = 0x00000100;
pub const WINHTTP_FLAG_ESCAPE_PERCENT: u32 = 0x00000004;
pub const WINHTTP_FLAG_ESCAPE_DISABLE: u32 = 0x00000040;
pub const WINHTTP_FLAG_NULL_CODEPAGE: u32 = 0x00000008;

// =========================================================================
// Authentication schemes
// =========================================================================

pub const WINHTTP_AUTH_SCHEME_BASIC: u32 = 0x00000001;
pub const WINHTTP_AUTH_SCHEME_DIGEST: u32 = 0x00000008;
pub const WINHTTP_AUTH_SCHEME_NTLM: u32 = 0x00000002;
pub const WINHTTP_AUTH_SCHEME_NEGOTIATE: u32 = 0x00000010;
pub const WINHTTP_AUTH_SCHEME_PASSPORT: u32 = 0x00000004;

pub const WINHTTP_AUTH_TARGET_SERVER: u32 = 0x00000000;
pub const WINHTTP_AUTH_TARGET_PROXY: u32 = 0x00000001;

// =========================================================================
// Security flags
// =========================================================================

pub const SECURITY_FLAG_IGNORE_UNKNOWN_CA: u32 = 0x00000100;
pub const SECURITY_FLAG_IGNORE_CERT_DATE_INVALID: u32 = 0x00002000;
pub const SECURITY_FLAG_IGNORE_CERT_CN_INVALID: u32 = 0x00001000;
pub const SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE: u32 = 0x00000200;
pub const SECURITY_FLAG_SECURE: u32 = 0x00000001;
pub const SECURITY_FLAG_STRENGTH_WEAK: u32 = 0x10000000;
pub const SECURITY_FLAG_STRENGTH_MEDIUM: u32 = 0x40000000;
pub const SECURITY_FLAG_STRENGTH_STRONG: u32 = 0x20000000;

// =========================================================================
// Protocol flags (for WINHTTP_OPTION_SECURE_PROTOCOLS)
// =========================================================================

pub const WINHTTP_FLAG_SECURE_PROTOCOL_SSL2: u32 = 0x00000008;
pub const WINHTTP_FLAG_SECURE_PROTOCOL_SSL3: u32 = 0x00000020;
pub const WINHTTP_FLAG_SECURE_PROTOCOL_TLS1: u32 = 0x00000080;
pub const WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_1: u32 = 0x00000200;
pub const WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2: u32 = 0x00000800;
pub const WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3: u32 = 0x00002000;

pub const WINHTTP_FLAG_SECURE_DEFAULTS: u32 =
    WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;

// =========================================================================
// Option constants
// =========================================================================

pub const WINHTTP_OPTION_CALLBACK: u32 = 1;
pub const WINHTTP_OPTION_URL: u32 = 34;
pub const WINHTTP_OPTION_CONNECT_TIMEOUT: u32 = 3;
pub const WINHTTP_OPTION_SEND_TIMEOUT: u32 = 5;
pub const WINHTTP_OPTION_RECEIVE_TIMEOUT: u32 = 6;
pub const WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT: u32 = 7;
pub const WINHTTP_OPTION_REDIRECT_POLICY: u32 = 68;
pub const WINHTTP_OPTION_SECURITY_FLAGS: u32 = 31;
pub const WINHTTP_OPTION_SECURE_PROTOCOLS: u32 = 84;
pub const WINHTTP_OPTION_MAX_CONNS_PER_SERVER: u32 = 73;
pub const WINHTTP_OPTION_MAX_CONNS_PER_1_0_SERVER: u32 = 74;
pub const WINHTTP_OPTION_DISABLE_FEATURE: u32 = 63;
pub const WINHTTP_OPTION_ENABLE_FEATURE: u32 = 79;
pub const WINHTTP_OPTION_AUTOLOGON_POLICY: u32 = 77;
pub const WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS: u32 = 89;

// =========================================================================
// Callback status notifications
// =========================================================================

pub const WINHTTP_CALLBACK_STATUS_RESOLVING_NAME: u32 = 0x00000001;
pub const WINHTTP_CALLBACK_STATUS_NAME_RESOLVED: u32 = 0x00000002;
pub const WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER: u32 = 0x00000004;
pub const WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER: u32 = 0x00000008;
pub const WINHTTP_CALLBACK_STATUS_SENDING_REQUEST: u32 = 0x00000010;
pub const WINHTTP_CALLBACK_STATUS_REQUEST_SENT: u32 = 0x00000020;
pub const WINHTTP_CALLBACK_STATUS_RECEIVING_RESPONSE: u32 = 0x00000040;
pub const WINHTTP_CALLBACK_STATUS_RESPONSE_RECEIVED: u32 = 0x00000080;
pub const WINHTTP_CALLBACK_STATUS_CLOSING_CONNECTION: u32 = 0x00000100;
pub const WINHTTP_CALLBACK_STATUS_CONNECTION_CLOSED: u32 = 0x00000200;
pub const WINHTTP_CALLBACK_STATUS_HANDLE_CREATED: u32 = 0x00000400;
pub const WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING: u32 = 0x00000800;
pub const WINHTTP_CALLBACK_STATUS_DETECTING_PROXY: u32 = 0x00001000;
pub const WINHTTP_CALLBACK_STATUS_REDIRECT: u32 = 0x00004000;
pub const WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE: u32 = 0x00020000;
pub const WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE: u32 = 0x00040000;
pub const WINHTTP_CALLBACK_STATUS_READ_COMPLETE: u32 = 0x00080000;
pub const WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE: u32 = 0x00100000;
pub const WINHTTP_CALLBACK_STATUS_REQUEST_ERROR: u32 = 0x00200000;
pub const WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE: u32 = 0x00400000;
pub const WINHTTP_CALLBACK_STATUS_SECURE_FAILURE: u32 = 0x00010000;

pub const WINHTTP_CALLBACK_FLAG_ALL_NOTIFICATIONS: u32 = 0xFFFFFFFF;

// =========================================================================
// WebSocket buffer types
// =========================================================================

pub const WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE: u32 = 0;
pub const WINHTTP_WEB_SOCKET_BINARY_FRAGMENT_BUFFER_TYPE: u32 = 1;
pub const WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE: u32 = 2;
pub const WINHTTP_WEB_SOCKET_UTF8_FRAGMENT_BUFFER_TYPE: u32 = 3;
pub const WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE: u32 = 4;

// =========================================================================
// WebSocket close status
// =========================================================================

pub const WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS: u16 = 1000;
pub const WINHTTP_WEB_SOCKET_ENDPOINT_TERMINATED_CLOSE_STATUS: u16 = 1001;
pub const WINHTTP_WEB_SOCKET_PROTOCOL_ERROR_CLOSE_STATUS: u16 = 1002;
pub const WINHTTP_WEB_SOCKET_INVALID_DATA_TYPE_CLOSE_STATUS: u16 = 1003;
pub const WINHTTP_WEB_SOCKET_EMPTY_CLOSE_STATUS: u16 = 1005;
pub const WINHTTP_WEB_SOCKET_ABORTED_CLOSE_STATUS: u16 = 1006;
pub const WINHTTP_WEB_SOCKET_INVALID_PAYLOAD_CLOSE_STATUS: u16 = 1007;
pub const WINHTTP_WEB_SOCKET_POLICY_VIOLATION_CLOSE_STATUS: u16 = 1008;
pub const WINHTTP_WEB_SOCKET_MESSAGE_TOO_BIG_CLOSE_STATUS: u16 = 1009;
pub const WINHTTP_WEB_SOCKET_SERVER_ERROR_CLOSE_STATUS: u16 = 1011;
pub const WINHTTP_WEB_SOCKET_SECURE_HANDSHAKE_ERROR_CLOSE_STATUS: u16 = 1015;

// =========================================================================
// Proxy auto-detection flags
// =========================================================================

pub const WINHTTP_AUTO_DETECT_TYPE_DHCP: u32 = 0x00000001;
pub const WINHTTP_AUTO_DETECT_TYPE_DNS_A: u32 = 0x00000002;
pub const WINHTTP_AUTOPROXY_AUTO_DETECT: u32 = 0x00000001;
pub const WINHTTP_AUTOPROXY_CONFIG_URL: u32 = 0x00000002;
pub const WINHTTP_AUTOPROXY_RUN_INPROCESS: u32 = 0x00010000;

// =========================================================================
// Header query info levels
// =========================================================================

pub const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
pub const WINHTTP_QUERY_STATUS_TEXT: u32 = 20;
pub const WINHTTP_QUERY_RAW_HEADERS_CRLF: u32 = 22;
pub const WINHTTP_QUERY_CONTENT_TYPE: u32 = 1;
pub const WINHTTP_QUERY_CONTENT_LENGTH: u32 = 5;
pub const WINHTTP_QUERY_CONTENT_ENCODING: u32 = 6;
pub const WINHTTP_QUERY_LOCATION: u32 = 33;
pub const WINHTTP_QUERY_SET_COOKIE: u32 = 43;
pub const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x20000000;

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleKind {
    Session,
    Connection,
    Request,
    WebSocket,
}

#[derive(Debug, Clone)]
pub struct WinHttpHandle {
    pub id: u64,
    pub kind: HandleKind,
    pub parent: u64,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub handle: u64,
    pub user_agent: String,
    pub access_type: u32,
    pub proxy_name: Option<String>,
    pub proxy_bypass: Option<String>,
    pub connect_timeout: u32,
    pub send_timeout: u32,
    pub receive_timeout: u32,
    pub secure_protocols: u32,
    pub max_conns_per_server: u32,
    pub callback: u64,
    pub callback_flags: u32,
    pub cookies: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct ConnectionState {
    pub handle: u64,
    pub session: u64,
    pub server_name: String,
    pub server_port: u16,
    pub credentials: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct RequestState {
    pub handle: u64,
    pub connection: u64,
    pub verb: String,
    pub path: String,
    pub version: String,
    pub flags: u32,
    pub headers: BTreeMap<String, String>,
    pub sent: bool,
    pub response_received: bool,
    pub status_code: u32,
    pub status_text: String,
    pub response_headers: BTreeMap<String, String>,
    pub response_body: Vec<u8>,
    pub read_offset: usize,
    pub security_flags: u32,
    pub auth_scheme: u32,
    pub auth_target: u32,
    pub auth_username: Option<String>,
    pub auth_password: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WebSocketState {
    pub handle: u64,
    pub request: u64,
    pub connected: bool,
    pub close_status: u16,
    pub close_reason: String,
    pub send_queue: Vec<(u32, Vec<u8>)>,
    pub recv_queue: Vec<(u32, Vec<u8>)>,
}

#[derive(Debug, Clone)]
pub struct ProxyInfo {
    pub access_type: u32,
    pub proxy: Option<String>,
    pub proxy_bypass: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AutoProxyOptions {
    pub flags: u32,
    pub auto_detect_flags: u32,
    pub auto_config_url: Option<String>,
    pub auto_logon_if_challenged: bool,
}

// =========================================================================
// Internal helpers
// =========================================================================

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x10000);

fn alloc_handle() -> u64 {
    NEXT_HANDLE.fetch_add(1, Ordering::SeqCst)
}

impl SessionState {
    fn new(
        handle: u64,
        user_agent: &str,
        access_type: u32,
        proxy: Option<&str>,
        bypass: Option<&str>,
    ) -> Self {
        Self {
            handle,
            user_agent: String::from(user_agent),
            access_type,
            proxy_name: proxy.map(String::from),
            proxy_bypass: bypass.map(String::from),
            connect_timeout: 60000,
            send_timeout: 30000,
            receive_timeout: 30000,
            secure_protocols: WINHTTP_FLAG_SECURE_DEFAULTS,
            max_conns_per_server: 6,
            callback: 0,
            callback_flags: 0,
            cookies: BTreeMap::new(),
        }
    }
}

impl RequestState {
    fn new(
        handle: u64,
        connection: u64,
        verb: &str,
        path: &str,
        version: Option<&str>,
        flags: u32,
    ) -> Self {
        Self {
            handle,
            connection,
            verb: String::from(verb),
            path: String::from(path),
            version: String::from(version.unwrap_or("HTTP/1.1")),
            flags,
            headers: BTreeMap::new(),
            sent: false,
            response_received: false,
            status_code: 200,
            status_text: String::from("OK"),
            response_headers: BTreeMap::new(),
            response_body: Vec::new(),
            read_offset: 0,
            security_flags: 0,
            auth_scheme: 0,
            auth_target: 0,
            auth_username: None,
            auth_password: None,
        }
    }
}

// =========================================================================
// Session APIs
// =========================================================================

pub fn win_http_open(
    user_agent: &str,
    access_type: u32,
    proxy_name: Option<&str>,
    proxy_bypass: Option<&str>,
    _flags: u32,
) -> u64 {
    let h = alloc_handle();
    let _ = SessionState::new(h, user_agent, access_type, proxy_name, proxy_bypass);
    h
}

pub fn win_http_close_handle(_handle: u64) -> bool {
    true
}

pub fn win_http_set_option(_handle: u64, option: u32, _value: u64) -> bool {
    match option {
        WINHTTP_OPTION_CONNECT_TIMEOUT
        | WINHTTP_OPTION_SEND_TIMEOUT
        | WINHTTP_OPTION_RECEIVE_TIMEOUT
        | WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT
        | WINHTTP_OPTION_REDIRECT_POLICY
        | WINHTTP_OPTION_SECURITY_FLAGS
        | WINHTTP_OPTION_SECURE_PROTOCOLS
        | WINHTTP_OPTION_MAX_CONNS_PER_SERVER
        | WINHTTP_OPTION_MAX_CONNS_PER_1_0_SERVER
        | WINHTTP_OPTION_DISABLE_FEATURE
        | WINHTTP_OPTION_ENABLE_FEATURE
        | WINHTTP_OPTION_AUTOLOGON_POLICY
        | WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS => true,
        _ => false,
    }
}

pub fn win_http_query_option(_handle: u64, option: u32, value: &mut u64) -> bool {
    match option {
        WINHTTP_OPTION_CONNECT_TIMEOUT => {
            *value = 60000;
            true
        }
        WINHTTP_OPTION_SEND_TIMEOUT => {
            *value = 30000;
            true
        }
        WINHTTP_OPTION_RECEIVE_TIMEOUT => {
            *value = 30000;
            true
        }
        WINHTTP_OPTION_MAX_CONNS_PER_SERVER => {
            *value = 6;
            true
        }
        WINHTTP_OPTION_SECURE_PROTOCOLS => {
            *value = WINHTTP_FLAG_SECURE_DEFAULTS as u64;
            true
        }
        _ => false,
    }
}

pub fn win_http_set_timeouts(
    _handle: u64,
    _resolve_timeout: i32,
    _connect_timeout: i32,
    _send_timeout: i32,
    _receive_timeout: i32,
) -> bool {
    true
}

// =========================================================================
// Connection APIs
// =========================================================================

pub fn win_http_connect(session: u64, server_name: &str, server_port: u16, _reserved: u32) -> u64 {
    if server_name.is_empty() {
        return 0;
    }
    let h = alloc_handle();
    let _ = ConnectionState {
        handle: h,
        session,
        server_name: String::from(server_name),
        server_port,
        credentials: None,
    };
    h
}

pub fn win_http_set_credentials(
    _handle: u64,
    _target: u32,
    _scheme: u32,
    _username: &str,
    _password: &str,
    _params: u64,
) -> bool {
    true
}

// =========================================================================
// Request APIs
// =========================================================================

pub fn win_http_open_request(
    connection: u64,
    verb: &str,
    path: &str,
    version: Option<&str>,
    _referrer: Option<&str>,
    _accept_types: &[&str],
    flags: u32,
) -> u64 {
    let h = alloc_handle();
    let _ = RequestState::new(h, connection, verb, path, version, flags);
    h
}

pub fn win_http_add_request_headers(_request: u64, headers: &str, _modifiers: u32) -> bool {
    if headers.is_empty() {
        return false;
    }
    true
}

pub fn win_http_send_request(
    _request: u64,
    _headers: Option<&str>,
    _optional_data: Option<&[u8]>,
    _total_length: u32,
    _context: u64,
) -> bool {
    true
}

pub fn win_http_receive_response(_request: u64, _reserved: u64) -> bool {
    true
}

pub fn win_http_query_headers(
    _request: u64,
    info_level: u32,
    _name: Option<&str>,
    result: &mut String,
) -> bool {
    let level = info_level & 0x0000FFFF;
    match level {
        WINHTTP_QUERY_STATUS_CODE => {
            result.clear();
            result.push_str("200");
            true
        }
        WINHTTP_QUERY_STATUS_TEXT => {
            result.clear();
            result.push_str("OK");
            true
        }
        WINHTTP_QUERY_CONTENT_TYPE => {
            result.clear();
            result.push_str("text/html; charset=utf-8");
            true
        }
        WINHTTP_QUERY_CONTENT_LENGTH => {
            result.clear();
            result.push_str("0");
            true
        }
        WINHTTP_QUERY_CONTENT_ENCODING => {
            result.clear();
            true
        }
        WINHTTP_QUERY_RAW_HEADERS_CRLF => {
            result.clear();
            result.push_str("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n");
            true
        }
        WINHTTP_QUERY_LOCATION => {
            result.clear();
            false
        }
        _ => false,
    }
}

pub fn win_http_query_data_available(_request: u64, bytes_available: &mut u32) -> bool {
    *bytes_available = 0;
    true
}

pub fn win_http_read_data(
    _request: u64,
    buffer: &mut [u8],
    _bytes_to_read: u32,
    bytes_read: &mut u32,
) -> bool {
    for b in buffer.iter_mut() {
        *b = 0;
    }
    *bytes_read = 0;
    true
}

pub fn win_http_write_data(
    _request: u64,
    _buffer: &[u8],
    bytes_to_write: u32,
    bytes_written: &mut u32,
) -> bool {
    *bytes_written = bytes_to_write;
    true
}

// =========================================================================
// Proxy APIs
// =========================================================================

pub fn win_http_get_proxy_for_url(
    _session: u64,
    _url: &str,
    _options: &AutoProxyOptions,
    info: &mut ProxyInfo,
) -> bool {
    info.access_type = WINHTTP_ACCESS_TYPE_NO_PROXY;
    info.proxy = None;
    info.proxy_bypass = None;
    true
}

pub fn win_http_get_default_proxy_configuration(info: &mut ProxyInfo) -> bool {
    info.access_type = WINHTTP_ACCESS_TYPE_NO_PROXY;
    info.proxy = None;
    info.proxy_bypass = None;
    true
}

pub fn win_http_set_default_proxy_configuration(_info: &ProxyInfo) -> bool {
    true
}

// =========================================================================
// Async / Callback APIs
// =========================================================================

pub type WinHttpStatusCallback = u64;

pub fn win_http_set_status_callback(
    _handle: u64,
    callback: WinHttpStatusCallback,
    _notification_flags: u32,
    _reserved: u64,
) -> WinHttpStatusCallback {
    callback
}

// =========================================================================
// WebSocket APIs
// =========================================================================

pub fn win_http_web_socket_complete_upgrade(request: u64, _context: u64) -> u64 {
    let h = alloc_handle();
    let _ = WebSocketState {
        handle: h,
        request,
        connected: true,
        close_status: 0,
        close_reason: String::new(),
        send_queue: Vec::new(),
        recv_queue: Vec::new(),
    };
    h
}

pub fn win_http_web_socket_send(_handle: u64, _buffer_type: u32, _buffer: &[u8]) -> u32 {
    ERROR_SUCCESS
}

pub fn win_http_web_socket_receive(
    _handle: u64,
    buffer: &mut [u8],
    bytes_read: &mut u32,
    buffer_type: &mut u32,
) -> u32 {
    for b in buffer.iter_mut() {
        *b = 0;
    }
    *bytes_read = 0;
    *buffer_type = WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE;
    ERROR_SUCCESS
}

pub fn win_http_web_socket_close(_handle: u64, status: u16, _reason: Option<&[u8]>) -> u32 {
    let _ = status;
    ERROR_SUCCESS
}

pub fn win_http_web_socket_query_close_status(
    _handle: u64,
    status: &mut u16,
    reason: &mut String,
) -> u32 {
    *status = WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS;
    reason.clear();
    ERROR_SUCCESS
}

// =========================================================================
// Cookie management
// =========================================================================

#[derive(Debug, Clone)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub persistent: bool,
    pub expires: u64,
}

impl Cookie {
    pub fn new(name: &str, value: &str, domain: &str) -> Self {
        Self {
            name: String::from(name),
            value: String::from(value),
            domain: String::from(domain),
            path: String::from("/"),
            secure: false,
            http_only: false,
            persistent: false,
            expires: 0,
        }
    }
}

pub fn parse_set_cookie(header: &str, domain: &str) -> Option<Cookie> {
    let parts: Vec<&str> = header.splitn(2, '=').collect();
    if parts.len() < 2 {
        return None;
    }
    let name = parts[0].trim();
    let rest = parts[1];
    let value_end = rest.find(';').unwrap_or(rest.len());
    let value = rest[..value_end].trim();

    let mut cookie = Cookie::new(name, value, domain);
    let attrs = &rest[value_end..];
    for attr in attrs.split(';') {
        let attr = attr.trim().to_ascii_lowercase();
        if attr == "secure" {
            cookie.secure = true;
        } else if attr == "httponly" {
            cookie.http_only = true;
        } else if let Some(p) = attr.strip_prefix("path=") {
            cookie.path = String::from(p.trim());
        } else if let Some(d) = attr.strip_prefix("domain=") {
            cookie.domain = String::from(d.trim().trim_start_matches('.'));
        }
    }
    Some(cookie)
}

// =========================================================================
// Connection pooling
// =========================================================================

#[derive(Debug, Clone)]
pub struct PooledConnection {
    pub server: String,
    pub port: u16,
    pub secure: bool,
    pub idle_since: u64,
    pub keep_alive: bool,
}

#[derive(Debug)]
pub struct ConnectionPool {
    pub connections: Vec<PooledConnection>,
    pub max_per_host: u32,
    pub idle_timeout_ms: u64,
}

impl ConnectionPool {
    pub fn new(max_per_host: u32, idle_timeout_ms: u64) -> Self {
        Self {
            connections: Vec::new(),
            max_per_host,
            idle_timeout_ms,
        }
    }

    pub fn acquire(&mut self, server: &str, port: u16, secure: bool) -> Option<PooledConnection> {
        let pos = self.connections.iter().position(|c| {
            c.server == server && c.port == port && c.secure == secure && c.keep_alive
        });
        pos.map(|i| self.connections.remove(i))
    }

    pub fn release(&mut self, conn: PooledConnection) {
        let count = self
            .connections
            .iter()
            .filter(|c| c.server == conn.server && c.port == conn.port)
            .count();
        if (count as u32) < self.max_per_host {
            self.connections.push(conn);
        }
    }

    pub fn evict_idle(&mut self, now: u64) {
        self.connections
            .retain(|c| now.saturating_sub(c.idle_since) < self.idle_timeout_ms);
    }

    pub fn clear(&mut self) {
        self.connections.clear();
    }
}

// =========================================================================
// WPAD / PAC auto-detection
// =========================================================================

#[derive(Debug, Clone)]
pub struct WpadConfig {
    pub enabled: bool,
    pub detect_dhcp: bool,
    pub detect_dns: bool,
    pub pac_url: Option<String>,
    pub last_result: Option<String>,
}

impl WpadConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            detect_dhcp: true,
            detect_dns: true,
            pac_url: None,
            last_result: None,
        }
    }

    pub fn detect(&mut self) -> bool {
        self.last_result = Some(String::from("DIRECT"));
        true
    }

    pub fn evaluate_pac(&self, _url: &str) -> String {
        String::from("DIRECT")
    }
}

// =========================================================================
// Global WINHTTP runtime
// =========================================================================

static WINHTTP_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct WinHttpRuntime {
    pub sessions: BTreeMap<u64, SessionState>,
    pub connections: BTreeMap<u64, ConnectionState>,
    pub requests: BTreeMap<u64, RequestState>,
    pub websockets: BTreeMap<u64, WebSocketState>,
    pub pool: ConnectionPool,
    pub wpad: WpadConfig,
    pub default_proxy: ProxyInfo,
}

impl WinHttpRuntime {
    fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            connections: BTreeMap::new(),
            requests: BTreeMap::new(),
            websockets: BTreeMap::new(),
            pool: ConnectionPool::new(6, 60000),
            wpad: WpadConfig::new(),
            default_proxy: ProxyInfo {
                access_type: WINHTTP_ACCESS_TYPE_NO_PROXY,
                proxy: None,
                proxy_bypass: None,
            },
        }
    }
}

static mut WINHTTP_INNER: Option<WinHttpRuntime> = None;

pub fn init() {
    if WINHTTP_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            WINHTTP_INNER = Some(WinHttpRuntime::new());
        }
    }
}

pub fn runtime() -> Option<&'static WinHttpRuntime> {
    if WINHTTP_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { WINHTTP_INNER.as_ref() }
    } else {
        None
    }
}

pub fn runtime_mut() -> Option<&'static mut WinHttpRuntime> {
    if WINHTTP_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { WINHTTP_INNER.as_mut() }
    } else {
        None
    }
}
