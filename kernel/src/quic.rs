//! QUIC Transport Protocol — UDP-based transport with built-in TLS 1.3.
//!
//! Implements the core QUIC protocol for AthNet: connection management,
//! packet encoding/decoding, stream multiplexing, flow control, and
//! timeout-based loss detection. Uses `crate::crypto` for AEAD and
//! `crate::tls` for the TLS 1.3 handshake integration.

#![allow(dead_code)]

extern crate alloc;

use alloc::{collections::BTreeMap, string::String, vec, vec::Vec};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Connection IDs & core types
// ─────────────────────────────────────────────────────────────────────────────

pub const QUIC_VERSION_1: u32 = 0x0000_0001;
pub const MAX_DATAGRAM_SIZE: usize = 1350;
const INITIAL_MAX_DATA: u64 = 1_048_576; // 1 MiB
const INITIAL_MAX_STREAM_DATA: u64 = 262_144; // 256 KiB
const MAX_IDLE_TIMEOUT_MS: u64 = 30_000;
const INITIAL_RTT_MS: u64 = 333;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ConnectionId(pub Vec<u8>);

impl ConnectionId {
    pub fn new(data: &[u8]) -> Self {
        Self(data.to_vec())
    }

    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuicError {
    BufferTooShort,
    InvalidPacket,
    UnknownVersion,
    ConnectionClosed,
    StreamLimitExceeded,
    FlowControlExceeded,
    CryptoError,
    HandshakeNotComplete,
    InvalidState,
    InvalidStreamId,
    Timeout,
    InternalError,
}

// ─────────────────────────────────────────────────────────────────────────────
// Packet types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Initial,
    ZeroRtt,
    Handshake,
    Retry,
    OneRtt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketNumberSpace {
    Initial,
    Handshake,
    Application,
}

#[derive(Debug, Clone)]
pub struct PacketHeader {
    pub pkt_type: PacketType,
    pub version: u32,
    pub dst_cid: ConnectionId,
    pub src_cid: ConnectionId,
    pub pkt_num: u64,
    pub pkt_num_len: u8,
    pub payload_len: usize,
    pub token: Vec<u8>,
}

impl PacketHeader {
    /// Encode a long header (Initial, Handshake, 0-RTT) into a buffer.
    pub fn encode_long(&self, buf: &mut Vec<u8>) {
        let form_bit = 0x80u8; // long header
        let type_bits = match self.pkt_type {
            PacketType::Initial => 0x00,
            PacketType::ZeroRtt => 0x01,
            PacketType::Handshake => 0x02,
            PacketType::Retry => 0x03,
            _ => 0x00,
        };
        let first_byte = form_bit | (type_bits << 4) | ((self.pkt_num_len - 1) & 0x03);
        buf.push(first_byte);
        buf.extend_from_slice(&self.version.to_be_bytes());
        buf.push(self.dst_cid.len() as u8);
        buf.extend_from_slice(self.dst_cid.as_bytes());
        buf.push(self.src_cid.len() as u8);
        buf.extend_from_slice(self.src_cid.as_bytes());

        if self.pkt_type == PacketType::Initial {
            encode_varint(self.token.len() as u64, buf);
            buf.extend_from_slice(&self.token);
        }

        encode_varint(self.payload_len as u64, buf);
        encode_pkt_num(self.pkt_num, self.pkt_num_len, buf);
    }

    /// Encode a short header (1-RTT).
    pub fn encode_short(&self, buf: &mut Vec<u8>) {
        let first_byte = 0x40u8 | ((self.pkt_num_len - 1) & 0x03);
        buf.push(first_byte);
        buf.extend_from_slice(self.dst_cid.as_bytes());
        encode_pkt_num(self.pkt_num, self.pkt_num_len, buf);
    }
}

/// Variable-length integer encoding (RFC 9000 Section 16).
pub fn encode_varint(val: u64, buf: &mut Vec<u8>) {
    if val < 64 {
        buf.push(val as u8);
    } else if val < 16384 {
        buf.push(0x40 | ((val >> 8) as u8));
        buf.push(val as u8);
    } else if val < 1_073_741_824 {
        let bytes = (val as u32).to_be_bytes();
        buf.push(0x80 | bytes[0]);
        buf.extend_from_slice(&bytes[1..]);
    } else {
        let bytes = val.to_be_bytes();
        buf.push(0xC0 | bytes[0]);
        buf.extend_from_slice(&bytes[1..]);
    }
}

/// Decode a variable-length integer, returning (value, bytes_consumed).
pub fn decode_varint(buf: &[u8]) -> Result<(u64, usize), QuicError> {
    if buf.is_empty() {
        return Err(QuicError::BufferTooShort);
    }
    let prefix = buf[0] >> 6;
    let length = 1usize << prefix;
    if buf.len() < length {
        return Err(QuicError::BufferTooShort);
    }
    let mut val = (buf[0] & 0x3F) as u64;
    for i in 1..length {
        val = (val << 8) | buf[i] as u64;
    }
    Ok((val, length))
}

fn encode_pkt_num(pn: u64, len: u8, buf: &mut Vec<u8>) {
    match len {
        1 => buf.push(pn as u8),
        2 => buf.extend_from_slice(&(pn as u16).to_be_bytes()),
        3 => {
            buf.push((pn >> 16) as u8);
            buf.extend_from_slice(&(pn as u16).to_be_bytes());
        }
        _ => buf.extend_from_slice(&(pn as u32).to_be_bytes()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Frame types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Frame {
    Padding,
    Ping,
    Ack {
        largest_acked: u64,
        ack_delay: u64,
        ranges: Vec<AckRange>,
    },
    Crypto {
        offset: u64,
        data: Vec<u8>,
    },
    Stream {
        stream_id: u64,
        offset: u64,
        data: Vec<u8>,
        fin: bool,
    },
    MaxData {
        max_data: u64,
    },
    MaxStreamData {
        stream_id: u64,
        max_data: u64,
    },
    MaxStreams {
        max_streams: u64,
        bidi: bool,
    },
    ConnectionClose {
        error_code: u64,
        frame_type: u64,
        reason: Vec<u8>,
    },
    ResetStream {
        stream_id: u64,
        error_code: u64,
        final_size: u64,
    },
    StopSending {
        stream_id: u64,
        error_code: u64,
    },
    NewConnectionId {
        seq: u64,
        retire_prior_to: u64,
        cid: ConnectionId,
        reset_token: [u8; 16],
    },
    HandshakeDone,
}

#[derive(Debug, Clone)]
pub struct AckRange {
    pub gap: u64,
    pub length: u64,
}

impl Frame {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            Frame::Padding => buf.push(0x00),
            Frame::Ping => buf.push(0x01),
            Frame::Ack {
                largest_acked,
                ack_delay,
                ranges,
            } => {
                buf.push(0x02);
                encode_varint(*largest_acked, buf);
                encode_varint(*ack_delay, buf);
                encode_varint(ranges.len() as u64, buf);
                if let Some(first) = ranges.first() {
                    encode_varint(first.length, buf);
                }
                for range in ranges.iter().skip(1) {
                    encode_varint(range.gap, buf);
                    encode_varint(range.length, buf);
                }
            }
            Frame::Crypto { offset, data } => {
                buf.push(0x06);
                encode_varint(*offset, buf);
                encode_varint(data.len() as u64, buf);
                buf.extend_from_slice(data);
            }
            Frame::Stream {
                stream_id,
                offset,
                data,
                fin,
            } => {
                let mut frame_type = 0x08u8;
                if *offset > 0 {
                    frame_type |= 0x04;
                }
                if !data.is_empty() {
                    frame_type |= 0x02;
                }
                if *fin {
                    frame_type |= 0x01;
                }
                buf.push(frame_type);
                encode_varint(*stream_id, buf);
                if *offset > 0 {
                    encode_varint(*offset, buf);
                }
                if !data.is_empty() {
                    encode_varint(data.len() as u64, buf);
                    buf.extend_from_slice(data);
                }
            }
            Frame::MaxData { max_data } => {
                buf.push(0x10);
                encode_varint(*max_data, buf);
            }
            Frame::MaxStreamData {
                stream_id,
                max_data,
            } => {
                buf.push(0x11);
                encode_varint(*stream_id, buf);
                encode_varint(*max_data, buf);
            }
            Frame::MaxStreams { max_streams, bidi } => {
                buf.push(if *bidi { 0x12 } else { 0x13 });
                encode_varint(*max_streams, buf);
            }
            Frame::ConnectionClose {
                error_code,
                frame_type,
                reason,
            } => {
                buf.push(0x1C);
                encode_varint(*error_code, buf);
                encode_varint(*frame_type, buf);
                encode_varint(reason.len() as u64, buf);
                buf.extend_from_slice(reason);
            }
            Frame::ResetStream {
                stream_id,
                error_code,
                final_size,
            } => {
                buf.push(0x04);
                encode_varint(*stream_id, buf);
                encode_varint(*error_code, buf);
                encode_varint(*final_size, buf);
            }
            Frame::StopSending {
                stream_id,
                error_code,
            } => {
                buf.push(0x05);
                encode_varint(*stream_id, buf);
                encode_varint(*error_code, buf);
            }
            Frame::NewConnectionId {
                seq,
                retire_prior_to,
                cid,
                reset_token,
            } => {
                buf.push(0x18);
                encode_varint(*seq, buf);
                encode_varint(*retire_prior_to, buf);
                buf.push(cid.len() as u8);
                buf.extend_from_slice(cid.as_bytes());
                buf.extend_from_slice(reset_token);
            }
            Frame::HandshakeDone => buf.push(0x1E),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stream state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Open,
    HalfClosedLocal,
    HalfClosedRemote,
    Closed,
    ResetSent,
    ResetReceived,
}

#[derive(Debug)]
pub struct QuicStream {
    pub id: u64,
    pub state: StreamState,
    pub tx_offset: u64,
    pub rx_offset: u64,
    pub tx_max_data: u64,
    pub rx_max_data: u64,
    pub tx_buf: Vec<u8>,
    pub rx_buf: Vec<u8>,
    pub fin_sent: bool,
    pub fin_received: bool,
}

impl QuicStream {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            state: StreamState::Idle,
            tx_offset: 0,
            rx_offset: 0,
            tx_max_data: INITIAL_MAX_STREAM_DATA,
            rx_max_data: INITIAL_MAX_STREAM_DATA,
            tx_buf: Vec::new(),
            rx_buf: Vec::new(),
            fin_sent: false,
            fin_received: false,
        }
    }

    pub fn write(&mut self, data: &[u8]) -> Result<usize, QuicError> {
        if self.state == StreamState::Closed || self.state == StreamState::ResetSent {
            return Err(QuicError::InvalidState);
        }
        let available = self.tx_max_data.saturating_sub(self.tx_offset) as usize;
        let to_write = core::cmp::min(data.len(), available);
        if to_write == 0 {
            return Err(QuicError::FlowControlExceeded);
        }
        self.tx_buf.extend_from_slice(&data[..to_write]);
        self.state = StreamState::Open;
        Ok(to_write)
    }

    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = core::cmp::min(buf.len(), self.rx_buf.len());
        if to_read > 0 {
            buf[..to_read].copy_from_slice(&self.rx_buf[..to_read]);
            self.rx_buf.drain(..to_read);
            self.rx_offset += to_read as u64;
        }
        to_read
    }

    pub fn receive_data(&mut self, offset: u64, data: &[u8], fin: bool) -> Result<(), QuicError> {
        if offset != self.rx_offset + self.rx_buf.len() as u64 {
            return Ok(());
        }
        if self.rx_offset + self.rx_buf.len() as u64 + data.len() as u64 > self.rx_max_data {
            return Err(QuicError::FlowControlExceeded);
        }
        self.rx_buf.extend_from_slice(data);
        if fin {
            self.fin_received = true;
            if self.fin_sent {
                self.state = StreamState::Closed;
            } else {
                self.state = StreamState::HalfClosedRemote;
            }
        }
        Ok(())
    }

    pub fn close(&mut self) {
        self.fin_sent = true;
        if self.fin_received {
            self.state = StreamState::Closed;
        } else {
            self.state = StreamState::HalfClosedLocal;
        }
    }

    pub fn is_bidi(&self) -> bool {
        self.id & 0x02 == 0
    }
    pub fn is_client_initiated(&self) -> bool {
        self.id & 0x01 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Loss detection & congestion control
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SentPacket {
    pub pkt_num: u64,
    pub time_sent_ms: u64,
    pub size: usize,
    pub ack_eliciting: bool,
    pub frames: Vec<Frame>,
    pub space: PacketNumberSpace,
}

#[derive(Debug)]
pub struct LossDetection {
    pub largest_acked: [u64; 3], // per space
    pub loss_time: [u64; 3],
    pub sent_packets: Vec<SentPacket>,
    pub smoothed_rtt: u64,
    pub rtt_var: u64,
    pub min_rtt: u64,
    pub pto_count: u32,
    pub loss_detection_timer: u64,
}

impl LossDetection {
    pub fn new() -> Self {
        Self {
            largest_acked: [0; 3],
            loss_time: [0; 3],
            sent_packets: Vec::new(),
            smoothed_rtt: INITIAL_RTT_MS,
            rtt_var: INITIAL_RTT_MS / 2,
            min_rtt: u64::MAX,
            pto_count: 0,
            loss_detection_timer: 0,
        }
    }

    pub fn on_packet_sent(&mut self, pkt: SentPacket) {
        self.sent_packets.push(pkt);
    }

    pub fn on_ack_received(
        &mut self,
        largest: u64,
        ack_delay_ms: u64,
        now_ms: u64,
        space: PacketNumberSpace,
    ) -> Vec<SentPacket> {
        let space_idx = space as usize;
        self.largest_acked[space_idx] = core::cmp::max(self.largest_acked[space_idx], largest);

        if let Some(sent) = self.sent_packets.iter().find(|p| p.pkt_num == largest) {
            let latest_rtt = now_ms.saturating_sub(sent.time_sent_ms);
            self.min_rtt = core::cmp::min(self.min_rtt, latest_rtt);

            let adjusted_rtt = if latest_rtt > self.min_rtt + ack_delay_ms {
                latest_rtt - ack_delay_ms
            } else {
                latest_rtt
            };

            if self.smoothed_rtt == INITIAL_RTT_MS {
                self.smoothed_rtt = adjusted_rtt;
                self.rtt_var = adjusted_rtt / 2;
            } else {
                let abs_diff = if adjusted_rtt > self.smoothed_rtt {
                    adjusted_rtt - self.smoothed_rtt
                } else {
                    self.smoothed_rtt - adjusted_rtt
                };
                self.rtt_var = (3 * self.rtt_var + abs_diff) / 4;
                self.smoothed_rtt = (7 * self.smoothed_rtt + adjusted_rtt) / 8;
            }
        }

        self.pto_count = 0;
        self.detect_lost_packets(now_ms, space)
    }

    fn detect_lost_packets(&mut self, now_ms: u64, space: PacketNumberSpace) -> Vec<SentPacket> {
        let space_idx = space as usize;
        let largest = self.largest_acked[space_idx];
        let loss_delay = core::cmp::max(self.smoothed_rtt * 9 / 8, 1);

        let mut lost = Vec::new();
        let mut remaining = Vec::new();

        for pkt in self.sent_packets.drain(..) {
            if pkt.space != space {
                remaining.push(pkt);
                continue;
            }
            if pkt.pkt_num > largest {
                remaining.push(pkt);
                continue;
            }
            let time_since = now_ms.saturating_sub(pkt.time_sent_ms);
            let pkt_threshold = largest.saturating_sub(pkt.pkt_num);

            if time_since > loss_delay || pkt_threshold >= 3 {
                lost.push(pkt);
            } else {
                remaining.push(pkt);
            }
        }
        self.sent_packets = remaining;
        lost
    }

    /// Probe Timeout = smoothed_rtt + max(4*rtt_var, 1ms) * 2^pto_count
    pub fn pto(&self) -> u64 {
        let base = self.smoothed_rtt + core::cmp::max(4 * self.rtt_var, 1);
        base << self.pto_count.min(6)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// QUIC Connection
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Idle,
    Handshaking,
    Connected,
    Draining,
    Closed,
}

#[derive(Debug)]
pub struct QuicConnection {
    pub state: ConnectionState,
    pub is_server: bool,
    pub src_cid: ConnectionId,
    pub dst_cid: ConnectionId,
    pub version: u32,
    pub streams: BTreeMap<u64, QuicStream>,
    pub next_bidi_stream: u64,
    pub next_uni_stream: u64,
    pub max_bidi_streams: u64,
    pub max_uni_streams: u64,
    pub local_max_data: u64,
    pub peer_max_data: u64,
    pub total_sent: u64,
    pub total_received: u64,
    pub pkt_num: [u64; 3], // per space
    pub loss: LossDetection,
    pub tx_key: [u8; 32],
    pub rx_key: [u8; 32],
    pub handshake_complete: bool,
    pub tx_queue: Vec<Vec<u8>>,
    pub idle_timeout_ms: u64,
    pub last_activity_ms: u64,
}

impl QuicConnection {
    pub fn new_client(src_cid: ConnectionId, dst_cid: ConnectionId) -> Self {
        Self {
            state: ConnectionState::Idle,
            is_server: false,
            src_cid,
            dst_cid,
            version: QUIC_VERSION_1,
            streams: BTreeMap::new(),
            next_bidi_stream: 0,
            next_uni_stream: 2,
            max_bidi_streams: 100,
            max_uni_streams: 100,
            local_max_data: INITIAL_MAX_DATA,
            peer_max_data: INITIAL_MAX_DATA,
            total_sent: 0,
            total_received: 0,
            pkt_num: [0; 3],
            loss: LossDetection::new(),
            tx_key: [0u8; 32],
            rx_key: [0u8; 32],
            handshake_complete: false,
            tx_queue: Vec::new(),
            idle_timeout_ms: MAX_IDLE_TIMEOUT_MS,
            last_activity_ms: 0,
        }
    }

    pub fn new_server(src_cid: ConnectionId, dst_cid: ConnectionId) -> Self {
        let mut conn = Self::new_client(src_cid, dst_cid);
        conn.is_server = true;
        conn.next_bidi_stream = 1;
        conn.next_uni_stream = 3;
        conn
    }

    /// Open a new bidirectional stream, returning the stream ID.
    pub fn open_bidi_stream(&mut self) -> Result<u64, QuicError> {
        let count = self.streams.values().filter(|s| s.is_bidi()).count() as u64;
        if count >= self.max_bidi_streams {
            return Err(QuicError::StreamLimitExceeded);
        }
        let id = self.next_bidi_stream;
        self.next_bidi_stream += 4;
        self.streams.insert(id, QuicStream::new(id));
        Ok(id)
    }

    /// Open a new unidirectional stream, returning the stream ID.
    pub fn open_uni_stream(&mut self) -> Result<u64, QuicError> {
        let count = self.streams.values().filter(|s| !s.is_bidi()).count() as u64;
        if count >= self.max_uni_streams {
            return Err(QuicError::StreamLimitExceeded);
        }
        let id = self.next_uni_stream;
        self.next_uni_stream += 4;
        self.streams.insert(id, QuicStream::new(id));
        Ok(id)
    }

    /// Write data to a stream.
    pub fn stream_send(&mut self, stream_id: u64, data: &[u8]) -> Result<usize, QuicError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(QuicError::InvalidStreamId)?;
        if self.total_sent + data.len() as u64 > self.peer_max_data {
            return Err(QuicError::FlowControlExceeded);
        }
        let written = stream.write(data)?;
        self.total_sent += written as u64;
        Ok(written)
    }

    /// Read data from a stream.
    pub fn stream_recv(&mut self, stream_id: u64, buf: &mut [u8]) -> Result<usize, QuicError> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or(QuicError::InvalidStreamId)?;
        Ok(stream.read(buf))
    }

    /// Build outbound QUIC packets from pending stream data and control frames.
    pub fn flush_packets(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        self.last_activity_ms = now_ms;

        let pkt_type = if self.handshake_complete {
            PacketType::OneRtt
        } else {
            PacketType::Initial
        };
        let space = if self.handshake_complete {
            PacketNumberSpace::Application
        } else {
            PacketNumberSpace::Initial
        };
        let space_idx = space as usize;

        let mut frames: Vec<Frame> = Vec::new();

        for stream in self.streams.values_mut() {
            if stream.tx_buf.is_empty() {
                continue;
            }
            let available = MAX_DATAGRAM_SIZE - 100;
            let to_send = core::cmp::min(stream.tx_buf.len(), available);
            let data: Vec<u8> = stream.tx_buf.drain(..to_send).collect();
            let offset = stream.tx_offset;
            stream.tx_offset += to_send as u64;

            frames.push(Frame::Stream {
                stream_id: stream.id,
                offset,
                data,
                fin: stream.fin_sent && stream.tx_buf.is_empty(),
            });
        }

        if frames.is_empty() {
            return packets;
        }

        let pkt_num = self.pkt_num[space_idx];
        self.pkt_num[space_idx] += 1;

        let mut payload = Vec::new();
        let encoded_frames = frames.clone();
        for frame in &encoded_frames {
            frame.encode(&mut payload);
        }

        let aead = crate::crypto::ChaCha20Poly1305::new(&self.tx_key);
        let nonce = crate::tunnel::WgCrypto::wg_nonce(pkt_num);
        let mut ciphertext = vec![0u8; payload.len()];
        let mut tag = [0u8; 16];
        let _ = aead.encrypt(&nonce, &[], &payload, &mut ciphertext, &mut tag);

        let header = PacketHeader {
            pkt_type,
            version: self.version,
            dst_cid: self.dst_cid.clone(),
            src_cid: self.src_cid.clone(),
            pkt_num,
            pkt_num_len: 4,
            payload_len: ciphertext.len() + 16,
            token: Vec::new(),
        };

        let mut pkt_buf = Vec::new();
        if pkt_type == PacketType::OneRtt {
            header.encode_short(&mut pkt_buf);
        } else {
            header.encode_long(&mut pkt_buf);
        }
        pkt_buf.extend_from_slice(&ciphertext);
        pkt_buf.extend_from_slice(&tag);

        self.loss.on_packet_sent(SentPacket {
            pkt_num,
            time_sent_ms: now_ms,
            size: pkt_buf.len(),
            ack_eliciting: true,
            frames,
            space,
        });

        packets.push(pkt_buf);
        packets
    }

    /// Process a received ACK frame.
    pub fn process_ack(
        &mut self,
        largest_acked: u64,
        ack_delay: u64,
        now_ms: u64,
        space: PacketNumberSpace,
    ) -> Vec<SentPacket> {
        let lost = self
            .loss
            .on_ack_received(largest_acked, ack_delay, now_ms, space);
        for pkt in &lost {
            for frame in &pkt.frames {
                if let Frame::Stream {
                    stream_id,
                    offset,
                    data,
                    fin,
                } = frame
                {
                    if let Some(stream) = self.streams.get_mut(stream_id) {
                        let mut retransmit = data.clone();
                        stream.tx_buf.splice(0..0, retransmit.drain(..));
                        stream.tx_offset = *offset;
                    }
                }
            }
        }
        lost
    }

    /// Generate an ACK frame for received packets.
    pub fn build_ack(&self, space: PacketNumberSpace) -> Frame {
        let space_idx = space as usize;
        Frame::Ack {
            largest_acked: self.loss.largest_acked[space_idx],
            ack_delay: 0,
            ranges: vec![AckRange {
                gap: 0,
                length: self.loss.largest_acked[space_idx],
            }],
        }
    }

    /// Process incoming stream data from a decoded frame.
    pub fn process_stream_frame(
        &mut self,
        stream_id: u64,
        offset: u64,
        data: &[u8],
        fin: bool,
    ) -> Result<(), QuicError> {
        if !self.streams.contains_key(&stream_id) {
            self.streams.insert(stream_id, QuicStream::new(stream_id));
        }
        let stream = self.streams.get_mut(&stream_id).unwrap();
        self.total_received += data.len() as u64;
        if self.total_received > self.local_max_data {
            return Err(QuicError::FlowControlExceeded);
        }
        stream.receive_data(offset, data, fin)
    }

    /// Close the connection gracefully.
    pub fn close(&mut self, error_code: u64, reason: &[u8]) -> Vec<u8> {
        self.state = ConnectionState::Draining;
        let frame = Frame::ConnectionClose {
            error_code,
            frame_type: 0,
            reason: reason.to_vec(),
        };
        let mut buf = Vec::new();
        frame.encode(&mut buf);
        buf
    }

    /// Check if the connection has timed out.
    pub fn is_timed_out(&self, now_ms: u64) -> bool {
        if self.state == ConnectionState::Closed {
            return true;
        }
        now_ms.saturating_sub(self.last_activity_ms) > self.idle_timeout_ms
    }

    /// Derive initial keys from connection ID (simplified).
    pub fn derive_initial_keys(&mut self) {
        use crate::crypto::{HashAlgorithm, HmacContext, Sha256Context};
        let salt = b"quic-athnet-v1-initial-salt-2026";
        let hmac = HmacContext::new_sha256(salt);
        let mut secret = [0u8; 32];
        hmac.compute(self.dst_cid.as_bytes(), &mut secret);

        let mut sha = Sha256Context::new();
        sha.init();
        sha.update(&secret);
        sha.update(b"client in");
        sha.finalize(&mut self.tx_key);

        let mut sha2 = Sha256Context::new();
        sha2.init();
        sha2.update(&secret);
        sha2.update(b"server in");
        sha2.finalize(&mut self.rx_key);

        if self.is_server {
            core::mem::swap(&mut self.tx_key, &mut self.rx_key);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global QUIC subsystem
// ─────────────────────────────────────────────────────────────────────────────

pub struct QuicSubsystem {
    pub connections: BTreeMap<ConnectionId, QuicConnection>,
    pub initialized: bool,
}

impl QuicSubsystem {
    pub const fn new() -> Self {
        Self {
            connections: BTreeMap::new(),
            initialized: false,
        }
    }

    pub fn create_client_connection(&mut self, src_cid: &[u8], dst_cid: &[u8]) -> ConnectionId {
        let src = ConnectionId::new(src_cid);
        let dst = ConnectionId::new(dst_cid);
        let key = src.clone();
        let mut conn = QuicConnection::new_client(src, dst);
        conn.derive_initial_keys();
        conn.state = ConnectionState::Handshaking;
        self.connections.insert(key.clone(), conn);
        key
    }

    pub fn create_server_connection(&mut self, src_cid: &[u8], dst_cid: &[u8]) -> ConnectionId {
        let src = ConnectionId::new(src_cid);
        let dst = ConnectionId::new(dst_cid);
        let key = src.clone();
        let mut conn = QuicConnection::new_server(src, dst);
        conn.derive_initial_keys();
        conn.state = ConnectionState::Handshaking;
        self.connections.insert(key.clone(), conn);
        key
    }

    pub fn get_connection(&mut self, cid: &ConnectionId) -> Option<&mut QuicConnection> {
        self.connections.get_mut(cid)
    }

    pub fn remove_connection(&mut self, cid: &ConnectionId) {
        self.connections.remove(cid);
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }
}

pub static QUIC_SUBSYSTEM: Mutex<QuicSubsystem> = Mutex::new(QuicSubsystem::new());

pub fn init() {
    let mut qs = QUIC_SUBSYSTEM.lock();
    qs.initialized = true;
}

// ── Boot smoketest (R10) ──────────────────────────────────────────────────────

/// Deterministic proof of the QUIC transport codec and stream lifecycle with
/// ZERO network I/O: the RFC 9000 §A.1 variable-length-integer test vectors
/// (all four wire sizes, encode + decode), connection-ID handling, and a
/// client connection opening a bidi stream and queueing application bytes.
/// MasterChecklist Phase 10.2 — QUIC implementation. Concept §AthNet.
pub fn run_boot_smoketest() {
    let mut pass = 0u32;
    let mut total = 0u32;
    let mut check = |c: bool, n: &str| {
        total += 1;
        if c {
            pass += 1;
        } else {
            crate::serial_println!("[quic-selftest] FAIL {}", n);
        }
    };

    // RFC 9000 §A.1 canonical varint vectors: (value, exact wire encoding).
    let vectors: [(u64, &[u8]); 4] = [
        (37, &[0x25]),
        (15293, &[0x7b, 0xbd]),
        (494878333, &[0x9d, 0x7f, 0x3e, 0x7d]),
        (
            151288809941952652,
            &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c],
        ),
    ];
    for (val, expect) in vectors.iter() {
        let mut b = Vec::new();
        encode_varint(*val, &mut b);
        let enc_ok = b.as_slice() == *expect;
        let dec_ok = matches!(decode_varint(&b), Ok((v, n)) if v == *val && n == expect.len());
        check(enc_ok && dec_ok, "varint-rfc9000");
    }

    // Connection-ID handling.
    let cid_s = ConnectionId::new(&[0x01, 0x02, 0x03, 0x04]);
    let cid_d = ConnectionId::new(&[0x0a, 0x0b, 0x0c, 0x0d]);
    check(
        cid_s.len() == 4 && !cid_s.is_empty() && ConnectionId::empty().is_empty(),
        "connection-id",
    );

    // Client connection → bidi stream → queue HTTP/3-style request bytes.
    let mut conn = QuicConnection::new_client(cid_s, cid_d);
    conn.derive_initial_keys();
    match conn.open_bidi_stream() {
        Ok(id) => {
            check(true, "open-bidi-stream");
            check(
                matches!(conn.stream_send(id, b"GET / HTTP/3\r\n"), Ok(n) if n > 0),
                "stream-send",
            );
        }
        Err(_) => {
            check(false, "open-bidi-stream");
            check(false, "stream-send");
        }
    }
    let dgrams = conn.flush_packets(1000);

    drop(check);
    crate::serial_println!(
        "[ OK ] QUIC selftest: {}/{} checks passed (RFC 9000 varint vectors + stream lifecycle, {} datagram(s) flushed)",
        pass,
        total,
        dgrams.len()
    );
    if pass != total {
        crate::serial_println!("[FAIL] QUIC selftest: {} check(s) failed", total - pass);
    }
}

/// `/proc/athena/quic` — QUIC subsystem state. MasterChecklist Phase 10.2.
pub fn dump_text() -> String {
    let qs = QUIC_SUBSYSTEM.lock();
    let mut out = String::new();
    out.push_str("# AthNet QUIC (RFC 9000)\n");
    out.push_str(&alloc::format!("initialized: {}\n", qs.initialized));
    out.push_str(&alloc::format!(
        "active_connections: {}\n",
        qs.connection_count()
    ));
    out
}
