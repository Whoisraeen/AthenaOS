extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsError {
    NotConnected,
    AlreadyConnected,
    InvalidState(String),
    HandshakeFailed(String),
    ProtocolError(String),
    FrameTooLarge(usize),
    MessageTooLarge(usize),
    InvalidUtf8,
    InvalidCloseCode(u16),
    ConnectionClosed,
    IoError(String),
}

// ---------------------------------------------------------------------------
// WebSocket state and roles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsState {
    Connecting,
    Open,
    Closing,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsRole {
    Client,
    Server,
}

// ---------------------------------------------------------------------------
// Frame types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WsOpcode {
    Continuation = 0,
    Text = 1,
    Binary = 2,
    Close = 8,
    Ping = 9,
    Pong = 10,
}

impl WsOpcode {
    pub fn from_u8(val: u8) -> Result<Self, WsError> {
        match val {
            0 => Ok(Self::Continuation),
            1 => Ok(Self::Text),
            2 => Ok(Self::Binary),
            8 => Ok(Self::Close),
            9 => Ok(Self::Ping),
            10 => Ok(Self::Pong),
            _ => Err(WsError::ProtocolError(format!("unknown opcode: {}", val))),
        }
    }

    pub fn is_control(&self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }

    pub fn is_data(&self) -> bool {
        matches!(self, Self::Text | Self::Binary | Self::Continuation)
    }
}

#[derive(Debug, Clone)]
pub struct WsFrame {
    pub fin: bool,
    pub opcode: WsOpcode,
    pub mask: bool,
    pub masking_key: Option<[u8; 4]>,
    pub payload: Vec<u8>,
}

impl WsFrame {
    pub fn text(data: &str, mask: bool, masking_key: Option<[u8; 4]>) -> Self {
        Self {
            fin: true,
            opcode: WsOpcode::Text,
            mask,
            masking_key,
            payload: data.as_bytes().to_vec(),
        }
    }

    pub fn binary(data: &[u8], mask: bool, masking_key: Option<[u8; 4]>) -> Self {
        Self {
            fin: true,
            opcode: WsOpcode::Binary,
            mask,
            masking_key,
            payload: data.to_vec(),
        }
    }

    pub fn ping(data: &[u8]) -> Self {
        Self {
            fin: true,
            opcode: WsOpcode::Ping,
            mask: false,
            masking_key: None,
            payload: data.to_vec(),
        }
    }

    pub fn pong(data: &[u8]) -> Self {
        Self {
            fin: true,
            opcode: WsOpcode::Pong,
            mask: false,
            masking_key: None,
            payload: data.to_vec(),
        }
    }

    pub fn close(code: u16, reason: &str) -> Self {
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.push((code >> 8) as u8);
        payload.push((code & 0xFF) as u8);
        payload.extend_from_slice(reason.as_bytes());
        Self {
            fin: true,
            opcode: WsOpcode::Close,
            mask: false,
            masking_key: None,
            payload,
        }
    }
}

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsMessageType {
    Text,
    Binary,
}

#[derive(Debug, Clone)]
pub struct WsMessage {
    pub message_type: WsMessageType,
    pub data: Vec<u8>,
    pub timestamp: u64,
}

impl WsMessage {
    pub fn as_text(&self) -> Option<&str> {
        if self.message_type == WsMessageType::Text {
            core::str::from_utf8(&self.data).ok()
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Fragment buffer and extensions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FragmentBuffer {
    pub opcode: WsOpcode,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct WsExtension {
    pub name: String,
    pub params: Vec<(String, Option<String>)>,
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct WsStats {
    pub messages_sent: u64,
    pub messages_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub pings_sent: u64,
    pub pongs_received: u64,
    pub frames_sent: u64,
    pub frames_received: u64,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum WsEvent {
    Connected,
    Message(WsMessage),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(u16, String),
    Error(WsError),
}

// ---------------------------------------------------------------------------
// WebSocket implementation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WebSocket {
    pub state: WsState,
    pub role: WsRole,
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub extensions: Vec<WsExtension>,
    pub subprotocol: Option<String>,
    send_queue: Vec<WsFrame>,
    recv_queue: Vec<WsMessage>,
    fragment_buffer: Option<FragmentBuffer>,
    ping_interval_ms: Option<u64>,
    last_ping: u64,
    last_pong: u64,
    close_code: Option<u16>,
    close_reason: Option<String>,
    max_frame_size: usize,
    max_message_size: usize,
    stats: WsStats,
    /// Explicit per-frame masking-key OVERRIDE (tests / deterministic replay). When
    /// `None`, the client derives a fresh key per frame from `rng_state`.
    masking_key: Option<[u8; 4]>,
    /// The client's generated `Sec-WebSocket-Key` (base64 nonce), retained so
    /// [`WebSocket::complete_handshake`] can verify the server's `Accept`.
    sec_key: Option<String>,
    /// PRNG state for masking keys + the handshake nonce. Seed it from a real
    /// entropy source via [`WebSocket::set_entropy_seed`] in production; the fixed
    /// default still yields a DIFFERENT key per frame (no static-key RFC violation),
    /// just reproducible until seeded.
    rng_state: u64,
    /// Raw handshake-request bytes queued by `connect`, emitted first by
    /// [`WebSocket::get_outgoing`] (they are HTTP bytes, not a WS frame).
    pending_handshake: Option<Vec<u8>>,
}

/// Case-insensitive header lookup in a raw HTTP response. The header NAME match
/// is case-insensitive; the returned value keeps its original case (the base64
/// `Sec-WebSocket-Accept` value is case-sensitive), OWS-trimmed. Never panics.
fn header_value_ci<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    for line in response.lines() {
        if let Some(colon) = line.find(':') {
            let (hname, hval) = line.split_at(colon);
            if hname.trim().eq_ignore_ascii_case(name) {
                return Some(hval[1..].trim());
            }
        }
    }
    None
}

/// Offset just past the `\r\n\r\n` HTTP header terminator, if present. Never panics.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let mut i = 0;
    while i + 4 <= buf.len() {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i + 4);
        }
        i += 1;
    }
    None
}

impl WebSocket {
    pub fn new_client() -> Self {
        Self {
            state: WsState::Closed,
            role: WsRole::Client,
            url: None,
            headers: Vec::new(),
            extensions: Vec::new(),
            subprotocol: None,
            send_queue: Vec::new(),
            recv_queue: Vec::new(),
            fragment_buffer: None,
            ping_interval_ms: Some(30_000),
            last_ping: 0,
            last_pong: 0,
            close_code: None,
            close_reason: None,
            max_frame_size: 64 * 1024,
            max_message_size: 1024 * 1024,
            stats: WsStats::default(),
            masking_key: None,
            sec_key: None,
            rng_state: 0x9E37_79B9_7F4A_7C15,
            pending_handshake: None,
        }
    }

    pub fn new_server() -> Self {
        Self {
            state: WsState::Closed,
            role: WsRole::Server,
            url: None,
            headers: Vec::new(),
            extensions: Vec::new(),
            subprotocol: None,
            send_queue: Vec::new(),
            recv_queue: Vec::new(),
            fragment_buffer: None,
            ping_interval_ms: Some(30_000),
            last_ping: 0,
            last_pong: 0,
            close_code: None,
            close_reason: None,
            max_frame_size: 64 * 1024,
            max_message_size: 1024 * 1024,
            stats: WsStats::default(),
            masking_key: None,
            sec_key: None,
            rng_state: 0xD1B5_4A32_D192_ED03,
            pending_handshake: None,
        }
    }

    /// Seed the masking-key / handshake-nonce PRNG from a real entropy source
    /// (e.g. a `getrandom` syscall) so client masking keys are unpredictable per
    /// RFC 6455 5.3. Without this the keys still vary per frame but are
    /// reproducible -- fine for tests, NOT for a production client facing a proxy.
    pub fn set_entropy_seed(&mut self, seed: u64) {
        self.rng_state = seed | 1;
    }

    pub fn connect(&mut self, url: &str, protocols: &[&str]) -> Result<(), WsError> {
        if self.state != WsState::Closed {
            return Err(WsError::AlreadyConnected);
        }

        self.state = WsState::Connecting;
        self.url = Some(String::from(url));

        self.send_queue.clear();
        self.recv_queue.clear();
        self.fragment_buffer = None;

        // Build the upgrade request (generates + stores a fresh Sec-WebSocket-Key)
        // and queue its bytes for the transport to send first.
        let handshake = self.build_handshake_request(url, protocols);
        self.pending_handshake = Some(handshake);

        Ok(())
    }

    /// Feed the server's HTTP upgrade response to complete the client handshake.
    /// Verifies the `101` status, the `Upgrade`/`Connection` headers, AND that
    /// `Sec-WebSocket-Accept` equals `base64(SHA1(our_key + GUID))` -- proving the
    /// peer is a real RFC 6455 endpoint that saw our key. On success the socket
    /// transitions to `Open`. NEVER panics on a malformed/hostile response.
    pub fn complete_handshake(&mut self, response: &[u8]) -> Result<(), WsError> {
        if self.role != WsRole::Client {
            return Err(WsError::InvalidState(String::from("not a client socket")));
        }
        if self.state != WsState::Connecting {
            return Err(WsError::InvalidState(String::from(
                "not awaiting a handshake",
            )));
        }
        let text = core::str::from_utf8(response)
            .map_err(|_| WsError::HandshakeFailed(String::from("non-UTF-8 response")))?;
        self.validate_handshake_response(text)?;
        self.state = WsState::Open;
        Ok(())
    }

    /// Drive the opening handshake to completion over a byte transport (the same
    /// `HttpTransport` abstraction the HTTP client uses — DRY). Connects, sends
    /// the upgrade request, reads the server's HTTP response up to the header
    /// terminator, and validates it. On `Ok` the socket is `Open` and ready for
    /// `send_text`/`process_incoming`. Client sockets only; never panics on a
    /// malformed/hostile response (it maps to a `WsError`). The response header
    /// block is bounded so a peer that never terminates it can't grow memory.
    pub fn connect_over<T: crate::http1::HttpTransport>(
        &mut self,
        host: &str,
        port: u16,
        url: &str,
        protocols: &[&str],
        transport: &mut T,
    ) -> Result<(), WsError> {
        if self.role != WsRole::Client {
            return Err(WsError::InvalidState(String::from("not a client socket")));
        }
        self.connect(url, protocols)?;
        transport
            .connect(host, port)
            .map_err(|e| WsError::IoError(format!("connect: {e:?}")))?;
        for buf in self.get_outgoing() {
            transport
                .send(&buf)
                .map_err(|e| WsError::IoError(format!("send: {e:?}")))?;
        }

        // Read until the end of the HTTP header block (`\r\n\r\n`). A 101 has no
        // body; anything after the terminator would be early WS frames, which
        // this minimal driver does not buffer (documented limitation).
        let mut resp: Vec<u8> = Vec::new();
        let mut tmp = [0u8; 1024];
        let header_end = loop {
            if let Some(pos) = find_header_end(&resp) {
                break pos;
            }
            if resp.len() > 16 * 1024 {
                return Err(WsError::HandshakeFailed(String::from(
                    "handshake response too large",
                )));
            }
            let n = transport
                .recv(&mut tmp)
                .map_err(|e| WsError::IoError(format!("recv: {e:?}")))?;
            if n == 0 {
                // EOF: try what we have (validate_handshake_response will reject
                // an incomplete one).
                break resp.len();
            }
            resp.extend_from_slice(&tmp[..n]);
        };
        let end = header_end.min(resp.len());
        self.complete_handshake(&resp[..end])
    }

    pub fn accept(&mut self, request_headers: &[(String, String)]) -> Result<Vec<u8>, WsError> {
        if self.role != WsRole::Server {
            return Err(WsError::InvalidState(String::from("not a server socket")));
        }

        let key = request_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Sec-WebSocket-Key"))
            .map(|(_, v)| v.as_str())
            .ok_or_else(|| WsError::HandshakeFailed(String::from("missing Sec-WebSocket-Key")))?;

        let accept_key = Self::compute_accept_key(key);

        let mut response = Vec::new();
        response.extend_from_slice(b"HTTP/1.1 101 Switching Protocols\r\n");
        response.extend_from_slice(b"Upgrade: websocket\r\n");
        response.extend_from_slice(b"Connection: Upgrade\r\n");
        response.extend_from_slice(b"Sec-WebSocket-Accept: ");
        response.extend_from_slice(accept_key.as_bytes());
        response.extend_from_slice(b"\r\n\r\n");

        self.state = WsState::Open;
        Ok(response)
    }

    pub fn send_text(&mut self, text: &str) -> Result<(), WsError> {
        if self.state != WsState::Open {
            return Err(WsError::NotConnected);
        }

        let should_mask = self.role == WsRole::Client;
        let key = if should_mask {
            Some(self.generate_masking_key())
        } else {
            None
        };
        let frame = WsFrame::text(text, should_mask, key);
        self.stats.messages_sent += 1;
        self.stats.bytes_sent += text.len() as u64;
        self.send_queue.push(frame);
        Ok(())
    }

    pub fn send_binary(&mut self, data: &[u8]) -> Result<(), WsError> {
        if self.state != WsState::Open {
            return Err(WsError::NotConnected);
        }

        let should_mask = self.role == WsRole::Client;
        let key = if should_mask {
            Some(self.generate_masking_key())
        } else {
            None
        };
        let frame = WsFrame::binary(data, should_mask, key);
        self.stats.messages_sent += 1;
        self.stats.bytes_sent += data.len() as u64;
        self.send_queue.push(frame);
        Ok(())
    }

    pub fn send_ping(&mut self, data: &[u8]) -> Result<(), WsError> {
        if self.state != WsState::Open {
            return Err(WsError::NotConnected);
        }

        let frame = WsFrame::ping(data);
        self.stats.pings_sent += 1;
        self.send_queue.push(frame);
        Ok(())
    }

    pub fn send_pong(&mut self, data: &[u8]) -> Result<(), WsError> {
        if self.state != WsState::Open {
            return Err(WsError::NotConnected);
        }

        let frame = WsFrame::pong(data);
        self.send_queue.push(frame);
        Ok(())
    }

    pub fn close(&mut self, code: u16, reason: &str) -> Result<(), WsError> {
        if self.state != WsState::Open {
            return Err(WsError::NotConnected);
        }

        self.state = WsState::Closing;
        self.close_code = Some(code);
        self.close_reason = Some(String::from(reason));

        let frame = WsFrame::close(code, reason);
        self.send_queue.push(frame);
        Ok(())
    }

    pub fn receive(&mut self) -> Option<WsMessage> {
        if self.recv_queue.is_empty() {
            None
        } else {
            Some(self.recv_queue.remove(0))
        }
    }

    pub fn process_incoming(&mut self, data: &[u8]) -> Result<Vec<WsMessage>, WsError> {
        let mut messages = Vec::new();
        let mut offset = 0;

        while offset < data.len() {
            let (frame, consumed) = self.decode_frame(&data[offset..])?;
            offset += consumed;
            self.stats.frames_received += 1;
            self.stats.bytes_received += frame.payload.len() as u64;

            if frame.opcode.is_control() {
                if let Some(event) = self.handle_control_frame(&frame) {
                    match event {
                        WsEvent::Message(msg) => messages.push(msg),
                        WsEvent::Close(code, reason) => {
                            self.close_code = Some(code);
                            self.close_reason = Some(reason);
                            self.state = WsState::Closed;
                        }
                        _ => {}
                    }
                }
            } else if let Some(msg) = self.handle_data_frame(frame) {
                self.stats.messages_received += 1;
                messages.push(msg);
            }
        }

        self.recv_queue.extend(messages.clone());
        Ok(messages)
    }

    pub fn get_outgoing(&mut self) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = Vec::new();
        // The upgrade request (raw HTTP bytes) goes first if a connect is pending.
        if let Some(handshake) = self.pending_handshake.take() {
            out.push(handshake);
        }
        let frames: Vec<WsFrame> = self.send_queue.drain(..).collect();
        for f in &frames {
            self.stats.frames_sent += 1;
            out.push(self.encode_frame(f));
        }
        out
    }

    pub fn is_open(&self) -> bool {
        self.state == WsState::Open
    }

    pub fn tick(&mut self, now_ms: u64) -> Vec<WsEvent> {
        let mut events = Vec::new();

        if self.state != WsState::Open {
            return events;
        }

        if let Some(interval) = self.ping_interval_ms {
            if now_ms.saturating_sub(self.last_ping) >= interval {
                self.last_ping = now_ms;
                let ping_data = now_ms.to_be_bytes();
                let frame = WsFrame::ping(&ping_data);
                self.send_queue.push(frame);
                self.stats.pings_sent += 1;
                events.push(WsEvent::Ping(ping_data.to_vec()));
            }
        }

        events
    }

    fn encode_frame(&self, frame: &WsFrame) -> Vec<u8> {
        let mut buf = Vec::with_capacity(14 + frame.payload.len());

        let first_byte = (if frame.fin { 0x80 } else { 0x00 }) | (frame.opcode as u8);
        buf.push(first_byte);

        let mask_bit = if frame.mask { 0x80u8 } else { 0x00u8 };
        let payload_len = frame.payload.len();

        if payload_len < 126 {
            buf.push(mask_bit | payload_len as u8);
        } else if payload_len <= 0xFFFF {
            buf.push(mask_bit | 126);
            buf.push((payload_len >> 8) as u8);
            buf.push((payload_len & 0xFF) as u8);
        } else {
            buf.push(mask_bit | 127);
            for i in (0..8).rev() {
                buf.push(((payload_len >> (i * 8)) & 0xFF) as u8);
            }
        }

        if frame.mask {
            if let Some(key) = frame.masking_key {
                buf.extend_from_slice(&key);
                let mut masked = frame.payload.clone();
                Self::apply_mask(&mut masked, &key);
                buf.extend_from_slice(&masked);
            } else {
                buf.extend_from_slice(&frame.payload);
            }
        } else {
            buf.extend_from_slice(&frame.payload);
        }

        buf
    }

    fn decode_frame(&self, data: &[u8]) -> Result<(WsFrame, usize), WsError> {
        if data.len() < 2 {
            return Err(WsError::ProtocolError(String::from("frame too short")));
        }

        let first = data[0];
        let second = data[1];

        let fin = (first & 0x80) != 0;
        let opcode = WsOpcode::from_u8(first & 0x0F)?;
        let mask = (second & 0x80) != 0;
        let mut payload_len = (second & 0x7F) as usize;
        let mut offset = 2;

        if payload_len == 126 {
            if data.len() < 4 {
                return Err(WsError::ProtocolError(String::from("incomplete length")));
            }
            payload_len = ((data[2] as usize) << 8) | (data[3] as usize);
            offset = 4;
        } else if payload_len == 127 {
            if data.len() < 10 {
                return Err(WsError::ProtocolError(String::from("incomplete length")));
            }
            payload_len = 0;
            for i in 0..8 {
                payload_len = (payload_len << 8) | (data[2 + i] as usize);
            }
            offset = 10;
        }

        if payload_len > self.max_frame_size {
            return Err(WsError::FrameTooLarge(payload_len));
        }

        let masking_key = if mask {
            if data.len() < offset + 4 {
                return Err(WsError::ProtocolError(String::from("incomplete mask key")));
            }
            let key = [
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ];
            offset += 4;
            Some(key)
        } else {
            None
        };

        if data.len() < offset + payload_len {
            return Err(WsError::ProtocolError(String::from("incomplete payload")));
        }

        let mut payload = data[offset..offset + payload_len].to_vec();

        if let Some(key) = masking_key {
            Self::apply_mask(&mut payload, &key);
        }

        let total_consumed = offset + payload_len;

        Ok((
            WsFrame {
                fin,
                opcode,
                mask,
                masking_key,
                payload,
            },
            total_consumed,
        ))
    }

    fn apply_mask(data: &mut [u8], key: &[u8; 4]) {
        for (i, byte) in data.iter_mut().enumerate() {
            *byte ^= key[i % 4];
        }
    }

    /// xorshift64* -- advances `rng_state` and returns 32 bits. Fast, no_std, and
    /// non-zero-cycle. The entropy QUALITY comes from the seed
    /// ([`WebSocket::set_entropy_seed`]); the sequence itself never repeats a value
    /// across consecutive frames, satisfying "fresh key per frame".
    fn next_rand_u32(&mut self) -> u32 {
        let mut x = self.rng_state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng_state = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    /// A fresh masking key for the next client frame. An explicit `masking_key`
    /// override (set for deterministic tests) wins; otherwise it is drawn from the
    /// PRNG so no two consecutive frames share a key (RFC 6455 5.3 intent).
    fn generate_masking_key(&mut self) -> [u8; 4] {
        if let Some(k) = self.masking_key {
            return k;
        }
        self.next_rand_u32().to_be_bytes()
    }

    fn build_handshake_request(&mut self, url: &str, protocols: &[&str]) -> Vec<u8> {
        let path = if let Some(pos) = url.find("://") {
            let after_scheme = &url[pos + 3..];
            match after_scheme.find('/') {
                Some(slash) => &after_scheme[slash..],
                None => "/",
            }
        } else {
            "/"
        };

        let host = if let Some(pos) = url.find("://") {
            let after_scheme = &url[pos + 3..];
            match after_scheme.find('/') {
                Some(slash) => &after_scheme[..slash],
                None => after_scheme,
            }
        } else {
            url
        };

        // RFC 6455 4.1: a fresh, randomly-selected 16-byte nonce, base64-encoded.
        let mut nonce = [0u8; 16];
        for chunk in nonce.chunks_mut(4) {
            let r = self.next_rand_u32().to_be_bytes();
            chunk.copy_from_slice(&r[..chunk.len()]);
        }
        let key = rae_encode::base64_encode(&nonce);
        self.sec_key = Some(key.clone());

        let mut buf = Vec::with_capacity(256);
        buf.extend_from_slice(b"GET ");
        buf.extend_from_slice(path.as_bytes());
        buf.extend_from_slice(b" HTTP/1.1\r\n");
        buf.extend_from_slice(b"Host: ");
        buf.extend_from_slice(host.as_bytes());
        buf.extend_from_slice(b"\r\nUpgrade: websocket\r\n");
        buf.extend_from_slice(b"Connection: Upgrade\r\n");
        buf.extend_from_slice(b"Sec-WebSocket-Key: ");
        buf.extend_from_slice(key.as_bytes());
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(b"Sec-WebSocket-Version: 13\r\n");

        if !protocols.is_empty() {
            buf.extend_from_slice(b"Sec-WebSocket-Protocol: ");
            for (i, proto) in protocols.iter().enumerate() {
                if i > 0 {
                    buf.extend_from_slice(b", ");
                }
                buf.extend_from_slice(proto.as_bytes());
            }
            buf.extend_from_slice(b"\r\n");
        }

        buf.extend_from_slice(b"\r\n");
        buf
    }

    fn validate_handshake_response(&self, response: &str) -> Result<(), WsError> {
        if !response.starts_with("HTTP/1.1 101") {
            return Err(WsError::HandshakeFailed(String::from("unexpected status")));
        }

        let lower = response.to_ascii_lowercase();
        if !lower.contains("upgrade: websocket") {
            return Err(WsError::HandshakeFailed(String::from(
                "missing Upgrade header",
            )));
        }
        if !lower.contains("connection: upgrade") {
            return Err(WsError::HandshakeFailed(String::from(
                "missing Connection header",
            )));
        }

        // The peer must echo base64(SHA1(our key + GUID)) -- this is what proves it
        // is a real WebSocket endpoint that saw OUR key (not an arbitrary 101).
        let expected = match &self.sec_key {
            Some(k) => Self::compute_accept_key(k),
            None => {
                return Err(WsError::HandshakeFailed(String::from(
                    "no client key to validate against",
                )))
            }
        };
        match header_value_ci(response, "sec-websocket-accept") {
            Some(v) if v == expected => Ok(()),
            Some(_) => Err(WsError::HandshakeFailed(String::from(
                "Sec-WebSocket-Accept mismatch",
            ))),
            None => Err(WsError::HandshakeFailed(String::from(
                "missing Sec-WebSocket-Accept",
            ))),
        }
    }

    /// RFC 6455 1.3: `Sec-WebSocket-Accept = base64( SHA1( key + GUID ) )`.
    fn compute_accept_key(key: &str) -> String {
        const MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
        let mut combined = String::with_capacity(key.len() + MAGIC.len());
        combined.push_str(key);
        combined.push_str(MAGIC);
        let digest = rae_hash::sha1(combined.as_bytes());
        rae_encode::base64_encode(&digest)
    }

    fn handle_control_frame(&mut self, frame: &WsFrame) -> Option<WsEvent> {
        match frame.opcode {
            WsOpcode::Ping => {
                let pong = WsFrame::pong(&frame.payload);
                self.send_queue.push(pong);
                Some(WsEvent::Ping(frame.payload.clone()))
            }
            WsOpcode::Pong => {
                self.last_pong = 0; // would use real timestamp
                self.stats.pongs_received += 1;
                Some(WsEvent::Pong(frame.payload.clone()))
            }
            WsOpcode::Close => {
                let (code, reason) = if frame.payload.len() >= 2 {
                    let code = ((frame.payload[0] as u16) << 8) | (frame.payload[1] as u16);
                    let reason = if frame.payload.len() > 2 {
                        core::str::from_utf8(&frame.payload[2..])
                            .unwrap_or("")
                            .to_string()
                    } else {
                        String::new()
                    };
                    (code, reason)
                } else {
                    (1000, String::new())
                };

                if self.state == WsState::Open {
                    let close_frame = WsFrame::close(code, &reason);
                    self.send_queue.push(close_frame);
                }

                self.state = WsState::Closed;
                Some(WsEvent::Close(code, reason))
            }
            _ => None,
        }
    }

    fn handle_data_frame(&mut self, frame: WsFrame) -> Option<WsMessage> {
        if frame.fin && frame.opcode != WsOpcode::Continuation {
            let message_type = match frame.opcode {
                WsOpcode::Text => WsMessageType::Text,
                _ => WsMessageType::Binary,
            };
            return Some(WsMessage {
                message_type,
                data: frame.payload,
                timestamp: 0,
            });
        }

        if frame.opcode != WsOpcode::Continuation {
            self.fragment_buffer = Some(FragmentBuffer {
                opcode: frame.opcode,
                data: frame.payload,
            });
            return None;
        }

        if let Some(ref mut buffer) = self.fragment_buffer {
            buffer.data.extend_from_slice(&frame.payload);

            if buffer.data.len() > self.max_message_size {
                self.fragment_buffer = None;
                return None;
            }

            if frame.fin {
                let message_type = match buffer.opcode {
                    WsOpcode::Text => WsMessageType::Text,
                    _ => WsMessageType::Binary,
                };
                let data = core::mem::take(&mut buffer.data);
                self.fragment_buffer = None;
                return Some(WsMessage {
                    message_type,
                    data,
                    timestamp: 0,
                });
            }
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_encode_decode() {
        let ws = WebSocket::new_server();
        let frame = WsFrame::text("hello", false, None);
        let encoded = ws.encode_frame(&frame);
        let (decoded, consumed) = ws.decode_frame(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.payload, b"hello");
        assert!(decoded.fin);
        assert_eq!(decoded.opcode, WsOpcode::Text);
    }

    #[test]
    fn test_masked_frame() {
        let ws = WebSocket::new_server();
        let key = [0x37, 0xfa, 0x21, 0x3d];
        let frame = WsFrame::text("test", true, Some(key));
        let encoded = ws.encode_frame(&frame);
        let (decoded, _) = ws.decode_frame(&encoded).unwrap();
        assert_eq!(decoded.payload, b"test");
    }

    #[test]
    fn test_close_frame() {
        let frame = WsFrame::close(1000, "normal");
        assert_eq!(frame.opcode, WsOpcode::Close);
        assert_eq!(frame.payload[0], 0x03);
        assert_eq!(frame.payload[1], 0xE8);
        assert_eq!(&frame.payload[2..], b"normal");
    }

    #[test]
    fn test_client_state_transitions() {
        let mut ws = WebSocket::new_client();
        assert_eq!(ws.state, WsState::Closed);
        assert!(ws.send_text("hello").is_err());

        ws.state = WsState::Open;
        assert!(ws.send_text("hello").is_ok());
        assert_eq!(ws.stats.messages_sent, 1);
    }

    #[test]
    fn test_apply_mask() {
        let key = [0xAA, 0xBB, 0xCC, 0xDD];
        let mut data = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        WebSocket::apply_mask(&mut data, &key);
        assert_eq!(
            data,
            vec![
                0x01 ^ 0xAA,
                0x02 ^ 0xBB,
                0x03 ^ 0xCC,
                0x04 ^ 0xDD,
                0x05 ^ 0xAA
            ]
        );

        WebSocket::apply_mask(&mut data, &key);
        assert_eq!(data, vec![0x01, 0x02, 0x03, 0x04, 0x05]);
    }

    // RFC 6455 handshake (real SHA-1 + base64)

    /// THE canonical RFC 6455 1.3 worked example -- proves compute_accept_key is
    /// real (base64(SHA1(key + GUID))), not the former hardcoded placeholder.
    #[test]
    fn accept_key_rfc6455_example() {
        assert_eq!(
            WebSocket::compute_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_ne!(
            WebSocket::compute_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            WebSocket::compute_accept_key("AnotherKeyValue123456==")
        );
    }

    /// Full client<->server handshake interop: the client emits a real upgrade
    /// request with a generated key, a server computes the matching accept, and
    /// the client validates it and goes Open.
    #[test]
    fn client_server_handshake_interop() {
        let mut client = WebSocket::new_client();
        client.set_entropy_seed(0x00C0_FFEE);
        client.connect("ws://example.com/chat", &["chat"]).unwrap();

        let out = client.get_outgoing();
        assert_eq!(out.len(), 1, "connect should queue the upgrade request");
        let req = String::from_utf8(out[0].clone()).unwrap();
        assert!(req.starts_with("GET /chat HTTP/1.1\r\n"));
        assert!(req.contains("Upgrade: websocket"));
        assert!(req.contains("Sec-WebSocket-Version: 13"));
        let key = header_value_ci(&req, "sec-websocket-key").expect("client sent a key");
        assert_ne!(
            key, "dGhlIHNhbXBsZSBub25jZQ==",
            "key must be generated, not static"
        );

        let mut server = WebSocket::new_server();
        let resp = server
            .accept(&[(String::from("Sec-WebSocket-Key"), String::from(key))])
            .unwrap();

        client.complete_handshake(&resp).unwrap();
        assert!(
            client.is_open(),
            "client must be Open after a valid handshake"
        );
    }

    /// A 101 response with a WRONG Sec-WebSocket-Accept is rejected (fail-closed).
    #[test]
    fn complete_handshake_rejects_wrong_accept() {
        let mut client = WebSocket::new_client();
        client.connect("ws://h/p", &[]).unwrap();
        let _ = client.get_outgoing();

        let bad = b"HTTP/1.1 101 Switching Protocols\r\n\
                    Upgrade: websocket\r\nConnection: Upgrade\r\n\
                    Sec-WebSocket-Accept: AAAAAAAAAAAAAAAAAAAAAAAAAAA=\r\n\r\n";
        let err = client.complete_handshake(bad).unwrap_err();
        assert!(matches!(err, WsError::HandshakeFailed(_)));
        assert!(!client.is_open());

        let mut c2 = WebSocket::new_client();
        c2.connect("ws://h/p", &[]).unwrap();
        let _ = c2.get_outgoing();
        let no_accept = b"HTTP/1.1 101 Switching Protocols\r\n\
                          Upgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
        assert!(c2.complete_handshake(no_accept).is_err());
    }

    /// Consecutive client frames must use DIFFERENT masking keys (RFC 6455 5.3
    /// intent) -- proves the former static key is gone.
    #[test]
    fn client_masking_keys_vary_per_frame() {
        let mut client = WebSocket::new_client();
        client.set_entropy_seed(0xABCD_EF01);
        client.state = WsState::Open;
        client.send_text("one").unwrap();
        client.send_text("two").unwrap();
        let out = client.get_outgoing();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0][1] & 0x80, 0x80, "client frame must set the mask bit");
        let k1 = &out[0][2..6];
        let k2 = &out[1][2..6];
        assert_ne!(k1, k2, "consecutive frames must not reuse a masking key");
    }

    /// A client-masked frame round-trips: a server process_incoming unmasks it
    /// back to the original text (real masking, not a no-op).
    #[test]
    fn masked_client_frame_round_trips_through_server() {
        let mut client = WebSocket::new_client();
        client.set_entropy_seed(0x1357_9BDF);
        client.state = WsState::Open;
        client.send_text("hello ws").unwrap();
        let wire = client.get_outgoing();
        assert_eq!(wire.len(), 1);

        let mut server = WebSocket::new_server();
        server.state = WsState::Open;
        let msgs = server.process_incoming(&wire[0]).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].as_text(), Some("hello ws"));
    }

    /// A transport that bridges to a server WebSocket: it captures the client's
    /// upgrade request and serves the matching `101` (real Accept key) on recv.
    struct WsServerTransport {
        server: WebSocket,
        request: Vec<u8>,
        response: Vec<u8>,
        read_pos: usize,
    }
    impl crate::http1::HttpTransport for WsServerTransport {
        fn connect(&mut self, _host: &str, _port: u16) -> crate::http1::Http1Result<()> {
            Ok(())
        }
        fn send(&mut self, buf: &[u8]) -> crate::http1::Http1Result<()> {
            self.request.extend_from_slice(buf);
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> crate::http1::Http1Result<usize> {
            if self.response.is_empty() {
                let req = String::from_utf8_lossy(&self.request);
                if let Some(key) = header_value_ci(&req, "sec-websocket-key") {
                    if let Ok(r) = self
                        .server
                        .accept(&[(String::from("Sec-WebSocket-Key"), String::from(key))])
                    {
                        self.response = r;
                    }
                }
            }
            let remaining = self.response.len().saturating_sub(self.read_pos);
            if remaining == 0 {
                return Ok(0);
            }
            let n = remaining.min(buf.len());
            buf[..n].copy_from_slice(&self.response[self.read_pos..self.read_pos + n]);
            self.read_pos += n;
            Ok(n)
        }
    }

    /// `connect_over` drives the full handshake over a transport: the client
    /// sends a real upgrade request, the server responds with a matching 101,
    /// and the client validates it and goes Open — end-to-end connectable.
    #[test]
    fn connect_over_completes_handshake() {
        let mut client = WebSocket::new_client();
        client.set_entropy_seed(0xFEED_FACE);
        let mut t = WsServerTransport {
            server: WebSocket::new_server(),
            request: Vec::new(),
            response: Vec::new(),
            read_pos: 0,
        };
        client
            .connect_over(
                "example.com",
                80,
                "ws://example.com/chat",
                &["chat"],
                &mut t,
            )
            .unwrap();
        assert!(client.is_open(), "client must be Open after connect_over");
        let req = String::from_utf8_lossy(&t.request);
        assert!(req.starts_with("GET /chat HTTP/1.1\r\n"));
        assert!(req.contains("Upgrade: websocket"));
        // The open socket can now produce a masked frame.
        client.send_text("hi").unwrap();
        assert!(!client.get_outgoing().is_empty());
    }

    /// connect_over fails closed on a non-101 / bad server response.
    #[test]
    fn connect_over_rejects_bad_server() {
        struct BadServer {
            resp: Vec<u8>,
            pos: usize,
        }
        impl crate::http1::HttpTransport for BadServer {
            fn connect(&mut self, _h: &str, _p: u16) -> crate::http1::Http1Result<()> {
                Ok(())
            }
            fn send(&mut self, _b: &[u8]) -> crate::http1::Http1Result<()> {
                Ok(())
            }
            fn recv(&mut self, buf: &mut [u8]) -> crate::http1::Http1Result<usize> {
                let rem = self.resp.len().saturating_sub(self.pos);
                if rem == 0 {
                    return Ok(0);
                }
                let n = rem.min(buf.len());
                buf[..n].copy_from_slice(&self.resp[self.pos..self.pos + n]);
                self.pos += n;
                Ok(n)
            }
        }
        let mut client = WebSocket::new_client();
        let mut t = BadServer {
            resp: b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
            pos: 0,
        };
        assert!(client
            .connect_over("h", 80, "ws://h/", &[], &mut t)
            .is_err());
        assert!(!client.is_open());
    }

    // -----------------------------------------------------------------------
    // Fuzz / property hardening for decode_frame — the last untrusted-network
    // input parser flagged in raenet. decode_frame is fed raw bytes straight
    // off the wire (process_incoming loops it over a TCP byte stream), so it
    // must NEVER panic, NEVER read out of bounds, and NEVER allocate an
    // unbounded payload no matter how hostile the input. A malicious 64-bit
    // length field claiming a multi-exabyte payload must be rejected against
    // max_frame_size BEFORE any allocation/slice; a missing mask-key bounds
    // check would OOB-index data[offset..offset+4].
    //
    // Self-contained xorshift PRNG (no external fuzz crate, no_std-safe). Same
    // shape as the discovery.rs / http1.rs fuzz waves in this session.
    // -----------------------------------------------------------------------

    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
            // Avoid the zero fixed-point of xorshift.
            Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn byte(&mut self) -> u8 {
            (self.next_u64() & 0xFF) as u8
        }
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next_u64() % n as u64) as usize
            }
        }
    }

    /// Whatever decode_frame returns (Ok frame or Err), the payload it hands
    /// back can never exceed the configured cap, and the bytes it claims to
    /// have consumed can never exceed the buffer it was given. This is the
    /// invariant a downstream caller (process_incoming) relies on to advance
    /// its offset safely.
    fn assert_decode_invariants(ws: &WebSocket, data: &[u8]) {
        if let Ok((frame, consumed)) = ws.decode_frame(data) {
            assert!(
                frame.payload.len() <= ws.max_frame_size,
                "decoded payload {} exceeds max_frame_size {}",
                frame.payload.len(),
                ws.max_frame_size
            );
            assert!(
                consumed <= data.len(),
                "claimed consumed {} exceeds buffer len {}",
                consumed,
                data.len()
            );
        }
    }

    /// Pure random bytes through the decoder must always return (Ok or Err) and
    /// never panic / OOB / over-allocate. FAIL-able: if any length/mask bounds
    /// check were missing, a crafted random prefix would index out of bounds
    /// and abort the test process.
    #[test]
    fn fuzz_decode_frame_random_never_panics() {
        let ws = WebSocket::new_server();
        let mut rng = Rng::new(0xF522_0001);
        for _ in 0..50_000 {
            let len = rng.below(40);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            assert_decode_invariants(&ws, &buf);
        }
    }

    /// Build a syntactically valid frame, then truncate it at EVERY offset.
    /// Each prefix must decode to Ok (only when complete) or Err (incomplete) —
    /// never panic. Covers the incomplete-length, incomplete-mask-key, and
    /// incomplete-payload bounds checks at each boundary.
    #[test]
    fn fuzz_decode_truncated_at_every_offset_never_panics() {
        let ws_server = WebSocket::new_server();
        let key = [0x37, 0xfa, 0x21, 0x3d];

        // A spread of frames exercising all three length encodings, masked and
        // unmasked, data and control opcodes.
        let frames = vec![
            ws_server.encode_frame(&WsFrame::text("", false, None)), // zero-len
            ws_server.encode_frame(&WsFrame::text("short", false, None)), // 7-bit
            ws_server.encode_frame(&WsFrame::text("masked!", true, Some(key))), // 7-bit masked
            ws_server.encode_frame(&WsFrame::binary(&vec![0xAB; 200], false, None)), // 16-bit len
            ws_server.encode_frame(&WsFrame::binary(&vec![0xCD; 300], true, Some(key))), // 16-bit masked
            ws_server.encode_frame(&WsFrame::ping(&vec![0x01, 0x02, 0x03])),             // control
            ws_server.encode_frame(&WsFrame::close(1000, "bye")),
        ];

        for full in &frames {
            for cut in 0..=full.len() {
                let prefix = &full[..cut];
                assert_decode_invariants(&ws_server, prefix);
            }
        }
    }

    /// 64-bit extended length (127) claiming an enormous payload must be
    /// rejected via FrameTooLarge BEFORE any allocation or slice — not OOB and
    /// not OOM. FAIL-able: were the `payload_len > max_frame_size` guard absent,
    /// `data[offset..offset+payload_len]` would slice past the 2-byte buffer
    /// (OOB panic) or `.to_vec()` of an exabyte length would abort on OOM.
    #[test]
    fn fuzz_decode_huge_64bit_length_is_bounded() {
        let ws = WebSocket::new_server();

        // FIN + Binary, length marker 127, then 8 bytes = u64::MAX-ish.
        let huge_lengths: [u64; 5] = [
            u64::MAX,
            0xFFFF_FFFF_FFFF_FFFF,
            0x7FFF_FFFF_FFFF_FFFF,
            0x0000_1000_0000_0000, // ~16 TiB
            (ws.max_frame_size as u64) + 1,
        ];
        for &claimed in &huge_lengths {
            let mut buf = vec![0x82u8, 127u8];
            buf.extend_from_slice(&claimed.to_be_bytes());
            // No actual payload bytes follow.
            let res = ws.decode_frame(&buf);
            assert!(
                matches!(res, Err(WsError::FrameTooLarge(_))),
                "huge 64-bit length {} should be FrameTooLarge, got {:?}",
                claimed,
                res
            );
            // And of course no panic / no OOM occurred to reach this line.
            assert_decode_invariants(&ws, &buf);
        }

        // Same hostile header but with the mask bit set — must still reject on
        // length before it ever reaches the mask-key read.
        let mut masked = vec![0x82u8, 0xFFu8]; // mask bit | 127
        masked.extend_from_slice(&u64::MAX.to_be_bytes());
        assert!(matches!(
            ws.decode_frame(&masked),
            Err(WsError::FrameTooLarge(_))
        ));
    }

    /// 16-bit extended length (126) claiming more than the buffer holds must be
    /// an incomplete-payload Err, never an OOB slice.
    #[test]
    fn fuzz_decode_16bit_length_overruns_buffer() {
        let ws = WebSocket::new_server();
        // 126 marker, length 0xFFFF (65535, <= 64KiB cap so passes the size
        // gate) but no payload bytes present.
        let buf = vec![0x82u8, 126u8, 0xFF, 0xFF];
        let res = ws.decode_frame(&buf);
        assert!(
            matches!(res, Err(WsError::ProtocolError(_))),
            "length-exceeds-buffer should be incomplete payload Err, got {:?}",
            res
        );
        assert_decode_invariants(&ws, &buf);
    }

    /// Masked frames with a truncated masking key must Err, not OOB-read the
    /// 4-byte key. FAIL-able: dropping the `data.len() < offset + 4` check would
    /// index data[offset..offset+3] past the buffer end here.
    #[test]
    fn fuzz_decode_truncated_mask_key() {
        let ws = WebSocket::new_server();
        // Mask bit set, 7-bit length 5, but supply 0..3 mask-key bytes only.
        for partial_key in 0..4usize {
            let mut buf = vec![0x81u8, 0x85u8]; // FIN|Text, mask|len=5
            for i in 0..partial_key {
                buf.push(0xA0 | i as u8);
            }
            let res = ws.decode_frame(&buf);
            assert!(
                matches!(res, Err(WsError::ProtocolError(_))),
                "truncated mask key ({} bytes) must Err, got {:?}",
                partial_key,
                res
            );
            assert_decode_invariants(&ws, &buf);
        }
    }

    /// All 256 opcode/RSV/FIN first-byte combinations against a minimal 2-byte
    /// frame. Reserved/invalid opcodes must Err cleanly; RSV bits and FIN
    /// variations must not crash the decoder.
    #[test]
    fn fuzz_decode_all_first_byte_combinations() {
        let ws = WebSocket::new_server();
        for first in 0u16..=255 {
            // Second byte: zero-length, unmasked -> a complete 2-byte frame.
            let buf = vec![first as u8, 0x00u8];
            assert_decode_invariants(&ws, &buf);
            // Second byte with mask bit + tiny length -> exercises mask path.
            let buf2 = vec![first as u8, 0x82u8]; // mask | len 2
            assert_decode_invariants(&ws, &buf2);
        }
    }

    /// Reserved opcodes (3..7, 11..15) must be rejected, never silently
    /// accepted. FAIL-able: WsOpcode::from_u8 returning Ok for these would let
    /// an unknown frame type through.
    #[test]
    fn decode_rejects_reserved_opcodes() {
        let ws = WebSocket::new_server();
        let reserved = [3u8, 4, 5, 6, 7, 11, 12, 13, 14, 15];
        for &op in &reserved {
            let buf = vec![0x80u8 | op, 0x00u8]; // FIN, that opcode, len 0
            let res = ws.decode_frame(&buf);
            assert!(
                matches!(res, Err(WsError::ProtocolError(_))),
                "reserved opcode {} should be rejected, got {:?}",
                op,
                res
            );
        }
    }

    /// Mutate valid encoded frames byte-by-byte (flip random bytes / inject
    /// random length markers) and feed back. The nastiest case is mutating the
    /// length field of an otherwise valid masked frame so the claimed length
    /// disagrees with the real buffer. Must hold all bounds.
    #[test]
    fn fuzz_decode_mutated_valid_frames_never_panics() {
        let ws = WebSocket::new_server();
        let key = [0x11, 0x22, 0x33, 0x44];
        let seeds = [
            ws.encode_frame(&WsFrame::text("hello world", true, Some(key))),
            ws.encode_frame(&WsFrame::binary(&vec![0x5A; 130], true, Some(key))),
            ws.encode_frame(&WsFrame::ping(&vec![0x09; 10])),
        ];

        let mut rng = Rng::new(0xC0FF_EE12);
        for _ in 0..40_000 {
            let base = &seeds[rng.below(seeds.len())];
            let mut buf = base.clone();
            if buf.is_empty() {
                continue;
            }
            // Apply 1..4 random byte mutations.
            let muts = 1 + rng.below(4);
            for _ in 0..muts {
                let idx = rng.below(buf.len());
                buf[idx] = rng.byte();
            }
            // Optionally truncate.
            if rng.below(3) == 0 {
                let new_len = rng.below(buf.len() + 1);
                buf.truncate(new_len);
            }
            assert_decode_invariants(&ws, &buf);
        }
    }

    /// process_incoming is the real public entry: it loops decode_frame over a
    /// concatenated byte stream and advances by `consumed`. If decode_frame ever
    /// returned consumed==0 on Ok, or consumed>data.len(), this loop could hang
    /// or OOB. Drive it with random and structured streams; it must always
    /// return (Ok messages or Err) without hanging or panicking.
    #[test]
    fn fuzz_process_incoming_random_stream_terminates() {
        let mut rng = Rng::new(0x5EED_F00D);
        for _ in 0..20_000 {
            let mut ws = WebSocket::new_server();
            ws.state = WsState::Open;
            let len = rng.below(120);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            // Must terminate: a zero-progress decode would infinite-loop here.
            let _ = ws.process_incoming(&buf);
        }
    }

    /// Concatenate several valid frames into one stream and confirm
    /// process_incoming walks all of them, then fuzz-truncate the concatenation
    /// at every offset — never a hang or panic.
    #[test]
    fn fuzz_process_incoming_concatenated_frames() {
        let ws_enc = WebSocket::new_server();
        let key = [0xAA, 0xBB, 0xCC, 0xDD];
        let mut stream = Vec::new();
        stream.extend(ws_enc.encode_frame(&WsFrame::text("one", true, Some(key))));
        stream.extend(ws_enc.encode_frame(&WsFrame::binary(&vec![0x1; 50], true, Some(key))));
        stream.extend(ws_enc.encode_frame(&WsFrame::ping(&vec![0xFE; 4])));
        stream.extend(ws_enc.encode_frame(&WsFrame::text("two", false, None)));

        for cut in 0..=stream.len() {
            let mut ws = WebSocket::new_server();
            ws.state = WsState::Open;
            // Each prefix must return without hang/panic.
            let _ = ws.process_incoming(&stream[..cut]);
        }
    }

    /// Documents the CURRENT decode behavior for an oversized control frame.
    /// RFC 6455 §5.5 requires control frames to carry <=125 bytes; this
    /// decoder does NOT enforce that limit at decode time — it only bounds the
    /// payload against max_frame_size (64 KiB). That is a spec-compliance gap,
    /// NOT a memory-safety defect: the payload is fully bounds-checked, so there
    /// is no OOB read and no unbounded allocation. This test pins the observed
    /// behavior (decodes Ok within the size cap) so a future change that adds
    /// the RFC 125-byte rejection is a deliberate, visible update here rather
    /// than a silent surprise. See REPORT note for the library follow-up.
    #[test]
    fn decode_oversized_control_frame_is_bounded_not_oob() {
        let ws = WebSocket::new_server();
        // 130-byte Ping (>125): build a 16-bit-length frame by hand.
        let payload = vec![0x42u8; 130];
        let mut buf = vec![0x89u8, 126u8]; // FIN|Ping, length marker 126
        buf.push((payload.len() >> 8) as u8);
        buf.push((payload.len() & 0xFF) as u8);
        buf.extend_from_slice(&payload);

        let res = ws.decode_frame(&buf);
        // The contract this test enforces: whatever the decode result, it is
        // bounded (no OOB / no OOM). Today it happens to be Ok with the exact
        // payload — assert that explicitly so the invariant AND the observed
        // behavior are both pinned and FAIL-able.
        assert_decode_invariants(&ws, &buf);
        let (frame, consumed) = res.expect("oversized control frame decodes within size cap today");
        assert_eq!(frame.opcode, WsOpcode::Ping);
        assert_eq!(frame.payload, payload);
        assert_eq!(consumed, buf.len());
    }
}
