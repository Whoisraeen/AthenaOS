//! Live TCP/DNS transport adapter for the HTTP/1.1 client.
//!
//! RaeenOS Concept §RaeNet / "web apps that feel native": `http1.rs` is the
//! pure, host-KAT'd protocol engine; this module is the thin bridge that makes
//! it talk to a REAL socket. It implements [`http1::HttpTransport`] over the
//! userspace socket syscalls (`SYS_NET_SOCKET`/`CONNECT`/`SEND`/`RECV`/`CLOSE`
//! 121-125 + `SYS_NET_DNS` 264 — see `docs/SYSCALL_TABLE.md`).
//!
//! Why a backend trait instead of calling `raekit::sys` directly:
//! * raenet links into the **kernel** (no_std) and into the **host test**
//!   harness (std). raekit installs a `#[global_allocator]` + `#[panic_handler]`
//!   for a userspace ELF and emits the raw `syscall` instruction — neither is
//!   valid in those two link contexts. Pulling raekit in here would break both.
//! * So the *capability* (fd-based connect/send/recv/close + DNS resolve) is
//!   abstracted behind [`SocketSyscalls`]. A userspace app/daemon provides the
//!   real impl in three lines (forwarding to `raekit::sys::{tcp_connect,
//!   sock_send, sock_recv, sock_close, dns_resolve}` — see the doc example),
//!   while the host KATs drive a deterministic in-memory mock. The protocol is
//!   already proven (25 KATs in `http1.rs`); this adds the wiring proof.
//!
//! The result: `raenet::http_get(url, &backend)` performs a real `http://`
//! fetch end-to-end once a live NIC + DHCP lease exist (the live fetch itself is
//! network/iron-gated; the adapter logic is host-proven against the mock).

extern crate alloc;

use alloc::string::ToString;

use crate::http1::{self, Http1Error, Http1Response, Http1Result, HttpTransport, Limits};

/// The minimal fd-based socket capability the transport needs. A userspace
/// caller implements this by forwarding to the raekit wrappers; the host KATs
/// implement it with an in-memory mock.
///
/// ```ignore
/// // In a userspace app/daemon that links raekit:
/// struct RaekitSyscalls;
/// impl raenet::socket_transport::SocketSyscalls for RaekitSyscalls {
///     fn dns_resolve(&self, host: &str) -> Option<[u8; 4]> {
///         raekit::sys::dns_resolve(host)
///     }
///     fn tcp_connect(&self, ip: [u8; 4], port: u16) -> Option<u64> {
///         raekit::sys::tcp_connect(ip, port)
///     }
///     fn send(&self, fd: u64, buf: &[u8]) -> isize { raekit::sys::sock_send(fd, buf) }
///     fn recv(&self, fd: u64, buf: &mut [u8]) -> isize { raekit::sys::sock_recv(fd, buf) }
///     fn close(&self, fd: u64) { raekit::sys::sock_close(fd) }
/// }
/// ```
pub trait SocketSyscalls {
    /// Resolve `host` to an IPv4 (`SYS_NET_DNS`). `None` on failure.
    fn dns_resolve(&self, host: &str) -> Option<[u8; 4]>;
    /// Open a TCP socket + connect to `ip:port` (`SYS_NET_SOCKET` + `CONNECT`).
    /// Returns the connected fd, or `None`.
    fn tcp_connect(&self, ip: [u8; 4], port: u16) -> Option<u64>;
    /// Send on `fd` (`SYS_NET_SEND`). Bytes accepted (may be short), or `-1`.
    fn send(&self, fd: u64, buf: &[u8]) -> isize;
    /// Receive on `fd` (`SYS_NET_RECV`). Bytes read (`0` = none yet), or `-1`.
    fn recv(&self, fd: u64, buf: &mut [u8]) -> isize;
    /// Close `fd` (`SYS_NET_CLOSE`). Idempotent.
    fn close(&self, fd: u64);
}

/// A live HTTP transport: DNS-resolves the host, opens a TCP connection, and
/// streams bytes through the [`SocketSyscalls`] backend. Closes its fd on drop
/// so a dropped/failed fetch never leaks a socket.
///
/// `recv` returning `0` from the kernel means "no data available *yet*" (non-
/// blocking), NOT EOF — so the transport spins a bounded number of times,
/// yielding via the backend, before treating a persistent `0` as a closed peer.
/// This keeps `http1::send_request`'s EOF-framing contract intact without a
/// blocking-recv syscall.
pub struct RaeSocketTransport<'a, S: SocketSyscalls> {
    sys: &'a S,
    fd: Option<u64>,
    /// Max consecutive empty `recv`s tolerated before declaring EOF. Bounds the
    /// spin so a wedged peer cannot hang the fetch forever.
    pub max_idle_polls: u32,
}

impl<'a, S: SocketSyscalls> RaeSocketTransport<'a, S> {
    /// Build a transport over `sys`. Not connected until [`HttpTransport::connect`]
    /// runs (driven by `http1::fetch`/`fetch_with`).
    pub fn new(sys: &'a S) -> Self {
        Self {
            sys,
            fd: None,
            max_idle_polls: 4096,
        }
    }

    /// Override the idle-poll bound (default 4096). Lower = give up sooner on a
    /// silent peer; higher = tolerate more latency before declaring EOF.
    pub fn with_max_idle_polls(mut self, n: u32) -> Self {
        self.max_idle_polls = n.max(1);
        self
    }

    /// The connected fd, if any (test/introspection aid).
    pub fn fd(&self) -> Option<u64> {
        self.fd
    }
}

impl<'a, S: SocketSyscalls> HttpTransport for RaeSocketTransport<'a, S> {
    fn connect(&mut self, host: &str, port: u16) -> Http1Result<()> {
        // Accept a literal dotted-quad host without a DNS round-trip; otherwise
        // resolve via SYS_NET_DNS.
        let ip = match parse_dotted_quad(host) {
            Some(ip) => ip,
            None => self
                .sys
                .dns_resolve(host)
                .ok_or_else(|| Http1Error::Transport("dns resolution failed".to_string()))?,
        };

        let fd = self
            .sys
            .tcp_connect(ip, port)
            .ok_or_else(|| Http1Error::Transport("tcp connect failed".to_string()))?;
        // If a prior fd was open (reused transport), close it first.
        if let Some(old) = self.fd.replace(fd) {
            self.sys.close(old);
        }
        Ok(())
    }

    fn send(&mut self, buf: &[u8]) -> Http1Result<()> {
        let fd = self
            .fd
            .ok_or_else(|| Http1Error::Transport("send before connect".to_string()))?;
        let mut off = 0usize;
        // Loop on short writes; a persistent zero-progress write is a hard error
        // (bounded so we never spin forever on a dead socket).
        let mut stalls = 0u32;
        while off < buf.len() {
            let n = self.sys.send(fd, &buf[off..]);
            if n < 0 {
                return Err(Http1Error::Transport("send failed".to_string()));
            }
            if n == 0 {
                stalls += 1;
                if stalls > self.max_idle_polls {
                    return Err(Http1Error::Transport("send stalled".to_string()));
                }
                continue;
            }
            stalls = 0;
            off += n as usize;
        }
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Http1Result<usize> {
        let fd = self
            .fd
            .ok_or_else(|| Http1Error::Transport("recv before connect".to_string()))?;
        // Non-blocking kernel recv returns 0 for "nothing yet". Spin a bounded
        // number of times so a slow-but-live peer is read fully, then treat a
        // persistent 0 as EOF (peer closed) — exactly the signal
        // `http1::send_request` needs to finish an EOF-framed body.
        let mut idle = 0u32;
        loop {
            let n = self.sys.recv(fd, buf);
            if n < 0 {
                return Err(Http1Error::Transport("recv failed".to_string()));
            }
            if n == 0 {
                idle += 1;
                if idle >= self.max_idle_polls {
                    return Ok(0); // EOF: peer is done.
                }
                continue;
            }
            return Ok(n as usize);
        }
    }
}

impl<'a, S: SocketSyscalls> Drop for RaeSocketTransport<'a, S> {
    fn drop(&mut self) {
        if let Some(fd) = self.fd.take() {
            self.sys.close(fd);
        }
    }
}

/// Parse a literal `a.b.c.d` IPv4 (no DNS). Returns `None` for anything that is
/// not exactly four `0..=255` decimal octets — a real hostname falls through to
/// DNS resolution. NEVER panics.
fn parse_dotted_quad(host: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut parts = 0usize;
    for (i, part) in host.split('.').enumerate() {
        if i >= 4 || part.is_empty() || part.len() > 3 {
            return None;
        }
        let mut val: u16 = 0;
        for b in part.bytes() {
            if !b.is_ascii_digit() {
                return None;
            }
            val = val * 10 + (b - b'0') as u16;
        }
        if val > 255 {
            return None;
        }
        octets[i] = val as u8;
        parts = i + 1;
    }
    if parts == 4 {
        Some(octets)
    } else {
        None
    }
}

/// One-shot `http://` GET over a live socket backend. Resolves + connects via
/// `sys`, sends the request, and returns the parsed response. The high-level
/// entry point a browser/app calls.
///
/// HTTPS is a deferred follow-up (the `tls13` feature has the crypto, but a
/// handshake-driving transport is out of scope here) — this is `http://` only,
/// mirroring `http1::fetch`.
pub fn http_get<S: SocketSyscalls>(url: &str, sys: &S) -> Http1Result<Http1Response> {
    let mut transport = RaeSocketTransport::new(sys);
    http1::fetch(url, &mut transport)
}

/// `http_get` with caller-chosen response [`Limits`] (a hostile-peer hardening
/// knob — bound headers/body for an untrusted server).
pub fn http_get_with<S: SocketSyscalls>(
    url: &str,
    sys: &S,
    limits: &Limits,
) -> Http1Result<Http1Response> {
    let mut transport = RaeSocketTransport::new(sys);
    http1::fetch_with(url, http1::Method::Get, None, &[], &mut transport, limits)
}

// ---------------------------------------------------------------------------
// Host KATs — wiring proof against a deterministic mock backend (FAIL-able).
// The protocol itself is proven in http1.rs; THESE prove the transport adapter:
// DNS-then-connect ordering, the BE-octet path, short-write send loop, the
// non-blocking recv→EOF spin, and fd-close-on-drop.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    /// A scripted in-memory backend. Records the resolve/connect/close calls and
    /// drip-feeds a canned response, optionally returning a few empty `recv`s
    /// first to exercise the non-blocking spin.
    struct MockSys {
        resolve_to: Option<[u8; 4]>,
        connect_ok: bool,
        inner: RefCell<Inner>,
    }
    struct Inner {
        sent: Vec<u8>,
        response: Vec<u8>,
        read_pos: usize,
        /// Empty recvs to emit before delivering data (models "nothing yet").
        empties_before_data: u32,
        empties_seen: u32,
        connected_to: Option<([u8; 4], u16)>,
        resolved_host: Option<String>,
        closed_fds: Vec<u64>,
        open_fd: Option<u64>,
        recv_chunk: usize,
    }

    impl MockSys {
        fn new(response: Vec<u8>) -> Self {
            Self {
                resolve_to: Some([93, 184, 216, 34]),
                connect_ok: true,
                inner: RefCell::new(Inner {
                    sent: Vec::new(),
                    response,
                    read_pos: 0,
                    empties_before_data: 0,
                    empties_seen: 0,
                    connected_to: None,
                    resolved_host: None,
                    closed_fds: Vec::new(),
                    open_fd: None,
                    recv_chunk: usize::MAX,
                }),
            }
        }
        fn with_empties(self, n: u32) -> Self {
            self.inner.borrow_mut().empties_before_data = n;
            self
        }
        fn with_recv_chunk(self, c: usize) -> Self {
            self.inner.borrow_mut().recv_chunk = c.max(1);
            self
        }
    }

    impl SocketSyscalls for MockSys {
        fn dns_resolve(&self, host: &str) -> Option<[u8; 4]> {
            self.inner.borrow_mut().resolved_host = Some(host.to_string());
            self.resolve_to
        }
        fn tcp_connect(&self, ip: [u8; 4], port: u16) -> Option<u64> {
            if !self.connect_ok {
                return None;
            }
            let mut inner = self.inner.borrow_mut();
            inner.connected_to = Some((ip, port));
            inner.open_fd = Some(7);
            Some(7)
        }
        fn send(&self, _fd: u64, buf: &[u8]) -> isize {
            // Short-write model: accept at most 5 bytes per call to exercise the
            // send loop.
            let n = buf.len().min(5);
            self.inner.borrow_mut().sent.extend_from_slice(&buf[..n]);
            n as isize
        }
        fn recv(&self, _fd: u64, buf: &mut [u8]) -> isize {
            let mut inner = self.inner.borrow_mut();
            if inner.empties_seen < inner.empties_before_data {
                inner.empties_seen += 1;
                return 0;
            }
            let remaining = inner.response.len().saturating_sub(inner.read_pos);
            if remaining == 0 {
                return 0; // EOF after data drained.
            }
            let n = remaining.min(buf.len()).min(inner.recv_chunk);
            let start = inner.read_pos;
            buf[..n].copy_from_slice(&inner.response[start..start + n]);
            inner.read_pos += n;
            n as isize
        }
        fn close(&self, fd: u64) {
            let mut inner = self.inner.borrow_mut();
            inner.closed_fds.push(fd);
            if inner.open_fd == Some(fd) {
                inner.open_fd = None;
            }
        }
    }

    #[test]
    fn dotted_quad_parse_accepts_and_rejects() {
        assert_eq!(parse_dotted_quad("10.0.0.1"), Some([10, 0, 0, 1]));
        assert_eq!(
            parse_dotted_quad("255.255.255.255"),
            Some([255, 255, 255, 255])
        );
        assert_eq!(parse_dotted_quad("0.0.0.0"), Some([0, 0, 0, 0]));
        // Rejected: hostnames, out-of-range, wrong part count, empty parts.
        assert_eq!(parse_dotted_quad("example.com"), None);
        assert_eq!(parse_dotted_quad("256.0.0.1"), None);
        assert_eq!(parse_dotted_quad("1.2.3"), None);
        assert_eq!(parse_dotted_quad("1.2.3.4.5"), None);
        assert_eq!(parse_dotted_quad("1..3.4"), None);
        assert_eq!(parse_dotted_quad("1.2.3.x"), None);
        assert_eq!(parse_dotted_quad(""), None);
    }

    #[test]
    fn http_get_round_trip_with_dns() {
        let canned = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".to_vec();
        let sys = MockSys::new(canned);
        let resp = http_get("http://example.com:8080/greet", &sys).unwrap();

        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello world");
        // DNS was consulted for the hostname, then we connected to the resolved
        // IP on the right port.
        let inner = sys.inner.borrow();
        assert_eq!(inner.resolved_host.as_deref(), Some("example.com"));
        assert_eq!(inner.connected_to, Some(([93, 184, 216, 34], 8080)));
        // The request bytes are well-formed.
        let sent = String::from_utf8_lossy(&inner.sent);
        assert!(sent.starts_with("GET /greet HTTP/1.1\r\n"));
        assert!(sent.contains("Host: example.com:8080\r\n"));
    }

    #[test]
    fn http_get_literal_ip_skips_dns() {
        let canned = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec();
        let sys = MockSys::new(canned);
        let resp = http_get("http://10.0.0.5/health", &sys).unwrap();
        assert_eq!(resp.status, 204);
        let inner = sys.inner.borrow();
        // No DNS for a literal dotted-quad; connected straight to it on port 80.
        assert_eq!(inner.resolved_host, None);
        assert_eq!(inner.connected_to, Some(([10, 0, 0, 5], 80)));
    }

    #[test]
    fn recv_spins_through_empty_polls_then_reads() {
        // Backend returns 3 empty recvs ("nothing yet") before delivering — the
        // transport must keep polling, not treat the first 0 as EOF.
        let canned = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata".to_vec();
        let sys = MockSys::new(canned).with_empties(3).with_recv_chunk(2);
        let resp = http_get("http://1.1.1.1/", &sys).unwrap();
        assert_eq!(resp.body, b"data");
    }

    #[test]
    fn eof_framed_body_then_persistent_empty_recv_is_eof() {
        // No Content-Length, no chunked → body is framed by EOF. The whole
        // response arrives in one delivery (matching http1's EOF contract:
        // `send_request` completes an EOF-framed body once the peer is read);
        // the backend then returns 0 forever and the bounded idle spin reports
        // EOF rather than hanging.
        let canned = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nstreamed body".to_vec();
        let sys = MockSys::new(canned); // default recv_chunk = whole buffer
        let mut t = RaeSocketTransport::new(&sys).with_max_idle_polls(8);
        let r = http1::fetch("http://2.2.2.2/", &mut t).unwrap();
        assert_eq!(r.body, b"streamed body");
        // And the idle spin terminated (did not hang): we got here.
    }

    #[test]
    fn recv_returns_eof_after_bounded_idle_spin() {
        // Direct transport-level proof of the non-blocking-recv → EOF bound:
        // a backend that ALWAYS returns 0 must make `recv` return Ok(0) (EOF)
        // after exactly `max_idle_polls` polls, never spinning forever.
        let sys = MockSys::new(vec![]).with_empties(u32::MAX); // never delivers
        let mut t = RaeSocketTransport::new(&sys).with_max_idle_polls(3);
        // Manually connect (literal IP, no DNS) then recv.
        t.connect("9.9.9.9", 80).unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(t.recv(&mut buf).unwrap(), 0, "persistent empty recv => EOF");
    }

    #[test]
    fn send_loop_handles_short_writes() {
        // The mock accepts only 5 bytes/send; a >5-byte request must still be
        // fully delivered by the send loop.
        let canned = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec();
        let sys = MockSys::new(canned);
        let _ = http_get("http://3.3.3.3/some/long/path?with=query", &sys).unwrap();
        let inner = sys.inner.borrow();
        let sent = String::from_utf8_lossy(&inner.sent);
        // The whole request line + headers arrived despite 5-byte chunking.
        assert!(sent.starts_with("GET /some/long/path?with=query HTTP/1.1\r\n"));
        assert!(sent.ends_with("\r\n\r\n"));
    }

    #[test]
    fn dns_failure_is_transport_error_not_panic() {
        let mut bad = MockSys::new(vec![]);
        bad.resolve_to = None;
        let err = http_get("http://nxdomain.invalid/", &bad).unwrap_err();
        match err {
            Http1Error::Transport(_) => {}
            other => panic!("expected Transport err, got {:?}", other),
        }
    }

    #[test]
    fn connect_failure_is_transport_error() {
        let mut sys = MockSys::new(vec![]);
        sys.connect_ok = false;
        let err = http_get("http://4.4.4.4/", &sys).unwrap_err();
        assert!(matches!(err, Http1Error::Transport(_)));
    }

    #[test]
    fn fd_closed_on_drop() {
        let canned = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec();
        let sys = MockSys::new(canned);
        {
            let _ = http_get("http://5.5.5.5/", &sys).unwrap();
        }
        // After the transport dropped, the fd it opened (7) was closed.
        assert!(sys.inner.borrow().closed_fds.contains(&7));
        assert_eq!(sys.inner.borrow().open_fd, None);
    }
}
