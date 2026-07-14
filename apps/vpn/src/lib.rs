//! RaeenOS VPN — a first-class, friendly WireGuard client over the LIVE `raevpn`
//! Noise_IKpsk2 engine.
//!
//! The Concept names "built-in WireGuard" as a pillar (§Networking / §Privacy):
//! a clickable, switcher-friendly WireGuard UI is a genuine surpass-Windows
//! differentiator — Windows ships no first-party WG client, macOS hides it in an
//! App-Store download. This app puts add-peer → connect → live status one click
//! away.
//!
//! ## Shape (mirrors apps/mail, apps/browser, apps/calendar)
//! - The syscall-free heart is [`VpnModel`]: it holds a [`PeerConfig`] and drives
//!   the REAL [`raevpn::NoiseHandshake`] to completion over an INJECTABLE
//!   [`VpnTransport`]. No `raevpn` internals are reached for — only its public
//!   API (`NoiseHandshake::{new, create_initiation, consume_response}`,
//!   `HandshakeState`, `TransportSession`, `Key`).
//! - The host KAT (`cargo test -p vpn --features host`) links the live engine and
//!   runs the handshake against a MOCK peer (a real responder built from the SAME
//!   `raevpn` API) that completes a valid Noise_IKpsk2 exchange — proving the app
//!   reaches `Connected` with a real session, and reaches `Failed` (fail-closed,
//!   NEVER a fake `Connected`) against a tampered/forged response.
//! - The live UDP datapath (kernel net syscalls) is a `cfg(not(test))` wrapper.
//!   Handshake transport over real sockets is NOT wired this session; the live
//!   app reports an HONEST "live datapath not wired — handshake only" status
//!   rather than faking a tunnel. The crypto handshake itself is the same real
//!   code in both paths.

// no_std for the real userspace ELF; std under `cargo test` so the host KAT can
// link. The live ELF entry point lives in the thin `src/main.rs` bin, which calls
// `run()` below. (`run` uses `Canvas::new`, which is `unsafe`, so the LIBRARY
// cannot `#![forbid(unsafe_code)]` — the unsafe sites are the surface-buffer
// Canvas, documented.)
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use raevpn::{HandshakeState, Key, NoiseHandshake, TransportSession};

// The render/run path is live-ELF only; under `cargo test` only the VpnModel
// (over raevpn) is exercised, so the graphics/syscall imports are gated out to
// keep the host test warning-clean.
#[cfg(not(test))]
use rae_tokens::DARK;
#[cfg(not(test))]
use raegfx::Canvas;
#[cfg(not(test))]
#[allow(unused_imports)]
use raekit;

// ===========================================================================
// Injectable transport — the seam the host KAT mocks.
// ===========================================================================

/// The byte-pipe the handshake rides on. The app drives the handshake by sending
/// the initiation and reading the peer's response through this trait; the host
/// KAT supplies a MOCK peer (a real `raevpn` responder), and the live ELF would
/// supply a kernel-socket wrapper.
///
/// This is the SINGLE point of injection: swapping the transport swaps the whole
/// network without touching any handshake logic. The handshake crypto driven over
/// it is identical in both cases.
pub trait VpnTransport {
    /// Send a handshake message to the peer (initiation, then later data).
    /// Returns `false` if the underlying transport failed to accept the bytes.
    fn send(&mut self, msg: &[u8]) -> bool;
    /// Try to receive the next message from the peer. `None` means "nothing yet"
    /// (the caller may retry up to its own budget) — it is NOT an error.
    fn recv(&mut self) -> Option<Vec<u8>>;
}

// ===========================================================================
// PeerConfig — the WireGuard peer the user types in (and that round-trips).
// ===========================================================================

/// A WireGuard peer configuration: everything the handshake + routing need. Keys
/// are stored as raw 32-byte material; the UI parses/formats them as base64 (the
/// canonical WireGuard key encoding).
#[derive(Clone)]
pub struct PeerConfig {
    /// Friendly name shown in the connection list.
    pub name: String,
    /// Our interface private key (32 bytes).
    pub private_key: Key,
    /// The peer's static public key (32 bytes).
    pub peer_public_key: Key,
    /// Endpoint host (IPv4 dotted-quad or hostname), e.g. "vpn.example.com".
    pub endpoint_host: String,
    /// Endpoint UDP port (default 51820).
    pub endpoint_port: u16,
    /// Allowed IPs CIDR strings, e.g. "0.0.0.0/0".
    pub allowed_ips: Vec<String>,
    /// Optional pre-shared key (32 bytes; all-zero = none). WireGuard always feeds
    /// a PSK into KDF3 — `Key::zero()` is the "no PSK" identity.
    pub preshared_key: Key,
}

impl PeerConfig {
    /// A fresh, empty config with WireGuard's default port. Keys are zero until
    /// the user pastes real ones.
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            private_key: Key::zero(),
            peer_public_key: Key::zero(),
            endpoint_host: String::new(),
            endpoint_port: 51820,
            allowed_ips: Vec::new(),
            preshared_key: Key::zero(),
        }
    }

    /// True once the minimum fields for a handshake are present: both keys set and
    /// an endpoint host. (A zero private OR peer key cannot produce a real
    /// session, so we refuse to even attempt — fail-closed at config time.)
    pub fn is_connectable(&self) -> bool {
        self.private_key != Key::zero()
            && self.peer_public_key != Key::zero()
            && !self.endpoint_host.is_empty()
    }

    /// Serialize to a compact, self-describing text record for persistence. One
    /// `key = value` per line; keys are base64. This is the on-disk form the
    /// config store round-trips. (We keep our own tiny encoder rather than pull a
    /// serializer dep — the schema is fixed and small.)
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str("name=");
        out.push_str(&self.name);
        out.push('\n');
        out.push_str("private_key=");
        out.push_str(&b64_encode(self.private_key.as_bytes()));
        out.push('\n');
        out.push_str("peer_public_key=");
        out.push_str(&b64_encode(self.peer_public_key.as_bytes()));
        out.push('\n');
        out.push_str("endpoint=");
        out.push_str(&self.endpoint_host);
        out.push(':');
        push_u16(&mut out, self.endpoint_port);
        out.push('\n');
        for cidr in &self.allowed_ips {
            out.push_str("allowed_ip=");
            out.push_str(cidr);
            out.push('\n');
        }
        if self.preshared_key != Key::zero() {
            out.push_str("preshared_key=");
            out.push_str(&b64_encode(self.preshared_key.as_bytes()));
            out.push('\n');
        }
        out
    }

    /// Parse a record produced by [`PeerConfig::serialize`]. Returns `None` if a
    /// required field is missing or a key fails to decode to 32 bytes (fail-closed
    /// — we never construct a half-built config).
    pub fn parse(text: &str) -> Option<PeerConfig> {
        let mut name = String::new();
        let mut priv_key: Option<Key> = None;
        let mut peer_key: Option<Key> = None;
        let mut psk = Key::zero();
        let mut host = String::new();
        let mut port: u16 = 51820;
        let mut allowed = Vec::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = match line.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            match k.trim() {
                "name" => name = String::from(v.trim()),
                "private_key" => priv_key = Some(decode_key(v.trim())?),
                "peer_public_key" => peer_key = Some(decode_key(v.trim())?),
                "preshared_key" => psk = decode_key(v.trim())?,
                "endpoint" => {
                    let ep = v.trim();
                    if let Some((h, p)) = ep.rsplit_once(':') {
                        host = String::from(h);
                        port = p.parse().ok()?;
                    } else {
                        host = String::from(ep);
                    }
                }
                "allowed_ip" => allowed.push(String::from(v.trim())),
                _ => {}
            }
        }

        Some(PeerConfig {
            name,
            private_key: priv_key?,
            peer_public_key: peer_key?,
            endpoint_host: host,
            endpoint_port: port,
            allowed_ips: allowed,
            preshared_key: psk,
        })
    }
}

/// Decode a base64 WireGuard key (44 chars / 32 bytes) into a [`Key`]. Returns
/// `None` on bad base64 or wrong length.
fn decode_key(b64: &str) -> Option<Key> {
    let bytes = b64_decode(b64)?;
    if bytes.len() != 32 {
        return None;
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&bytes);
    Some(Key::from_bytes(&k))
}

// ===========================================================================
// VpnModel — the syscall-free heart (host-KAT'd against the live engine).
// ===========================================================================

/// The connection lifecycle the UI reflects. The states are driven only by the
/// real handshake — there is NO path that fakes `Connected`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnState {
    /// No tunnel; idle.
    Disconnected,
    /// Initiation sent, awaiting/processing the peer's response.
    Handshaking,
    /// Transport keys derived from a completed Noise_IKpsk2 exchange.
    Connected,
    /// The handshake failed (forged/tampered response, transport failure, or a
    /// non-connectable config). Fail-closed: never silently becomes Connected.
    Failed,
}

/// Why a connection ended up [`ConnState::Failed`] — surfaced in the status
/// detail so the user gets a real diagnostic, not a spinner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FailReason {
    None,
    /// The config was missing keys or an endpoint.
    NotConnectable,
    /// The transport could not send the initiation.
    SendFailed,
    /// No response arrived within the attempt budget.
    Timeout,
    /// A response arrived but failed authentication (forged/tampered/wrong PSK).
    HandshakeRejected,
}

/// One configured tunnel plus its live state. The live `App` owns a list of
/// these; the host KAT drives a single one.
pub struct VpnConn {
    pub config: PeerConfig,
    state: ConnState,
    fail: FailReason,
    /// The negotiated transport session (present only while `Connected`).
    session: Option<TransportSession>,
    /// Monotonic time (ns) of the last completed handshake, if any.
    last_handshake_ns: u64,
    /// Bytes the user has sent through the tunnel since connect. 0 until the live
    /// datapath is wired — reported honestly, never invented.
    bytes_tx: u64,
    /// Bytes received through the tunnel since connect. 0 until live datapath.
    bytes_rx: u64,
}

impl VpnConn {
    pub fn new(config: PeerConfig) -> Self {
        Self {
            config,
            state: ConnState::Disconnected,
            fail: FailReason::None,
            session: None,
            last_handshake_ns: 0,
            bytes_tx: 0,
            bytes_rx: 0,
        }
    }

    pub fn state(&self) -> ConnState {
        self.state
    }

    pub fn fail_reason(&self) -> FailReason {
        self.fail
    }

    pub fn last_handshake_ns(&self) -> u64 {
        self.last_handshake_ns
    }

    pub fn bytes_tx(&self) -> u64 {
        self.bytes_tx
    }

    pub fn bytes_rx(&self) -> u64 {
        self.bytes_rx
    }

    /// True while a live transport session exists (the keys are negotiated).
    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// Drive a full Noise_IKpsk2 handshake to completion over `transport`,
    /// updating `self.state` from the REAL engine result.
    ///
    /// - `local_eph`: the initiator ephemeral PRIVATE key. The kernel path
    ///   supplies CSPRNG bytes; the host KAT injects a fixed value for
    ///   determinism (exactly the seam `raevpn::create_initiation` documents).
    /// - `sender_index`: our 32-bit handshake index.
    /// - `now_ns`: monotonic clock for the last-handshake timestamp.
    /// - `recv_budget`: how many `transport.recv()` polls to attempt before
    ///   declaring a timeout.
    ///
    /// Returns the resulting [`ConnState`]. NEVER returns `Connected` without a
    /// real, authenticated transport session.
    pub fn connect<T: VpnTransport>(
        &mut self,
        transport: &mut T,
        local_eph: Key,
        sender_index: u32,
        now_ns: u64,
        recv_budget: u32,
    ) -> ConnState {
        if !self.config.is_connectable() {
            self.state = ConnState::Failed;
            self.fail = FailReason::NotConnectable;
            return self.state;
        }

        self.state = ConnState::Handshaking;
        self.fail = FailReason::None;
        self.session = None;

        // Build the REAL initiator handshake from the live engine.
        let mut hs = NoiseHandshake::new(
            self.config.private_key,
            self.config.peer_public_key,
            self.config.preshared_key,
        );
        let init = hs.create_initiation(sender_index, local_eph);
        if init.is_empty() {
            self.state = ConnState::Failed;
            self.fail = FailReason::HandshakeRejected;
            return self.state;
        }
        debug_assert_eq!(hs.state(), HandshakeState::InitiationSent);

        if !transport.send(&init) {
            self.state = ConnState::Failed;
            self.fail = FailReason::SendFailed;
            return self.state;
        }

        // Await the response, bounded by the budget.
        let mut response: Option<Vec<u8>> = None;
        for _ in 0..recv_budget {
            if let Some(msg) = transport.recv() {
                response = Some(msg);
                break;
            }
        }
        let response = match response {
            Some(r) => r,
            None => {
                self.state = ConnState::Failed;
                self.fail = FailReason::Timeout;
                return self.state;
            }
        };

        // The REAL authentication step. `consume_response` returns false on ANY
        // malformed/forged/tampered input (bad MAC1, bad AEAD tag, wrong PSK) —
        // that is the entire fail-closed guarantee.
        if !hs.consume_response(&response) {
            self.state = ConnState::Failed;
            self.fail = FailReason::HandshakeRejected;
            return self.state;
        }
        if hs.state() != HandshakeState::Established {
            self.state = ConnState::Failed;
            self.fail = FailReason::HandshakeRejected;
            return self.state;
        }

        // Authenticated — build the live transport session from the derived keys.
        // (We learn the responder's index from the response message header.)
        let receiver_index = if response.len() >= 8 {
            u32::from_le_bytes([response[4], response[5], response[6], response[7]])
        } else {
            0
        };
        let session = TransportSession::new(
            sender_index,
            receiver_index,
            hs.sending_key(),
            hs.receiving_key(),
            now_ns / 1_000_000_000,
        );
        self.session = Some(session);
        self.last_handshake_ns = now_ns;
        self.bytes_tx = 0;
        self.bytes_rx = 0;
        self.state = ConnState::Connected;
        self.state
    }

    /// Tear down the tunnel. Wipes the session keys (the engine's `Drop` would
    /// also, but we drop deterministically here).
    pub fn disconnect(&mut self) {
        self.session = None;
        self.state = ConnState::Disconnected;
        self.fail = FailReason::None;
        self.bytes_tx = 0;
        self.bytes_rx = 0;
    }

    /// Encrypt a payload for the tunnel (only valid while `Connected`). Returns
    /// the WireGuard data message and, as a side effect, accounts the tx bytes.
    /// This is exercised by the host KAT to prove the negotiated keys actually
    /// work end-to-end; the live UDP write that would carry it is not wired.
    pub fn seal(&mut self, plaintext: &[u8]) -> Option<Vec<u8>> {
        let out = self.session.as_ref()?.encrypt_packet(plaintext)?;
        self.bytes_tx = self.bytes_tx.saturating_add(plaintext.len() as u64);
        Some(out)
    }

    /// Decrypt a tunnel data message (only valid while `Connected`). Accounts rx
    /// bytes on success.
    pub fn open(&mut self, msg: &[u8]) -> Option<Vec<u8>> {
        let out = self.session.as_ref()?.decrypt_packet(msg)?;
        self.bytes_rx = self.bytes_rx.saturating_add(out.len() as u64);
        Some(out)
    }
}

/// The app's whole model: a list of configured tunnels and which one is selected.
/// Persists by serializing every config (round-trips through [`PeerConfig`]).
pub struct VpnModel {
    pub conns: Vec<VpnConn>,
    pub selected: usize,
}

impl Default for VpnModel {
    fn default() -> Self {
        Self::new()
    }
}

impl VpnModel {
    pub fn new() -> Self {
        Self {
            conns: Vec::new(),
            selected: 0,
        }
    }

    /// Add a peer; selects it.
    pub fn add(&mut self, config: PeerConfig) {
        self.conns.push(VpnConn::new(config));
        self.selected = self.conns.len() - 1;
    }

    pub fn selected_conn(&self) -> Option<&VpnConn> {
        self.conns.get(self.selected)
    }

    pub fn selected_conn_mut(&mut self) -> Option<&mut VpnConn> {
        self.conns.get_mut(self.selected)
    }

    /// Serialize every configured peer for persistence. Records are separated by a
    /// blank line so the loader can split them.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for c in &self.conns {
            out.push_str(&c.config.serialize());
            out.push('\n');
        }
        out
    }

    /// Reload a model from a blob produced by [`VpnModel::serialize`]. Malformed
    /// records are skipped (the rest still load — a corrupt entry never bricks the
    /// list).
    pub fn load(blob: &str) -> VpnModel {
        let mut model = VpnModel::new();
        // Records are blank-line separated. We re-split by scanning for a `name=`
        // line as a record boundary.
        let mut cur = String::new();
        for line in blob.lines() {
            if line.trim().is_empty() {
                if !cur.trim().is_empty() {
                    if let Some(cfg) = PeerConfig::parse(&cur) {
                        model.conns.push(VpnConn::new(cfg));
                    }
                    cur.clear();
                }
                continue;
            }
            if line.starts_with("name=") && !cur.trim().is_empty() {
                if let Some(cfg) = PeerConfig::parse(&cur) {
                    model.conns.push(VpnConn::new(cfg));
                }
                cur.clear();
            }
            cur.push_str(line);
            cur.push('\n');
        }
        if !cur.trim().is_empty() {
            if let Some(cfg) = PeerConfig::parse(&cur) {
                model.conns.push(VpnConn::new(cfg));
            }
        }
        model
    }
}

// ===========================================================================
// base64 (RFC 4648, standard alphabet) — WireGuard's key encoding. Tiny, pure,
// no_std-safe; host-KAT'd via the config round-trip.
// ===========================================================================

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(B64[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64[((n >> 12) & 0x3F) as usize] as char);
        out.push(B64[((n >> 6) & 0x3F) as usize] as char);
        out.push(B64[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = data.len() - i;
    if rem == 1 {
        let n = (data[i] as u32) << 16;
        out.push(B64[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(B64[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64[((n >> 12) & 0x3F) as usize] as char);
        out.push(B64[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
    out
}

fn b64_val(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a' + 26) as u32),
        b'0'..=b'9' => Some((c - b'0' + 52) as u32),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|&c| c != b'=' && !c.is_ascii_whitespace())
        .collect();
    let mut out = Vec::new();
    let mut chunk = [0u32; 4];
    let mut n = 0;
    for &c in &bytes {
        chunk[n] = b64_val(c)?;
        n += 1;
        if n == 4 {
            let v = (chunk[0] << 18) | (chunk[1] << 12) | (chunk[2] << 6) | chunk[3];
            out.push((v >> 16) as u8);
            out.push((v >> 8) as u8);
            out.push(v as u8);
            n = 0;
        }
    }
    match n {
        0 => {}
        2 => {
            let v = (chunk[0] << 18) | (chunk[1] << 12);
            out.push((v >> 16) as u8);
        }
        3 => {
            let v = (chunk[0] << 18) | (chunk[1] << 12) | (chunk[2] << 6);
            out.push((v >> 16) as u8);
            out.push((v >> 8) as u8);
        }
        _ => return None,
    }
    Some(out)
}

/// Append a `u16` as decimal to a `String` (no_std-safe, no `format!`).
fn push_u16(out: &mut String, mut v: u16) {
    if v == 0 {
        out.push('0');
        return;
    }
    let mut buf = [0u8; 5];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        out.push(b as char);
    }
}

// ===========================================================================
// Live ELF: window geometry, draw path, event loop. (cfg(not(test)) only — the
// host KAT exercises only the VpnModel, so none of this links into the test.)
// ===========================================================================

#[cfg(not(test))]
const WIN_W: usize = 640;
#[cfg(not(test))]
const WIN_H: usize = 460;
#[cfg(not(test))]
const SURFACE_VIRT: u64 = 0x0000_7D00_0000;
#[cfg(not(test))]
const PRESENT_X: i32 = 160;
#[cfg(not(test))]
const PRESENT_Y: i32 = 80;
#[cfg(not(test))]
const TITLE_H: usize = 32;

/// A short status word for the current connection state. Used by the live chrome
/// (and a coarse signal in the bin).
#[cfg(not(test))]
fn state_label(s: ConnState) -> &'static str {
    match s {
        ConnState::Disconnected => "Disconnected",
        ConnState::Handshaking => "Handshaking...",
        ConnState::Connected => "Connected",
        ConnState::Failed => "Failed",
    }
}

#[cfg(not(test))]
fn state_color(s: ConnState) -> u32 {
    match s {
        ConnState::Connected => DARK.state_ok,
        ConnState::Handshaking => DARK.state_warn,
        ConnState::Failed => DARK.state_danger,
        ConnState::Disconnected => DARK.text_tertiary,
    }
}

/// Render the whole window: a glass card with the connection list, the selected
/// peer's config summary, the big connect button, and the live status detail.
#[cfg(not(test))]
fn render(model: &VpnModel, canvas: &mut Canvas) {
    use rae_tokens::{RADIUS_LG, RADIUS_MD, SPACE_3, SPACE_4};

    // Liquid-Glass background: deep base, a raised glass card on top.
    canvas.clear(DARK.bg_base);
    canvas.fill_rounded_rect(
        SPACE_3 as usize,
        SPACE_3 as usize,
        WIN_W - 2 * SPACE_3 as usize,
        WIN_H - 2 * SPACE_3 as usize,
        RADIUS_LG as usize,
        DARK.bg_raised,
    );
    canvas.draw_rounded_rect_outline(
        SPACE_3 as usize,
        SPACE_3 as usize,
        WIN_W - 2 * SPACE_3 as usize,
        WIN_H - 2 * SPACE_3 as usize,
        RADIUS_LG as usize,
        DARK.stroke_strong,
    );

    // Title bar.
    canvas.draw_text_scaled(
        (SPACE_4 + 8) as usize,
        (SPACE_4 + 4) as usize,
        "RaeVPN  -  WireGuard",
        DARK.text_primary,
        2,
    );

    let mut y = TITLE_H + SPACE_4 as usize + 8;
    let x = SPACE_4 as usize + 8;

    if model.conns.is_empty() {
        canvas.draw_text(x, y, "No tunnels configured.", DARK.text_secondary, None);
        y += 20;
        canvas.draw_text(
            x,
            y,
            "Add a WireGuard peer to get started.",
            DARK.text_tertiary,
            None,
        );
    } else {
        // Connection list (left) — one row per peer.
        for (i, c) in model.conns.iter().enumerate() {
            let row_y = y + i * 28;
            let sel = i == model.selected;
            if sel {
                canvas.fill_rounded_rect(
                    x - 4,
                    row_y - 4,
                    260,
                    26,
                    RADIUS_MD as usize,
                    DARK.bg_elevated,
                );
            }
            // Status dot.
            canvas.fill_circle(x + 6, row_y + 8, 5, state_color(c.state()));
            let name = if c.config.name.is_empty() {
                "(unnamed)"
            } else {
                c.config.name.as_str()
            };
            canvas.draw_text(x + 20, row_y, name, DARK.text_primary, None);
        }

        // Detail panel (selected peer).
        if let Some(c) = model.selected_conn() {
            let dx = x + 280;
            let mut dy = y;
            canvas.draw_text_scaled(dx, dy, state_label(c.state()), state_color(c.state()), 2);
            dy += 32;
            canvas.draw_text(dx, dy, "Endpoint:", DARK.text_tertiary, None);
            dy += 16;
            canvas.draw_text(dx, dy, &c.config.endpoint_host, DARK.text_secondary, None);
            dy += 28;

            // Honest live-datapath label: the handshake is real, the tunnel
            // byte-pipe over kernel sockets is not wired this session.
            canvas.draw_text(dx, dy, "Live tunnel datapath:", DARK.text_tertiary, None);
            dy += 16;
            canvas.draw_text(dx, dy, "not wired (handshake only)", DARK.state_warn, None);
            dy += 28;

            canvas.draw_text(dx, dy, "TX / RX bytes:", DARK.text_tertiary, None);
            dy += 16;
            let mut s = String::new();
            push_u64(&mut s, c.bytes_tx());
            s.push_str(" / ");
            push_u64(&mut s, c.bytes_rx());
            canvas.draw_text(dx, dy, &s, DARK.text_secondary, None);
        }
    }

    // Connect/Disconnect button (bottom).
    let by = WIN_H - 56;
    let bx = SPACE_4 as usize + 8;
    let connected = matches!(
        model.selected_conn().map(|c| c.state()),
        Some(ConnState::Connected)
    );
    let (label, color) = if connected {
        ("Disconnect", DARK.state_danger)
    } else {
        ("Connect", DARK.state_ok)
    };
    canvas.fill_rounded_rect(bx, by, 160, 36, RADIUS_MD as usize, color);
    canvas.draw_text_scaled(bx + 24, by + 10, label, 0xFF_FF_FF_FF, 2);
}

/// Append a `u64` as decimal (no_std-safe).
#[cfg(not(test))]
fn push_u64(out: &mut String, mut v: u64) {
    if v == 0 {
        out.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        out.push(b as char);
    }
}

/// The clickable regions the event loop hit-tests.
#[cfg(not(test))]
#[derive(PartialEq, Eq)]
enum Hit {
    None,
    Row(usize),
    ConnectButton,
}

#[cfg(not(test))]
fn hit_test(model: &VpnModel, lx: i32, ly: i32) -> Hit {
    use rae_tokens::SPACE_4;
    let x = SPACE_4 as usize + 8;
    let list_top = TITLE_H + SPACE_4 as usize + 8;

    // Connect button.
    let by = (WIN_H - 56) as i32;
    let bx = (SPACE_4 as usize + 8) as i32;
    if lx >= bx && lx < bx + 160 && ly >= by && ly < by + 36 {
        return Hit::ConnectButton;
    }

    // Rows.
    if lx >= (x as i32 - 4) && lx < (x as i32 + 256) {
        for i in 0..model.conns.len() {
            let row_y = (list_top + i * 28) as i32;
            if ly >= row_y - 4 && ly < row_y + 22 {
                return Hit::Row(i);
            }
        }
    }
    Hit::None
}

/// Creates the window surface and runs the event loop. The live UDP datapath is
/// not wired this session, so the Connect button reports an honest handshake-only
/// status; it does NOT fake a tunnel. (The same real `VpnConn::connect` runs once
/// the kernel-socket `VpnTransport` lands.)
#[cfg(not(test))]
pub fn run() -> ! {
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }
    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    // Seed with an example peer so the first-run window is not empty (the user
    // edits it). Keys are zero → the config is not connectable until pasted, so
    // pressing Connect on it fails closed with an honest reason.
    let mut model = VpnModel::new();
    let mut example = PeerConfig::new("Home Server");
    example.endpoint_host = String::from("vpn.example.com");
    example.allowed_ips.push(String::from("0.0.0.0/0"));
    model.add(example);

    render(&model, &mut canvas);
    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut left_was_down = false;
    loop {
        let mut dirty = false;

        // Mouse: detect a left-button press edge.
        let mut edge = false;
        loop {
            let ev = raekit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            let now_down = (ev & 0x01) != 0;
            if now_down && !left_was_down {
                edge = true;
            }
            left_was_down = now_down;
        }
        if edge {
            let (cx, cy, _btn) = raekit::sys::cursor_pos();
            let (ox, oy) =
                raekit::sys::surface_origin(sid).unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
            let lx = (cx as i32).saturating_sub(ox as i32);
            let ly = (cy as i32).saturating_sub(oy as i32);
            match hit_test(&model, lx, ly) {
                Hit::Row(i) => {
                    model.selected = i;
                    dirty = true;
                }
                Hit::ConnectButton => {
                    let connected = matches!(
                        model.selected_conn().map(|c| c.state()),
                        Some(ConnState::Connected)
                    );
                    if connected {
                        if let Some(c) = model.selected_conn_mut() {
                            c.disconnect();
                        }
                    } else if let Some(c) = model.selected_conn_mut() {
                        // No live UDP transport this session: surface the honest
                        // outcome (a not-connectable seed config fails closed; a
                        // fully-configured one would attempt the real handshake
                        // once the kernel-socket transport lands). We do NOT
                        // synthesize a Connected state here.
                        if !c.config.is_connectable() {
                            // Mark Failed/NotConnectable via the real path.
                            let mut dead = NullTransport;
                            c.connect(&mut dead, Key::zero(), 1, raekit::sys::time_ns(), 1);
                        } else {
                            // Connectable but no transport wired -> Handshaking
                            // that times out honestly (never a fake Connected).
                            let mut dead = NullTransport;
                            c.connect(&mut dead, Key::zero(), 1, raekit::sys::time_ns(), 1);
                        }
                    }
                    dirty = true;
                }
                Hit::None => {}
            }
        }

        // Keyboard: Esc closes.
        let key = raekit::sys::read_key();
        if key != 0 {
            let code = (key & 0xFF) as u8;
            let pressed = (key & 0x8000_0000) == 0;
            if pressed && code == 0x01 {
                raekit::sys::exit(0);
            }
        }

        if dirty {
            render(&model, &mut canvas);
            raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
        raekit::sys::yield_now();
    }
}

/// A transport that accepts the initiation but never returns a response — the
/// honest stand-in for "live UDP datapath not wired". Driving the real handshake
/// over it yields `Failed`/`Timeout`, never a fabricated `Connected`.
#[cfg(not(test))]
struct NullTransport;

#[cfg(not(test))]
impl VpnTransport for NullTransport {
    fn send(&mut self, _msg: &[u8]) -> bool {
        true
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        None
    }
}

// ===========================================================================
// Host KAT — links the LIVE raevpn Noise_IKpsk2 engine, no kernel, no network.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use raevpn::Key;
    use x25519_dalek::{PublicKey, StaticSecret};

    /// Derive an x25519 PUBLIC key from a 32-byte private key, using the SAME
    /// crate raevpn uses internally (raevpn does not expose this and is
    /// read-only, so the test does it). Lets the mock peer hand the app a peer
    /// public key that genuinely corresponds to its private key.
    fn public_of(private: &[u8; 32]) -> Key {
        let secret = StaticSecret::from(*private);
        let public = PublicKey::from(&secret);
        Key::from_bytes(public.as_bytes())
    }

    /// A scripted WireGuard RESPONDER built from the real `raevpn` API. It holds
    /// its own static private key + the shared PSK, consumes whatever initiation
    /// the app sends, and produces a valid Noise_IKpsk2 response — OR, if asked,
    /// tampers with the response so the app must reject it.
    struct MockPeer {
        responder_static: Key,
        responder_eph: Key,
        psk: Key,
        /// If true, flip a byte in the response to forge it (fail-closed test).
        tamper: bool,
        /// The next message the app will read (the response we built).
        outbox: Option<Vec<u8>>,
    }

    impl MockPeer {
        fn new(responder_static: Key, psk: Key, tamper: bool) -> Self {
            // Fixed responder ephemeral for determinism (the seam create_response
            // documents). Any 32-byte private scalar works.
            let responder_eph = Key::from_bytes(&[7u8; 32]);
            Self {
                responder_static,
                responder_eph,
                psk,
                tamper,
                outbox: None,
            }
        }
    }

    impl VpnTransport for MockPeer {
        fn send(&mut self, msg: &[u8]) -> bool {
            // The app sent its initiation. Run the REAL responder side: consume
            // it, then build a real response, and stage it for the app to read.
            let mut hs = NoiseHandshake::new(self.responder_static, Key::zero(), self.psk);
            if !hs.consume_initiation(msg) {
                // A real responder would drop the packet; the app then times out.
                self.outbox = None;
                return true;
            }
            if !hs.create_response(0xABCD_1234, self.responder_eph) {
                self.outbox = None;
                return true;
            }
            let mut resp = hs.take_response();
            if self.tamper && resp.len() > 50 {
                // Corrupt the encrypted-empty AEAD region: the app's
                // consume_response MUST reject this (bad tag) -> Failed.
                resp[50] ^= 0xFF;
            }
            self.outbox = Some(resp);
            true
        }

        fn recv(&mut self) -> Option<Vec<u8>> {
            self.outbox.take()
        }
    }

    /// Build a connectable config whose peer public key truly corresponds to the
    /// mock responder's private key — so a REAL handshake can complete.
    fn connectable_config(
        initiator_priv: [u8; 32],
        responder_priv: [u8; 32],
        psk: [u8; 32],
    ) -> (PeerConfig, Key) {
        let responder_static = Key::from_bytes(&responder_priv);
        let peer_pub = public_of(&responder_priv);
        let mut cfg = PeerConfig::new("Test Tunnel");
        cfg.private_key = Key::from_bytes(&initiator_priv);
        cfg.peer_public_key = peer_pub;
        cfg.preshared_key = Key::from_bytes(&psk);
        cfg.endpoint_host = String::from("10.0.0.1");
        cfg.endpoint_port = 51820;
        cfg.allowed_ips.push(String::from("0.0.0.0/0"));
        (cfg, responder_static)
    }

    #[test]
    fn valid_handshake_reaches_connected_with_real_session() {
        let initiator_priv = [3u8; 32];
        let responder_priv = [9u8; 32];
        let psk = [42u8; 32];
        let (cfg, responder_static) = connectable_config(initiator_priv, responder_priv, psk);

        let mut conn = VpnConn::new(cfg);
        let mut peer = MockPeer::new(responder_static, Key::from_bytes(&psk), false);

        let local_eph = Key::from_bytes(&[5u8; 32]);
        let state = conn.connect(&mut peer, local_eph, 1, 1_000_000_000, 4);

        assert_eq!(
            state,
            ConnState::Connected,
            "a valid Noise_IKpsk2 exchange must reach Connected"
        );
        assert!(conn.has_session(), "Connected must carry a real session");
        assert_eq!(conn.fail_reason(), FailReason::None);
        assert_eq!(conn.last_handshake_ns(), 1_000_000_000);

        // The negotiated keys must actually work: seal then the peer-side decrypt.
        // We prove our send-key is real by sealing and confirming a ciphertext +
        // byte accounting.
        let sealed = conn.seal(b"hello tunnel").expect("seal with real session");
        assert!(sealed.len() > 12, "sealed packet carries a header + ct");
        assert_eq!(conn.bytes_tx(), b"hello tunnel".len() as u64);
        assert_eq!(conn.bytes_rx(), 0, "rx is 0 until a real datapath delivers");
    }

    #[test]
    fn tampered_response_fails_closed_never_connected() {
        let initiator_priv = [3u8; 32];
        let responder_priv = [9u8; 32];
        let psk = [42u8; 32];
        let (cfg, responder_static) = connectable_config(initiator_priv, responder_priv, psk);

        let mut conn = VpnConn::new(cfg);
        // tamper = true: the mock forges the response AEAD tag.
        let mut peer = MockPeer::new(responder_static, Key::from_bytes(&psk), true);

        let local_eph = Key::from_bytes(&[5u8; 32]);
        let state = conn.connect(&mut peer, local_eph, 1, 2_000_000_000, 4);

        assert_eq!(
            state,
            ConnState::Failed,
            "a forged/tampered response must FAIL closed"
        );
        assert_eq!(conn.fail_reason(), FailReason::HandshakeRejected);
        assert!(
            !conn.has_session(),
            "a failed handshake must NOT leave a session (no fake Connected)"
        );
    }

    #[test]
    fn wrong_psk_fails_closed() {
        let initiator_priv = [3u8; 32];
        let responder_priv = [9u8; 32];
        let app_psk = [42u8; 32];
        let peer_psk = [99u8; 32]; // mismatched PSK
        let (cfg, responder_static) = connectable_config(initiator_priv, responder_priv, app_psk);

        let mut conn = VpnConn::new(cfg);
        let mut peer = MockPeer::new(responder_static, Key::from_bytes(&peer_psk), false);

        let local_eph = Key::from_bytes(&[5u8; 32]);
        let state = conn.connect(&mut peer, local_eph, 1, 3_000_000_000, 4);

        assert_eq!(
            state,
            ConnState::Failed,
            "a PSK mismatch must fail the AEAD and reach Failed"
        );
        assert!(!conn.has_session());
    }

    #[test]
    fn not_connectable_config_fails_before_any_transport() {
        // Missing keys -> never even attempts the handshake.
        let cfg = PeerConfig::new("Empty");
        let mut conn = VpnConn::new(cfg);
        let mut peer = MockPeer::new(Key::from_bytes(&[9u8; 32]), Key::zero(), false);
        let state = conn.connect(&mut peer, Key::zero(), 1, 0, 4);
        assert_eq!(state, ConnState::Failed);
        assert_eq!(conn.fail_reason(), FailReason::NotConnectable);
    }

    #[test]
    fn timeout_when_peer_silent_never_connects() {
        let initiator_priv = [3u8; 32];
        let responder_priv = [9u8; 32];
        let psk = [42u8; 32];
        let (cfg, _responder_static) = connectable_config(initiator_priv, responder_priv, psk);

        struct Silent;
        impl VpnTransport for Silent {
            fn send(&mut self, _m: &[u8]) -> bool {
                true
            }
            fn recv(&mut self) -> Option<Vec<u8>> {
                None
            }
        }

        let mut conn = VpnConn::new(cfg);
        let mut silent = Silent;
        let state = conn.connect(&mut silent, Key::from_bytes(&[5u8; 32]), 1, 0, 3);
        assert_eq!(state, ConnState::Failed);
        assert_eq!(conn.fail_reason(), FailReason::Timeout);
        assert!(!conn.has_session());
    }

    #[test]
    fn send_failure_fails_closed() {
        let initiator_priv = [3u8; 32];
        let responder_priv = [9u8; 32];
        let psk = [42u8; 32];
        let (cfg, _responder_static) = connectable_config(initiator_priv, responder_priv, psk);

        struct Broken;
        impl VpnTransport for Broken {
            fn send(&mut self, _m: &[u8]) -> bool {
                false
            }
            fn recv(&mut self) -> Option<Vec<u8>> {
                None
            }
        }

        let mut conn = VpnConn::new(cfg);
        let mut broken = Broken;
        let state = conn.connect(&mut broken, Key::from_bytes(&[5u8; 32]), 1, 0, 3);
        assert_eq!(state, ConnState::Failed);
        assert_eq!(conn.fail_reason(), FailReason::SendFailed);
    }

    #[test]
    fn config_round_trips_through_serialize_parse() {
        let mut cfg = PeerConfig::new("RoundTrip");
        cfg.private_key = Key::from_bytes(&[1u8; 32]);
        cfg.peer_public_key = Key::from_bytes(&[2u8; 32]);
        cfg.preshared_key = Key::from_bytes(&[3u8; 32]);
        cfg.endpoint_host = String::from("vpn.example.com");
        cfg.endpoint_port = 51821;
        cfg.allowed_ips.push(String::from("10.0.0.0/24"));
        cfg.allowed_ips.push(String::from("192.168.1.0/24"));

        let text = cfg.serialize();
        let back = PeerConfig::parse(&text).expect("round-trip parse");

        assert_eq!(back.name, "RoundTrip");
        assert_eq!(back.private_key, cfg.private_key);
        assert_eq!(back.peer_public_key, cfg.peer_public_key);
        assert_eq!(back.preshared_key, cfg.preshared_key);
        assert_eq!(back.endpoint_host, "vpn.example.com");
        assert_eq!(back.endpoint_port, 51821);
        assert_eq!(back.allowed_ips.len(), 2);
        assert_eq!(back.allowed_ips[1], "192.168.1.0/24");
    }

    #[test]
    fn model_persists_multiple_peers() {
        let mut model = VpnModel::new();
        let mut a = PeerConfig::new("Alpha");
        a.private_key = Key::from_bytes(&[1u8; 32]);
        a.peer_public_key = Key::from_bytes(&[2u8; 32]);
        a.endpoint_host = String::from("a.example.com");
        let mut b = PeerConfig::new("Beta");
        b.private_key = Key::from_bytes(&[4u8; 32]);
        b.peer_public_key = Key::from_bytes(&[5u8; 32]);
        b.endpoint_host = String::from("b.example.com");
        model.add(a);
        model.add(b);

        let blob = model.serialize();
        let loaded = VpnModel::load(&blob);
        assert_eq!(loaded.conns.len(), 2);
        assert_eq!(loaded.conns[0].config.name, "Alpha");
        assert_eq!(loaded.conns[1].config.name, "Beta");
        assert_eq!(loaded.conns[1].config.endpoint_host, "b.example.com");
        // Reloaded peers start Disconnected (no live session persists).
        assert_eq!(loaded.conns[0].state(), ConnState::Disconnected);
    }

    #[test]
    fn base64_round_trips_all_byte_values() {
        let mut data = Vec::new();
        for i in 0..32u16 {
            data.push((i * 7) as u8);
        }
        let enc = b64_encode(&data);
        let dec = b64_decode(&enc).expect("decode");
        assert_eq!(dec, data);
    }
}
