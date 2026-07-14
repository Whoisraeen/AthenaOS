//! The sans-io server handshake driver — the piece that COMPOSES the protocol
//! slices into an actual SSH server. Given the ed25519 host key and a per-
//! connection ephemeral secret it walks the unencrypted prologue (version
//! exchange → KEXINIT negotiation → curve25519 KEX → NEWKEYS), captures the
//! transcript that feeds the exchange hash, and emits the two directional
//! [`ChaChaPolyCipher`]s + the session identifier the encrypted phase needs.
//!
//! "Sans-io" means it touches no socket: the caller feeds it inbound message
//! payloads (already de-framed by [`crate::parse_packet`]) and transmits the
//! framed bytes it returns. That makes the whole handshake — the trickiest,
//! most interop-sensitive part of an SSH server — host-KAT-provable by
//! simulating a client, before any smoltcp/TCP wiring exists.

use crate::kex::{parse_ecdh_init, server_ecdh, X25519_BASEPOINT};
use crate::kexinit::{negotiate, KexInit};
use crate::transport::ChaChaPolyCipher;
use crate::{build_packet, ident_line, SshError};
use crate::{SSH_MSG_KEXINIT, SSH_MSG_NEWKEYS};
use alloc::vec::Vec;

/// The negotiated session's directional ciphers + identifier, produced once the
/// handshake completes.
pub struct HandshakeKeys {
    /// The exchange hash `H` of the first KEX — also the SSH session identifier.
    pub session_id: [u8; 32],
    /// Cipher for client→server packets (derived with letter `C`).
    pub cipher_c2s: ChaChaPolyCipher,
    /// Cipher for server→client packets (derived with letter `D`).
    pub cipher_s2c: ChaChaPolyCipher,
    /// The packet sequence number of the FIRST encrypted packet we will SEND.
    /// The SSH sequence number (the cipher nonce) counts every binary packet
    /// since the start, INCLUDING the unencrypted handshake packets — so the
    /// encrypted phase does not start at 0.
    pub next_tx_seq: u64,
    /// The packet sequence number of the first encrypted packet we will RECEIVE.
    pub next_rx_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Awaiting the client's `SSH_MSG_KEXINIT`.
    NeedClientKexInit,
    /// KEXINIT negotiated; awaiting `SSH_MSG_KEX_ECDH_INIT`.
    NeedEcdhInit,
    /// Sent REPLY + our NEWKEYS; awaiting the client's `SSH_MSG_NEWKEYS`.
    NeedClientNewKeys,
    /// Handshake complete — encrypted transport is live.
    Established,
}

/// The server side of the SSH handshake, as a step function.
pub struct ServerHandshake {
    host_seed: [u8; 32],
    host_pub: [u8; 32],
    eph_secret: [u8; 32],
    /// `V_S` — our identification string, no CR-LF (feeds the exchange hash).
    v_s: Vec<u8>,
    /// `V_C` — the client's identification string (set via [`Self::set_client_ident`]).
    v_c: Vec<u8>,
    /// `I_S` — our KEXINIT payload.
    i_s: Vec<u8>,
    /// `I_C` — the client's KEXINIT payload.
    i_c: Vec<u8>,
    phase: Phase,
    keys: Option<HandshakeKeys>,
    /// Count of binary packets sent / received so far (the SSH sequence numbers).
    tx_seq: u64,
    rx_seq: u64,
}

impl ServerHandshake {
    /// Create a handshake for one connection. `host_seed`/`host_pub` are the
    /// server's long-term ed25519 host key; `eph_secret` is a fresh 32-byte
    /// random scalar (per connection); `cookie` is 16 random bytes for our
    /// KEXINIT. (Tests pass fixed values for determinism; production passes
    /// CSPRNG output.)
    pub fn new(
        host_seed: [u8; 32],
        host_pub: [u8; 32],
        eph_secret: [u8; 32],
        cookie: [u8; 16],
    ) -> Self {
        let i_s = KexInit::server_default().build_payload(&cookie);
        Self {
            host_seed,
            host_pub,
            eph_secret,
            v_s: crate::IDENT.to_vec(),
            v_c: Vec::new(),
            i_s,
            i_c: Vec::new(),
            phase: Phase::NeedClientKexInit,
            keys: None,
            tx_seq: 0,
            rx_seq: 0,
        }
    }

    /// The bytes to send first: our identification line (`SSH-2.0-…\r\n`)
    /// followed by our framed KEXINIT packet. Call EXACTLY once (it counts the
    /// KEXINIT toward the outbound sequence number).
    pub fn banner(&mut self) -> Vec<u8> {
        let mut out = ident_line();
        out.extend_from_slice(&build_packet(&self.i_s, 0));
        self.tx_seq += 1; // the KEXINIT packet (seq 0)
        out
    }

    /// Record the client's identification string (`V_C`, WITHOUT CR-LF — as
    /// returned by [`crate::parse_peer_ident`]). Must be called before the KEX
    /// completes, since it feeds the exchange hash.
    pub fn set_client_ident(&mut self, v_c: &[u8]) {
        self.v_c = v_c.to_vec();
    }

    /// True once the encrypted transport is live.
    pub fn is_established(&self) -> bool {
        self.phase == Phase::Established
    }

    /// Take the derived keys after [`Self::is_established`] (consumes them).
    pub fn take_keys(&mut self) -> Option<HandshakeKeys> {
        self.keys.take()
    }

    /// Feed one inbound handshake message payload (de-framed). Returns the framed
    /// bytes to transmit in response (possibly empty). Advances the state
    /// machine; on the ECDH step it derives the session keys. Any structurally
    /// bad or out-of-order message is a typed error, never a panic.
    pub fn on_payload(&mut self, payload: &[u8]) -> Result<Vec<u8>, SshError> {
        match self.phase {
            Phase::NeedClientKexInit => {
                if payload.first() != Some(&SSH_MSG_KEXINIT) {
                    return Err(SshError::Unexpected);
                }
                // Negotiate against our offer; a disjoint client fails closed.
                let client = KexInit::parse_payload(payload)?;
                let _ = negotiate(&client, &KexInit::server_default())?;
                self.i_c = payload.to_vec();
                self.rx_seq += 1; // client KEXINIT (seq 0)
                self.phase = Phase::NeedEcdhInit;
                Ok(Vec::new())
            }
            Phase::NeedEcdhInit => {
                if self.v_c.is_empty() {
                    // The client ident must have been recorded first — without
                    // V_C the exchange hash would be wrong. Refuse rather than
                    // silently hash an empty V_C.
                    return Err(SshError::Unexpected);
                }
                let q_c = parse_ecdh_init(payload)?;
                let res = server_ecdh(
                    &self.v_c,
                    &self.v_s,
                    &self.i_c,
                    &self.i_s,
                    &self.host_seed,
                    &self.host_pub,
                    &self.eph_secret,
                    &q_c,
                );
                // RFC 4253 §7.2 key material; chacha20-poly1305@openssh needs 64
                // bytes per direction (no separate IV/MAC keys). Session id == H
                // on the first (only) KEX.
                let c2s = crate::kex::derive_key(&res.k_unsigned, &res.h, b'C', &res.h, 64);
                let d2c = crate::kex::derive_key(&res.k_unsigned, &res.h, b'D', &res.h, 64);
                let mut km_c = [0u8; 64];
                let mut km_d = [0u8; 64];
                km_c.copy_from_slice(&c2s);
                km_d.copy_from_slice(&d2c);
                self.keys = Some(HandshakeKeys {
                    session_id: res.h,
                    cipher_c2s: ChaChaPolyCipher::from_key_material(&km_c),
                    cipher_s2c: ChaChaPolyCipher::from_key_material(&km_d),
                    next_tx_seq: 0, // finalized at the Established transition
                    next_rx_seq: 0,
                });
                self.rx_seq += 1; // client ECDH_INIT (seq 1)
                self.phase = Phase::NeedClientNewKeys;
                // Send: ECDH_REPLY then our NEWKEYS (both still unencrypted).
                let mut out = build_packet(&res.reply_payload, 0);
                out.extend_from_slice(&build_packet(&[SSH_MSG_NEWKEYS], 0));
                self.tx_seq += 2; // ECDH_REPLY (seq 1) + NEWKEYS (seq 2)
                Ok(out)
            }
            Phase::NeedClientNewKeys => {
                if payload.first() != Some(&SSH_MSG_NEWKEYS) {
                    return Err(SshError::Unexpected);
                }
                self.rx_seq += 1; // client NEWKEYS (seq 2)
                                  // The encrypted phase begins at the current counts (3 / 3 for a
                                  // standard KEXINIT+ECDH+NEWKEYS exchange).
                if let Some(k) = &mut self.keys {
                    k.next_tx_seq = self.tx_seq;
                    k.next_rx_seq = self.rx_seq;
                }
                self.phase = Phase::Established;
                Ok(Vec::new())
            }
            Phase::Established => Err(SshError::Unexpected),
        }
    }

    /// The Curve25519 public for a secret (helper for callers/tests).
    pub fn public_of(secret: &[u8; 32]) -> [u8; 32] {
        rae_crypto::x25519::x25519(secret, &X25519_BASEPOINT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kex::{ed25519_hostkey_blob, exchange_hash};
    use crate::kex::{SSH_MSG_KEX_ECDH_INIT, SSH_MSG_KEX_ECDH_REPLY};
    use crate::{parse_packet, parse_peer_ident, read_string, write_string};
    use rae_crypto::{ed25519, x25519::x25519};

    /// Drive the server through a COMPLETE handshake from a simulated client and
    /// prove: (1) the client independently computes the SAME session id `H`,
    /// (2) the host-key signature over `H` verifies, (3) a message the server
    /// seals with cipher_s2c opens under the client's independently-derived
    /// cipher_s2c (and the reverse for c2s). This exercises Slices 1/2/3 wired
    /// together — the actual server handshake — end to end.
    #[test]
    fn full_server_handshake_yields_interoperable_keys() {
        // Server config.
        let host_seed = [9u8; 32];
        let host_pub = ed25519::derive_public_key(&host_seed);
        let server_eph = [3u8; 32];
        let mut hs = ServerHandshake::new(host_seed, host_pub, server_eph, [0u8; 16]);

        // Server's first bytes: ident line + framed KEXINIT.
        let banner = hs.banner();
        let (v_s, ident_consumed) = parse_peer_ident(&banner).unwrap();
        assert_eq!(v_s.as_slice(), crate::IDENT);
        // The rest of the banner (after the CR-LF ident) is the framed KEXINIT.
        let after_ident = &banner[ident_consumed..];
        let (i_s, _) = parse_packet(after_ident).unwrap();
        assert_eq!(i_s.first(), Some(&SSH_MSG_KEXINIT));

        // Client → server: ident, then KEXINIT.
        let v_c = b"SSH-2.0-TestClient".to_vec();
        hs.set_client_ident(&v_c);
        let i_c = KexInit::server_default().build_payload(&[1u8; 16]);
        assert!(hs.on_payload(&i_c).unwrap().is_empty());

        // Client → server: ECDH_INIT with its ephemeral public.
        let client_secret = [7u8; 32];
        let q_c = x25519(&client_secret, &X25519_BASEPOINT);
        let mut ecdh_init = alloc::vec![SSH_MSG_KEX_ECDH_INIT];
        write_string(&mut ecdh_init, &q_c);
        let reply_bytes = hs.on_payload(&ecdh_init).unwrap();

        // Server → client: ECDH_REPLY then NEWKEYS (two framed packets).
        let (reply, consumed) = parse_packet(&reply_bytes).unwrap();
        assert_eq!(reply.first(), Some(&SSH_MSG_KEX_ECDH_REPLY));
        let (newkeys, _) = parse_packet(&reply_bytes[consumed..]).unwrap();
        assert_eq!(newkeys.first(), Some(&SSH_MSG_NEWKEYS));

        // Client parses the REPLY: K_S | Q_S | signature.
        let (k_s, p) = read_string(&reply, 1).unwrap();
        let (q_s_bytes, p) = read_string(&reply, p).unwrap();
        let (sig_blob, _) = read_string(&reply, p).unwrap();
        let q_s: [u8; 32] = q_s_bytes.try_into().unwrap();
        // Host-key blob and sig blob unwrap.
        assert_eq!(k_s, ed25519_hostkey_blob(&host_pub));
        let (_algo, sp) = read_string(sig_blob, 0).unwrap();
        let (raw_sig, _) = read_string(sig_blob, sp).unwrap();
        let sig: [u8; 64] = raw_sig.try_into().unwrap();

        // Client computes the shared secret + the SAME exchange hash H.
        let k = x25519(&client_secret, &q_s);
        let h = exchange_hash(&v_c, &v_s, &i_c, &i_s, k_s, &q_c, &q_s, &k);
        // (2) The host signature over H verifies.
        assert!(ed25519::verify(&host_pub, &h, &sig));

        // Client → server: NEWKEYS. Handshake established.
        assert!(hs.on_payload(&[SSH_MSG_NEWKEYS]).unwrap().is_empty());
        assert!(hs.is_established());
        let keys = hs.take_keys().unwrap();

        // (1) Same session id.
        assert_eq!(keys.session_id, h);

        // (3) Interoperable ciphers: derive the client's view of both directions
        // and cross-decrypt.
        let derive = |letter| {
            let km = crate::kex::derive_key(&k, &h, letter, &h, 64);
            let mut a = [0u8; 64];
            a.copy_from_slice(&km);
            ChaChaPolyCipher::from_key_material(&a)
        };
        let client_c2s = derive(b'C');
        let client_s2c = derive(b'D');

        // Server sends on s2c; client opens with its s2c.
        let wire = keys.cipher_s2c.seal(0, b"hello from RaeSSH", 0);
        assert_eq!(client_s2c.open(0, &wire).unwrap(), b"hello from RaeSSH");
        // Client sends on c2s; server opens with its c2s.
        let wire2 = client_c2s.seal(0, b"ls -la", 0);
        assert_eq!(keys.cipher_c2s.open(0, &wire2).unwrap(), b"ls -la");
    }

    #[test]
    fn out_of_order_and_hostile_messages_fail_closed() {
        let host_seed = [1u8; 32];
        let host_pub = ed25519::derive_public_key(&host_seed);
        let mut hs = ServerHandshake::new(host_seed, host_pub, [2u8; 32], [0u8; 16]);
        // An ECDH_INIT before KEXINIT is refused.
        let mut ecdh = alloc::vec![SSH_MSG_KEX_ECDH_INIT];
        write_string(&mut ecdh, &[0u8; 32]);
        assert_eq!(hs.on_payload(&ecdh), Err(SshError::Unexpected));
        // A non-KEXINIT first message is refused.
        assert_eq!(hs.on_payload(&[SSH_MSG_NEWKEYS]), Err(SshError::Unexpected));
        // Truncated KEXINIT is a typed error, not a panic.
        assert!(hs.on_payload(&[SSH_MSG_KEXINIT, 0, 0]).is_err());
    }

    #[test]
    fn ecdh_without_client_ident_is_refused() {
        // Skipping set_client_ident would poison the exchange hash — refuse it.
        let host_seed = [4u8; 32];
        let host_pub = ed25519::derive_public_key(&host_seed);
        let mut hs = ServerHandshake::new(host_seed, host_pub, [5u8; 32], [0u8; 16]);
        hs.on_payload(&KexInit::server_default().build_payload(&[0u8; 16]))
            .unwrap();
        let mut ecdh = alloc::vec![SSH_MSG_KEX_ECDH_INIT];
        write_string(&mut ecdh, &ServerHandshake::public_of(&[7u8; 32]));
        assert_eq!(hs.on_payload(&ecdh), Err(SshError::Unexpected));
    }
}
