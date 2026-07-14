//! `ssh-connection` — the channel layer (RFC 4254): after authentication the
//! client opens a `session` channel, requests a `pty-req` and a `shell` (or an
//! `exec`), and interactive data flows over `CHANNEL_DATA` with per-channel flow
//! control (windows). This module is the pure-logic half: parse/build every
//! channel message, dispatch the session requests, and run the window
//! accounting state machine. The actual shell/pty backing (wiring a channel to
//! a RaeShell session) is the integration slice; here everything is
//! host-KAT-provable and never panics on a hostile peer.

use crate::{read_string, read_u32, write_string, write_u32, SshError};
use crate::{
    SSH_MSG_CHANNEL_DATA, SSH_MSG_CHANNEL_OPEN, SSH_MSG_CHANNEL_OPEN_CONFIRMATION,
    SSH_MSG_CHANNEL_OPEN_FAILURE, SSH_MSG_CHANNEL_REQUEST, SSH_MSG_CHANNEL_WINDOW_ADJUST,
};
use alloc::string::String;
use alloc::vec::Vec;

/// A parsed `SSH_MSG_CHANNEL_OPEN` (RFC 4254 §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelOpen {
    pub channel_type: String,
    pub sender_channel: u32,
    pub initial_window: u32,
    pub max_packet: u32,
}

/// The session-request variants we understand (RFC 4254 §6). `Other` keeps the
/// name so the server can `CHANNEL_FAILURE` it without crashing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRequestKind {
    /// `pty-req` — allocate a pseudo-terminal.
    PtyReq {
        term: String,
        cols: u32,
        rows: u32,
        width_px: u32,
        height_px: u32,
    },
    /// `shell` — start the user's default shell.
    Shell,
    /// `exec` — run a single command.
    Exec { command: String },
    /// `env` — set an environment variable.
    Env { name: String, value: String },
    /// Any other request type (window-change, signal, subsystem, …).
    Other(String),
}

/// A parsed `SSH_MSG_CHANNEL_REQUEST` (RFC 4254 §5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRequest {
    pub recipient: u32,
    pub want_reply: bool,
    pub kind: ChannelRequestKind,
}

/// Reason codes for `SSH_MSG_CHANNEL_OPEN_FAILURE` (RFC 4254 §5.1).
pub const OPEN_ADMINISTRATIVELY_PROHIBITED: u32 = 1;
pub const OPEN_UNKNOWN_CHANNEL_TYPE: u32 = 3;

// ── Parsers ──────────────────────────────────────────────────────────────────

/// Parse `SSH_MSG_CHANNEL_OPEN`. Rejects wrong message code / bad structure.
pub fn parse_channel_open(payload: &[u8]) -> Result<ChannelOpen, SshError> {
    if payload.first() != Some(&SSH_MSG_CHANNEL_OPEN) {
        return Err(SshError::Unexpected);
    }
    let mut pos = 1;
    let type_bytes;
    (type_bytes, pos) = read_string(payload, pos)?;
    let sender_channel;
    (sender_channel, pos) = read_u32(payload, pos)?;
    let initial_window;
    (initial_window, pos) = read_u32(payload, pos)?;
    let max_packet;
    (max_packet, _) = read_u32(payload, pos)?;
    Ok(ChannelOpen {
        channel_type: String::from_utf8(type_bytes.to_vec()).map_err(|_| SshError::Malformed)?,
        sender_channel,
        initial_window,
        max_packet,
    })
}

/// Parse `SSH_MSG_CHANNEL_REQUEST`, decoding the common session request types.
pub fn parse_channel_request(payload: &[u8]) -> Result<ChannelRequest, SshError> {
    if payload.first() != Some(&SSH_MSG_CHANNEL_REQUEST) {
        return Err(SshError::Unexpected);
    }
    let mut pos = 1;
    let recipient;
    (recipient, pos) = read_u32(payload, pos)?;
    let type_bytes;
    (type_bytes, pos) = read_string(payload, pos)?;
    if pos >= payload.len() {
        return Err(SshError::NeedMoreData);
    }
    let want_reply = payload[pos] != 0;
    pos += 1;
    let rtype = core::str::from_utf8(type_bytes).map_err(|_| SshError::Malformed)?;

    let kind = match rtype {
        "shell" => ChannelRequestKind::Shell,
        "exec" => {
            let (cmd, _) = read_string(payload, pos)?;
            ChannelRequestKind::Exec {
                command: String::from_utf8(cmd.to_vec()).map_err(|_| SshError::Malformed)?,
            }
        }
        "env" => {
            let name;
            (name, pos) = read_string(payload, pos)?;
            let (value, _) = read_string(payload, pos)?;
            ChannelRequestKind::Env {
                name: String::from_utf8(name.to_vec()).map_err(|_| SshError::Malformed)?,
                value: String::from_utf8(value.to_vec()).map_err(|_| SshError::Malformed)?,
            }
        }
        "pty-req" => {
            let term;
            (term, pos) = read_string(payload, pos)?;
            let cols;
            (cols, pos) = read_u32(payload, pos)?;
            let rows;
            (rows, pos) = read_u32(payload, pos)?;
            let width_px;
            (width_px, pos) = read_u32(payload, pos)?;
            let height_px;
            (height_px, _) = read_u32(payload, pos)?;
            ChannelRequestKind::PtyReq {
                term: String::from_utf8(term.to_vec()).map_err(|_| SshError::Malformed)?,
                cols,
                rows,
                width_px,
                height_px,
            }
        }
        other => ChannelRequestKind::Other(String::from(other)),
    };
    Ok(ChannelRequest {
        recipient,
        want_reply,
        kind,
    })
}

/// Parse a `SSH_MSG_CHANNEL_DATA` payload, returning `(recipient, data)`.
pub fn parse_channel_data(payload: &[u8]) -> Result<(u32, Vec<u8>), SshError> {
    if payload.first() != Some(&SSH_MSG_CHANNEL_DATA) {
        return Err(SshError::Unexpected);
    }
    let (recipient, pos) = read_u32(payload, 1)?;
    let (data, _) = read_string(payload, pos)?;
    Ok((recipient, data.to_vec()))
}

/// Parse a bare `recipient`-only channel message (EOF / CLOSE), checking the code.
pub fn parse_channel_id_msg(payload: &[u8], expect_code: u8) -> Result<u32, SshError> {
    if payload.first() != Some(&expect_code) {
        return Err(SshError::Unexpected);
    }
    let (recipient, _) = read_u32(payload, 1)?;
    Ok(recipient)
}

/// Parse `SSH_MSG_CHANNEL_WINDOW_ADJUST`, returning `(recipient, bytes_to_add)`.
pub fn parse_window_adjust(payload: &[u8]) -> Result<(u32, u32), SshError> {
    if payload.first() != Some(&SSH_MSG_CHANNEL_WINDOW_ADJUST) {
        return Err(SshError::Unexpected);
    }
    let (recipient, pos) = read_u32(payload, 1)?;
    let (add, _) = read_u32(payload, pos)?;
    Ok((recipient, add))
}

// ── Builders ─────────────────────────────────────────────────────────────────

/// `SSH_MSG_CHANNEL_OPEN_CONFIRMATION` — accept a channel open.
pub fn build_open_confirmation(
    recipient: u32,
    sender: u32,
    initial_window: u32,
    max_packet: u32,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(17);
    out.push(SSH_MSG_CHANNEL_OPEN_CONFIRMATION);
    write_u32(&mut out, recipient);
    write_u32(&mut out, sender);
    write_u32(&mut out, initial_window);
    write_u32(&mut out, max_packet);
    out
}

/// `SSH_MSG_CHANNEL_OPEN_FAILURE` — refuse a channel open.
pub fn build_open_failure(recipient: u32, reason: u32, description: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + description.len());
    out.push(SSH_MSG_CHANNEL_OPEN_FAILURE);
    write_u32(&mut out, recipient);
    write_u32(&mut out, reason);
    write_string(&mut out, description.as_bytes());
    write_string(&mut out, b""); // language tag
    out
}

/// `SSH_MSG_CHANNEL_DATA` — a data segment for a channel.
pub fn build_channel_data(recipient: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + data.len());
    out.push(SSH_MSG_CHANNEL_DATA);
    write_u32(&mut out, recipient);
    write_string(&mut out, data);
    out
}

/// A bare `recipient`-only channel message (EOF / CLOSE / SUCCESS / FAILURE).
pub fn build_channel_id_msg(code: u8, recipient: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.push(code);
    write_u32(&mut out, recipient);
    out
}

/// `SSH_MSG_CHANNEL_WINDOW_ADJUST` — grant the peer `add` more bytes.
pub fn build_window_adjust(recipient: u32, add: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    out.push(SSH_MSG_CHANNEL_WINDOW_ADJUST);
    write_u32(&mut out, recipient);
    write_u32(&mut out, add);
    out
}

// ── Channel state machine (RFC 4254 §5.2 flow control) ───────────────────────

/// One open channel's flow-control + lifecycle state. Windows are byte credits:
/// we may send at most `remote_window` bytes (and never more than `max_packet`
/// per `CHANNEL_DATA`); we top our `local_window` back up as the peer consumes
/// it, emitting a `WINDOW_ADJUST` so it can keep sending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel {
    pub local_id: u32,
    pub remote_id: u32,
    /// Bytes we are still willing to receive before the peer must wait.
    pub local_window: u32,
    /// Bytes the peer will still accept from us.
    pub remote_window: u32,
    /// Max bytes per `CHANNEL_DATA` we send (the peer's advertised limit).
    pub max_packet: u32,
    pub local_eof: bool,
    pub remote_eof: bool,
    pub closed: bool,
    initial_window: u32,
}

impl Channel {
    /// Accept a peer's `CHANNEL_OPEN` as `local_id`, advertising our own window.
    pub fn accept(open: &ChannelOpen, local_id: u32, our_window: u32) -> Self {
        Self {
            local_id,
            remote_id: open.sender_channel,
            local_window: our_window,
            remote_window: open.initial_window,
            max_packet: open.max_packet,
            local_eof: false,
            remote_eof: false,
            closed: false,
            initial_window: our_window,
        }
    }

    /// The confirmation message that answers the open we just `accept`ed.
    pub fn confirmation(&self, our_max_packet: u32) -> Vec<u8> {
        build_open_confirmation(
            self.remote_id,
            self.local_id,
            self.local_window,
            our_max_packet,
        )
    }

    /// Account for `len` bytes RECEIVED from the peer. Returns `Some(add)` if the
    /// local window fell to/under half and should be topped back up — the caller
    /// sends `build_window_adjust(remote_id, add)` and the window is already
    /// credited here. Rejects an over-run (peer exceeded the window we granted).
    pub fn on_receive(&mut self, len: u32) -> Result<Option<u32>, SshError> {
        if len > self.local_window {
            return Err(SshError::Malformed); // peer violated flow control
        }
        self.local_window -= len;
        if self.local_window * 2 <= self.initial_window {
            let add = self.initial_window - self.local_window;
            self.local_window = self.initial_window;
            Ok(Some(add))
        } else {
            Ok(None)
        }
    }

    /// The peer granted us `add` more send-credit (`WINDOW_ADJUST`).
    pub fn on_window_adjust(&mut self, add: u32) {
        self.remote_window = self.remote_window.saturating_add(add);
    }

    /// How many bytes of a `want`-byte write we may send RIGHT NOW: bounded by
    /// the remote window and `max_packet`. Caller sends that many then calls
    /// [`Self::consume_send`]. Returns 0 when the window is closed (must wait for
    /// a `WINDOW_ADJUST`).
    pub fn sendable(&self, want: u32) -> u32 {
        want.min(self.remote_window).min(self.max_packet.max(1))
    }

    /// Debit `n` bytes actually sent from the remote window.
    pub fn consume_send(&mut self, n: u32) {
        self.remote_window = self.remote_window.saturating_sub(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SSH_MSG_CHANNEL_CLOSE, SSH_MSG_CHANNEL_EOF};

    fn open_msg(ch: u32, win: u32, max: u32, ty: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(SSH_MSG_CHANNEL_OPEN);
        write_string(&mut out, ty.as_bytes());
        write_u32(&mut out, ch);
        write_u32(&mut out, win);
        write_u32(&mut out, max);
        out
    }

    #[test]
    fn channel_open_parses_and_confirms() {
        let msg = open_msg(3, 2_000_000, 32768, "session");
        let open = parse_channel_open(&msg).unwrap();
        assert_eq!(open.channel_type, "session");
        assert_eq!(open.sender_channel, 3);
        assert_eq!(open.initial_window, 2_000_000);
        assert_eq!(open.max_packet, 32768);

        let ch = Channel::accept(&open, 0, 1_048_576);
        let conf = ch.confirmation(32768);
        // Confirmation: recipient = peer's sender (3), sender = our local id (0).
        assert_eq!(conf[0], SSH_MSG_CHANNEL_OPEN_CONFIRMATION);
        let (recipient, p) = read_u32(&conf, 1).unwrap();
        let (sender, _) = read_u32(&conf, p).unwrap();
        assert_eq!(recipient, 3);
        assert_eq!(sender, 0);
    }

    #[test]
    fn channel_requests_decode() {
        // shell
        let mut shell = Vec::new();
        shell.push(SSH_MSG_CHANNEL_REQUEST);
        write_u32(&mut shell, 0);
        write_string(&mut shell, b"shell");
        shell.push(1);
        let r = parse_channel_request(&shell).unwrap();
        assert_eq!(r.kind, ChannelRequestKind::Shell);
        assert!(r.want_reply);

        // exec
        let mut exec = Vec::new();
        exec.push(SSH_MSG_CHANNEL_REQUEST);
        write_u32(&mut exec, 0);
        write_string(&mut exec, b"exec");
        exec.push(0);
        write_string(&mut exec, b"uname -a");
        let r = parse_channel_request(&exec).unwrap();
        assert_eq!(
            r.kind,
            ChannelRequestKind::Exec {
                command: String::from("uname -a")
            }
        );

        // pty-req
        let mut pty = Vec::new();
        pty.push(SSH_MSG_CHANNEL_REQUEST);
        write_u32(&mut pty, 0);
        write_string(&mut pty, b"pty-req");
        pty.push(1);
        write_string(&mut pty, b"xterm-256color");
        write_u32(&mut pty, 80);
        write_u32(&mut pty, 24);
        write_u32(&mut pty, 640);
        write_u32(&mut pty, 480);
        write_string(&mut pty, b""); // modes
        let r = parse_channel_request(&pty).unwrap();
        assert_eq!(
            r.kind,
            ChannelRequestKind::PtyReq {
                term: String::from("xterm-256color"),
                cols: 80,
                rows: 24,
                width_px: 640,
                height_px: 480,
            }
        );

        // unknown -> Other(name), never an error/panic.
        let mut sig = Vec::new();
        sig.push(SSH_MSG_CHANNEL_REQUEST);
        write_u32(&mut sig, 0);
        write_string(&mut sig, b"signal");
        sig.push(0);
        assert_eq!(
            parse_channel_request(&sig).unwrap().kind,
            ChannelRequestKind::Other(String::from("signal"))
        );
    }

    #[test]
    fn channel_data_roundtrips() {
        let msg = build_channel_data(7, b"ls -la\n");
        let (recipient, data) = parse_channel_data(&msg).unwrap();
        assert_eq!(recipient, 7);
        assert_eq!(data, b"ls -la\n");
    }

    #[test]
    fn eof_and_close_roundtrip() {
        let eof = build_channel_id_msg(SSH_MSG_CHANNEL_EOF, 5);
        assert_eq!(parse_channel_id_msg(&eof, SSH_MSG_CHANNEL_EOF).unwrap(), 5);
        let close = build_channel_id_msg(SSH_MSG_CHANNEL_CLOSE, 5);
        assert_eq!(
            parse_channel_id_msg(&close, SSH_MSG_CHANNEL_CLOSE).unwrap(),
            5
        );
        // Wrong expected code is rejected, not mis-parsed.
        assert_eq!(
            parse_channel_id_msg(&eof, SSH_MSG_CHANNEL_CLOSE),
            Err(SshError::Unexpected)
        );
    }

    #[test]
    fn send_window_bounds_and_max_packet() {
        let open = parse_channel_open(&open_msg(1, 100, 10, "session")).unwrap();
        let mut ch = Channel::accept(&open, 0, 1000);
        // remote_window=100, max_packet=10 -> a 50-byte write is clamped to 10.
        assert_eq!(ch.sendable(50), 10);
        ch.consume_send(10);
        assert_eq!(ch.remote_window, 90);
        // Drain the window; then sendable is 0 until a WINDOW_ADJUST arrives.
        for _ in 0..9 {
            ch.consume_send(10);
        }
        assert_eq!(ch.remote_window, 0);
        assert_eq!(ch.sendable(10), 0);
        ch.on_window_adjust(25);
        assert_eq!(ch.sendable(100), 10); // window 25, clamped by max_packet 10
    }

    #[test]
    fn receive_tops_up_window_at_half() {
        let open = parse_channel_open(&open_msg(1, 100, 32768, "session")).unwrap();
        let mut ch = Channel::accept(&open, 0, 100); // our window = 100
                                                     // Consume 40 -> 60 left (> half) -> no adjust.
        assert_eq!(ch.on_receive(40).unwrap(), None);
        assert_eq!(ch.local_window, 60);
        // Consume 20 more -> 40 left (<= half of 100) -> top up by 60 to 100.
        assert_eq!(ch.on_receive(20).unwrap(), Some(60));
        assert_eq!(ch.local_window, 100);
        // A peer that exceeds the granted window is rejected (flow-control abuse).
        assert_eq!(ch.on_receive(101), Err(SshError::Malformed));
    }

    #[test]
    fn hostile_bytes_never_panic() {
        assert_eq!(parse_channel_open(&[]), Err(SshError::Unexpected));
        assert!(matches!(
            parse_channel_open(&[SSH_MSG_CHANNEL_OPEN, 0, 0, 0, 7]),
            Err(SshError::NeedMoreData)
        ));
        assert!(matches!(
            parse_channel_request(&[SSH_MSG_CHANNEL_REQUEST, 0, 0]),
            Err(SshError::NeedMoreData)
        ));
        assert!(matches!(
            parse_channel_data(&[SSH_MSG_CHANNEL_DATA, 0, 0, 0, 1, 0, 0, 0, 200]),
            Err(SshError::NeedMoreData)
        ));
    }
}
