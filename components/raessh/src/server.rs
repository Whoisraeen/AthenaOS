//! `ServerSession` — the authenticated (encrypted) phase driver, the second
//! half of the sans-io server. Once [`crate::session::ServerHandshake`] yields
//! the directional ciphers, this drives everything after NEWKEYS: it decrypts
//! each inbound packet (bumping the receive sequence number), runs the
//! `ssh-userauth` publickey exchange, opens the `session` channel, and turns
//! channel traffic into semantic [`SessionEvent`]s for the host to act on —
//! encrypting every reply on the way out. The host binds `ShellRequested` /
//! `Data` events to a RaeShell session and calls [`ServerSession::channel_output`]
//! to send shell output back; this module owns ALL of the protocol so that the
//! integration layer is pure byte-shuffling. Never panics on a hostile peer.

use crate::connection::{
    build_channel_data, build_channel_id_msg, build_open_confirmation, build_open_failure,
    build_window_adjust, parse_channel_data, parse_channel_open, parse_channel_request,
    parse_window_adjust, Channel, ChannelRequestKind, OPEN_UNKNOWN_CHANNEL_TYPE,
};
use crate::session::HandshakeKeys;
use crate::transport::ChaChaPolyCipher;
use crate::userauth::{
    authenticate, build_pk_ok, parse_publickey_request, AuthOutcome, AuthorizedKey,
};
use crate::{read_string, write_name_list, write_string, SshError};
use crate::{
    SSH_MSG_CHANNEL_CLOSE, SSH_MSG_CHANNEL_DATA, SSH_MSG_CHANNEL_EOF, SSH_MSG_CHANNEL_FAILURE,
    SSH_MSG_CHANNEL_OPEN, SSH_MSG_CHANNEL_REQUEST, SSH_MSG_CHANNEL_SUCCESS, SSH_MSG_DISCONNECT,
    SSH_MSG_SERVICE_ACCEPT, SSH_MSG_SERVICE_REQUEST, SSH_MSG_USERAUTH_FAILURE,
    SSH_MSG_USERAUTH_REQUEST, SSH_MSG_USERAUTH_SUCCESS,
};
use alloc::string::String;
use alloc::vec::Vec;

/// The window we advertise for the session channel (bytes) and our max packet.
const LOCAL_WINDOW: u32 = 1 << 20; // 1 MiB
const LOCAL_MAX_PACKET: u32 = 32_768;

/// A semantic event the host must act on (bind to a RaeShell session, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Nothing for the host to do (the protocol reply, if any, is in `reply`).
    None,
    /// Publickey auth succeeded for `user` — the host may set up their session.
    Authenticated { user: String },
    /// The client asked for an interactive shell on `channel` (its channel id).
    ShellRequested { channel: u32 },
    /// The client asked to run `command` on `channel`.
    ExecRequested { channel: u32, command: String },
    /// Terminal input (or any channel data) arrived — feed it to the shell.
    Data { channel: u32, data: Vec<u8> },
    /// The client sent EOF on `channel`.
    Eof { channel: u32 },
    /// The channel closed — tear down the shell.
    ChannelClosed { channel: u32 },
    /// The client asked to disconnect (or a fatal protocol error) — drop the TCP.
    Disconnect,
}

/// The result of feeding one inbound packet: encrypted bytes to transmit
/// (possibly empty) plus the semantic event.
pub struct SessionStep {
    pub reply: Vec<u8>,
    pub event: SessionEvent,
}

/// The post-handshake SSH server session.
pub struct ServerSession {
    session_id: [u8; 32],
    /// Decrypts inbound (client→server) packets.
    cipher_c2s: ChaChaPolyCipher,
    /// Encrypts outbound (server→client) packets.
    cipher_s2c: ChaChaPolyCipher,
    rx_seq: u64,
    tx_seq: u64,
    authorized: Vec<AuthorizedKey>,
    authed: bool,
    user: Option<String>,
    /// The single `session` channel (this server serves one per connection).
    channel: Option<Channel>,
}

impl ServerSession {
    /// Build the session from a completed handshake + the `authorized_keys`
    /// allow-list. Consumes the handshake keys (ciphers + starting seq numbers).
    pub fn from_handshake(keys: HandshakeKeys, authorized: Vec<AuthorizedKey>) -> Self {
        Self {
            session_id: keys.session_id,
            cipher_c2s: keys.cipher_c2s,
            cipher_s2c: keys.cipher_s2c,
            rx_seq: keys.next_rx_seq,
            tx_seq: keys.next_tx_seq,
            authorized,
            authed: false,
            user: None,
            channel: None,
        }
    }

    /// Whether the client has authenticated.
    pub fn is_authenticated(&self) -> bool {
        self.authed
    }
    /// The authenticated user name, if any.
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Seal one outbound plaintext payload into a wire packet, advancing the
    /// send sequence number.
    fn send(&mut self, payload: &[u8]) -> Vec<u8> {
        let wire = self.cipher_s2c.seal(self.tx_seq, payload, 0);
        self.tx_seq = self.tx_seq.wrapping_add(1);
        wire
    }

    /// Feed one inbound WIRE packet (`enc_len || enc_payload || tag`). Decrypts
    /// it (advancing the receive sequence number), dispatches by message type,
    /// and returns the encrypted reply + a semantic event. A decryption/parse
    /// failure is a typed error (the caller drops the connection); it never
    /// panics.
    pub fn on_encrypted(&mut self, wire: &[u8]) -> Result<SessionStep, SshError> {
        let inner = self.cipher_c2s.open(self.rx_seq, wire)?;
        self.rx_seq = self.rx_seq.wrapping_add(1);
        let code = *inner.first().ok_or(SshError::Malformed)?;
        match code {
            SSH_MSG_SERVICE_REQUEST => self.on_service_request(&inner),
            SSH_MSG_USERAUTH_REQUEST => self.on_userauth(&inner),
            SSH_MSG_CHANNEL_OPEN => self.on_channel_open(&inner),
            SSH_MSG_CHANNEL_REQUEST => self.on_channel_request(&inner),
            SSH_MSG_CHANNEL_DATA => self.on_channel_data(&inner),
            SSH_MSG_CHANNEL_EOF => {
                let ch = crate::connection::parse_channel_id_msg(&inner, SSH_MSG_CHANNEL_EOF)?;
                Ok(SessionStep {
                    reply: Vec::new(),
                    event: SessionEvent::Eof { channel: ch },
                })
            }
            SSH_MSG_CHANNEL_CLOSE => {
                let ch = crate::connection::parse_channel_id_msg(&inner, SSH_MSG_CHANNEL_CLOSE)?;
                // Echo CLOSE (RFC 4254 §5.3: each side sends its own CLOSE).
                let reply = match &self.channel {
                    Some(c) => self.echo_close(c.remote_id),
                    None => Vec::new(),
                };
                self.channel = None;
                Ok(SessionStep {
                    reply,
                    event: SessionEvent::ChannelClosed { channel: ch },
                })
            }
            SSH_MSG_DISCONNECT => Ok(SessionStep {
                reply: Vec::new(),
                event: SessionEvent::Disconnect,
            }),
            _ => Ok(SessionStep {
                // Unknown/unhandled message: ignore rather than crash (a minimal
                // server; a fuller one would answer SSH_MSG_UNIMPLEMENTED).
                reply: Vec::new(),
                event: SessionEvent::None,
            }),
        }
    }

    fn echo_close(&mut self, remote_id: u32) -> Vec<u8> {
        let msg = build_channel_id_msg(SSH_MSG_CHANNEL_CLOSE, remote_id);
        self.send(&msg)
    }

    fn on_service_request(&mut self, inner: &[u8]) -> Result<SessionStep, SshError> {
        let (service, _) = read_string(inner, 1)?;
        if service == b"ssh-userauth" {
            let mut accept = Vec::with_capacity(20);
            accept.push(SSH_MSG_SERVICE_ACCEPT);
            write_string(&mut accept, b"ssh-userauth");
            let reply = self.send(&accept);
            Ok(SessionStep {
                reply,
                event: SessionEvent::None,
            })
        } else {
            // Only ssh-userauth is offered before auth.
            Ok(SessionStep {
                reply: Vec::new(),
                event: SessionEvent::Disconnect,
            })
        }
    }

    fn on_userauth(&mut self, inner: &[u8]) -> Result<SessionStep, SshError> {
        let req = parse_publickey_request(inner)?;
        match authenticate(&self.session_id, &req, &self.authorized) {
            AuthOutcome::PkOk => {
                let reply = self.send(&build_pk_ok(&req.pubkey));
                Ok(SessionStep {
                    reply,
                    event: SessionEvent::None,
                })
            }
            AuthOutcome::Success => {
                self.authed = true;
                self.user = Some(req.user.clone());
                let reply = self.send(&[SSH_MSG_USERAUTH_SUCCESS]);
                Ok(SessionStep {
                    reply,
                    event: SessionEvent::Authenticated { user: req.user },
                })
            }
            AuthOutcome::Failure => {
                // Name-list of methods that may continue + partial_success=false.
                let mut fail = Vec::with_capacity(24);
                fail.push(SSH_MSG_USERAUTH_FAILURE);
                write_name_list(&mut fail, &["publickey"]);
                fail.push(0);
                let reply = self.send(&fail);
                Ok(SessionStep {
                    reply,
                    event: SessionEvent::None,
                })
            }
        }
    }

    fn on_channel_open(&mut self, inner: &[u8]) -> Result<SessionStep, SshError> {
        if !self.authed {
            // No channels before authentication.
            return Ok(SessionStep {
                reply: Vec::new(),
                event: SessionEvent::Disconnect,
            });
        }
        let open = parse_channel_open(inner)?;
        if open.channel_type != "session" {
            let reply = self.send(&build_open_failure(
                open.sender_channel,
                OPEN_UNKNOWN_CHANNEL_TYPE,
                "only session channels are supported",
            ));
            return Ok(SessionStep {
                reply,
                event: SessionEvent::None,
            });
        }
        // Accept as our local channel 0 (one channel per connection).
        let ch = Channel::accept(&open, 0, LOCAL_WINDOW);
        let reply = self.send(&build_open_confirmation(
            ch.remote_id,
            ch.local_id,
            LOCAL_WINDOW,
            LOCAL_MAX_PACKET,
        ));
        self.channel = Some(ch);
        Ok(SessionStep {
            reply,
            event: SessionEvent::None,
        })
    }

    fn on_channel_request(&mut self, inner: &[u8]) -> Result<SessionStep, SshError> {
        let req = parse_channel_request(inner)?;
        let remote_id = match &self.channel {
            Some(c) => c.remote_id,
            None => {
                return Ok(SessionStep {
                    reply: Vec::new(),
                    event: SessionEvent::Disconnect,
                })
            }
        };
        let (accepted, event) = match req.kind {
            ChannelRequestKind::Shell => {
                (true, SessionEvent::ShellRequested { channel: remote_id })
            }
            ChannelRequestKind::Exec { command } => (
                true,
                SessionEvent::ExecRequested {
                    channel: remote_id,
                    command,
                },
            ),
            // A pty or env request is accepted but produces no standalone event
            // (it configures the shell that a later `shell` request starts).
            ChannelRequestKind::PtyReq { .. } | ChannelRequestKind::Env { .. } => {
                (true, SessionEvent::None)
            }
            ChannelRequestKind::Other(_) => (false, SessionEvent::None),
        };
        let reply = if req.want_reply {
            let code = if accepted {
                SSH_MSG_CHANNEL_SUCCESS
            } else {
                SSH_MSG_CHANNEL_FAILURE
            };
            self.send(&build_channel_id_msg(code, remote_id))
        } else {
            Vec::new()
        };
        Ok(SessionStep { reply, event })
    }

    fn on_channel_data(&mut self, inner: &[u8]) -> Result<SessionStep, SshError> {
        let (recipient, data) = parse_channel_data(inner)?;
        // Flow control: debit our receive window; top it back up if needed.
        let adjust = match &mut self.channel {
            Some(c) if c.local_id == recipient => c.on_receive(data.len() as u32)?,
            _ => return Err(SshError::Malformed), // data for an unknown channel
        };
        let remote_id = self.channel.as_ref().map(|c| c.remote_id).unwrap_or(0);
        let reply = match adjust {
            Some(add) => self.send(&build_window_adjust(remote_id, add)),
            None => Vec::new(),
        };
        Ok(SessionStep {
            reply,
            event: SessionEvent::Data {
                channel: remote_id,
                data,
            },
        })
    }

    /// Encrypt shell/command OUTPUT for the channel, honoring the peer's window
    /// and max-packet size (chunking as needed). Returns the concatenated wire
    /// packets (empty if there is no channel or the window is currently closed —
    /// in which case the caller retries after a `WINDOW_ADJUST`). Advances the
    /// send sequence number once per emitted packet.
    pub fn channel_output(&mut self, data: &[u8]) -> Vec<u8> {
        let (remote_id, mut offset) = match &self.channel {
            Some(_) => (self.channel.as_ref().unwrap().remote_id, 0usize),
            None => return Vec::new(),
        };
        let mut out = Vec::new();
        while offset < data.len() {
            let want = (data.len() - offset) as u32;
            let n = match &self.channel {
                Some(c) => c.sendable(want),
                None => 0,
            };
            if n == 0 {
                break; // window closed — buffer the rest for a later WINDOW_ADJUST
            }
            let end = offset + n as usize;
            let msg = build_channel_data(remote_id, &data[offset..end]);
            out.extend_from_slice(&self.send(&msg));
            if let Some(c) = &mut self.channel {
                c.consume_send(n);
            }
            offset = end;
        }
        out
    }

    /// The peer granted us more send window (`WINDOW_ADJUST`) — credit it.
    pub fn on_window_adjust_packet(&mut self, inner: &[u8]) -> Result<(), SshError> {
        let (_recipient, add) = parse_window_adjust(inner)?;
        if let Some(c) = &mut self.channel {
            c.on_window_adjust(add);
        }
        Ok(())
    }

    /// Encrypt an EOF + CLOSE to end the channel (host calls this when the shell
    /// exits).
    pub fn close_channel(&mut self) -> Vec<u8> {
        let remote_id = match &self.channel {
            Some(c) => c.remote_id,
            None => return Vec::new(),
        };
        let mut out = self.send(&build_channel_id_msg(SSH_MSG_CHANNEL_EOF, remote_id));
        out.extend_from_slice(&self.send(&build_channel_id_msg(SSH_MSG_CHANNEL_CLOSE, remote_id)));
        self.channel = None;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{build_channel_data as cdata, build_channel_id_msg as cid};
    use crate::userauth::{
        build_publickey_request, signed_data, PublickeyRequest, SERVICE_CONNECTION,
    };
    use crate::write_u32;
    use crate::{SSH_MSG_CHANNEL_OPEN_CONFIRMATION, SSH_MSG_CHANNEL_SUCCESS, SSH_MSG_KEXINIT};
    use rae_crypto::ed25519;

    // A client that mirrors the server's ciphers to drive the encrypted phase.
    struct Client {
        c2s: ChaChaPolyCipher, // client seals with this (server opens)
        s2c: ChaChaPolyCipher, // client opens with this (server seals)
        tx: u64,
        rx: u64,
    }
    impl Client {
        fn send(&mut self, payload: &[u8]) -> Vec<u8> {
            let w = self.c2s.seal(self.tx, payload, 0);
            self.tx += 1;
            w
        }
        fn open(&mut self, wire: &[u8]) -> Vec<u8> {
            let p = self.s2c.open(self.rx, wire).unwrap();
            self.rx += 1;
            p
        }
    }

    fn setup() -> (ServerSession, Client, [u8; 32], [u8; 32]) {
        let session_id = [0x42u8; 32];
        let km_c2s = [1u8; 64];
        let km_s2c = [2u8; 64];
        let seed = [7u8; 32];
        let pubkey = ed25519::derive_public_key(&seed);
        let keys = HandshakeKeys {
            session_id,
            cipher_c2s: ChaChaPolyCipher::from_key_material(&km_c2s),
            cipher_s2c: ChaChaPolyCipher::from_key_material(&km_s2c),
            next_tx_seq: 3,
            next_rx_seq: 3,
        };
        let srv = ServerSession::from_handshake(keys, alloc::vec![AuthorizedKey { pubkey }]);
        let client = Client {
            c2s: ChaChaPolyCipher::from_key_material(&km_c2s),
            s2c: ChaChaPolyCipher::from_key_material(&km_s2c),
            tx: 3,
            rx: 3,
        };
        (srv, client, seed, pubkey)
    }

    /// The whole encrypted phase, client-driven: service request → publickey auth
    /// (real signature) → open session → shell request → server shell output →
    /// client input. Proves ServerSession + userauth + connection + transport
    /// interoperate end to end over the sealed channel.
    #[test]
    fn full_encrypted_session_flow() {
        let (mut srv, mut client, seed, pubkey) = setup();

        // 1) SERVICE_REQUEST("ssh-userauth") -> SERVICE_ACCEPT.
        let mut sreq = alloc::vec![SSH_MSG_SERVICE_REQUEST];
        write_string(&mut sreq, b"ssh-userauth");
        let step = srv.on_encrypted(&client.send(&sreq)).unwrap();
        let accept = client.open(&step.reply);
        assert_eq!(accept[0], SSH_MSG_SERVICE_ACCEPT);

        // 2) USERAUTH_REQUEST publickey with a real signature -> SUCCESS.
        let base = parse_publickey_request(&build_publickey_request(
            "raeen",
            SERVICE_CONNECTION,
            &pubkey,
            None,
        ))
        .unwrap();
        let unsigned = PublickeyRequest {
            signature: None,
            ..base
        };
        let sig = ed25519::sign(&seed, &signed_data(&[0x42u8; 32], &unsigned));
        let auth = build_publickey_request("raeen", SERVICE_CONNECTION, &pubkey, Some(&sig));
        let step = srv.on_encrypted(&client.send(&auth)).unwrap();
        assert_eq!(
            step.event,
            SessionEvent::Authenticated {
                user: String::from("raeen")
            }
        );
        assert_eq!(client.open(&step.reply)[0], SSH_MSG_USERAUTH_SUCCESS);
        assert!(srv.is_authenticated());
        assert_eq!(srv.user(), Some("raeen"));

        // 3) CHANNEL_OPEN("session") -> OPEN_CONFIRMATION.
        let mut open = alloc::vec![SSH_MSG_CHANNEL_OPEN];
        write_string(&mut open, b"session");
        write_u32(&mut open, 55); // client's channel id
        write_u32(&mut open, 200_000); // initial window
        write_u32(&mut open, 32_768); // max packet
        let step = srv.on_encrypted(&client.send(&open)).unwrap();
        assert_eq!(
            client.open(&step.reply)[0],
            SSH_MSG_CHANNEL_OPEN_CONFIRMATION
        );

        // 4) CHANNEL_REQUEST("shell", want_reply) -> CHANNEL_SUCCESS + event.
        let mut shell = alloc::vec![SSH_MSG_CHANNEL_REQUEST];
        write_u32(&mut shell, 0); // recipient = our local channel 0
        write_string(&mut shell, b"shell");
        shell.push(1); // want_reply
        let step = srv.on_encrypted(&client.send(&shell)).unwrap();
        assert_eq!(step.event, SessionEvent::ShellRequested { channel: 55 });
        assert_eq!(client.open(&step.reply)[0], SSH_MSG_CHANNEL_SUCCESS);

        // 5) Server sends shell output; client receives it as CHANNEL_DATA.
        let wire = srv.channel_output(b"raeen@raeenos:~$ ");
        let data = client.open(&wire);
        let (_ch, payload) = parse_channel_data(&data).unwrap();
        assert_eq!(payload, b"raeen@raeenos:~$ ");

        // 6) Client types a command; server surfaces it as a Data event.
        let step = srv
            .on_encrypted(&client.send(&cdata(0, b"ls -la\n")))
            .unwrap();
        assert_eq!(
            step.event,
            SessionEvent::Data {
                channel: 55,
                data: b"ls -la\n".to_vec()
            }
        );

        // 7) Client closes the channel; server echoes CLOSE + event.
        let step = srv
            .on_encrypted(&client.send(&cid(SSH_MSG_CHANNEL_CLOSE, 0)))
            .unwrap();
        assert_eq!(step.event, SessionEvent::ChannelClosed { channel: 0 });
        assert_eq!(client.open(&step.reply)[0], SSH_MSG_CHANNEL_CLOSE);
    }

    #[test]
    fn unauthorized_key_fails_and_no_channel_before_auth() {
        let (mut srv, mut client, _seed, _pubkey) = setup();
        // An unknown key (not on the allow-list) -> USERAUTH_FAILURE, not authed.
        let (bad_seed, bad_pub) = ([9u8; 32], ed25519::derive_public_key(&[9u8; 32]));
        let base = parse_publickey_request(&build_publickey_request(
            "raeen",
            SERVICE_CONNECTION,
            &bad_pub,
            None,
        ))
        .unwrap();
        let sig = ed25519::sign(&bad_seed, &signed_data(&[0x42u8; 32], &base));
        let auth = build_publickey_request("raeen", SERVICE_CONNECTION, &bad_pub, Some(&sig));
        let step = srv.on_encrypted(&client.send(&auth)).unwrap();
        assert_eq!(client.open(&step.reply)[0], SSH_MSG_USERAUTH_FAILURE);
        assert!(!srv.is_authenticated());

        // A CHANNEL_OPEN before auth -> Disconnect.
        let mut open = alloc::vec![SSH_MSG_CHANNEL_OPEN];
        write_string(&mut open, b"session");
        write_u32(&mut open, 1);
        write_u32(&mut open, 1000);
        write_u32(&mut open, 1000);
        let step = srv.on_encrypted(&client.send(&open)).unwrap();
        assert_eq!(step.event, SessionEvent::Disconnect);
    }

    #[test]
    fn wrong_sequence_or_tamper_breaks_the_mac() {
        let (mut srv, client, _s, _p) = setup();
        // A packet sealed at the WRONG seqnr (replay/reorder) fails to open.
        let mut sreq = alloc::vec![SSH_MSG_SERVICE_REQUEST];
        write_string(&mut sreq, b"ssh-userauth");
        let wire = client.c2s.seal(99, &sreq, 0); // seqnr 99, server expects 3
        assert!(matches!(
            srv.on_encrypted(&wire),
            Err(SshError::BadMac) | Err(SshError::Malformed)
        ));
    }

    #[test]
    fn hostile_first_packet_never_panics() {
        let (mut srv, mut client, _s, _p) = setup();
        // A validly-sealed but garbage inner payload dispatches to the ignore arm.
        let step = srv
            .on_encrypted(&client.send(&[SSH_MSG_KEXINIT, 1, 2, 3]))
            .unwrap();
        assert_eq!(step.event, SessionEvent::None);
        assert!(step.reply.is_empty());
    }
}
