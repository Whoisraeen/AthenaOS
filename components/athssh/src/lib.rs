//! AthenaOS SSH server — the SSH-2.0 transport/auth/connection protocol as pure
//! logic on top of `ath_crypto`. You SSH into a running AthenaOS and get a
//! AthShell session (Concept §"the user owns the machine": real remote access,
//! no cloud in the middle). No OpenSSL / libssh — every primitive is AthenaOS's
//! own: curve25519-sha256 KEX, ssh-ed25519 host key + publickey auth,
//! chacha20-poly1305@openssh packet cipher.
//!
//! This crate is layered so each slice is host-KAT-provable against real
//! OpenSSH wire bytes before it touches a socket:
//!   Slice 1 (here): version exchange · binary packet framing (RFC 4253 §6) ·
//!                   name-lists (RFC 4251 §5) · KEXINIT negotiation (§7.1).
//!   Slice 2+: curve25519 KEX + key derivation → encrypted packets → publickey
//!             auth → session/pty/shell → the smoltcp TCP service.
//!
//! Every peer byte is attacker-controlled, so nothing here panics or indexes
//! unchecked: malformed input is always an `Err`, never a crash.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod connection;
pub mod kex;
pub mod kexinit;
pub mod selftest;
pub mod server;
pub mod session;
pub mod transport;
pub mod userauth;

/// Our SSH identification string (RFC 4253 §4.2), WITHOUT the trailing CR LF —
/// this is the exact `V_S` byte string that feeds the key-exchange hash.
pub const IDENT: &[u8] = b"SSH-2.0-RaeSSH_0.1";

/// SSH message numbers we handle (RFC 4253 §12 / 4252 / 4254). Only the ones
/// Slice 1 needs are defined; later slices add the rest.
pub const SSH_MSG_DISCONNECT: u8 = 1;
pub const SSH_MSG_SERVICE_REQUEST: u8 = 5;
pub const SSH_MSG_SERVICE_ACCEPT: u8 = 6;
pub const SSH_MSG_KEXINIT: u8 = 20;
pub const SSH_MSG_NEWKEYS: u8 = 21;
// User authentication (RFC 4252).
pub const SSH_MSG_USERAUTH_REQUEST: u8 = 50;
pub const SSH_MSG_USERAUTH_FAILURE: u8 = 51;
pub const SSH_MSG_USERAUTH_SUCCESS: u8 = 52;
pub const SSH_MSG_USERAUTH_PK_OK: u8 = 60;
// Connection protocol — channels (RFC 4254).
pub const SSH_MSG_GLOBAL_REQUEST: u8 = 80;
pub const SSH_MSG_REQUEST_SUCCESS: u8 = 81;
pub const SSH_MSG_REQUEST_FAILURE: u8 = 82;
pub const SSH_MSG_CHANNEL_OPEN: u8 = 90;
pub const SSH_MSG_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
pub const SSH_MSG_CHANNEL_OPEN_FAILURE: u8 = 92;
pub const SSH_MSG_CHANNEL_WINDOW_ADJUST: u8 = 93;
pub const SSH_MSG_CHANNEL_DATA: u8 = 94;
pub const SSH_MSG_CHANNEL_EXTENDED_DATA: u8 = 95;
pub const SSH_MSG_CHANNEL_EOF: u8 = 96;
pub const SSH_MSG_CHANNEL_CLOSE: u8 = 97;
pub const SSH_MSG_CHANNEL_REQUEST: u8 = 98;
pub const SSH_MSG_CHANNEL_SUCCESS: u8 = 99;
pub const SSH_MSG_CHANNEL_FAILURE: u8 = 100;

/// SSH packet block size for the `none` cipher / minimum alignment (RFC 4253 §6:
/// "the total length ... MUST be a multiple of the cipher block size or 8,
/// whichever is larger"). Slice 1 frames unencrypted, so 8.
const BLOCK_SIZE: usize = 8;

/// A remote peer must never crash us — every parse failure is one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshError {
    /// The buffer does not yet hold a whole item; read more from the socket.
    NeedMoreData,
    /// A length/field is structurally impossible (overflow, out of range).
    Malformed,
    /// The peer's identification string is not a valid `SSH-2.0-` / `SSH-1.99-`.
    BadIdent,
    /// A negotiated set had no algorithm in common.
    NoMatch,
    /// The message was not the expected type.
    Unexpected,
    /// An encrypted packet's Poly1305 tag did not verify (forgery / corruption /
    /// wrong key or sequence number) — the packet is discarded, no plaintext
    /// released.
    BadMac,
}

// ── Version exchange (RFC 4253 §4.2) ─────────────────────────────────────────

/// Our identification line as sent on the wire: `IDENT` + CR LF.
pub fn ident_line() -> Vec<u8> {
    let mut v = Vec::with_capacity(IDENT.len() + 2);
    v.extend_from_slice(IDENT);
    v.extend_from_slice(b"\r\n");
    v
}

/// Parse the peer's identification, returning its `SSH-2.0-...` line WITHOUT the
/// CR LF (the `V_C` that feeds the exchange hash). A server MAY send banner
/// lines before its ident (§4.2), so we skip lines that do not start with
/// `SSH-`. Bounded to the 255-byte limit so a peer cannot make us buffer
/// forever. `NeedMoreData` until a full CR-LF-terminated ident line is present.
pub fn parse_peer_ident(buf: &[u8]) -> Result<(Vec<u8>, usize), SshError> {
    let mut start = 0;
    loop {
        // Find the next line terminator (LF; tolerate bare LF or CR LF).
        let rel = match buf[start..].iter().position(|&b| b == b'\n') {
            Some(i) => i,
            None => {
                if buf.len() - start > 255 {
                    return Err(SshError::BadIdent);
                }
                return Err(SshError::NeedMoreData);
            }
        };
        let line_end = start + rel; // index of the LF
                                    // Line content without CR/LF.
        let mut content_end = line_end;
        if content_end > start && buf[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let line = &buf[start..content_end];
        if line.len() > 255 {
            return Err(SshError::BadIdent);
        }
        if line.starts_with(b"SSH-") {
            if !(line.starts_with(b"SSH-2.0-") || line.starts_with(b"SSH-1.99-")) {
                return Err(SshError::BadIdent);
            }
            return Ok((line.to_vec(), line_end + 1));
        }
        // A pre-ident banner line — skip it and look at the next line.
        start = line_end + 1;
        if start >= buf.len() {
            return Err(SshError::NeedMoreData);
        }
    }
}

// ── SSH strings & name-lists (RFC 4251 §5) ───────────────────────────────────

/// Append an SSH `uint32` (big-endian) to `out`.
pub fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Read an SSH `uint32` (big-endian) at `pos`, returning it and the new position.
pub fn read_u32(buf: &[u8], pos: usize) -> Result<(u32, usize), SshError> {
    let end = pos.checked_add(4).ok_or(SshError::Malformed)?;
    if buf.len() < end {
        return Err(SshError::NeedMoreData);
    }
    let v = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
    Ok((v, end))
}

/// Append an SSH `string`/`byte[]` (uint32 length + bytes) to `out`.
pub fn write_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Read an SSH `string` at `pos`, returning the bytes and the new position.
pub fn read_string(buf: &[u8], pos: usize) -> Result<(&[u8], usize), SshError> {
    let end_len = pos.checked_add(4).ok_or(SshError::Malformed)?;
    if buf.len() < end_len {
        return Err(SshError::NeedMoreData);
    }
    let len = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
    let end = end_len.checked_add(len).ok_or(SshError::Malformed)?;
    if buf.len() < end {
        return Err(SshError::NeedMoreData);
    }
    Ok((&buf[end_len..end], end))
}

/// A `name-list` is a comma-separated string of ASCII names. Append one.
pub fn write_name_list(out: &mut Vec<u8>, names: &[&str]) {
    let joined = names.join(",");
    write_string(out, joined.as_bytes());
}

/// Read a `name-list` at `pos` into owned names (empty list = no names).
pub fn read_name_list(buf: &[u8], pos: usize) -> Result<(Vec<String>, usize), SshError> {
    let (bytes, next) = read_string(buf, pos)?;
    let s = core::str::from_utf8(bytes).map_err(|_| SshError::Malformed)?;
    let names = if s.is_empty() {
        Vec::new()
    } else {
        s.split(',').map(String::from).collect()
    };
    Ok((names, next))
}

// ── Binary packet protocol (RFC 4253 §6) ─────────────────────────────────────

/// Frame `payload` into an UNENCRYPTED SSH binary packet:
/// `uint32 packet_length | byte padding_length | payload | padding`. The total
/// on-wire length is padded to a multiple of `BLOCK_SIZE` with at least 4 bytes
/// of padding. `pad_fill` supplies the padding bytes (RFC 4253 §6 requires
/// RANDOM padding for security; a real transport passes CSPRNG output — tests
/// pass a fixed byte for determinism).
pub fn build_packet(payload: &[u8], pad_fill: u8) -> Vec<u8> {
    // total-so-far before padding = 4 (len) + 1 (pad_len) + payload
    let unpadded = 5 + payload.len();
    let mut pad_len = BLOCK_SIZE - (unpadded % BLOCK_SIZE);
    if pad_len < 4 {
        pad_len += BLOCK_SIZE;
    }
    let packet_len = 1 + payload.len() + pad_len; // pad_len byte + payload + padding
    let mut out = Vec::with_capacity(4 + packet_len);
    out.extend_from_slice(&(packet_len as u32).to_be_bytes());
    out.push(pad_len as u8);
    out.extend_from_slice(payload);
    out.extend(core::iter::repeat(pad_fill).take(pad_len));
    out
}

/// The largest packet we will accept, matching OpenSSH's default (§6 leaves it
/// implementation-defined; a bound stops a hostile `packet_length` from making
/// us wait for gigabytes).
pub const MAX_PACKET: usize = 35_000;

/// Parse one UNENCRYPTED packet from the front of `buf`. Returns the payload and
/// the number of bytes consumed (so the caller advances its read cursor).
/// `NeedMoreData` until the whole packet is buffered; `Malformed` on any
/// structurally invalid framing.
pub fn parse_packet(buf: &[u8]) -> Result<(Vec<u8>, usize), SshError> {
    if buf.len() < 4 {
        return Err(SshError::NeedMoreData);
    }
    let packet_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    // packet_len covers padding_length(1) + payload + padding, so >= 1 + 4.
    if packet_len < 5 || packet_len > MAX_PACKET {
        return Err(SshError::Malformed);
    }
    let total = 4 + packet_len;
    if buf.len() < total {
        return Err(SshError::NeedMoreData);
    }
    let pad_len = buf[4] as usize;
    // payload occupies packet_len - pad_len - 1 bytes; both must fit.
    if pad_len < 4 || pad_len + 1 > packet_len {
        return Err(SshError::Malformed);
    }
    let payload_len = packet_len - pad_len - 1;
    let payload = buf[5..5 + payload_len].to_vec();
    Ok((payload, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ident_line_is_crlf_terminated_and_hash_form_has_none() {
        assert_eq!(ident_line(), b"SSH-2.0-RaeSSH_0.1\r\n");
        assert_eq!(IDENT, b"SSH-2.0-RaeSSH_0.1"); // no CR/LF in the hash input
    }

    #[test]
    fn parse_peer_ident_openssh_line() {
        let wire = b"SSH-2.0-OpenSSH_9.6\r\nrest";
        let (id, consumed) = parse_peer_ident(wire).unwrap();
        assert_eq!(id, b"SSH-2.0-OpenSSH_9.6");
        assert_eq!(&wire[consumed..], b"rest");
    }

    #[test]
    fn parse_peer_ident_skips_pre_banner_lines() {
        let wire = b"hello banner\r\nSSH-2.0-OpenSSH_9.6\r\n";
        let (id, _) = parse_peer_ident(wire).unwrap();
        assert_eq!(id, b"SSH-2.0-OpenSSH_9.6");
    }

    #[test]
    fn parse_peer_ident_partial_needs_more() {
        assert_eq!(
            parse_peer_ident(b"SSH-2.0-Open"),
            Err(SshError::NeedMoreData)
        );
    }

    #[test]
    fn parse_peer_ident_wrong_protocol_is_bad() {
        assert_eq!(
            parse_peer_ident(b"SSH-3.0-nope\r\n"),
            Err(SshError::BadIdent)
        );
    }

    #[test]
    fn name_list_round_trips_and_empty_is_empty() {
        let mut out = Vec::new();
        write_name_list(&mut out, &["curve25519-sha256", "ecdh-sha2-nistp256"]);
        let (names, next) = read_name_list(&out, 0).unwrap();
        assert_eq!(names, ["curve25519-sha256", "ecdh-sha2-nistp256"]);
        assert_eq!(next, out.len());

        let mut e = Vec::new();
        write_name_list(&mut e, &[]);
        assert_eq!(e, [0, 0, 0, 0]); // zero-length string
        assert_eq!(read_name_list(&e, 0).unwrap().0.len(), 0);
    }

    #[test]
    fn read_string_truncated_needs_more() {
        // length says 8 but only 2 bytes follow.
        let buf = [0, 0, 0, 8, 1, 2];
        assert_eq!(read_string(&buf, 0), Err(SshError::NeedMoreData));
    }

    #[test]
    fn packet_round_trips_and_is_block_aligned() {
        for payload_len in 0..40usize {
            let payload: Vec<u8> = (0..payload_len as u8).collect();
            let pkt = build_packet(&payload, 0);
            assert_eq!(pkt.len() % BLOCK_SIZE, 0, "total must be block-aligned");
            let pad_len = pkt[4] as usize;
            assert!(pad_len >= 4, "at least 4 bytes of padding (§6)");
            let (got, consumed) = parse_packet(&pkt).unwrap();
            assert_eq!(got, payload);
            assert_eq!(consumed, pkt.len());
        }
    }

    #[test]
    fn parse_packet_rejects_hostile_length_and_padding() {
        // packet_length past MAX_PACKET.
        let mut big = (MAX_PACKET as u32 + 1).to_be_bytes().to_vec();
        big.extend_from_slice(&[0u8; 8]);
        assert_eq!(parse_packet(&big), Err(SshError::Malformed));

        // padding_length larger than the packet.
        let mut bad = 6u32.to_be_bytes().to_vec(); // packet_len = 6
        bad.push(200); // pad_len 200 > packet_len
        bad.extend_from_slice(&[0u8; 6]);
        assert_eq!(parse_packet(&bad), Err(SshError::Malformed));

        // pad_len < 4 is illegal.
        let mut small = 6u32.to_be_bytes().to_vec();
        small.push(1);
        small.extend_from_slice(&[0u8; 6]);
        assert_eq!(parse_packet(&small), Err(SshError::Malformed));
    }

    #[test]
    fn parse_packet_incomplete_needs_more() {
        let pkt = build_packet(b"hello", 0);
        assert_eq!(
            parse_packet(&pkt[..pkt.len() - 1]),
            Err(SshError::NeedMoreData)
        );
        assert_eq!(parse_packet(&[0, 0]), Err(SshError::NeedMoreData));
    }
}
