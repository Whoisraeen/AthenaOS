//! IPsec Stack — XFRM framework, ESP/AH/IPComp, PF_KEY, XFRM netlink,
//! key management, tunnel/transport mode, and VTI.

#![allow(dead_code)]

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeMap, string::String, vec, vec::Vec};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Common types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Addr(pub [u8; 4]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv6Addr(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpAddr {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpsecError {
    InvalidSpi,
    SaNotFound,
    PolicyNotFound,
    ReplayDetected,
    AuthenticationFailed,
    DecryptionFailed,
    EncryptionFailed,
    CompressionFailed,
    DecompressionFailed,
    BufferTooShort,
    InvalidHeader,
    UnsupportedAlgorithm,
    LifetimeExpired,
    InvalidMode,
    InvalidProtocol,
    PmtuTooSmall,
    NatTRequired,
    KeyNotFound,
    DatabaseFull,
    InvalidSelector,
    InvalidTemplate,
    MigrationFailed,
}

// ─────────────────────────────────────────────────────────────────────────────
// XFRM selector & address types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XfrmSelector {
    pub src: IpAddr,
    pub dst: IpAddr,
    pub src_prefix: u8,
    pub dst_prefix: u8,
    pub sport: u16,
    pub dport: u16,
    pub sport_mask: u16,
    pub dport_mask: u16,
    pub proto: u8,
    pub ifindex: u32,
    pub user: u32,
}

impl XfrmSelector {
    pub fn new_any() -> Self {
        Self {
            src: IpAddr::V4(Ipv4Addr([0; 4])),
            dst: IpAddr::V4(Ipv4Addr([0; 4])),
            src_prefix: 0,
            dst_prefix: 0,
            sport: 0,
            dport: 0,
            sport_mask: 0,
            dport_mask: 0,
            proto: 0,
            ifindex: 0,
            user: 0,
        }
    }

    pub fn matches(&self, src: &IpAddr, dst: &IpAddr, proto: u8, sport: u16, dport: u16) -> bool {
        if self.proto != 0 && self.proto != proto {
            return false;
        }
        if self.sport_mask != 0 && (sport & self.sport_mask) != (self.sport & self.sport_mask) {
            return false;
        }
        if self.dport_mask != 0 && (dport & self.dport_mask) != (self.dport & self.dport_mask) {
            return false;
        }
        self.addr_matches(src, &self.src, self.src_prefix)
            && self.addr_matches(dst, &self.dst, self.dst_prefix)
    }

    fn addr_matches(&self, addr: &IpAddr, net: &IpAddr, prefix: u8) -> bool {
        if prefix == 0 {
            return true;
        }
        match (addr, net) {
            (IpAddr::V4(a), IpAddr::V4(b)) => {
                let mask = if prefix >= 32 {
                    0xFFFF_FFFFu32
                } else {
                    !((1u32 << (32 - prefix)) - 1)
                };
                let a_u32 = u32::from_be_bytes(a.0);
                let b_u32 = u32::from_be_bytes(b.0);
                (a_u32 & mask) == (b_u32 & mask)
            }
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                let full = (prefix / 8) as usize;
                let rem = prefix % 8;
                if a.0[..full] != b.0[..full] {
                    return false;
                }
                if rem > 0 && full < 16 {
                    let mask = !((1u8 << (8 - rem)) - 1);
                    return (a.0[full] & mask) == (b.0[full] & mask);
                }
                true
            }
            _ => false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cryptographic algorithm identifiers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionAlgorithm {
    AesCbc128,
    AesCbc256,
    AesGcm128,
    AesGcm256,
    ChaCha20Poly1305,
    Null,
}

impl EncryptionAlgorithm {
    pub fn key_len(&self) -> usize {
        match self {
            Self::AesCbc128 | Self::AesGcm128 => 16,
            Self::AesCbc256 | Self::AesGcm256 => 32,
            Self::ChaCha20Poly1305 => 32,
            Self::Null => 0,
        }
    }

    pub fn iv_len(&self) -> usize {
        match self {
            Self::AesCbc128 | Self::AesCbc256 => 16,
            Self::AesGcm128 | Self::AesGcm256 => 8,
            Self::ChaCha20Poly1305 => 8,
            Self::Null => 0,
        }
    }

    pub fn block_size(&self) -> usize {
        match self {
            Self::AesCbc128 | Self::AesCbc256 => 16,
            Self::AesGcm128 | Self::AesGcm256 => 1,
            Self::ChaCha20Poly1305 => 1,
            Self::Null => 1,
        }
    }

    pub fn is_aead(&self) -> bool {
        matches!(
            self,
            Self::AesGcm128 | Self::AesGcm256 | Self::ChaCha20Poly1305
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAlgorithm {
    HmacSha256,
    HmacSha384,
    HmacSha512,
    AesGmac128,
    AesGmac256,
    None,
}

impl AuthAlgorithm {
    pub fn key_len(&self) -> usize {
        match self {
            Self::HmacSha256 => 32,
            Self::HmacSha384 => 48,
            Self::HmacSha512 => 64,
            Self::AesGmac128 => 16,
            Self::AesGmac256 => 32,
            Self::None => 0,
        }
    }

    pub fn icv_len(&self) -> usize {
        match self {
            Self::HmacSha256 => 16,
            Self::HmacSha384 => 24,
            Self::HmacSha512 => 32,
            Self::AesGmac128 | Self::AesGmac256 => 16,
            Self::None => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompAlgorithm {
    Deflate,
    Lzs,
    Lzjh,
}

impl CompAlgorithm {
    pub fn cpi(&self) -> u16 {
        match self {
            Self::Deflate => 2,
            Self::Lzs => 3,
            Self::Lzjh => 4,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// XFRM state — Security Association (SA)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum XfrmProto {
    Esp = 50,
    Ah = 51,
    IpComp = 108,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfrmMode {
    Transport,
    Tunnel,
    Beet,
}

#[derive(Debug, Clone)]
pub struct ReplayWindow {
    pub size: u32,
    pub seq_high: u32,
    pub bitmap: Vec<u32>,
    pub extended: bool,
    pub seq_hi_high: u32,
}

impl ReplayWindow {
    pub fn new(size: u32) -> Self {
        let bitmap_words = if size == 0 {
            1
        } else {
            ((size + 31) / 32) as usize
        };
        Self {
            size,
            seq_high: 0,
            bitmap: vec![0u32; bitmap_words],
            extended: false,
            seq_hi_high: 0,
        }
    }

    pub fn new_extended(size: u32) -> Self {
        let mut rw = Self::new(size);
        rw.extended = true;
        rw
    }

    pub fn check(&self, seq: u32) -> bool {
        if self.size == 0 {
            return true;
        }
        if seq == 0 {
            return false;
        }
        if seq > self.seq_high {
            return true;
        }
        let diff = self.seq_high - seq;
        if diff >= self.size {
            return false;
        }
        let word_idx = (diff / 32) as usize;
        let bit_idx = diff % 32;
        if word_idx >= self.bitmap.len() {
            return false;
        }
        self.bitmap[word_idx] & (1 << bit_idx) == 0
    }

    pub fn advance(&mut self, seq: u32) {
        if self.size == 0 {
            return;
        }
        if seq > self.seq_high {
            let shift = seq - self.seq_high;
            if shift >= self.size {
                for w in self.bitmap.iter_mut() {
                    *w = 0;
                }
            } else {
                let word_shift = (shift / 32) as usize;
                let bit_shift = (shift % 32) as usize;
                if word_shift > 0 && word_shift < self.bitmap.len() {
                    let len = self.bitmap.len();
                    for i in (word_shift..len).rev() {
                        self.bitmap[i] = self.bitmap[i - word_shift];
                    }
                    for i in 0..word_shift.min(len) {
                        self.bitmap[i] = 0;
                    }
                }
                if bit_shift > 0 {
                    let len = self.bitmap.len();
                    for i in (1..len).rev() {
                        self.bitmap[i] = (self.bitmap[i] << bit_shift)
                            | (self.bitmap[i - 1] >> (32 - bit_shift));
                    }
                    self.bitmap[0] <<= bit_shift;
                }
            }
            self.seq_high = seq;
        }
        let diff = self.seq_high - seq;
        let word_idx = (diff / 32) as usize;
        let bit_idx = diff % 32;
        if word_idx < self.bitmap.len() {
            self.bitmap[word_idx] |= 1 << bit_idx;
        }
    }

    pub fn check_and_advance(&mut self, seq: u32) -> bool {
        if !self.check(seq) {
            return false;
        }
        self.advance(seq);
        true
    }
}

#[derive(Debug, Clone)]
pub struct SaLifetime {
    pub hard_byte_limit: u64,
    pub soft_byte_limit: u64,
    pub hard_packet_limit: u64,
    pub soft_packet_limit: u64,
    pub hard_time_limit: u64,
    pub soft_time_limit: u64,
    pub hard_use_time: u64,
    pub soft_use_time: u64,
}

impl SaLifetime {
    pub fn unlimited() -> Self {
        Self {
            hard_byte_limit: u64::MAX,
            soft_byte_limit: u64::MAX,
            hard_packet_limit: u64::MAX,
            soft_packet_limit: u64::MAX,
            hard_time_limit: u64::MAX,
            soft_time_limit: u64::MAX,
            hard_use_time: u64::MAX,
            soft_use_time: u64::MAX,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SaCurrentLifetime {
    pub bytes: u64,
    pub packets: u64,
    pub add_time: u64,
    pub use_time: u64,
}

impl SaCurrentLifetime {
    pub fn new(add_time: u64) -> Self {
        Self {
            bytes: 0,
            packets: 0,
            add_time,
            use_time: 0,
        }
    }

    pub fn exceeded_hard(&self, limits: &SaLifetime) -> bool {
        self.bytes >= limits.hard_byte_limit || self.packets >= limits.hard_packet_limit
    }

    pub fn exceeded_soft(&self, limits: &SaLifetime) -> bool {
        self.bytes >= limits.soft_byte_limit || self.packets >= limits.soft_packet_limit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatTEncap {
    None,
    UdpEncap4500,
    UdpEncapCustom(u16),
}

#[derive(Debug, Clone)]
pub struct XfrmState {
    pub spi: u32,
    pub proto: XfrmProto,
    pub mode: XfrmMode,
    pub src: IpAddr,
    pub dst: IpAddr,
    pub selector: XfrmSelector,
    pub enc_alg: EncryptionAlgorithm,
    pub enc_key: Vec<u8>,
    pub auth_alg: AuthAlgorithm,
    pub auth_key: Vec<u8>,
    pub comp_alg: Option<CompAlgorithm>,
    pub replay: ReplayWindow,
    pub seq_number: u64,
    pub lifetime: SaLifetime,
    pub current_lifetime: SaCurrentLifetime,
    pub nat_t: NatTEncap,
    pub reqid: u32,
    pub flags: u32,
    pub extra_flags: u32,
    pub output_mark: u32,
    pub if_id: u32,
    pub tfcpad: u16,
}

impl XfrmState {
    pub fn new(spi: u32, proto: XfrmProto, mode: XfrmMode, src: IpAddr, dst: IpAddr) -> Self {
        Self {
            spi,
            proto,
            mode,
            src,
            dst,
            selector: XfrmSelector::new_any(),
            enc_alg: EncryptionAlgorithm::Null,
            enc_key: Vec::new(),
            auth_alg: AuthAlgorithm::None,
            auth_key: Vec::new(),
            comp_alg: None,
            replay: ReplayWindow::new(64),
            seq_number: 0,
            lifetime: SaLifetime::unlimited(),
            current_lifetime: SaCurrentLifetime::new(0),
            nat_t: NatTEncap::None,
            reqid: 0,
            flags: 0,
            extra_flags: 0,
            output_mark: 0,
            if_id: 0,
            tfcpad: 0,
        }
    }

    pub fn set_encryption(
        &mut self,
        alg: EncryptionAlgorithm,
        key: Vec<u8>,
    ) -> Result<(), IpsecError> {
        if key.len() != alg.key_len() {
            return Err(IpsecError::UnsupportedAlgorithm);
        }
        self.enc_alg = alg;
        self.enc_key = key;
        Ok(())
    }

    pub fn set_auth(&mut self, alg: AuthAlgorithm, key: Vec<u8>) -> Result<(), IpsecError> {
        if key.len() != alg.key_len() {
            return Err(IpsecError::UnsupportedAlgorithm);
        }
        self.auth_alg = alg;
        self.auth_key = key;
        Ok(())
    }

    pub fn set_compression(&mut self, alg: CompAlgorithm) {
        self.comp_alg = Some(alg);
    }

    pub fn next_seq(&mut self) -> u64 {
        self.seq_number += 1;
        self.seq_number
    }

    pub fn is_expired_hard(&self) -> bool {
        self.current_lifetime.exceeded_hard(&self.lifetime)
    }

    pub fn is_expired_soft(&self) -> bool {
        self.current_lifetime.exceeded_soft(&self.lifetime)
    }

    pub fn account(&mut self, bytes: u64) {
        self.current_lifetime.bytes += bytes;
        self.current_lifetime.packets += 1;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ESP — Encapsulating Security Payload (RFC 4303)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct EspHeader {
    pub spi: u32,
    pub seq: u32,
}

pub const ESP_HEADER_LEN: usize = 8;

pub struct EspProcessor;

impl EspProcessor {
    pub fn encapsulate(
        sa: &mut XfrmState,
        payload: &[u8],
        next_header: u8,
    ) -> Result<Vec<u8>, IpsecError> {
        if sa.is_expired_hard() {
            return Err(IpsecError::LifetimeExpired);
        }
        let seq = sa.next_seq();
        let mut pkt = Vec::new();

        pkt.extend_from_slice(&sa.spi.to_be_bytes());
        pkt.extend_from_slice(&(seq as u32).to_be_bytes());

        let iv_len = sa.enc_alg.iv_len();
        let iv = vec![0u8; iv_len];
        pkt.extend_from_slice(&iv);

        pkt.extend_from_slice(payload);

        let block_size = sa.enc_alg.block_size().max(4);
        let pad_len = {
            let current = payload.len() + 2;
            let remainder = current % block_size;
            if remainder == 0 {
                0
            } else {
                block_size - remainder
            }
        };
        for i in 0..pad_len {
            pkt.push((i + 1) as u8);
        }
        pkt.push(pad_len as u8);
        pkt.push(next_header);

        if sa.tfcpad > 0 {
            let tfc = vec![0u8; sa.tfcpad as usize];
            let insert_pos = ESP_HEADER_LEN + iv_len;
            let tail = pkt.split_off(insert_pos);
            pkt.extend_from_slice(&tfc);
            pkt.extend(tail);
        }

        if sa.enc_alg.is_aead() {
            let icv = vec![0u8; 16];
            pkt.extend_from_slice(&icv);
        } else {
            let icv_len = sa.auth_alg.icv_len();
            if icv_len > 0 {
                let icv = Self::compute_auth(&sa.auth_alg, &sa.auth_key, &pkt);
                pkt.extend_from_slice(&icv[..icv_len]);
            }
        }

        if let NatTEncap::UdpEncap4500 | NatTEncap::UdpEncapCustom(_) = sa.nat_t {
            let mut udp_pkt = Vec::new();
            let port = match sa.nat_t {
                NatTEncap::UdpEncap4500 => 4500u16,
                NatTEncap::UdpEncapCustom(p) => p,
                _ => 4500,
            };
            udp_pkt.extend_from_slice(&port.to_be_bytes());
            udp_pkt.extend_from_slice(&port.to_be_bytes());
            let udp_len = (8 + pkt.len()) as u16;
            udp_pkt.extend_from_slice(&udp_len.to_be_bytes());
            udp_pkt.extend_from_slice(&0u16.to_be_bytes());
            udp_pkt.extend_from_slice(&pkt);
            pkt = udp_pkt;
        }

        sa.account(pkt.len() as u64);
        Ok(pkt)
    }

    pub fn decapsulate(sa: &mut XfrmState, packet: &[u8]) -> Result<(Vec<u8>, u8), IpsecError> {
        if sa.is_expired_hard() {
            return Err(IpsecError::LifetimeExpired);
        }
        let mut data = packet;
        if let NatTEncap::UdpEncap4500 | NatTEncap::UdpEncapCustom(_) = sa.nat_t {
            if data.len() < 8 {
                return Err(IpsecError::BufferTooShort);
            }
            data = &data[8..];
        }
        if data.len() < ESP_HEADER_LEN {
            return Err(IpsecError::BufferTooShort);
        }
        let spi = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if spi != sa.spi {
            return Err(IpsecError::InvalidSpi);
        }
        let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        if !sa.replay.check_and_advance(seq) {
            return Err(IpsecError::ReplayDetected);
        }

        let icv_len = if sa.enc_alg.is_aead() {
            16
        } else {
            sa.auth_alg.icv_len()
        };

        if !sa.enc_alg.is_aead() && icv_len > 0 {
            if data.len() < icv_len {
                return Err(IpsecError::BufferTooShort);
            }
            let auth_data = &data[..data.len() - icv_len];
            let received_icv = &data[data.len() - icv_len..];
            let computed = Self::compute_auth(&sa.auth_alg, &sa.auth_key, auth_data);
            if computed[..icv_len] != *received_icv {
                return Err(IpsecError::AuthenticationFailed);
            }
        }

        let iv_len = sa.enc_alg.iv_len();
        let cipher_start = ESP_HEADER_LEN + iv_len;
        let cipher_end = data.len() - icv_len;
        if cipher_start >= cipher_end {
            return Err(IpsecError::BufferTooShort);
        }

        let plaintext = data[cipher_start..cipher_end].to_vec();

        if plaintext.len() < 2 {
            return Err(IpsecError::InvalidHeader);
        }
        let pad_len = plaintext[plaintext.len() - 2] as usize;
        let next_header = plaintext[plaintext.len() - 1];
        let payload_end = plaintext.len() - 2 - pad_len;

        sa.account(packet.len() as u64);
        Ok((plaintext[..payload_end].to_vec(), next_header))
    }

    fn compute_auth(alg: &AuthAlgorithm, key: &[u8], data: &[u8]) -> Vec<u8> {
        let icv_len = alg.icv_len();
        match alg {
            AuthAlgorithm::HmacSha256 | AuthAlgorithm::HmacSha384 | AuthAlgorithm::HmacSha512 => {
                let block_size = 64usize;
                let mut k = vec![0u8; block_size];
                if key.len() > block_size {
                    let h = Self::sha256_stub(key);
                    k[..h.len().min(block_size)].copy_from_slice(&h[..h.len().min(block_size)]);
                } else {
                    k[..key.len()].copy_from_slice(key);
                }
                let mut ipad = vec![0x36u8; block_size];
                let mut opad = vec![0x5Cu8; block_size];
                for i in 0..block_size {
                    ipad[i] ^= k[i];
                    opad[i] ^= k[i];
                }
                ipad.extend_from_slice(data);
                let inner = Self::sha256_stub(&ipad);
                opad.extend_from_slice(&inner);
                let outer = Self::sha256_stub(&opad);
                outer[..icv_len.min(outer.len())].to_vec()
            }
            AuthAlgorithm::AesGmac128 | AuthAlgorithm::AesGmac256 => {
                vec![0u8; icv_len]
            }
            AuthAlgorithm::None => Vec::new(),
        }
    }

    fn sha256_stub(data: &[u8]) -> Vec<u8> {
        let mut hash = vec![0u8; 32];
        for (i, byte) in data.iter().enumerate() {
            hash[i % 32] ^= byte;
            hash[(i + 7) % 32] = hash[(i + 7) % 32].wrapping_add(*byte);
        }
        hash
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AH — Authentication Header (RFC 4302)
// ─────────────────────────────────────────────────────────────────────────────

pub const AH_HEADER_LEN: usize = 12;

#[derive(Debug)]
pub struct AhHeader {
    pub next_header: u8,
    pub payload_len: u8,
    pub reserved: u16,
    pub spi: u32,
    pub seq: u32,
}

pub struct AhProcessor;

impl AhProcessor {
    pub fn encapsulate(
        sa: &mut XfrmState,
        payload: &[u8],
        next_header: u8,
    ) -> Result<Vec<u8>, IpsecError> {
        if sa.is_expired_hard() {
            return Err(IpsecError::LifetimeExpired);
        }
        let seq = sa.next_seq();
        let icv_len = sa.auth_alg.icv_len();
        let ah_len = AH_HEADER_LEN + icv_len;
        let ah_payload_len = ((ah_len / 4) - 2) as u8;

        let mut pkt = Vec::new();
        pkt.push(next_header);
        pkt.push(ah_payload_len);
        pkt.extend_from_slice(&0u16.to_be_bytes());
        pkt.extend_from_slice(&sa.spi.to_be_bytes());
        pkt.extend_from_slice(&(seq as u32).to_be_bytes());

        let icv_offset = pkt.len();
        pkt.extend_from_slice(&vec![0u8; icv_len]);

        pkt.extend_from_slice(payload);

        let mut auth_data = pkt.clone();
        for i in icv_offset..icv_offset + icv_len {
            auth_data[i] = 0;
        }
        let icv = EspProcessor::compute_auth(&sa.auth_alg, &sa.auth_key, &auth_data);
        pkt[icv_offset..icv_offset + icv_len].copy_from_slice(&icv[..icv_len]);

        sa.account(pkt.len() as u64);
        Ok(pkt)
    }

    pub fn decapsulate(sa: &mut XfrmState, packet: &[u8]) -> Result<(Vec<u8>, u8), IpsecError> {
        if sa.is_expired_hard() {
            return Err(IpsecError::LifetimeExpired);
        }
        if packet.len() < AH_HEADER_LEN {
            return Err(IpsecError::BufferTooShort);
        }
        let next_header = packet[0];
        let payload_len_field = packet[1] as usize;
        let ah_len = (payload_len_field + 2) * 4;
        let spi = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
        if spi != sa.spi {
            return Err(IpsecError::InvalidSpi);
        }
        let seq = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
        if !sa.replay.check_and_advance(seq) {
            return Err(IpsecError::ReplayDetected);
        }

        let icv_len = sa.auth_alg.icv_len();
        let icv_offset = AH_HEADER_LEN;
        if packet.len() < icv_offset + icv_len {
            return Err(IpsecError::BufferTooShort);
        }
        let received_icv = &packet[icv_offset..icv_offset + icv_len];

        let mut verify_data = packet.to_vec();
        for i in icv_offset..icv_offset + icv_len {
            verify_data[i] = 0;
        }
        let computed = EspProcessor::compute_auth(&sa.auth_alg, &sa.auth_key, &verify_data);
        if computed[..icv_len] != *received_icv {
            return Err(IpsecError::AuthenticationFailed);
        }

        if packet.len() < ah_len {
            return Err(IpsecError::BufferTooShort);
        }
        let payload = packet[ah_len..].to_vec();
        sa.account(packet.len() as u64);
        Ok((payload, next_header))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IPComp — IP Payload Compression (RFC 3173)
// ─────────────────────────────────────────────────────────────────────────────

pub const IPCOMP_HEADER_LEN: usize = 4;

#[derive(Debug)]
pub struct IpCompHeader {
    pub next_header: u8,
    pub flags: u8,
    pub cpi: u16,
}

pub struct IpCompProcessor;

impl IpCompProcessor {
    pub fn compress(
        sa: &mut XfrmState,
        payload: &[u8],
        next_header: u8,
    ) -> Result<Vec<u8>, IpsecError> {
        let alg = sa.comp_alg.ok_or(IpsecError::UnsupportedAlgorithm)?;
        let cpi = alg.cpi();

        let compressed = Self::compress_data(alg, payload)?;

        if compressed.len() >= payload.len() {
            let mut pkt = Vec::with_capacity(payload.len());
            pkt.extend_from_slice(payload);
            return Ok(pkt);
        }

        let mut pkt = Vec::new();
        pkt.push(next_header);
        pkt.push(0);
        pkt.extend_from_slice(&cpi.to_be_bytes());
        pkt.extend_from_slice(&compressed);

        sa.account(pkt.len() as u64);
        Ok(pkt)
    }

    pub fn decompress(sa: &mut XfrmState, packet: &[u8]) -> Result<(Vec<u8>, u8), IpsecError> {
        if packet.len() < IPCOMP_HEADER_LEN {
            return Err(IpsecError::BufferTooShort);
        }
        let next_header = packet[0];
        let cpi = ((packet[2] as u16) << 8) | packet[3] as u16;

        let alg = match cpi {
            2 => CompAlgorithm::Deflate,
            3 => CompAlgorithm::Lzs,
            4 => CompAlgorithm::Lzjh,
            _ => return Err(IpsecError::UnsupportedAlgorithm),
        };

        let compressed = &packet[IPCOMP_HEADER_LEN..];
        let decompressed = Self::decompress_data(alg, compressed)?;

        sa.account(packet.len() as u64);
        Ok((decompressed, next_header))
    }

    fn compress_data(alg: CompAlgorithm, data: &[u8]) -> Result<Vec<u8>, IpsecError> {
        match alg {
            CompAlgorithm::Deflate => Ok(Self::deflate_stub(data)),
            CompAlgorithm::Lzs => Ok(Self::lzs_stub(data)),
            CompAlgorithm::Lzjh => Ok(Self::lzjh_stub(data)),
        }
    }

    fn decompress_data(alg: CompAlgorithm, data: &[u8]) -> Result<Vec<u8>, IpsecError> {
        match alg {
            CompAlgorithm::Deflate => Ok(Self::inflate_stub(data)),
            CompAlgorithm::Lzs => Ok(data.to_vec()),
            CompAlgorithm::Lzjh => Ok(data.to_vec()),
        }
    }

    fn deflate_stub(data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }

    fn inflate_stub(data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }

    fn lzs_stub(data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }

    fn lzjh_stub(data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// XFRM policy — Security Policy Database (SPD)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfrmPolicyDir {
    In,
    Out,
    Fwd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfrmPolicyAction {
    Allow,
    Block,
}

#[derive(Debug, Clone)]
pub struct XfrmTemplate {
    pub src: IpAddr,
    pub dst: IpAddr,
    pub spi: u32,
    pub proto: XfrmProto,
    pub mode: XfrmMode,
    pub reqid: u32,
    pub share: u8,
    pub optional: bool,
    pub aalgos: u32,
    pub ealgos: u32,
    pub calgos: u32,
}

impl XfrmTemplate {
    pub fn new(proto: XfrmProto, mode: XfrmMode, src: IpAddr, dst: IpAddr) -> Self {
        Self {
            src,
            dst,
            spi: 0,
            proto,
            mode,
            reqid: 0,
            share: 0,
            optional: false,
            aalgos: 0xFFFF_FFFF,
            ealgos: 0xFFFF_FFFF,
            calgos: 0xFFFF_FFFF,
        }
    }
}

#[derive(Debug, Clone)]
pub struct XfrmPolicy {
    pub index: u32,
    pub dir: XfrmPolicyDir,
    pub action: XfrmPolicyAction,
    pub priority: u32,
    pub selector: XfrmSelector,
    pub templates: Vec<XfrmTemplate>,
    pub lifetime: SaLifetime,
    pub current_lifetime: SaCurrentLifetime,
    pub flags: u32,
    pub if_id: u32,
    pub mark: u32,
    pub mark_mask: u32,
}

impl XfrmPolicy {
    pub fn new(index: u32, dir: XfrmPolicyDir, action: XfrmPolicyAction, priority: u32) -> Self {
        Self {
            index,
            dir,
            action,
            priority,
            selector: XfrmSelector::new_any(),
            templates: Vec::new(),
            lifetime: SaLifetime::unlimited(),
            current_lifetime: SaCurrentLifetime::new(0),
            flags: 0,
            if_id: 0,
            mark: 0,
            mark_mask: 0,
        }
    }

    pub fn add_template(&mut self, tmpl: XfrmTemplate) {
        self.templates.push(tmpl);
    }

    pub fn matches(&self, src: &IpAddr, dst: &IpAddr, proto: u8, sport: u16, dport: u16) -> bool {
        self.selector.matches(src, dst, proto, sport, dport)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PF_KEY socket — SADB messages (RFC 2367)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SadbMsgType {
    Reserved = 0,
    GetSpi = 1,
    Update = 2,
    Add = 3,
    Delete = 4,
    Get = 5,
    Acquire = 6,
    Register = 7,
    Expire = 8,
    Flush = 9,
    Dump = 10,
    XPrSaFlags = 11,
    XPolicyAdd = 12,
    XPolicyDelete = 13,
    XPolicyGet = 14,
    XPolicyAcquire = 15,
    XPolicyDump = 16,
    XPolicyFlush = 17,
    XPolicyChange = 18,
}

#[derive(Debug, Clone)]
pub struct SadbMessage {
    pub version: u8,
    pub msg_type: SadbMsgType,
    pub errno: u8,
    pub satype: u8,
    pub len: u16,
    pub seq: u32,
    pub pid: u32,
    pub sa: Option<SadbSaPayload>,
    pub address_src: Option<IpAddr>,
    pub address_dst: Option<IpAddr>,
    pub key_enc: Option<Vec<u8>>,
    pub key_auth: Option<Vec<u8>>,
    pub lifetime_hard: Option<SaLifetime>,
    pub lifetime_soft: Option<SaLifetime>,
    pub selector: Option<XfrmSelector>,
}

#[derive(Debug, Clone)]
pub struct SadbSaPayload {
    pub spi: u32,
    pub replay_window: u8,
    pub state: u8,
    pub auth: u8,
    pub encrypt: u8,
    pub flags: u32,
}

pub struct PfKeySocket {
    pub registered_sa_types: Vec<u8>,
    pub seq_counter: u32,
    pub pid: u32,
}

impl PfKeySocket {
    pub fn new(pid: u32) -> Self {
        Self {
            registered_sa_types: Vec::new(),
            seq_counter: 0,
            pid,
        }
    }

    pub fn next_seq(&mut self) -> u32 {
        self.seq_counter += 1;
        self.seq_counter
    }

    pub fn send_message(&mut self, msg: &SadbMessage) -> Result<(), IpsecError> {
        match msg.msg_type {
            SadbMsgType::GetSpi => self.handle_getspi(msg),
            SadbMsgType::Update => self.handle_update(msg),
            SadbMsgType::Add => self.handle_add(msg),
            SadbMsgType::Delete => self.handle_delete(msg),
            SadbMsgType::Get => self.handle_get(msg),
            SadbMsgType::Register => self.handle_register(msg),
            SadbMsgType::Flush => self.handle_flush(msg),
            SadbMsgType::Dump => self.handle_dump(msg),
            SadbMsgType::XPolicyAdd => self.handle_policy_add(msg),
            SadbMsgType::XPolicyDelete => self.handle_policy_delete(msg),
            SadbMsgType::XPolicyGet => self.handle_policy_get(msg),
            SadbMsgType::XPolicyDump => self.handle_policy_dump(msg),
            SadbMsgType::XPolicyFlush => self.handle_policy_flush(msg),
            _ => Ok(()),
        }
    }

    fn handle_getspi(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_update(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_add(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_delete(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_get(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_register(&mut self, msg: &SadbMessage) -> Result<(), IpsecError> {
        if !self.registered_sa_types.contains(&msg.satype) {
            self.registered_sa_types.push(msg.satype);
        }
        Ok(())
    }
    fn handle_flush(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_dump(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_policy_add(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_policy_delete(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_policy_get(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_policy_dump(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
    fn handle_policy_flush(&mut self, _msg: &SadbMessage) -> Result<(), IpsecError> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// XFRM netlink messages
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum XfrmMsgType {
    NewSa = 16,
    DelSa = 17,
    GetSa = 18,
    NewPolicy = 19,
    DelPolicy = 20,
    GetPolicy = 21,
    AllocSpi = 22,
    Acquire = 23,
    Expire = 24,
    UpdPolicy = 25,
    UpdSa = 26,
    PolExpire = 27,
    FlushSa = 28,
    FlushPolicy = 29,
    Migrate = 30,
    Report = 31,
    NewAe = 32,
    GetAe = 33,
    Mapping = 34,
}

#[derive(Debug, Clone)]
pub struct XfrmNetlinkMessage {
    pub msg_type: XfrmMsgType,
    pub flags: u16,
    pub seq: u32,
    pub pid: u32,
    pub sa_info: Option<XfrmState>,
    pub policy_info: Option<XfrmPolicy>,
    pub spi_info: Option<SpiAllocInfo>,
    pub expire_info: Option<ExpireInfo>,
    pub migrate_info: Option<MigrateInfo>,
}

#[derive(Debug, Clone)]
pub struct SpiAllocInfo {
    pub src: IpAddr,
    pub dst: IpAddr,
    pub proto: XfrmProto,
    pub min_spi: u32,
    pub max_spi: u32,
    pub allocated_spi: u32,
}

#[derive(Debug, Clone)]
pub struct ExpireInfo {
    pub spi: u32,
    pub proto: XfrmProto,
    pub src: IpAddr,
    pub dst: IpAddr,
    pub hard: bool,
}

#[derive(Debug, Clone)]
pub struct MigrateInfo {
    pub old_src: IpAddr,
    pub old_dst: IpAddr,
    pub new_src: IpAddr,
    pub new_dst: IpAddr,
    pub proto: XfrmProto,
    pub mode: XfrmMode,
    pub reqid: u32,
}

pub struct XfrmNetlinkSocket {
    pub pid: u32,
    pub groups: u32,
    pub seq_counter: u32,
}

impl XfrmNetlinkSocket {
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            groups: 0,
            seq_counter: 0,
        }
    }

    pub fn next_seq(&mut self) -> u32 {
        self.seq_counter += 1;
        self.seq_counter
    }

    pub fn process_message(
        &mut self,
        msg: &XfrmNetlinkMessage,
        stack: &mut IpsecStack,
    ) -> Result<Option<XfrmNetlinkMessage>, IpsecError> {
        match msg.msg_type {
            XfrmMsgType::NewSa | XfrmMsgType::UpdSa => {
                if let Some(ref sa) = msg.sa_info {
                    stack.add_sa(sa.clone());
                }
                Ok(None)
            }
            XfrmMsgType::DelSa => {
                if let Some(ref sa) = msg.sa_info {
                    stack.remove_sa(sa.spi, &sa.dst, sa.proto);
                }
                Ok(None)
            }
            XfrmMsgType::GetSa => {
                if let Some(ref sa) = msg.sa_info {
                    let found = stack.lookup_sa(sa.spi, &sa.dst, sa.proto);
                    if let Some(found_sa) = found {
                        let reply = XfrmNetlinkMessage {
                            msg_type: XfrmMsgType::NewSa,
                            flags: 0,
                            seq: msg.seq,
                            pid: msg.pid,
                            sa_info: Some(found_sa.clone()),
                            policy_info: None,
                            spi_info: None,
                            expire_info: None,
                            migrate_info: None,
                        };
                        Ok(Some(reply))
                    } else {
                        Err(IpsecError::SaNotFound)
                    }
                } else {
                    Err(IpsecError::SaNotFound)
                }
            }
            XfrmMsgType::NewPolicy | XfrmMsgType::UpdPolicy => {
                if let Some(ref policy) = msg.policy_info {
                    stack.add_policy(policy.clone());
                }
                Ok(None)
            }
            XfrmMsgType::DelPolicy => {
                if let Some(ref policy) = msg.policy_info {
                    stack.remove_policy(policy.index, policy.dir);
                }
                Ok(None)
            }
            XfrmMsgType::GetPolicy => {
                if let Some(ref policy) = msg.policy_info {
                    let found = stack.lookup_policy_by_index(policy.index, policy.dir);
                    if let Some(found_pol) = found {
                        let reply = XfrmNetlinkMessage {
                            msg_type: XfrmMsgType::NewPolicy,
                            flags: 0,
                            seq: msg.seq,
                            pid: msg.pid,
                            sa_info: None,
                            policy_info: Some(found_pol.clone()),
                            spi_info: None,
                            expire_info: None,
                            migrate_info: None,
                        };
                        Ok(Some(reply))
                    } else {
                        Err(IpsecError::PolicyNotFound)
                    }
                } else {
                    Err(IpsecError::PolicyNotFound)
                }
            }
            XfrmMsgType::AllocSpi => {
                if let Some(ref spi_info) = msg.spi_info {
                    let spi = stack.alloc_spi(spi_info.min_spi, spi_info.max_spi);
                    let reply = XfrmNetlinkMessage {
                        msg_type: XfrmMsgType::NewSa,
                        flags: 0,
                        seq: msg.seq,
                        pid: msg.pid,
                        sa_info: None,
                        policy_info: None,
                        spi_info: Some(SpiAllocInfo {
                            src: spi_info.src,
                            dst: spi_info.dst,
                            proto: spi_info.proto,
                            min_spi: spi_info.min_spi,
                            max_spi: spi_info.max_spi,
                            allocated_spi: spi,
                        }),
                        expire_info: None,
                        migrate_info: None,
                    };
                    Ok(Some(reply))
                } else {
                    Err(IpsecError::InvalidSpi)
                }
            }
            XfrmMsgType::FlushSa => {
                stack.flush_sa();
                Ok(None)
            }
            XfrmMsgType::FlushPolicy => {
                stack.flush_policy();
                Ok(None)
            }
            XfrmMsgType::Acquire => Ok(None),
            XfrmMsgType::Expire => Ok(None),
            XfrmMsgType::PolExpire => Ok(None),
            XfrmMsgType::Migrate => {
                if let Some(ref mig) = msg.migrate_info {
                    stack.migrate_sa(mig)?;
                }
                Ok(None)
            }
            XfrmMsgType::Report => Ok(None),
            _ => Ok(None),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Key management
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMgmtEvent {
    Acquire,
    ExpireSoft,
    ExpireHard,
    NewSa,
    DeleteSa,
    FlushSa,
}

#[derive(Debug, Clone)]
pub struct AcquireRequest {
    pub policy_index: u32,
    pub selector: XfrmSelector,
    pub templates: Vec<XfrmTemplate>,
    pub seq: u32,
}

#[derive(Debug, Clone)]
pub struct ManualKey {
    pub spi: u32,
    pub proto: XfrmProto,
    pub mode: XfrmMode,
    pub src: IpAddr,
    pub dst: IpAddr,
    pub enc_alg: EncryptionAlgorithm,
    pub enc_key: Vec<u8>,
    pub auth_alg: AuthAlgorithm,
    pub auth_key: Vec<u8>,
}

pub struct KeyManager {
    pub pending_acquires: Vec<AcquireRequest>,
    pub manual_keys: Vec<ManualKey>,
    pub registered_listeners: Vec<u32>,
    pub ike_daemon_pid: Option<u32>,
}

impl KeyManager {
    pub fn new() -> Self {
        Self {
            pending_acquires: Vec::new(),
            manual_keys: Vec::new(),
            registered_listeners: Vec::new(),
            ike_daemon_pid: None,
        }
    }

    pub fn register_ike_daemon(&mut self, pid: u32) {
        self.ike_daemon_pid = Some(pid);
    }

    pub fn send_acquire(&mut self, req: AcquireRequest) {
        self.pending_acquires.push(req);
    }

    pub fn add_manual_key(&mut self, key: ManualKey) -> Result<(), IpsecError> {
        if key.enc_key.len() != key.enc_alg.key_len() {
            return Err(IpsecError::UnsupportedAlgorithm);
        }
        if key.auth_key.len() != key.auth_alg.key_len() {
            return Err(IpsecError::UnsupportedAlgorithm);
        }
        self.manual_keys.push(key);
        Ok(())
    }

    pub fn install_manual_key(
        &self,
        key: &ManualKey,
        stack: &mut IpsecStack,
    ) -> Result<(), IpsecError> {
        let mut sa = XfrmState::new(key.spi, key.proto, key.mode, key.src, key.dst);
        sa.set_encryption(key.enc_alg, key.enc_key.clone())?;
        sa.set_auth(key.auth_alg, key.auth_key.clone())?;
        stack.add_sa(sa);
        Ok(())
    }

    pub fn register_listener(&mut self, pid: u32) {
        if !self.registered_listeners.contains(&pid) {
            self.registered_listeners.push(pid);
        }
    }

    pub fn notify_expire(&self, _spi: u32, _proto: XfrmProto, _hard: bool) {
        // Notification would be sent to IKE daemon in production
    }

    pub fn resolve_acquire(&mut self, seq: u32) -> Option<AcquireRequest> {
        if let Some(pos) = self.pending_acquires.iter().position(|a| a.seq == seq) {
            Some(self.pending_acquires.remove(pos))
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tunnel mode — outer header construction, PMTU, ECN
// ─────────────────────────────────────────────────────────────────────────────

pub struct TunnelModeProcessor;

impl TunnelModeProcessor {
    pub fn build_outer_ipv4(
        src: &Ipv4Addr,
        dst: &Ipv4Addr,
        inner: &[u8],
        proto: u8,
        df: bool,
        tos: u8,
        ttl: u8,
    ) -> Vec<u8> {
        let total_len = (20 + inner.len()) as u16;
        let mut pkt = Vec::with_capacity(20 + inner.len());
        pkt.push(0x45);
        pkt.push(tos);
        pkt.extend_from_slice(&total_len.to_be_bytes());
        pkt.extend_from_slice(&[0, 0]);
        let flags_frag: u16 = if df { 0x4000 } else { 0 };
        pkt.extend_from_slice(&flags_frag.to_be_bytes());
        pkt.push(ttl);
        pkt.push(proto);
        pkt.extend_from_slice(&[0, 0]);
        pkt.extend_from_slice(&src.0);
        pkt.extend_from_slice(&dst.0);
        pkt.extend_from_slice(inner);
        pkt
    }

    pub fn build_outer_ipv6(
        src: &Ipv6Addr,
        dst: &Ipv6Addr,
        inner: &[u8],
        next_header: u8,
        hop_limit: u8,
        traffic_class: u8,
    ) -> Vec<u8> {
        let payload_len = inner.len() as u16;
        let mut pkt = Vec::with_capacity(40 + inner.len());
        let ver_tc_fl: u32 = (6 << 28) | ((traffic_class as u32) << 20);
        pkt.extend_from_slice(&ver_tc_fl.to_be_bytes());
        pkt.extend_from_slice(&payload_len.to_be_bytes());
        pkt.push(next_header);
        pkt.push(hop_limit);
        pkt.extend_from_slice(&src.0);
        pkt.extend_from_slice(&dst.0);
        pkt.extend_from_slice(inner);
        pkt
    }

    pub fn pmtu_check(outer_mtu: u16, overhead: u16) -> u16 {
        if outer_mtu > overhead {
            outer_mtu - overhead
        } else {
            0
        }
    }

    pub fn ecn_encap(inner_tos: u8) -> u8 {
        inner_tos & 0x03
    }

    pub fn ecn_decap(outer_ecn: u8, inner_ecn: u8) -> u8 {
        if outer_ecn == 0x03 && inner_ecn != 0x00 {
            0x03
        } else {
            inner_ecn
        }
    }

    pub fn df_handling(inner_df: bool, outer_df_policy: DfPolicy) -> bool {
        match outer_df_policy {
            DfPolicy::Copy => inner_df,
            DfPolicy::Set => true,
            DfPolicy::Clear => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfPolicy {
    Copy,
    Set,
    Clear,
}

// ─────────────────────────────────────────────────────────────────────────────
// Transport mode — header insertion, NAT-T
// ─────────────────────────────────────────────────────────────────────────────

pub struct TransportModeProcessor;

impl TransportModeProcessor {
    pub fn insert_esp(
        sa: &mut XfrmState,
        ip_header: &[u8],
        payload: &[u8],
        next_header: u8,
    ) -> Result<Vec<u8>, IpsecError> {
        let esp = EspProcessor::encapsulate(sa, payload, next_header)?;
        let mut result = ip_header.to_vec();

        if ip_header.len() >= 20 && (ip_header[0] >> 4) == 4 {
            result[9] = XfrmProto::Esp as u8;
            let total = (ip_header.len() + esp.len()) as u16;
            result[2] = (total >> 8) as u8;
            result[3] = total as u8;
        }

        result.extend_from_slice(&esp);
        Ok(result)
    }

    pub fn insert_ah(
        sa: &mut XfrmState,
        ip_header: &[u8],
        payload: &[u8],
        next_header: u8,
    ) -> Result<Vec<u8>, IpsecError> {
        let ah = AhProcessor::encapsulate(sa, payload, next_header)?;
        let mut result = ip_header.to_vec();

        if ip_header.len() >= 20 && (ip_header[0] >> 4) == 4 {
            result[9] = XfrmProto::Ah as u8;
            let total = (ip_header.len() + ah.len()) as u16;
            result[2] = (total >> 8) as u8;
            result[3] = total as u8;
        }

        result.extend_from_slice(&ah);
        Ok(result)
    }

    pub fn nat_t_udp_encap(data: &[u8], src_port: u16, dst_port: u16) -> Vec<u8> {
        let udp_len = (8 + data.len()) as u16;
        let mut pkt = Vec::with_capacity(8 + data.len());
        pkt.extend_from_slice(&src_port.to_be_bytes());
        pkt.extend_from_slice(&dst_port.to_be_bytes());
        pkt.extend_from_slice(&udp_len.to_be_bytes());
        pkt.extend_from_slice(&0u16.to_be_bytes());
        pkt.extend_from_slice(data);
        pkt
    }

    pub fn is_nat_t_keepalive(packet: &[u8]) -> bool {
        packet.len() == 1 && packet[0] == 0xFF
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VTI — Virtual Tunnel Interface
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct VtiInterface {
    pub name: String,
    pub if_id: u32,
    pub local: IpAddr,
    pub remote: IpAddr,
    pub link_index: u32,
    pub fwmark: u32,
    pub mtu: u16,
    pub okey: u32,
    pub ikey: u32,
}

impl VtiInterface {
    pub fn new(name: String, if_id: u32, local: IpAddr, remote: IpAddr) -> Self {
        Self {
            name,
            if_id,
            local,
            remote,
            link_index: 0,
            fwmark: 0,
            mtu: 1400,
            okey: if_id,
            ikey: if_id,
        }
    }

    pub fn matches_sa(&self, sa: &XfrmState) -> bool {
        sa.if_id == self.if_id
    }

    pub fn matches_policy(&self, policy: &XfrmPolicy) -> bool {
        policy.if_id == self.if_id
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global IPsec stack
// ─────────────────────────────────────────────────────────────────────────────

pub struct IpsecStack {
    pub sa_database: Vec<XfrmState>,
    pub policy_database: Vec<XfrmPolicy>,
    pub vti_interfaces: Vec<VtiInterface>,
    pub key_manager: KeyManager,
    pub pfkey_sockets: Vec<PfKeySocket>,
    pub xfrm_sockets: Vec<XfrmNetlinkSocket>,
    pub next_spi: u32,
    pub next_policy_index: u32,
    pub df_policy: DfPolicy,
    pub default_ttl: u8,
    pub initialized: bool,
}

impl IpsecStack {
    pub const fn new() -> Self {
        Self {
            sa_database: Vec::new(),
            policy_database: Vec::new(),
            vti_interfaces: Vec::new(),
            key_manager: KeyManager {
                pending_acquires: Vec::new(),
                manual_keys: Vec::new(),
                registered_listeners: Vec::new(),
                ike_daemon_pid: None,
            },
            pfkey_sockets: Vec::new(),
            xfrm_sockets: Vec::new(),
            next_spi: 256,
            next_policy_index: 1,
            df_policy: DfPolicy::Copy,
            default_ttl: 64,
            initialized: false,
        }
    }

    pub fn add_sa(&mut self, sa: XfrmState) {
        self.remove_sa(sa.spi, &sa.dst, sa.proto);
        self.sa_database.push(sa);
    }

    pub fn remove_sa(&mut self, spi: u32, dst: &IpAddr, proto: XfrmProto) {
        self.sa_database
            .retain(|sa| !(sa.spi == spi && sa.dst == *dst && sa.proto == proto));
    }

    pub fn lookup_sa(&self, spi: u32, dst: &IpAddr, proto: XfrmProto) -> Option<&XfrmState> {
        self.sa_database
            .iter()
            .find(|sa| sa.spi == spi && sa.dst == *dst && sa.proto == proto)
    }

    pub fn lookup_sa_mut(
        &mut self,
        spi: u32,
        dst: &IpAddr,
        proto: XfrmProto,
    ) -> Option<&mut XfrmState> {
        self.sa_database
            .iter_mut()
            .find(|sa| sa.spi == spi && sa.dst == *dst && sa.proto == proto)
    }

    pub fn flush_sa(&mut self) {
        self.sa_database.clear();
    }

    pub fn add_policy(&mut self, policy: XfrmPolicy) {
        self.remove_policy(policy.index, policy.dir);
        self.policy_database.push(policy);
        self.policy_database
            .sort_by(|a, b| a.priority.cmp(&b.priority));
    }

    pub fn remove_policy(&mut self, index: u32, dir: XfrmPolicyDir) {
        self.policy_database
            .retain(|p| !(p.index == index && p.dir == dir));
    }

    pub fn lookup_policy(
        &self,
        dir: XfrmPolicyDir,
        src: &IpAddr,
        dst: &IpAddr,
        proto: u8,
        sport: u16,
        dport: u16,
    ) -> Option<&XfrmPolicy> {
        self.policy_database
            .iter()
            .find(|p| p.dir == dir && p.matches(src, dst, proto, sport, dport))
    }

    pub fn lookup_policy_by_index(&self, index: u32, dir: XfrmPolicyDir) -> Option<&XfrmPolicy> {
        self.policy_database
            .iter()
            .find(|p| p.index == index && p.dir == dir)
    }

    pub fn flush_policy(&mut self) {
        self.policy_database.clear();
    }

    pub fn alloc_spi(&mut self, min: u32, max: u32) -> u32 {
        let spi = if self.next_spi < min {
            min
        } else {
            self.next_spi
        };
        let spi = if spi > max { min } else { spi };
        self.next_spi = spi + 1;
        spi
    }

    pub fn alloc_policy_index(&mut self) -> u32 {
        let idx = self.next_policy_index;
        self.next_policy_index += 1;
        idx
    }

    pub fn add_vti(&mut self, vti: VtiInterface) {
        self.vti_interfaces.push(vti);
    }

    pub fn remove_vti(&mut self, if_id: u32) {
        self.vti_interfaces.retain(|v| v.if_id != if_id);
    }

    pub fn lookup_vti(&self, if_id: u32) -> Option<&VtiInterface> {
        self.vti_interfaces.iter().find(|v| v.if_id == if_id)
    }

    pub fn migrate_sa(&mut self, info: &MigrateInfo) -> Result<(), IpsecError> {
        let mut migrated = false;
        for sa in &mut self.sa_database {
            if sa.src == info.old_src && sa.dst == info.old_dst && sa.proto == info.proto {
                sa.src = info.new_src;
                sa.dst = info.new_dst;
                migrated = true;
            }
        }
        if migrated {
            Ok(())
        } else {
            Err(IpsecError::MigrationFailed)
        }
    }

    pub fn check_expiry(&mut self) -> Vec<ExpireInfo> {
        let mut expired = Vec::new();
        for sa in &self.sa_database {
            if sa.is_expired_hard() {
                expired.push(ExpireInfo {
                    spi: sa.spi,
                    proto: sa.proto,
                    src: sa.src,
                    dst: sa.dst,
                    hard: true,
                });
            } else if sa.is_expired_soft() {
                expired.push(ExpireInfo {
                    spi: sa.spi,
                    proto: sa.proto,
                    src: sa.src,
                    dst: sa.dst,
                    hard: false,
                });
            }
        }
        expired
    }

    pub fn process_outbound(
        &mut self,
        src: &IpAddr,
        dst: &IpAddr,
        proto: u8,
        sport: u16,
        dport: u16,
        payload: &[u8],
    ) -> Result<Option<Vec<u8>>, IpsecError> {
        let policy = match self.lookup_policy(XfrmPolicyDir::Out, src, dst, proto, sport, dport) {
            Some(p) => p.clone(),
            None => return Ok(None),
        };
        match policy.action {
            XfrmPolicyAction::Block => return Ok(Some(Vec::new())),
            XfrmPolicyAction::Allow => {}
        }
        if policy.templates.is_empty() {
            return Ok(None);
        }
        let tmpl = &policy.templates[0];
        let sa = self
            .lookup_sa_mut(tmpl.spi, &tmpl.dst, tmpl.proto)
            .ok_or(IpsecError::SaNotFound)?;

        match tmpl.proto {
            XfrmProto::Esp => {
                let result = EspProcessor::encapsulate(sa, payload, proto)?;
                Ok(Some(result))
            }
            XfrmProto::Ah => {
                let result = AhProcessor::encapsulate(sa, payload, proto)?;
                Ok(Some(result))
            }
            XfrmProto::IpComp => {
                let result = IpCompProcessor::compress(sa, payload, proto)?;
                Ok(Some(result))
            }
        }
    }

    pub fn process_inbound(
        &mut self,
        spi: u32,
        dst: &IpAddr,
        proto: XfrmProto,
        packet: &[u8],
    ) -> Result<(Vec<u8>, u8), IpsecError> {
        let sa = self
            .lookup_sa_mut(spi, dst, proto)
            .ok_or(IpsecError::SaNotFound)?;

        match proto {
            XfrmProto::Esp => EspProcessor::decapsulate(sa, packet),
            XfrmProto::Ah => AhProcessor::decapsulate(sa, packet),
            XfrmProto::IpComp => IpCompProcessor::decompress(sa, packet),
        }
    }

    pub fn sa_count(&self) -> usize {
        self.sa_database.len()
    }

    pub fn policy_count(&self) -> usize {
        self.policy_database.len()
    }

    pub fn vti_count(&self) -> usize {
        self.vti_interfaces.len()
    }
}

pub static IPSEC_STACK: Mutex<IpsecStack> = Mutex::new(IpsecStack::new());

pub fn init() {
    let mut stack = IPSEC_STACK.lock();
    stack.initialized = true;
}
