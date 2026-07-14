extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpsError {
    InvalidUrl(String),
    ConnectionFailed(String),
    TlsHandshakeFailed(String),
    CertificateInvalid(String),
    CertificateExpired(String),
    HostnameVerificationFailed,
    Timeout,
    TooManyRedirects,
    InvalidResponse(String),
    IoError(String),
    ProxyError(String),
    ProtocolError(String),
}

// ---------------------------------------------------------------------------
// TLS types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsVersion {
    Tls10,
    Tls11,
    Tls12,
    Tls13,
}

impl TlsVersion {
    pub fn as_u16(&self) -> u16 {
        match self {
            Self::Tls10 => 0x0301,
            Self::Tls11 => 0x0302,
            Self::Tls12 => 0x0303,
            Self::Tls13 => 0x0304,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsConnectionState {
    Handshaking,
    Established,
    Closed,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub min_version: TlsVersion,
    pub max_version: TlsVersion,
    pub verify_hostname: bool,
    pub verify_cert: bool,
    pub client_cert: Option<ClientCert>,
    pub alpn_protocols: Vec<String>,
    pub session_cache: bool,
    pub sni: bool,
}

impl TlsConfig {
    pub fn new() -> Self {
        Self {
            min_version: TlsVersion::Tls12,
            max_version: TlsVersion::Tls13,
            verify_hostname: true,
            verify_cert: true,
            client_cert: None,
            alpn_protocols: Vec::new(),
            session_cache: true,
            sni: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientCert {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Certificate Authority store
// ---------------------------------------------------------------------------

// NOTE: the former `CaStore` + `CaStore::verify_chain` lived here — a
// name-chain-only cert "verifier" (matched subject/issuer strings + a
// self-signed root, NEVER verifying an issuer signature). It was dead (nothing
// constructed a `CaCertificate` or called `verify_chain`) and superseded by the
// real `tls_crypto::validate_chain` (RustCrypto x509 + Ed25519/RSA issuer-sig
// verification + a pinned `TrustStore`). Removed so it can never be revived as a
// forgeable-pass trust path. The verified TLS 1.3 handshake uses
// `HttpsClient::root_anchors_der` → `build_trust_store()` → `tls_crypto`.

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Url {
    pub fn authority(&self) -> String {
        if self.port == 443 && self.scheme == "https" {
            self.host.clone()
        } else if self.port == 80 && self.scheme == "http" {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn full_path(&self) -> String {
        match &self.query {
            Some(q) => format!("{}?{}", self.path, q),
            None => self.path.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP method and request/response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
    Trace,
    Connect,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
            Self::Connect => "CONNECT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpsRequest {
    pub method: HttpMethod,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: Option<u64>,
    pub follow_redirects: bool,
    pub max_redirects: u32,
}

impl HttpsRequest {
    pub fn new(method: HttpMethod, url: Url) -> Self {
        Self {
            method,
            url,
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
            follow_redirects: true,
            max_redirects: 10,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((String::from(name), String::from(value)));
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }
}

#[derive(Debug, Clone)]
pub struct TlsInfo {
    pub version: TlsVersion,
    pub cipher_suite: String,
    pub alpn_protocol: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResponseTiming {
    pub dns_ms: u64,
    pub connect_ms: u64,
    pub tls_ms: u64,
    pub first_byte_ms: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone)]
pub struct HttpsResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub url: Url,
    pub redirected: bool,
    pub tls_info: Option<TlsInfo>,
    pub timing: ResponseTiming,
}

impl HttpsResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_length(&self) -> Option<usize> {
        self.header("content-length")
            .and_then(|v| v.parse::<usize>().ok())
    }
}

// ---------------------------------------------------------------------------
// Cookie types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

#[derive(Debug, Clone)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<u64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

impl Cookie {
    pub fn is_expired(&self, now: u64) -> bool {
        self.expires.map_or(false, |exp| now > exp)
    }

    pub fn matches_domain(&self, domain: &str) -> bool {
        domain == self.domain || domain.ends_with(&format!(".{}", self.domain))
    }

    pub fn matches_path(&self, path: &str) -> bool {
        path.starts_with(&self.path)
    }
}

#[derive(Debug, Clone)]
pub struct CookieJar {
    pub cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self {
            cookies: Vec::new(),
        }
    }

    pub fn add(&mut self, cookie: Cookie) {
        if let Some(pos) = self.cookies.iter().position(|c| {
            c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path
        }) {
            self.cookies[pos] = cookie;
        } else {
            self.cookies.push(cookie);
        }
    }

    pub fn get_for_url(&self, url: &Url, now: u64) -> Vec<&Cookie> {
        self.cookies
            .iter()
            .filter(|c| {
                !c.is_expired(now)
                    && c.matches_domain(&url.host)
                    && c.matches_path(&url.path)
                    && (!c.secure || url.scheme == "https")
            })
            .collect()
    }

    pub fn remove_expired(&mut self, now: u64) {
        self.cookies.retain(|c| !c.is_expired(now));
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    pub fn count(&self) -> usize {
        self.cookies.len()
    }
}

// ---------------------------------------------------------------------------
// Proxy configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyType {
    Http,
    Https,
    Socks5,
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub proxy_type: ProxyType,
    pub host: String,
    pub port: u16,
    pub auth: Option<(String, String)>,
}

// ---------------------------------------------------------------------------
// HTTPS Connection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HttpsConnection {
    pub host: String,
    pub port: u16,
    pub tls_state: TlsConnectionState,
    pub alpn: Option<String>,
    pub keep_alive: bool,
    pub idle_timeout_ms: u64,
    pub last_used: u64,
}

impl HttpsConnection {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            tls_state: TlsConnectionState::Handshaking,
            alpn: None,
            keep_alive: true,
            idle_timeout_ms: 60_000,
            last_used: 0,
        }
    }

    pub fn is_idle(&self, now: u64) -> bool {
        now.saturating_sub(self.last_used) > self.idle_timeout_ms
    }

    pub fn is_established(&self) -> bool {
        matches!(self.tls_state, TlsConnectionState::Established)
    }
}

// ---------------------------------------------------------------------------
// HTTPS Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HttpsClient {
    pub connections: Vec<HttpsConnection>,
    pub default_config: TlsConfig,
    pub cookie_jar: CookieJar,
    pub redirect_limit: u32,
    pub timeout_ms: u64,
    pub user_agent: String,
    pub proxy: Option<ProxyConfig>,
    default_headers: Vec<(String, String)>,
    /// Trusted root certificates in DER form, consumed by the TLS 1.3 path
    /// (`request_over`) to build a `tls_crypto::TrustStore`. Separate from the
    /// descriptive `ca_store` because the verified handshake needs raw DER to
    /// match issuer Names byte-for-byte (RFC 5280 path building). A client with
    /// none configured fails closed — it cannot authenticate any server.
    root_anchors_der: Vec<Vec<u8>>,
}

impl HttpsClient {
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            default_config: TlsConfig::new(),
            cookie_jar: CookieJar::new(),
            redirect_limit: 10,
            timeout_ms: 30_000,
            user_agent: String::from("AthNet/1.0"),
            proxy: None,
            default_headers: Vec::new(),
            root_anchors_der: Vec::new(),
        }
    }

    /// Add a trusted root certificate (DER) for the verified TLS 1.3 path. The
    /// server's certificate chain MUST anchor to one of these for an https://
    /// fetch to succeed. No anchors => every https:// request fails closed.
    pub fn add_root_der_anchor(&mut self, der: Vec<u8>) {
        self.root_anchors_der.push(der);
    }

    pub fn get(&mut self, url: &str) -> Result<HttpsResponse, HttpsError> {
        let parsed = Self::parse_url(url)?;
        let req = HttpsRequest::new(HttpMethod::Get, parsed);
        self.request(req)
    }

    pub fn post(
        &mut self,
        url: &str,
        body: &[u8],
        content_type: &str,
    ) -> Result<HttpsResponse, HttpsError> {
        let parsed = Self::parse_url(url)?;
        let req = HttpsRequest::new(HttpMethod::Post, parsed)
            .with_header("Content-Type", content_type)
            .with_body(body.to_vec());
        self.request(req)
    }

    pub fn put(
        &mut self,
        url: &str,
        body: &[u8],
        content_type: &str,
    ) -> Result<HttpsResponse, HttpsError> {
        let parsed = Self::parse_url(url)?;
        let req = HttpsRequest::new(HttpMethod::Put, parsed)
            .with_header("Content-Type", content_type)
            .with_body(body.to_vec());
        self.request(req)
    }

    pub fn delete(&mut self, url: &str) -> Result<HttpsResponse, HttpsError> {
        let parsed = Self::parse_url(url)?;
        let req = HttpsRequest::new(HttpMethod::Delete, parsed);
        self.request(req)
    }

    pub fn head(&mut self, url: &str) -> Result<HttpsResponse, HttpsError> {
        let parsed = Self::parse_url(url)?;
        let req = HttpsRequest::new(HttpMethod::Head, parsed);
        self.request(req)
    }

    pub fn download(&mut self, url: &str, max_size: usize) -> Result<Vec<u8>, HttpsError> {
        let resp = self.get(url)?;
        if resp.body.len() > max_size {
            return Err(HttpsError::IoError(String::from(
                "response exceeds max_size",
            )));
        }
        Ok(resp.body)
    }

    pub fn request(&mut self, req: HttpsRequest) -> Result<HttpsResponse, HttpsError> {
        let mut headers = req.headers.clone();
        self.apply_cookies(&req.url, &mut headers);

        for (name, value) in &self.default_headers {
            if !headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(name)) {
                headers.push((name.clone(), value.clone()));
            }
        }

        if !headers
            .iter()
            .any(|(h, _)| h.eq_ignore_ascii_case("User-Agent"))
        {
            headers.push((String::from("User-Agent"), self.user_agent.clone()));
        }

        if !headers.iter().any(|(h, _)| h.eq_ignore_ascii_case("Host")) {
            headers.push((String::from("Host"), req.url.authority()));
        }

        let full_req = HttpsRequest {
            method: req.method,
            url: req.url.clone(),
            headers,
            body: req.body.clone(),
            timeout_ms: req.timeout_ms.or(Some(self.timeout_ms)),
            follow_redirects: req.follow_redirects,
            max_redirects: req.max_redirects,
        };

        let _request_bytes = self.build_request_bytes(&full_req);

        let conn = HttpsConnection::new(req.url.host.clone(), req.url.port);
        self.connections.push(conn);

        // The actual bytes need a byte transport (a live kernel TCP socket, or a
        // mock in the host KATs). `request` itself owns no socket, so the verified
        // HTTPS flow lives in `request_over` / `get_over`; this entry point reports
        // the missing capability rather than silently doing nothing.
        //
        // NOTE (live wiring): a userspace daemon/app provides a
        // [`tls13::TlsByteTransport`] backed by the raekit socket syscalls and
        // calls `request_over`. See the module-level `tls13` docs.
        Err(HttpsError::ConnectionFailed(format!(
            "no transport available for {}:{} — call request_over(req, transport)",
            req.url.host, req.url.port
        )))
    }

    pub fn set_header(&mut self, name: &str, value: &str) {
        if let Some(existing) = self
            .default_headers
            .iter_mut()
            .find(|(h, _)| h.eq_ignore_ascii_case(name))
        {
            existing.1 = String::from(value);
        } else {
            self.default_headers
                .push((String::from(name), String::from(value)));
        }
    }

    pub fn set_user_agent(&mut self, ua: &str) {
        self.user_agent = String::from(ua);
    }

    pub fn set_proxy(&mut self, proxy: ProxyConfig) {
        self.proxy = Some(proxy);
    }

    fn parse_url(url: &str) -> Result<Url, HttpsError> {
        let (scheme, rest) = if let Some(pos) = url.find("://") {
            let s = &url[..pos];
            (String::from(s), &url[pos + 3..])
        } else {
            return Err(HttpsError::InvalidUrl(String::from("missing scheme")));
        };

        let default_port = match scheme.as_str() {
            "https" => 443u16,
            "http" => 80u16,
            _ => {
                return Err(HttpsError::InvalidUrl(format!(
                    "unsupported scheme: {}",
                    scheme
                )))
            }
        };

        let (authority, path_and_query) = match rest.find('/') {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => (rest, "/"),
        };

        let (userinfo, hostport) = match authority.find('@') {
            Some(pos) => (Some(&authority[..pos]), &authority[pos + 1..]),
            None => (None, authority),
        };

        let (username, password) = match userinfo {
            Some(ui) => match ui.find(':') {
                Some(pos) => (
                    Some(String::from(&ui[..pos])),
                    Some(String::from(&ui[pos + 1..])),
                ),
                None => (Some(String::from(ui)), None),
            },
            None => (None, None),
        };

        let (host, port) = match hostport.find(':') {
            Some(pos) => {
                let port_str = &hostport[pos + 1..];
                let port = port_str
                    .parse::<u16>()
                    .map_err(|_| HttpsError::InvalidUrl(String::from("invalid port")))?;
                (String::from(&hostport[..pos]), port)
            }
            None => (String::from(hostport), default_port),
        };

        let (path_query, fragment) = match path_and_query.find('#') {
            Some(pos) => (
                &path_and_query[..pos],
                Some(String::from(&path_and_query[pos + 1..])),
            ),
            None => (path_and_query, None),
        };

        let (path, query) = match path_query.find('?') {
            Some(pos) => (
                String::from(&path_query[..pos]),
                Some(String::from(&path_query[pos + 1..])),
            ),
            None => (String::from(path_query), None),
        };

        if host.is_empty() {
            return Err(HttpsError::InvalidUrl(String::from("empty host")));
        }

        Ok(Url {
            scheme,
            host,
            port,
            path,
            query,
            fragment,
            username,
            password,
        })
    }

    fn build_request_bytes(&self, req: &HttpsRequest) -> Vec<u8> {
        let mut buf = Vec::with_capacity(512);

        buf.extend_from_slice(req.method.as_str().as_bytes());
        buf.push(b' ');
        buf.extend_from_slice(req.url.full_path().as_bytes());
        buf.extend_from_slice(b" HTTP/1.1\r\n");

        for (name, value) in &req.headers {
            buf.extend_from_slice(name.as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        if let Some(body) = &req.body {
            let len_str = format!("{}", body.len());
            buf.extend_from_slice(b"Content-Length: ");
            buf.extend_from_slice(len_str.as_bytes());
            buf.extend_from_slice(b"\r\n\r\n");
            buf.extend_from_slice(body);
        } else {
            buf.extend_from_slice(b"\r\n");
        }

        buf
    }

    fn parse_response(data: &[u8]) -> Result<HttpsResponse, HttpsError> {
        let text = core::str::from_utf8(data)
            .map_err(|_| HttpsError::InvalidResponse(String::from("invalid UTF-8")))?;

        let header_end = text
            .find("\r\n\r\n")
            .ok_or_else(|| HttpsError::InvalidResponse(String::from("no header terminator")))?;

        let header_section = &text[..header_end];
        let body_start = header_end + 4;

        let mut lines = header_section.split("\r\n");
        let status_line = lines
            .next()
            .ok_or_else(|| HttpsError::InvalidResponse(String::from("empty response")))?;

        let (status, status_text) = Self::parse_status_line(status_line)?;
        let headers = Self::parse_headers(&text[status_line.len() + 2..header_end]);

        let body = if body_start < data.len() {
            data[body_start..].to_vec()
        } else {
            Vec::new()
        };

        Ok(HttpsResponse {
            status,
            status_text,
            headers,
            body,
            url: Url {
                scheme: String::from("https"),
                host: String::new(),
                port: 443,
                path: String::from("/"),
                query: None,
                fragment: None,
                username: None,
                password: None,
            },
            redirected: false,
            tls_info: None,
            timing: ResponseTiming {
                dns_ms: 0,
                connect_ms: 0,
                tls_ms: 0,
                first_byte_ms: 0,
                total_ms: 0,
            },
        })
    }

    fn parse_status_line(line: &str) -> Result<(u16, String), HttpsError> {
        let mut parts = line.splitn(3, ' ');
        let _version = parts
            .next()
            .ok_or_else(|| HttpsError::InvalidResponse(String::from("missing HTTP version")))?;
        let status_str = parts
            .next()
            .ok_or_else(|| HttpsError::InvalidResponse(String::from("missing status code")))?;
        let reason = parts.next().unwrap_or("");

        let status = status_str
            .parse::<u16>()
            .map_err(|_| HttpsError::InvalidResponse(String::from("invalid status code")))?;

        Ok((status, String::from(reason)))
    }

    fn parse_headers(header_data: &str) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        for line in header_data.split("\r\n") {
            if line.is_empty() {
                continue;
            }
            if let Some(colon) = line.find(':') {
                let name = line[..colon].trim();
                let value = line[colon + 1..].trim();
                headers.push((String::from(name), String::from(value)));
            }
        }
        headers
    }

    fn handle_redirect(
        &mut self,
        resp: &HttpsResponse,
        req: &HttpsRequest,
        count: u32,
    ) -> Result<HttpsResponse, HttpsError> {
        if count >= req.max_redirects {
            return Err(HttpsError::TooManyRedirects);
        }

        let location = resp.header("location").ok_or_else(|| {
            HttpsError::InvalidResponse(String::from("redirect without Location"))
        })?;

        let new_url = if location.starts_with("http://") || location.starts_with("https://") {
            Self::parse_url(location)?
        } else {
            let mut url = req.url.clone();
            if location.starts_with('/') {
                url.path = String::from(location);
            } else {
                url.path = format!("{}/{}", url.path.trim_end_matches('/'), location);
            }
            url.query = None;
            url
        };

        let method = match resp.status {
            301 | 302 => HttpMethod::Get,
            307 | 308 => req.method,
            _ => req.method,
        };

        let new_req = HttpsRequest {
            method,
            url: new_url,
            headers: req.headers.clone(),
            body: if method == HttpMethod::Get {
                None
            } else {
                req.body.clone()
            },
            timeout_ms: req.timeout_ms,
            follow_redirects: true,
            max_redirects: req.max_redirects,
        };

        self.request(new_req)
    }

    fn apply_cookies(&self, url: &Url, headers: &mut Vec<(String, String)>) {
        let cookies = self.cookie_jar.get_for_url(url, 0);
        if cookies.is_empty() {
            return;
        }

        let mut cookie_str = String::new();
        for (i, cookie) in cookies.iter().enumerate() {
            if i > 0 {
                cookie_str.push_str("; ");
            }
            cookie_str.push_str(&cookie.name);
            cookie_str.push('=');
            cookie_str.push_str(&cookie.value);
        }

        headers.push((String::from("Cookie"), cookie_str));
    }

    fn store_cookies(&mut self, url: &Url, headers: &[(String, String)]) {
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case("set-cookie") {
                continue;
            }

            if let Some(cookie) = Self::parse_set_cookie(value, url) {
                self.cookie_jar.add(cookie);
            }
        }
    }

    fn parse_set_cookie(header_value: &str, url: &Url) -> Option<Cookie> {
        let mut parts = header_value.split(';');
        let name_value = parts.next()?;
        let eq_pos = name_value.find('=')?;

        let name = name_value[..eq_pos].trim();
        let value = name_value[eq_pos + 1..].trim();

        let mut cookie = Cookie {
            name: String::from(name),
            value: String::from(value),
            domain: url.host.clone(),
            path: String::from("/"),
            expires: None,
            secure: false,
            http_only: false,
            same_site: SameSite::Lax,
        };

        for attr in parts {
            let attr = attr.trim();
            let lower = attr.to_ascii_lowercase();

            if lower == "secure" {
                cookie.secure = true;
            } else if lower == "httponly" {
                cookie.http_only = true;
            } else if let Some(val) = lower.strip_prefix("domain=") {
                cookie.domain = String::from(val.trim_start_matches('.'));
            } else if let Some(val) = lower.strip_prefix("path=") {
                cookie.path = String::from(val);
            } else if let Some(val) = lower.strip_prefix("samesite=") {
                cookie.same_site = match val {
                    "strict" => SameSite::Strict,
                    "none" => SameSite::None,
                    _ => SameSite::Lax,
                };
            } else if let Some(val) = lower.strip_prefix("max-age=") {
                if let Ok(secs) = val.parse::<u64>() {
                    cookie.expires = Some(secs);
                }
            }
        }

        Some(cookie)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_basic() {
        let url = HttpsClient::parse_url("https://example.com/path?q=1#frag").unwrap();
        assert_eq!(url.scheme, "https");
        assert_eq!(url.host, "example.com");
        assert_eq!(url.port, 443);
        assert_eq!(url.path, "/path");
        assert_eq!(url.query.as_deref(), Some("q=1"));
        assert_eq!(url.fragment.as_deref(), Some("frag"));
    }

    #[test]
    fn test_parse_url_with_port() {
        let url = HttpsClient::parse_url("https://example.com:8443/api").unwrap();
        assert_eq!(url.port, 8443);
        assert_eq!(url.path, "/api");
    }

    #[test]
    fn test_parse_url_with_userinfo() {
        let url = HttpsClient::parse_url("https://user:pass@host.com/").unwrap();
        assert_eq!(url.username.as_deref(), Some("user"));
        assert_eq!(url.password.as_deref(), Some("pass"));
        assert_eq!(url.host, "host.com");
    }

    #[test]
    fn test_cookie_matching() {
        let cookie = Cookie {
            name: String::from("session"),
            value: String::from("abc123"),
            domain: String::from("example.com"),
            path: String::from("/"),
            expires: None,
            secure: true,
            http_only: false,
            same_site: SameSite::Lax,
        };

        assert!(cookie.matches_domain("example.com"));
        assert!(cookie.matches_domain("sub.example.com"));
        assert!(!cookie.matches_domain("other.com"));
    }

    #[test]
    fn test_tls_config_defaults() {
        let config = TlsConfig::new();
        assert_eq!(config.min_version, TlsVersion::Tls12);
        assert_eq!(config.max_version, TlsVersion::Tls13);
        assert!(config.verify_hostname);
        assert!(config.verify_cert);
        assert!(config.sni);
    }
}

// ===========================================================================
// Verified TLS 1.3 HTTPS request flow (feature `tls13`).
//
// Concept §"AthNet: real TLS 1.3, not a toy" + criterion #5 ("the apps people
// use need working HTTPS"): the orchestration that drives the `tls_crypto`
// engine end-to-end so `HttpsClient::get_over` returns a body that arrived over
// an AUTHENTICATED, encrypted channel -- or a clear error, never a plaintext
// fallback.
//
// The flow:
//   1. open a byte transport (the host-mockable `TlsByteTransport`),
//   2. ClientHello out; read ServerHello; derive handshake-traffic keys; decrypt
//      the server's encrypted flight (EncryptedExtensions, Certificate,
//      CertificateVerify, Finished); verify the chain to a trusted root, bind the
//      hostname, verify CertificateVerify + server Finished via the engine's
//      `ClientHandshake::recv_authenticated_flight`; send the client Finished,
//   3. on Connected, seal the HTTP/1 request as application_data records and open
//      the response records, feeding the proven `http1` response parser.
//
// API constraint: the `tls_crypto::ClientHandshake` does NOT expose its
// handshake-traffic secrets or build the client Finished -- those are private. So
// this module runs a PARALLEL `KeySchedule`+`Transcript` over the SAME message
// bytes purely for the record-layer keys (flight decryption + client Finished),
// while `ClientHandshake` remains the single source of truth for every
// AUTHENTICATION decision. Both consume identical transcript inputs, so they stay
// in lockstep; any divergence makes the server-Finished check fail closed.
//
// Randomness is INJECTED (caller supplies the x25519 secret + ClientHello random)
// so the orchestration is fully host-testable with fixed seeds and carries no RNG
// dependency. A userspace daemon passes CSPRNG bytes (getrandom/raekit) -- that is
// the only remaining live-wiring step.
// ===========================================================================
#[cfg(feature = "tls13")]
pub mod tls13 {
    extern crate alloc;

    use super::{HttpMethod, HttpsError, HttpsRequest, HttpsResponse, ResponseTiming, Url};
    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    use crate::http1::{self, Http1Error, Http1Request, Http1Response, Limits, Method};
    use crate::tls_crypto::{
        build_client_hello, finished_key, finished_verify_data, parse_server_hello, record_nonce,
        traffic_key_iv, CertError, CipherSuite, ClientHandshake, ContentType, HandshakeState,
        KeySchedule, RecordParse, TlsRecord, Transcript, TrustStore, MAX_RECORD_LEN,
    };

    /// Hard cap on how large `in_buf` may grow while still failing to yield a
    /// single parseable record. If a peer dribbles bytes without ever completing
    /// a record (or declares a record we will not accept), we MUST bail rather
    /// than buffer forever (criterion #6 — hostile bytes never OOM the host).
    /// One full max-size record framing (5-byte header + `MAX_RECORD_LEN`) plus a
    /// small slack: anything beyond this with no record means the stream is
    /// malformed/adversarial.
    const MAX_IN_BUF: usize = MAX_RECORD_LEN + 5 + 256;

    /// Hard cap on the encrypted handshake flight: the total decrypted handshake
    /// bytes AND the number of records we will read before the 4 expected
    /// messages (EE/Cert/CertVerify/Finished) assemble. A key-valid-but-malicious
    /// server could otherwise stream unbounded valid handshake records.
    const MAX_HS_FLIGHT_BYTES: usize = 256 * 1024; // generous for a real chain.
    const MAX_HS_FLIGHT_RECORDS: usize = 64;

    use x25519_dalek::{PublicKey, StaticSecret};

    /// A bidirectional byte stream the TLS record layer rides on. The only thing
    /// that touches a real socket: a userspace daemon implements it over the
    /// raekit socket syscalls; the host KATs implement it with a deterministic
    /// in-memory mock that replays a canned TLS-1.3 server script. Byte-oriented,
    /// not record-oriented, because TLS records split across TCP segments.
    pub trait TlsByteTransport {
        fn connect(&mut self, host: &str, port: u16) -> Result<(), HttpsError>;
        fn send(&mut self, buf: &[u8]) -> Result<(), HttpsError>;
        /// Read up to `buf.len()` bytes; `Ok(0)` = peer closed (EOF).
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpsError>;
    }

    /// Wrap a handshake body in its `type(1) || len(u24)` header (the engine's own
    /// `wrap_handshake` is private; this is the identical encoding for the client
    /// Finished we build here).
    fn wrap_handshake(msg_type: u8, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + body.len());
        out.push(msg_type);
        let len = body.len();
        out.push((len >> 16) as u8);
        out.push((len >> 8) as u8);
        out.push(len as u8);
        out.extend_from_slice(body);
        out
    }

    /// Suite-generic AEAD seal for a TLS 1.3 record (the engine only exposes the
    /// AES-256 variant). Returns ciphertext||tag or `None`.
    fn aead_seal(
        suite: CipherSuite,
        key: &[u8],
        nonce: &[u8; 12],
        aad: &[u8],
        pt: &[u8],
    ) -> Option<Vec<u8>> {
        use aes_gcm::aead::{Aead, KeyInit, Payload};
        match suite {
            CipherSuite::Aes128GcmSha256 => {
                use aes_gcm::{Aes128Gcm, Key, Nonce};
                if key.len() != 16 {
                    return None;
                }
                Aes128Gcm::new(Key::<Aes128Gcm>::from_slice(key))
                    .encrypt(Nonce::from_slice(nonce), Payload { msg: pt, aad })
                    .ok()
            }
            CipherSuite::Aes256GcmSha384 => {
                use aes_gcm::{Aes256Gcm, Key, Nonce};
                if key.len() != 32 {
                    return None;
                }
                Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
                    .encrypt(Nonce::from_slice(nonce), Payload { msg: pt, aad })
                    .ok()
            }
            CipherSuite::ChaCha20Poly1305Sha256 => {
                use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
                if key.len() != 32 {
                    return None;
                }
                ChaCha20Poly1305::new(Key::from_slice(key))
                    .encrypt(Nonce::from_slice(nonce), Payload { msg: pt, aad })
                    .ok()
            }
        }
    }

    /// Suite-generic AEAD open. `None` if the tag fails -- the whole point: a
    /// tampered record must not decrypt (fail-closed).
    fn aead_open(
        suite: CipherSuite,
        key: &[u8],
        nonce: &[u8; 12],
        aad: &[u8],
        ct: &[u8],
    ) -> Option<Vec<u8>> {
        use aes_gcm::aead::{Aead, KeyInit, Payload};
        match suite {
            CipherSuite::Aes128GcmSha256 => {
                use aes_gcm::{Aes128Gcm, Key, Nonce};
                if key.len() != 16 {
                    return None;
                }
                Aes128Gcm::new(Key::<Aes128Gcm>::from_slice(key))
                    .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
                    .ok()
            }
            CipherSuite::Aes256GcmSha384 => {
                use aes_gcm::{Aes256Gcm, Key, Nonce};
                if key.len() != 32 {
                    return None;
                }
                Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
                    .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
                    .ok()
            }
            CipherSuite::ChaCha20Poly1305Sha256 => {
                use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
                if key.len() != 32 {
                    return None;
                }
                ChaCha20Poly1305::new(Key::from_slice(key))
                    .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
                    .ok()
            }
        }
    }

    /// AEAD additional-authenticated-data for a TLS 1.3 record: the 5-byte header
    /// `type(23) || 0x0303 || u16 encrypted_len` (inner plaintext + 16-byte tag).
    fn record_aad(encrypted_len: usize) -> [u8; 5] {
        let l = encrypted_len as u16;
        [0x17, 0x03, 0x03, (l >> 8) as u8, (l & 0xff) as u8]
    }

    /// Seal `inner` (already including the trailing inner-type byte) as one
    /// application_data record and return the on-wire bytes.
    fn seal_record(
        suite: CipherSuite,
        key: &[u8],
        iv: &[u8; 12],
        seq: u64,
        inner: &[u8],
    ) -> Option<Vec<u8>> {
        let nonce = record_nonce(iv, seq);
        let aad = record_aad(inner.len() + 16);
        let sealed = aead_seal(suite, key, &nonce, &aad, inner)?;
        Some(
            TlsRecord {
                content_type: ContentType::ApplicationData,
                fragment: sealed,
            }
            .to_bytes(),
        )
    }

    /// Strip TLS 1.3 inner padding + the real content-type byte from a decrypted
    /// record. Returns `(content_type, content)` or `None` if all-zero/empty.
    fn split_inner(inner: &[u8]) -> Option<(u8, Vec<u8>)> {
        let mut end = inner.len();
        while end > 0 && inner[end - 1] == 0x00 {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        Some((inner[end - 1], inner[..end - 1].to_vec()))
    }

    /// The application record stream once Connected: implements
    /// `http1::HttpTransport` so the proven HTTP/1 driver runs unchanged over the
    /// encrypted channel.
    struct TlsAppStream<'a, T: TlsByteTransport> {
        transport: &'a mut T,
        suite: CipherSuite,
        client_key: Vec<u8>,
        client_iv: [u8; 12],
        server_key: Vec<u8>,
        server_iv: [u8; 12],
        write_seq: u64,
        read_seq: u64,
        in_buf: Vec<u8>,
        plaintext: Vec<u8>,
        plaintext_pos: usize,
        eof: bool,
    }

    impl<'a, T: TlsByteTransport> TlsAppStream<'a, T> {
        /// Decrypt one buffered inbound record into `plaintext`. `Ok(true)` if a
        /// record was consumed; `Ok(false)` if more wire bytes are needed.
        fn pump_one_record(&mut self) -> Result<bool, HttpsError> {
            let (record, used) = match TlsRecord::parse_state(&self.in_buf) {
                RecordParse::Record(r, used) => (r, used),
                RecordParse::NeedMore => return Ok(false),
                RecordParse::Invalid => {
                    // Hostile/oversized record header on the app stream: fail
                    // closed rather than spin recv'ing forever.
                    return Err(HttpsError::ProtocolError(
                        "invalid or oversized TLS record header".to_string(),
                    ));
                }
            };
            self.in_buf.drain(..used);
            if record.content_type == ContentType::ChangeCipherSpec {
                return Ok(true); // legacy middlebox compat; ignore.
            }
            if record.content_type != ContentType::ApplicationData {
                return Err(HttpsError::ProtocolError(
                    "unexpected plaintext record after handshake".to_string(),
                ));
            }
            let aad = record_aad(record.fragment.len());
            let nonce = record_nonce(&self.server_iv, self.read_seq);
            self.read_seq = self.read_seq.wrapping_add(1);
            let inner = aead_open(self.suite, &self.server_key, &nonce, &aad, &record.fragment)
                .ok_or_else(|| {
                    HttpsError::TlsHandshakeFailed(
                        "application record AEAD open failed".to_string(),
                    )
                })?;
            let (real_type, content) = split_inner(&inner)
                .ok_or_else(|| HttpsError::ProtocolError("empty inner record".to_string()))?;
            match ContentType::from_u8(real_type) {
                Some(ContentType::ApplicationData) => self.plaintext.extend_from_slice(&content),
                Some(ContentType::Alert) => self.eof = true, // close_notify / alert ends stream.
                Some(ContentType::Handshake) => {} // post-handshake (NewSessionTicket): skip.
                _ => {}
            }
            Ok(true)
        }
    }

    impl<'a, T: TlsByteTransport> http1::HttpTransport for TlsAppStream<'a, T> {
        fn connect(&mut self, _host: &str, _port: u16) -> http1::Http1Result<()> {
            Ok(()) // TCP + TLS already established by `request_over`.
        }

        fn send(&mut self, buf: &[u8]) -> http1::Http1Result<()> {
            const MAX_FRAGMENT: usize = 16384 - 1; // leave room for the inner type byte.
            let mut off = 0usize;
            while off < buf.len() {
                let end = core::cmp::min(off + MAX_FRAGMENT, buf.len());
                let mut inner = Vec::with_capacity(end - off + 1);
                inner.extend_from_slice(&buf[off..end]);
                inner.push(ContentType::ApplicationData as u8);
                let wire = seal_record(
                    self.suite,
                    &self.client_key,
                    &self.client_iv,
                    self.write_seq,
                    &inner,
                )
                .ok_or_else(|| Http1Error::Transport("application AEAD seal failed".to_string()))?;
                self.write_seq = self.write_seq.wrapping_add(1);
                self.transport
                    .send(&wire)
                    .map_err(|e| Http1Error::Transport(e_to_string(&e)))?;
                off = end;
            }
            Ok(())
        }

        fn recv(&mut self, out: &mut [u8]) -> http1::Http1Result<usize> {
            loop {
                if self.plaintext_pos < self.plaintext.len() {
                    let avail = self.plaintext.len() - self.plaintext_pos;
                    let n = core::cmp::min(avail, out.len());
                    out[..n].copy_from_slice(
                        &self.plaintext[self.plaintext_pos..self.plaintext_pos + n],
                    );
                    self.plaintext_pos += n;
                    return Ok(n);
                }
                let processed = self
                    .pump_one_record()
                    .map_err(|e| Http1Error::Transport(e_to_string(&e)))?;
                if processed {
                    continue;
                }
                if self.eof {
                    return Ok(0);
                }
                // Independent backstop against a peer that dribbles sub-record
                // bytes forever (parse stays NeedMore): cap `in_buf` growth.
                if self.in_buf.len() > MAX_IN_BUF {
                    return Err(Http1Error::Transport(
                        "TLS record buffer exceeded maximum without a complete record".to_string(),
                    ));
                }
                let mut tmp = [0u8; 4096];
                let n = self
                    .transport
                    .recv(&mut tmp)
                    .map_err(|e| Http1Error::Transport(e_to_string(&e)))?;
                if n == 0 {
                    self.eof = true;
                    if !self
                        .pump_one_record()
                        .map_err(|e| Http1Error::Transport(e_to_string(&e)))?
                    {
                        return Ok(0);
                    }
                    continue;
                }
                self.in_buf.extend_from_slice(&tmp[..n]);
            }
        }
    }

    fn e_to_string(e: &HttpsError) -> String {
        match e {
            HttpsError::ConnectionFailed(s)
            | HttpsError::IoError(s)
            | HttpsError::ProtocolError(s)
            | HttpsError::TlsHandshakeFailed(s) => s.clone(),
            other => format!("{:?}", other),
        }
    }

    /// Map a `tls_crypto` cert/auth error to the public `HttpsError` (fail-closed).
    fn cert_err_to_https(e: CertError) -> HttpsError {
        match e {
            CertError::HostnameMismatch => HttpsError::HostnameVerificationFailed,
            CertError::Expired => {
                HttpsError::CertificateExpired("certificate outside validity window".to_string())
            }
            CertError::UntrustedRoot => HttpsError::CertificateInvalid(
                "chain does not anchor to a trusted root".to_string(),
            ),
            CertError::BadSignature => {
                HttpsError::CertificateInvalid("issuer signature did not verify".to_string())
            }
            CertError::BadCertificateVerify => HttpsError::TlsHandshakeFailed(
                "CertificateVerify/Finished did not verify".to_string(),
            ),
            other => HttpsError::TlsHandshakeFailed(format!("certificate error: {:?}", other)),
        }
    }

    fn http1_err_to_https(e: Http1Error) -> HttpsError {
        match e {
            Http1Error::Transport(s) => HttpsError::IoError(s),
            Http1Error::InvalidUrl(s) => HttpsError::InvalidUrl(s),
            other => HttpsError::InvalidResponse(format!("{:?}", other)),
        }
    }

    /// Read from `transport` into `in_buf` until one full record is present.
    fn read_one_record<T: TlsByteTransport>(
        transport: &mut T,
        in_buf: &mut Vec<u8>,
    ) -> Result<TlsRecord, HttpsError> {
        loop {
            match TlsRecord::parse_state(in_buf) {
                RecordParse::Record(rec, used) => {
                    in_buf.drain(..used);
                    return Ok(rec);
                }
                RecordParse::Invalid => {
                    // Hostile/oversized length header: HARD reject, never retry.
                    return Err(HttpsError::TlsHandshakeFailed(
                        "invalid or oversized TLS record header".to_string(),
                    ));
                }
                RecordParse::NeedMore => {}
            }
            // Independent backstop: if we have buffered more than one max record
            // framing's worth of bytes and still cannot parse a record, the peer
            // is dribbling garbage — bail rather than grow `in_buf` unbounded.
            if in_buf.len() > MAX_IN_BUF {
                return Err(HttpsError::TlsHandshakeFailed(
                    "TLS record buffer exceeded maximum without a complete record".to_string(),
                ));
            }
            let mut tmp = [0u8; 4096];
            let n = transport.recv(&mut tmp)?;
            if n == 0 {
                return Err(HttpsError::TlsHandshakeFailed(
                    "peer closed during handshake".to_string(),
                ));
            }
            in_buf.extend_from_slice(&tmp[..n]);
        }
    }

    /// Decrypt one encrypted HANDSHAKE-flight record with the server handshake key,
    /// returning the inner (type-stripped) handshake bytes. Skips ChangeCipherSpec.
    fn decrypt_handshake_record<T: TlsByteTransport>(
        transport: &mut T,
        in_buf: &mut Vec<u8>,
        suite: CipherSuite,
        key: &[u8],
        iv: &[u8; 12],
        seq: &mut u64,
    ) -> Result<Vec<u8>, HttpsError> {
        loop {
            let rec = read_one_record(transport, in_buf)?;
            if rec.content_type == ContentType::ChangeCipherSpec {
                continue;
            }
            if rec.content_type != ContentType::ApplicationData {
                return Err(HttpsError::TlsHandshakeFailed(
                    "expected encrypted handshake record".to_string(),
                ));
            }
            let aad = record_aad(rec.fragment.len());
            let nonce = record_nonce(iv, *seq);
            *seq = seq.wrapping_add(1);
            let inner = aead_open(suite, key, &nonce, &aad, &rec.fragment).ok_or_else(|| {
                HttpsError::TlsHandshakeFailed("handshake record AEAD open failed".to_string())
            })?;
            let (real_type, content) = split_inner(&inner).ok_or_else(|| {
                HttpsError::TlsHandshakeFailed("empty handshake record".to_string())
            })?;
            if real_type != ContentType::Handshake as u8 {
                return Err(HttpsError::TlsHandshakeFailed(
                    "non-handshake content in encrypted flight".to_string(),
                ));
            }
            return Ok(content);
        }
    }

    /// Split concatenated handshake messages (`type(1) || len(u24) || body`) into
    /// individual full messages. Bounds-checked, fail-closed, never panics.
    fn split_handshake_messages(buf: &[u8]) -> Result<Vec<Vec<u8>>, HttpsError> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < buf.len() {
            if i + 4 > buf.len() {
                return Err(HttpsError::TlsHandshakeFailed(
                    "truncated handshake message header".to_string(),
                ));
            }
            let len =
                ((buf[i + 1] as usize) << 16) | ((buf[i + 2] as usize) << 8) | buf[i + 3] as usize;
            let end = i + 4 + len;
            if end > buf.len() {
                return Err(HttpsError::TlsHandshakeFailed(
                    "truncated handshake message body".to_string(),
                ));
            }
            out.push(buf[i..end].to_vec());
            i = end;
        }
        Ok(out)
    }

    /// Pull the server's x25519 key_share from a ServerHello message (header+body).
    fn extract_server_keyshare(sh_msg: &[u8]) -> Result<[u8; 32], HttpsError> {
        if sh_msg.len() < 4 || sh_msg[0] != 0x02 {
            return Err(HttpsError::TlsHandshakeFailed(
                "not a ServerHello".to_string(),
            ));
        }
        let parsed = parse_server_hello(&sh_msg[4..])
            .ok_or_else(|| HttpsError::TlsHandshakeFailed("malformed ServerHello".to_string()))?;
        if parsed.is_hello_retry_request {
            return Err(HttpsError::TlsHandshakeFailed(
                "HelloRetryRequest not supported".to_string(),
            ));
        }
        Ok(parsed.server_key_share)
    }

    impl super::HttpsClient {
        /// Verified GET of `url` over `transport`. `client_ecdhe_secret` and
        /// `client_random` are caller-supplied 32-byte CSPRNG values (injected for
        /// testability; a userspace daemon passes getrandom output). `now_unix` is
        /// the wall clock for certificate validity (None skips the window check).
        pub fn get_over<T: TlsByteTransport>(
            &mut self,
            url: &str,
            transport: &mut T,
            client_ecdhe_secret: &[u8; 32],
            client_random: &[u8; 32],
            now_unix: Option<u64>,
        ) -> Result<HttpsResponse, HttpsError> {
            let parsed = super::HttpsClient::parse_url(url)?;
            let req = HttpsRequest::new(HttpMethod::Get, parsed);
            self.request_over(req, transport, client_ecdhe_secret, client_random, now_unix)
        }

        /// Drive a verified HTTPS request through the TLS 1.3 engine. Fails closed:
        /// any cert/hostname/handshake failure returns an `HttpsError`, NEVER a
        /// plaintext body.
        pub fn request_over<T: TlsByteTransport>(
            &mut self,
            req: HttpsRequest,
            transport: &mut T,
            client_ecdhe_secret: &[u8; 32],
            client_random: &[u8; 32],
            now_unix: Option<u64>,
        ) -> Result<HttpsResponse, HttpsError> {
            if req.url.scheme != "https" {
                return Err(HttpsError::InvalidUrl(
                    "request_over requires an https:// URL".to_string(),
                ));
            }

            // Trust store (fail-closed: no anchors => cannot authenticate anyone).
            let trust = self.build_trust_store()?;
            let mut auth = ClientHandshake::new_authenticated(trust, &req.url.host, now_unix);

            // Parallel record-layer schedule/transcript (engine secrets are private).
            let mut transcript = Transcript::new();
            let mut schedule = KeySchedule::new();

            transport.connect(&req.url.host, req.url.port)?;

            // -- ClientHello --
            let client_sk = StaticSecret::from(*client_ecdhe_secret);
            let client_pub = PublicKey::from(&client_sk).to_bytes();
            let suites = [CipherSuite::Aes128GcmSha256, CipherSuite::Aes256GcmSha384];
            let ch = build_client_hello(client_random, &[], &suites, &client_pub, &req.url.host);
            auth.send_client_hello(&ch);
            transcript.update(&ch);
            transport.send(
                &TlsRecord {
                    content_type: ContentType::Handshake,
                    fragment: ch.clone(),
                }
                .to_bytes(),
            )?;

            // -- ServerHello (plaintext handshake record) --
            let mut in_buf: Vec<u8> = Vec::new();
            let sh_record = read_one_record(transport, &mut in_buf)?;
            if sh_record.content_type != ContentType::Handshake {
                return Err(HttpsError::TlsHandshakeFailed(
                    "expected ServerHello handshake record".to_string(),
                ));
            }
            let server_pub = extract_server_keyshare(&sh_record.fragment)?;
            let shared = client_sk
                .diffie_hellman(&PublicKey::from(server_pub))
                .to_bytes();

            // Engine processes ServerHello (its own transcript + schedule).
            let suite = auth
                .recv_server_hello(&sh_record.fragment, &shared)
                .ok_or_else(|| {
                    HttpsError::TlsHandshakeFailed("ServerHello rejected".to_string())
                })?;
            if auth.state != HandshakeState::WaitFlight {
                return Err(HttpsError::TlsHandshakeFailed(
                    "handshake did not reach WaitFlight".to_string(),
                ));
            }
            // Mirror into the parallel schedule for the record-layer keys.
            transcript.update(&sh_record.fragment);
            let th_sh = transcript.current_hash();
            schedule.derive_handshake_secrets(&shared, &th_sh);
            let server_hs = schedule
                .server_handshake_traffic
                .ok_or_else(|| HttpsError::TlsHandshakeFailed("no server hs secret".to_string()))?;
            let client_hs = schedule
                .client_handshake_traffic
                .ok_or_else(|| HttpsError::TlsHandshakeFailed("no client hs secret".to_string()))?;
            let (s_hs_key, s_hs_iv) = traffic_key_iv(&server_hs, suite.key_len());

            // -- Decrypt the server's encrypted handshake flight --
            let mut hs_read_seq = 0u64;
            let mut hs_bytes: Vec<u8> = Vec::new();
            let mut messages: Vec<Vec<u8>> = Vec::new();
            let mut hs_records = 0usize;
            while messages.len() < 4 {
                // MED: a key-valid-but-malicious server could stream unbounded
                // valid handshake records. Cap both the record count and the
                // accumulated bytes before the 4 messages assemble.
                if hs_records >= MAX_HS_FLIGHT_RECORDS {
                    return Err(HttpsError::TlsHandshakeFailed(
                        "handshake flight exceeded maximum record count".to_string(),
                    ));
                }
                let chunk = decrypt_handshake_record(
                    transport,
                    &mut in_buf,
                    suite,
                    &s_hs_key,
                    &s_hs_iv,
                    &mut hs_read_seq,
                )?;
                hs_records += 1;
                if hs_bytes.len() + chunk.len() > MAX_HS_FLIGHT_BYTES {
                    return Err(HttpsError::TlsHandshakeFailed(
                        "handshake flight exceeded maximum byte count".to_string(),
                    ));
                }
                hs_bytes.extend_from_slice(&chunk);
                messages = split_handshake_messages(&hs_bytes)?;
            }
            // EE(0x08), Certificate(0x0b), CertificateVerify(0x0f), Finished(0x14).
            let ee = &messages[0];
            let cert = &messages[1];
            let cv = &messages[2];
            let fin = &messages[3];

            // -- Authenticate (chain + hostname + CertVerify + server Finished) --
            auth.recv_authenticated_flight(ee, cert, cv, fin)
                .map_err(cert_err_to_https)?;
            if auth.state != HandshakeState::Connected {
                return Err(HttpsError::TlsHandshakeFailed(
                    "handshake did not reach Connected".to_string(),
                ));
            }

            // Mirror the flight + server Finished into the parallel transcript so
            // the client Finished + application keys match the engine's view.
            transcript.update(ee);
            transcript.update(cert);
            transcript.update(cv);
            transcript.update(fin);
            let th_after_sf = transcript.current_hash();
            if !schedule.derive_application_secrets(&th_after_sf) {
                return Err(HttpsError::TlsHandshakeFailed(
                    "application secret derivation failed".to_string(),
                ));
            }

            // -- Client Finished (encrypted with the CLIENT handshake key) --
            // verify_data = HMAC(finished_key(client_hs), transcript_through_SF).
            let fk = finished_key(&client_hs);
            let vd = finished_verify_data(&fk, &th_after_sf);
            let client_finished = wrap_handshake(0x14, &vd);
            let mut inner_fin = client_finished;
            inner_fin.push(ContentType::Handshake as u8);
            let (c_hs_key, c_hs_iv) = traffic_key_iv(&client_hs, suite.key_len());
            let fin_wire =
                seal_record(suite, &c_hs_key, &c_hs_iv, 0, &inner_fin).ok_or_else(|| {
                    HttpsError::TlsHandshakeFailed("client Finished seal failed".to_string())
                })?;
            transport.send(&fin_wire)?;

            // -- Application keys (from the engine -- single source of truth) --
            let (c_app_key, c_app_iv) = auth
                .client_app_key_iv()
                .ok_or_else(|| HttpsError::TlsHandshakeFailed("no client app keys".to_string()))?;
            let (s_app_key, s_app_iv) = auth
                .server_app_key_iv()
                .ok_or_else(|| HttpsError::TlsHandshakeFailed("no server app keys".to_string()))?;

            let mut stream = TlsAppStream {
                transport,
                suite,
                client_key: c_app_key,
                client_iv: c_app_iv,
                server_key: s_app_key,
                server_iv: s_app_iv,
                write_seq: 0,
                read_seq: 0,
                in_buf,
                plaintext: Vec::new(),
                plaintext_pos: 0,
                eof: false,
            };

            let h1 = build_http1_request(&req);
            let resp = http1::send_request(&mut stream, &h1, &Limits::new())
                .map_err(http1_err_to_https)?;
            Ok(to_https_response(resp, req.url.clone()))
        }

        /// Build a `tls_crypto::TrustStore` from the configured DER anchors. Errors
        /// (fail-closed) if none are configured.
        fn build_trust_store(&self) -> Result<TrustStore, HttpsError> {
            let mut ts = TrustStore::new();
            for der in &self.root_anchors_der {
                ts.add_root_der(der);
            }
            if ts.is_empty() {
                return Err(HttpsError::CertificateInvalid(
                    "no trusted root certificates configured".to_string(),
                ));
            }
            Ok(ts)
        }
    }

    /// Build the `http1::Http1Request` from the higher-level `HttpsRequest`.
    fn build_http1_request(req: &HttpsRequest) -> Http1Request {
        let method = match req.method {
            HttpMethod::Get => Method::Get,
            HttpMethod::Post => Method::Post,
            HttpMethod::Put => Method::Put,
            HttpMethod::Delete => Method::Delete,
            HttpMethod::Patch => Method::Patch,
            HttpMethod::Head => Method::Head,
            HttpMethod::Options => Method::Options,
            _ => Method::Get,
        };
        let mut h = Http1Request::new(method, req.url.full_path(), req.url.authority());
        for (name, value) in &req.headers {
            if name.eq_ignore_ascii_case("host") {
                continue; // builder emits Host.
            }
            h = h.header(name.clone(), value.clone());
        }
        h.body = req.body.clone();
        h
    }

    /// Map the parsed http1 response into the public `HttpsResponse`.
    fn to_https_response(r: Http1Response, url: Url) -> HttpsResponse {
        HttpsResponse {
            status: r.status,
            status_text: r.reason,
            headers: r.headers,
            body: r.body,
            url,
            redirected: false,
            tls_info: None,
            timing: ResponseTiming {
                dns_ms: 0,
                connect_ms: 0,
                tls_ms: 0,
                first_byte_ms: 0,
                total_ms: 0,
            },
        }
    }

    // -----------------------------------------------------------------------
    // Host KATs (cargo test -p raenet --features tls13): verified fetch +
    // fail-closed. A reactive mock server builds a self-consistent TLS-1.3 script
    // from an in-test PKI in REACTION to the client's actual ClientHello (so the
    // ECDHE matches the client's injected ephemeral). The verified path returns
    // the decrypted body; every auth failure returns an error and NO body.
    // -----------------------------------------------------------------------
    #[cfg(test)]
    mod kat {
        use super::*;
        use alloc::vec;
        use der::oid::ObjectIdentifier;
        use der::{
            asn1::{Any, BitString, GeneralizedTime, OctetString},
            Encode, Tag,
        };
        use p256::ecdsa::signature::Signer as P256Signer;
        use p256::ecdsa::{Signature as P256Sig, SigningKey as P256Signing};
        use x509_cert::ext::pkix::name::GeneralName;
        use x509_cert::ext::pkix::SubjectAltName;
        use x509_cert::{spki, Certificate, TbsCertificate};

        const OID_EC_PUBLIC_KEY: ObjectIdentifier =
            ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
        const OID_ECDSA_WITH_SHA256: ObjectIdentifier =
            ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
        const OID_SECP256R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
        const OID_SAN: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.17");

        fn p256_key(seed: u8) -> P256Signing {
            let mut bytes = [1u8; 32];
            bytes[0] = seed;
            P256Signing::from_bytes(&bytes.into()).expect("valid p256 scalar")
        }

        fn make_cert(
            subject_dn: &str,
            issuer_dn: &str,
            subject_key: &P256Signing,
            issuer_key: &P256Signing,
            san_dns: Option<&str>,
            nb: u64,
            na: u64,
            serial: u8,
        ) -> Vec<u8> {
            use core::str::FromStr;
            use core::time::Duration;
            use x509_cert::name::Name;
            use x509_cert::serial_number::SerialNumber;
            use x509_cert::time::{Time, Validity};

            let pub_point = subject_key.verifying_key().to_encoded_point(false);
            let spki_alg = spki::AlgorithmIdentifier {
                oid: OID_EC_PUBLIC_KEY,
                parameters: Some(
                    Any::new(Tag::ObjectIdentifier, OID_SECP256R1.as_bytes()).unwrap(),
                ),
            };
            let spki = spki::SubjectPublicKeyInfo {
                algorithm: spki_alg,
                subject_public_key: BitString::from_bytes(pub_point.as_bytes()).unwrap(),
            };
            let sig_alg = spki::AlgorithmIdentifier {
                oid: OID_ECDSA_WITH_SHA256,
                parameters: None,
            };
            let validity = Validity {
                not_before: Time::GeneralTime(
                    GeneralizedTime::from_unix_duration(Duration::from_secs(nb)).unwrap(),
                ),
                not_after: Time::GeneralTime(
                    GeneralizedTime::from_unix_duration(Duration::from_secs(na)).unwrap(),
                ),
            };
            let extensions = san_dns.map(|dns| {
                let san = SubjectAltName(vec![GeneralName::DnsName(
                    der::asn1::Ia5String::new(dns).unwrap(),
                )]);
                let san_der = san.to_der().unwrap();
                vec![x509_cert::ext::Extension {
                    extn_id: OID_SAN,
                    critical: false,
                    extn_value: OctetString::new(san_der).unwrap(),
                }]
            });
            let tbs = TbsCertificate {
                version: x509_cert::certificate::Version::V3,
                serial_number: SerialNumber::new(&[serial]).unwrap(),
                signature: sig_alg.clone(),
                issuer: Name::from_str(issuer_dn).unwrap(),
                validity,
                subject: Name::from_str(subject_dn).unwrap(),
                subject_public_key_info: spki,
                issuer_unique_id: None,
                subject_unique_id: None,
                extensions,
            };
            let tbs_der = tbs.to_der().unwrap();
            let sig: P256Sig = issuer_key.sign(&tbs_der);
            let cert = Certificate {
                tbs_certificate: tbs,
                signature_algorithm: sig_alg,
                signature: BitString::from_bytes(sig.to_der().as_bytes()).unwrap(),
            };
            cert.to_der().unwrap()
        }

        struct Pki {
            root_der: Vec<u8>,
            inter_der: Vec<u8>,
            leaf_der: Vec<u8>,
            leaf_key: P256Signing,
        }

        fn build_pki(leaf_dns: &str, root_seed: u8) -> Pki {
            let root_k = p256_key(root_seed);
            let inter_k = p256_key(root_seed.wrapping_add(1));
            let leaf_k = p256_key(root_seed.wrapping_add(2));
            let root_der = make_cert(
                "CN=KAT Root",
                "CN=KAT Root",
                &root_k,
                &root_k,
                None,
                0,
                100000,
                1,
            );
            let inter_der = make_cert(
                "CN=KAT Intermediate",
                "CN=KAT Root",
                &inter_k,
                &root_k,
                None,
                0,
                100000,
                2,
            );
            let leaf_der = make_cert(
                "CN=leaf",
                "CN=KAT Intermediate",
                &leaf_k,
                &inter_k,
                Some(leaf_dns),
                0,
                100000,
                3,
            );
            Pki {
                root_der,
                inter_der,
                leaf_der,
                leaf_key: leaf_k,
            }
        }

        fn make_certificate_msg(chain: &[&[u8]]) -> Vec<u8> {
            let mut list = Vec::new();
            for cert in chain {
                list.push((cert.len() >> 16) as u8);
                list.push((cert.len() >> 8) as u8);
                list.push(cert.len() as u8);
                list.extend_from_slice(cert);
                list.extend_from_slice(&[0x00, 0x00]);
            }
            let mut body = Vec::new();
            body.push(0x00);
            body.push((list.len() >> 16) as u8);
            body.push((list.len() >> 8) as u8);
            body.push(list.len() as u8);
            body.extend_from_slice(&list);
            wrap_handshake(0x0b, &body)
        }

        fn make_cert_verify(leaf_key: &P256Signing, transcript_hash: &[u8]) -> Vec<u8> {
            let mut content = Vec::new();
            content.extend_from_slice(&[0x20u8; 64]);
            content.extend_from_slice(b"TLS 1.3, server CertificateVerify");
            content.push(0x00);
            content.extend_from_slice(transcript_hash);
            let sig: P256Sig = leaf_key.sign(&content);
            let sig_der = sig.to_der();
            let mut body = Vec::new();
            body.extend_from_slice(&0x0403u16.to_be_bytes());
            body.extend_from_slice(&(sig_der.as_bytes().len() as u16).to_be_bytes());
            body.extend_from_slice(sig_der.as_bytes());
            wrap_handshake(0x0f, &body)
        }

        fn make_server_hello(suite: CipherSuite, server_pub: &[u8; 32]) -> Vec<u8> {
            let mut body = Vec::new();
            body.extend_from_slice(&[0x03, 0x03]);
            body.extend_from_slice(&[0x44u8; 32]);
            body.push(0);
            body.extend_from_slice(&suite.as_u16().to_be_bytes());
            body.push(0);
            let mut ext = Vec::new();
            ext.extend_from_slice(&0x002bu16.to_be_bytes());
            ext.extend_from_slice(&2u16.to_be_bytes());
            ext.extend_from_slice(&[0x03, 0x04]);
            let mut ks = Vec::new();
            ks.extend_from_slice(&[0x00, 0x1d, 0x00, 0x20]);
            ks.extend_from_slice(server_pub);
            ext.extend_from_slice(&0x0033u16.to_be_bytes());
            ext.extend_from_slice(&(ks.len() as u16).to_be_bytes());
            ext.extend_from_slice(&ks);
            body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
            body.extend_from_slice(&ext);
            wrap_handshake(0x02, &body)
        }

        fn enc_record(
            suite: CipherSuite,
            key: &[u8],
            iv: &[u8; 12],
            seq: u64,
            msg: &[u8],
            inner_type: u8,
        ) -> Vec<u8> {
            let mut inner = Vec::new();
            inner.extend_from_slice(msg);
            inner.push(inner_type);
            seal_record(suite, key, iv, seq, &inner).expect("seal")
        }

        fn fixed_x25519(seed: u8) -> (StaticSecret, [u8; 32]) {
            let sk = StaticSecret::from([seed; 32]);
            let pk = PublicKey::from(&sk);
            (sk, pk.to_bytes())
        }

        /// Extract the client's x25519 key_share from a ClientHello (header+body).
        fn extract_client_keyshare(ch_msg: &[u8]) -> [u8; 32] {
            let body = &ch_msg[4..];
            let mut i = 2 + 32;
            let sid_len = body[i] as usize;
            i += 1 + sid_len;
            let cs_len = ((body[i] as usize) << 8) | body[i + 1] as usize;
            i += 2 + cs_len;
            let cm_len = body[i] as usize;
            i += 1 + cm_len;
            i += 2; // ext total len
            while i + 4 <= body.len() {
                let etype = ((body[i] as usize) << 8) | body[i + 1] as usize;
                let elen = ((body[i + 2] as usize) << 8) | body[i + 3] as usize;
                let edata = &body[i + 4..i + 4 + elen];
                if etype == 0x0033 {
                    let mut j = 2;
                    while j + 4 <= edata.len() {
                        let group = ((edata[j] as usize) << 8) | edata[j + 1] as usize;
                        let klen = ((edata[j + 2] as usize) << 8) | edata[j + 3] as usize;
                        if group == 0x001d && klen == 32 {
                            let mut k = [0u8; 32];
                            k.copy_from_slice(&edata[j + 4..j + 4 + 32]);
                            return k;
                        }
                        j += 4 + klen;
                    }
                }
                i += 4 + elen;
            }
            panic!("client key_share not found");
        }

        struct ServerCfg {
            cert_msg: Vec<u8>,
            leaf_key: P256Signing,
            tamper_app: bool,
            tamper_finished: bool,
            /// Flip the last byte of the server's CertificateVerify signature so the
            /// ECDSA check over the transcript MUST fail (MITM with a stolen/forged
            /// proof-of-possession). The handshake must fail closed.
            tamper_cert_verify: bool,
        }

        #[derive(PartialEq)]
        enum Stage {
            BeforeReply,
            Replied,
        }

        /// Reactive mock TLS server: on the first `recv` (after the client sent its
        /// ClientHello) it computes the entire server script keyed to the captured
        /// ClientHello, then plays it back.
        struct ReactiveServer {
            stage: Stage,
            outbound: Vec<u8>,
            read_pos: usize,
            captured: Vec<u8>,
            connected: Option<(String, u16)>,
            cfg: ServerCfg,
        }

        impl ReactiveServer {
            fn new(cfg: ServerCfg) -> Self {
                Self {
                    stage: Stage::BeforeReply,
                    outbound: Vec::new(),
                    read_pos: 0,
                    captured: Vec::new(),
                    connected: None,
                    cfg,
                }
            }

            fn build_script(&mut self) {
                let (ch_rec, _used) =
                    TlsRecord::parse(&self.captured).expect("client hello record");
                let ch = ch_rec.fragment.clone();
                let client_pub = extract_client_keyshare(&ch);

                let (s_secret, s_pub) = fixed_x25519(0x5A);
                let suite = CipherSuite::Aes128GcmSha256;
                let sh = make_server_hello(suite, &s_pub);

                let mut ts = Transcript::new();
                ts.update(&ch);
                ts.update(&sh);
                let th_sh = ts.current_hash();
                let shared = s_secret
                    .diffie_hellman(&PublicKey::from(client_pub))
                    .to_bytes();
                let mut ks = KeySchedule::new();
                ks.derive_handshake_secrets(&shared, &th_sh);
                let s_hs = ks.server_handshake_traffic.unwrap();
                let (s_hs_key, s_hs_iv) = traffic_key_iv(&s_hs, suite.key_len());

                let ee = wrap_handshake(0x08, &[0x00, 0x00]);
                ts.update(&ee);
                ts.update(&self.cfg.cert_msg);
                let th_cert = ts.current_hash();
                let mut cv = make_cert_verify(&self.cfg.leaf_key, &th_cert);
                if self.cfg.tamper_cert_verify {
                    // Corrupt the final signature byte: the ECDSA verification over
                    // the transcript will reject, so the client must fail closed
                    // (the server failed to prove possession of the cert's key).
                    let l = cv.len();
                    cv[l - 1] ^= 0xFF;
                }
                ts.update(&cv);
                let th_before_fin = ts.current_hash();
                let fk = finished_key(&s_hs);
                let vd = finished_verify_data(&fk, &th_before_fin);
                let mut fin = wrap_handshake(0x14, &vd);
                if self.cfg.tamper_finished {
                    let l = fin.len();
                    fin[l - 1] ^= 0xFF;
                }
                ts.update(&fin);
                let th_after_sf = ts.current_hash();
                ks.derive_application_secrets(&th_after_sf);

                let mut out = Vec::new();
                out.extend_from_slice(
                    &TlsRecord {
                        content_type: ContentType::Handshake,
                        fragment: sh.clone(),
                    }
                    .to_bytes(),
                );
                let mut seq = 0u64;
                for msg in [&ee, &self.cfg.cert_msg, &cv, &fin] {
                    out.extend_from_slice(&enc_record(
                        suite,
                        &s_hs_key,
                        &s_hs_iv,
                        seq,
                        msg,
                        ContentType::Handshake as u8,
                    ));
                    seq += 1;
                }

                let s_app = ks.server_application_traffic.unwrap();
                let (s_app_key, s_app_iv) = traffic_key_iv(&s_app, suite.key_len());
                let http = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nhello, world!";
                let mut app = enc_record(
                    suite,
                    &s_app_key,
                    &s_app_iv,
                    0,
                    http,
                    ContentType::ApplicationData as u8,
                );
                if self.cfg.tamper_app {
                    app[5] ^= 0xFF; // corrupt first ciphertext byte (after 5-byte header).
                }
                out.extend_from_slice(&app);
                self.outbound = out;
            }
        }

        impl TlsByteTransport for ReactiveServer {
            fn connect(&mut self, host: &str, port: u16) -> Result<(), HttpsError> {
                self.connected = Some((host.to_string(), port));
                Ok(())
            }
            fn send(&mut self, buf: &[u8]) -> Result<(), HttpsError> {
                self.captured.extend_from_slice(buf);
                Ok(())
            }
            fn recv(&mut self, out: &mut [u8]) -> Result<usize, HttpsError> {
                if self.stage == Stage::BeforeReply {
                    self.build_script();
                    self.stage = Stage::Replied;
                }
                if self.read_pos >= self.outbound.len() {
                    return Ok(0);
                }
                let n = core::cmp::min(out.len(), self.outbound.len() - self.read_pos);
                out[..n].copy_from_slice(&self.outbound[self.read_pos..self.read_pos + n]);
                self.read_pos += n;
                Ok(n)
            }
        }

        fn client_with_root(root_der: &[u8]) -> super::super::HttpsClient {
            let mut c = super::super::HttpsClient::new();
            c.add_root_der_anchor(root_der.to_vec());
            c
        }

        const C_SECRET: [u8; 32] = [0x42u8; 32];
        const C_RANDOM: [u8; 32] = [0x77u8; 32];

        fn contains(hay: &[u8], needle: &[u8]) -> bool {
            !needle.is_empty()
                && hay.len() >= needle.len()
                && hay.windows(needle.len()).any(|w| w == needle)
        }

        #[test]
        fn verified_https_fetch_returns_decrypted_body() {
            let pki = build_pki("secure.example", 0x11);
            let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
            let mut server = ReactiveServer::new(ServerCfg {
                cert_msg,
                leaf_key: pki.leaf_key,
                tamper_app: false,
                tamper_finished: false,
                tamper_cert_verify: false,
            });
            let mut client = client_with_root(&pki.root_der);
            let resp = client
                .get_over(
                    "https://secure.example/index.html",
                    &mut server,
                    &C_SECRET,
                    &C_RANDOM,
                    Some(1000),
                )
                .expect("verified fetch must succeed");
            assert_eq!(resp.status, 200);
            assert_eq!(resp.body, b"hello, world!");
            assert_eq!(server.connected, Some(("secure.example".to_string(), 443)));
            // The request went out ENCRYPTED -- not the plaintext GET line.
            assert!(!server.captured.is_empty());
            assert!(
                !contains(&server.captured, b"GET /index.html"),
                "request must be encrypted on the wire"
            );
        }

        #[test]
        fn hostname_mismatch_fails_closed_no_body() {
            let pki = build_pki("other.example", 0x11); // SAN != dialed host
            let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
            let mut server = ReactiveServer::new(ServerCfg {
                cert_msg,
                leaf_key: pki.leaf_key,
                tamper_app: false,
                tamper_finished: false,
                tamper_cert_verify: false,
            });
            let mut client = client_with_root(&pki.root_der);
            let err = client
                .get_over(
                    "https://secure.example/",
                    &mut server,
                    &C_SECRET,
                    &C_RANDOM,
                    Some(1000),
                )
                .unwrap_err();
            assert_eq!(err, HttpsError::HostnameVerificationFailed, "got {:?}", err);
        }

        #[test]
        fn untrusted_root_fails_closed() {
            let pki = build_pki("secure.example", 0x11);
            let other = build_pki("secure.example", 0x40); // different keypairs/root
            let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
            let mut server = ReactiveServer::new(ServerCfg {
                cert_msg,
                leaf_key: pki.leaf_key,
                tamper_app: false,
                tamper_finished: false,
                tamper_cert_verify: false,
            });
            let mut client = client_with_root(&other.root_der); // trusts the WRONG root
            let err = client
                .get_over(
                    "https://secure.example/",
                    &mut server,
                    &C_SECRET,
                    &C_RANDOM,
                    Some(1000),
                )
                .unwrap_err();
            assert!(
                matches!(err, HttpsError::CertificateInvalid(_)),
                "got {:?}",
                err
            );
        }

        #[test]
        fn no_trust_anchors_fails_closed() {
            let pki = build_pki("secure.example", 0x11);
            let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
            let mut server = ReactiveServer::new(ServerCfg {
                cert_msg,
                leaf_key: pki.leaf_key,
                tamper_app: false,
                tamper_finished: false,
                tamper_cert_verify: false,
            });
            let mut client = super::super::HttpsClient::new(); // no anchors
            let err = client
                .get_over(
                    "https://secure.example/",
                    &mut server,
                    &C_SECRET,
                    &C_RANDOM,
                    Some(1000),
                )
                .unwrap_err();
            assert!(matches!(err, HttpsError::CertificateInvalid(_)));
        }

        #[test]
        fn forged_server_finished_fails_closed() {
            let pki = build_pki("secure.example", 0x11);
            let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
            let mut server = ReactiveServer::new(ServerCfg {
                cert_msg,
                leaf_key: pki.leaf_key,
                tamper_app: false,
                tamper_finished: true, // corrupt the server Finished MAC
                tamper_cert_verify: false,
            });
            let mut client = client_with_root(&pki.root_der);
            let err = client
                .get_over(
                    "https://secure.example/",
                    &mut server,
                    &C_SECRET,
                    &C_RANDOM,
                    Some(1000),
                )
                .unwrap_err();
            assert!(
                matches!(err, HttpsError::TlsHandshakeFailed(_)),
                "got {:?}",
                err
            );
        }

        // NEGATIVE (a): a flipped CertificateVerify signature byte. The chain is
        // valid and anchors to the trusted root, the hostname matches, and the
        // server Finished MAC is correct — but the server did NOT prove possession
        // of the leaf's private key (its CertificateVerify signature over the
        // transcript is corrupt). This is the exact MITM shape where an attacker
        // replays a real certificate it does not own the key for. It MUST fail
        // closed with no body.
        #[test]
        fn forged_certificate_verify_fails_closed_no_body() {
            let pki = build_pki("secure.example", 0x11);
            let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
            let mut server = ReactiveServer::new(ServerCfg {
                cert_msg,
                leaf_key: pki.leaf_key,
                tamper_app: false,
                tamper_finished: false,
                tamper_cert_verify: true, // corrupt the CertificateVerify signature
            });
            let mut client = client_with_root(&pki.root_der);
            let result = client.get_over(
                "https://secure.example/",
                &mut server,
                &C_SECRET,
                &C_RANDOM,
                Some(1000),
            );
            // Fail-closed: an Err, and crucially NO HttpsResponse/body escapes.
            let err = result.expect_err("forged CertificateVerify must NOT yield a body");
            assert!(
                matches!(err, HttpsError::TlsHandshakeFailed(_)),
                "a corrupt CertificateVerify must fail the handshake, got {:?}",
                err
            );
        }

        #[test]
        fn tampered_application_record_fails_closed_no_body() {
            let pki = build_pki("secure.example", 0x11);
            let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
            let mut server = ReactiveServer::new(ServerCfg {
                cert_msg,
                leaf_key: pki.leaf_key,
                tamper_app: true, // handshake authenticates; the response record is corrupted
                tamper_finished: false,
                tamper_cert_verify: false,
            });
            let mut client = client_with_root(&pki.root_der);
            let err = client
                .get_over(
                    "https://secure.example/",
                    &mut server,
                    &C_SECRET,
                    &C_RANDOM,
                    Some(1000),
                )
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    HttpsError::IoError(_) | HttpsError::TlsHandshakeFailed(_)
                ),
                "tampered record must error, got {:?}",
                err
            );
        }

        #[test]
        fn http_url_rejected_by_request_over() {
            let mut client = super::super::HttpsClient::new();
            let parsed = super::super::HttpsClient::parse_url("http://insecure.example/").unwrap();
            let req = HttpsRequest::new(HttpMethod::Get, parsed);
            let mut server = ReactiveServer::new(ServerCfg {
                cert_msg: Vec::new(),
                leaf_key: p256_key(1),
                tamper_app: false,
                tamper_finished: false,
                tamper_cert_verify: false,
            });
            let err = client
                .request_over(req, &mut server, &C_SECRET, &C_RANDOM, Some(1000))
                .unwrap_err();
            assert!(matches!(err, HttpsError::InvalidUrl(_)));
        }

        #[test]
        fn record_aad_encodes_application_data_header() {
            assert_eq!(record_aad(0x1234), [0x17, 0x03, 0x03, 0x12, 0x34]);
        }

        #[test]
        fn aead_seal_open_round_trip_and_tamper() {
            let key = [0x09u8; 16];
            let iv = [0x03u8; 12];
            let nonce = record_nonce(&iv, 0);
            let aad = record_aad(32);
            let pt = b"inner record plaintext";
            let mut ct = aead_seal(CipherSuite::Aes128GcmSha256, &key, &nonce, &aad, pt).unwrap();
            assert_eq!(
                aead_open(CipherSuite::Aes128GcmSha256, &key, &nonce, &aad, &ct).as_deref(),
                Some(&pt[..])
            );
            ct[0] ^= 0x01;
            assert!(aead_open(CipherSuite::Aes128GcmSha256, &key, &nonce, &aad, &ct).is_none());
        }

        // -------------------------------------------------------------------
        // Remote-DoS regression tests (criterion #6: attacker-controlled
        // network bytes must never hang or OOM the host). Each of these would
        // HANG or OOM on the pre-fix code, which treated an oversized length
        // header / endless dribble as "need more bytes" and recv'd forever.
        // -------------------------------------------------------------------

        /// A mock byte source for the record layer. It emits a fixed `header`
        /// prefix once, then on every subsequent `recv` either dribbles
        /// `dribble` bytes (a) or returns `Ok(0)` / nothing (b). `calls` is
        /// capped so that on the OLD code an infinite recv loop is OBSERVABLE
        /// as a panic (the test fails loudly) instead of actually hanging CI.
        struct HostileSource {
            header: Vec<u8>,
            header_sent: bool,
            dribble_byte: Option<u8>, // Some => emit one byte/call; None => Ok(0)
            calls: usize,
            max_calls: usize,
        }

        impl HostileSource {
            fn new(header: Vec<u8>, dribble_byte: Option<u8>) -> Self {
                Self {
                    header,
                    header_sent: false,
                    dribble_byte,
                    calls: 0,
                    // Far more than any bounded fix needs; an unbounded old-code
                    // loop blows past this and panics → test fails (not hangs).
                    max_calls: 1_000_000,
                }
            }
        }

        impl TlsByteTransport for HostileSource {
            fn connect(&mut self, _h: &str, _p: u16) -> Result<(), HttpsError> {
                Ok(())
            }
            fn send(&mut self, _b: &[u8]) -> Result<(), HttpsError> {
                Ok(())
            }
            fn recv(&mut self, out: &mut [u8]) -> Result<usize, HttpsError> {
                self.calls += 1;
                assert!(
                    self.calls <= self.max_calls,
                    "hostile source recv'd {} times — unbounded loop (OLD-CODE BUG)",
                    self.calls
                );
                if !self.header_sent {
                    self.header_sent = true;
                    let n = core::cmp::min(out.len(), self.header.len());
                    out[..n].copy_from_slice(&self.header[..n]);
                    return Ok(n);
                }
                match self.dribble_byte {
                    Some(b) if !out.is_empty() => {
                        out[0] = b;
                        Ok(1)
                    }
                    _ => Ok(0), // peer sends nothing further.
                }
            }
        }

        // HIGH (a): oversized length header [0x16,0x03,0x03,0xFF,0xFF] (decodes
        // to 65535 > MAX_RECORD_LEN) followed by an endless dribble. On the OLD
        // code parse() returned None == "need more", so read_one_record looped
        // forever growing in_buf (OOM). Now it is a HARD reject on the header.
        #[test]
        fn oversized_record_header_then_dribble_errs_bounded() {
            let header = vec![0x16, 0x03, 0x03, 0xFF, 0xFF];
            let mut src = HostileSource::new(header, Some(0xAB));
            let mut in_buf: Vec<u8> = Vec::new();
            let r = read_one_record(&mut src, &mut in_buf);
            assert!(
                matches!(r, Err(HttpsError::TlsHandshakeFailed(_))),
                "oversized header must hard-error, got {:?}",
                r
            );
            // Bounded: the Invalid is detected from the 5-byte header alone,
            // before ANY dribble byte is buffered.
            assert!(
                in_buf.len() <= MAX_IN_BUF,
                "in_buf must stay bounded, was {}",
                in_buf.len()
            );
        }

        // HIGH (b): same oversized header, then the peer sends NOTHING. OLD code
        // would block forever (parse None → recv → Ok(0)?). The header itself is
        // invalid so we error immediately without waiting on more bytes.
        #[test]
        fn oversized_record_header_then_silence_errs() {
            let header = vec![0x16, 0x03, 0x03, 0xFF, 0xFF];
            let mut src = HostileSource::new(header, None);
            let mut in_buf: Vec<u8> = Vec::new();
            let r = read_one_record(&mut src, &mut in_buf);
            assert!(
                matches!(r, Err(HttpsError::TlsHandshakeFailed(_))),
                "oversized header + silence must error, got {:?}",
                r
            );
        }

        // A legal MAX_RECORD_LEN record dribbled one body byte per recv must
        // TERMINATE (the record is returned) in bounded memory — proving the
        // dribble path doesn't hang for a valid-but-slow peer, and that in_buf
        // never grows past one record framing while assembling.
        #[test]
        fn legal_max_length_dribble_completes_bounded() {
            let header = vec![0x16, 0x03, 0x03, 0x41, 0x00]; // len 16640 = MAX_RECORD_LEN
            let mut src = HostileSource::new(header, Some(0x00));
            let mut in_buf: Vec<u8> = Vec::new();
            let r = read_one_record(&mut src, &mut in_buf);
            assert!(
                matches!(&r, Ok(rec) if rec.fragment.len() == MAX_RECORD_LEN),
                "legal max-length dribble must complete in bounded memory, got {:?}",
                r
            );
            assert!(
                in_buf.len() <= MAX_IN_BUF,
                "in_buf must stay bounded, was {}",
                in_buf.len()
            );
        }

        // The MAX_IN_BUF backstop, exercised directly via pump_one_record-style
        // accumulation: simulate the streaming layer buffering raw bytes that
        // never parse into a record and assert the loop bails at the cap. We use
        // read_one_record over a source that, after a legal-but-just-over-cap
        // header, would otherwise be retried forever on the OLD code; the cap +
        // the Invalid signal both stop it. (Header length 16641 > MAX_RECORD_LEN
        // is Invalid; this is the precise off-by-one above the raised cap.)
        #[test]
        fn length_one_over_cap_is_hard_rejected_no_body_buffered() {
            let header = vec![0x16, 0x03, 0x03, 0x41, 0x01]; // len 16641 > MAX_RECORD_LEN
            let mut src = HostileSource::new(header, Some(0x00));
            let mut in_buf: Vec<u8> = Vec::new();
            let r = read_one_record(&mut src, &mut in_buf);
            assert!(
                matches!(r, Err(HttpsError::TlsHandshakeFailed(_))),
                "length one over the cap must hard-error, got {:?}",
                r
            );
            assert!(
                in_buf.len() <= 5,
                "rejected from the 5-byte header alone, no body buffered, was {}",
                in_buf.len()
            );
        }

        // The raised cap (RFC 8446 §5.2 TLSCiphertext max = 2^14 + 256): a
        // record of length exactly 16640 parses; 16641 is Invalid.
        #[test]
        fn record_length_boundary_16640_accepted_16641_rejected() {
            // Exactly MAX_RECORD_LEN = 16640.
            let mut buf = vec![0x17, 0x03, 0x03, 0x41, 0x00]; // 0x4100 = 16640
            buf.extend(core::iter::repeat(0u8).take(16640));
            match TlsRecord::parse_state(&buf) {
                RecordParse::Record(r, used) => {
                    assert_eq!(r.fragment.len(), 16640);
                    assert_eq!(used, 5 + 16640);
                }
                other => panic!("16640 must parse as a record, got {:?}", other),
            }
            // 16641 > MAX_RECORD_LEN → Invalid (HARD reject, not NeedMore).
            let too_big = vec![0x17, 0x03, 0x03, 0x41, 0x01]; // 0x4101 = 16641
            assert_eq!(TlsRecord::parse_state(&too_big), RecordParse::Invalid);
            // And a short buffer that COULD still be NeedMore stays NeedMore.
            assert_eq!(TlsRecord::parse_state(&[0x17, 0x03]), RecordParse::NeedMore);
        }

        // MED: a flight of many valid-framed (but empty/garbage-inner) handshake
        // records before the 4 messages assemble must be bounded by the record
        // cap. We exercise the bound via a server that streams CCS records (which
        // decrypt_handshake_record skips) forever — read_one_record returns them,
        // the flight loop counts records, and bails past MAX_HS_FLIGHT_RECORDS.
        // Here we test the analogous bound directly on the record-count guard by
        // counting how many records read_one_record yields from a capped source.
        #[test]
        fn many_records_source_is_bounded_per_call() {
            // A source emitting back-to-back tiny valid ChangeCipherSpec records.
            // Each is [0x14,0x03,0x03,0x00,0x01,0x01] (len 1, body 0x01).
            struct CcsFlood {
                emitted: usize,
            }
            impl TlsByteTransport for CcsFlood {
                fn connect(&mut self, _h: &str, _p: u16) -> Result<(), HttpsError> {
                    Ok(())
                }
                fn send(&mut self, _b: &[u8]) -> Result<(), HttpsError> {
                    Ok(())
                }
                fn recv(&mut self, out: &mut [u8]) -> Result<usize, HttpsError> {
                    self.emitted += 1;
                    assert!(self.emitted < 10_000, "CcsFlood recv unbounded");
                    let rec = [0x14u8, 0x03, 0x03, 0x00, 0x01, 0x01];
                    let n = core::cmp::min(out.len(), rec.len());
                    out[..n].copy_from_slice(&rec[..n]);
                    Ok(n)
                }
            }
            let mut src = CcsFlood { emitted: 0 };
            let mut in_buf: Vec<u8> = Vec::new();
            // Read MAX_HS_FLIGHT_RECORDS records: each is a complete CCS record,
            // so read_one_record returns Ok and in_buf never grows unbounded.
            for _ in 0..MAX_HS_FLIGHT_RECORDS {
                let rec = read_one_record(&mut src, &mut in_buf)
                    .expect("each CCS record must parse cleanly");
                assert_eq!(rec.content_type, ContentType::ChangeCipherSpec);
                assert!(in_buf.len() <= MAX_IN_BUF);
            }
        }
    }
}
