//! SSH_MSG_KEXINIT (RFC 4253 §7.1): each side sends the algorithms it supports;
//! the negotiated set is, per §7.1, the CLIENT's first choice that the SERVER
//! also supports (checked independently for KEX, host key, and each direction's
//! cipher / MAC / compression). Pure logic — parse a peer's KEXINIT, build ours,
//! and negotiate — provable against real OpenSSH bytes without a socket.

use crate::{read_name_list, write_name_list, SshError, SSH_MSG_KEXINIT};
use alloc::string::String;
use alloc::vec::Vec;

/// The algorithms this server OFFERS, in preference order. All are backed by
/// `rae_crypto`: curve25519 KEX (`x25519` + `sha256`), ssh-ed25519 host key,
/// chacha20-poly1305 AEAD packet cipher.
pub const KEX_ALGS: &[&str] = &["curve25519-sha256", "curve25519-sha256@libssh.org"];
pub const HOST_KEY_ALGS: &[&str] = &["ssh-ed25519"];
pub const CIPHER_ALGS: &[&str] = &["chacha20-poly1305@openssh.com"];
pub const MAC_ALGS: &[&str] = &["hmac-sha2-256"];
pub const COMP_ALGS: &[&str] = &["none"];

/// A parsed KEXINIT — the ten name-lists plus the `first_kex_packet_follows`
/// guess flag (RFC 4253 §7.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KexInit {
    pub kex: Vec<String>,
    pub host_key: Vec<String>,
    pub cipher_c2s: Vec<String>,
    pub cipher_s2c: Vec<String>,
    pub mac_c2s: Vec<String>,
    pub mac_s2c: Vec<String>,
    pub comp_c2s: Vec<String>,
    pub comp_s2c: Vec<String>,
    pub lang_c2s: Vec<String>,
    pub lang_s2c: Vec<String>,
    pub first_kex_packet_follows: bool,
}

/// The result of negotiation — one chosen algorithm per role/direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    pub kex: String,
    pub host_key: String,
    pub cipher_c2s: String,
    pub cipher_s2c: String,
    pub mac_c2s: String,
    pub mac_s2c: String,
    pub comp_c2s: String,
    pub comp_s2c: String,
}

impl KexInit {
    /// This server's KEXINIT.
    pub fn server_default() -> Self {
        let to_vec = |a: &[&str]| a.iter().map(|s| String::from(*s)).collect();
        KexInit {
            kex: to_vec(KEX_ALGS),
            host_key: to_vec(HOST_KEY_ALGS),
            cipher_c2s: to_vec(CIPHER_ALGS),
            cipher_s2c: to_vec(CIPHER_ALGS),
            mac_c2s: to_vec(MAC_ALGS),
            mac_s2c: to_vec(MAC_ALGS),
            comp_c2s: to_vec(COMP_ALGS),
            comp_s2c: to_vec(COMP_ALGS),
            lang_c2s: Vec::new(),
            lang_s2c: Vec::new(),
            first_kex_packet_follows: false,
        }
    }

    /// Serialize as a KEXINIT payload (message code 20 + 16-byte cookie + the ten
    /// name-lists + guess flag + reserved uint32). `cookie` is 16 random bytes
    /// on the wire (tests pass a fixed value; production passes CSPRNG output).
    pub fn build_payload(&self, cookie: &[u8; 16]) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.push(SSH_MSG_KEXINIT);
        out.extend_from_slice(cookie);
        for list in [
            &self.kex,
            &self.host_key,
            &self.cipher_c2s,
            &self.cipher_s2c,
            &self.mac_c2s,
            &self.mac_s2c,
            &self.comp_c2s,
            &self.comp_s2c,
            &self.lang_c2s,
            &self.lang_s2c,
        ] {
            let refs: Vec<&str> = list.iter().map(String::as_str).collect();
            write_name_list(&mut out, &refs);
        }
        out.push(self.first_kex_packet_follows as u8);
        out.extend_from_slice(&0u32.to_be_bytes()); // reserved
        out
    }

    /// Parse a KEXINIT payload (the decrypted packet payload, starting with the
    /// message code). Rejects anything structurally invalid without panicking.
    pub fn parse_payload(payload: &[u8]) -> Result<KexInit, SshError> {
        if payload.first() != Some(&SSH_MSG_KEXINIT) {
            return Err(SshError::Unexpected);
        }
        // 1 (msg) + 16 (cookie) = 17 before the first name-list.
        let mut pos = 17;
        if payload.len() < pos {
            return Err(SshError::Malformed);
        }
        let mut lists: [Vec<String>; 10] = Default::default();
        for slot in lists.iter_mut() {
            let (names, next) = read_name_list(payload, pos)?;
            *slot = names;
            pos = next;
        }
        // first_kex_packet_follows (1) + reserved (4).
        if payload.len() < pos + 5 {
            return Err(SshError::Malformed);
        }
        let first_kex_packet_follows = payload[pos] != 0;
        let [kex, host_key, cipher_c2s, cipher_s2c, mac_c2s, mac_s2c, comp_c2s, comp_s2c, lang_c2s, lang_s2c] =
            lists;
        Ok(KexInit {
            kex,
            host_key,
            cipher_c2s,
            cipher_s2c,
            mac_c2s,
            mac_s2c,
            comp_c2s,
            comp_s2c,
            lang_c2s,
            lang_s2c,
            first_kex_packet_follows,
        })
    }
}

/// RFC 4253 §7.1: the chosen algorithm is the CLIENT's first preference that the
/// SERVER also supports. Returns `NoMatch` if the lists are disjoint.
fn first_match(client: &[String], server: &[String]) -> Result<String, SshError> {
    for c in client {
        if server.iter().any(|s| s == c) {
            return Ok(c.clone());
        }
    }
    Err(SshError::NoMatch)
}

/// Negotiate the full algorithm set from the client's and server's KEXINIT
/// (this side being the server). Each of KEX, host key, and each direction's
/// cipher/MAC/compression is resolved by client preference (§7.1).
pub fn negotiate(client: &KexInit, server: &KexInit) -> Result<Negotiated, SshError> {
    Ok(Negotiated {
        kex: first_match(&client.kex, &server.kex)?,
        host_key: first_match(&client.host_key, &server.host_key)?,
        cipher_c2s: first_match(&client.cipher_c2s, &server.cipher_c2s)?,
        cipher_s2c: first_match(&client.cipher_s2c, &server.cipher_s2c)?,
        mac_c2s: first_match(&client.mac_c2s, &server.mac_c2s)?,
        mac_s2c: first_match(&client.mac_s2c, &server.mac_s2c)?,
        comp_c2s: first_match(&client.comp_c2s, &server.comp_c2s)?,
        comp_s2c: first_match(&client.comp_s2c, &server.comp_s2c)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A KEXINIT shaped like OpenSSH 9.x's client offer (sntrup/curve25519 kex,
    /// ed25519 + rsa host keys, chacha/aes-gcm ciphers) so negotiation is tested
    /// against a realistic peer, not a toy.
    fn openssh_like_client() -> KexInit {
        let v = |a: &[&str]| a.iter().map(|s| String::from(*s)).collect::<Vec<_>>();
        KexInit {
            kex: v(&[
                "sntrup761x25519-sha512@openssh.com",
                "curve25519-sha256",
                "curve25519-sha256@libssh.org",
                "ecdh-sha2-nistp256",
            ]),
            host_key: v(&["ssh-ed25519", "rsa-sha2-512", "rsa-sha2-256"]),
            cipher_c2s: v(&["chacha20-poly1305@openssh.com", "aes256-gcm@openssh.com"]),
            cipher_s2c: v(&["chacha20-poly1305@openssh.com", "aes256-gcm@openssh.com"]),
            mac_c2s: v(&["umac-64-etm@openssh.com", "hmac-sha2-256"]),
            mac_s2c: v(&["umac-64-etm@openssh.com", "hmac-sha2-256"]),
            comp_c2s: v(&["none", "zlib@openssh.com"]),
            comp_s2c: v(&["none", "zlib@openssh.com"]),
            lang_c2s: Vec::new(),
            lang_s2c: Vec::new(),
            first_kex_packet_follows: false,
        }
    }

    #[test]
    fn kexinit_payload_round_trips() {
        let k = KexInit::server_default();
        let cookie = [7u8; 16];
        let payload = k.build_payload(&cookie);
        assert_eq!(payload[0], SSH_MSG_KEXINIT);
        assert_eq!(&payload[1..17], &cookie);
        let parsed = KexInit::parse_payload(&payload).unwrap();
        assert_eq!(parsed, k);
    }

    #[test]
    fn negotiates_our_stack_against_openssh() {
        let client = openssh_like_client();
        let server = KexInit::server_default();
        let n = negotiate(&client, &server).unwrap();
        // Client prefers sntrup (we don't have it) then curve25519-sha256 — which
        // we DO offer, so that wins. Host key ssh-ed25519, cipher chacha20.
        assert_eq!(n.kex, "curve25519-sha256");
        assert_eq!(n.host_key, "ssh-ed25519");
        assert_eq!(n.cipher_c2s, "chacha20-poly1305@openssh.com");
        assert_eq!(n.cipher_s2c, "chacha20-poly1305@openssh.com");
        assert_eq!(n.mac_c2s, "hmac-sha2-256");
        assert_eq!(n.comp_c2s, "none");
    }

    #[test]
    fn client_preference_wins_not_server() {
        // Server lists A then B; client lists B then A -> client's B wins (§7.1).
        let client = KexInit {
            kex: vec![
                String::from("curve25519-sha256@libssh.org"),
                String::from("curve25519-sha256"),
            ],
            host_key: vec![String::from("ssh-ed25519")],
            cipher_c2s: vec![String::from("chacha20-poly1305@openssh.com")],
            cipher_s2c: vec![String::from("chacha20-poly1305@openssh.com")],
            mac_c2s: vec![String::from("hmac-sha2-256")],
            mac_s2c: vec![String::from("hmac-sha2-256")],
            comp_c2s: vec![String::from("none")],
            comp_s2c: vec![String::from("none")],
            ..Default::default()
        };
        let n = negotiate(&client, &KexInit::server_default()).unwrap();
        assert_eq!(n.kex, "curve25519-sha256@libssh.org"); // client's first choice
    }

    #[test]
    fn disjoint_algorithms_fail_closed() {
        let mut client = openssh_like_client();
        client.kex = vec![String::from("diffie-hellman-group1-sha1")]; // we don't offer it
        assert_eq!(
            negotiate(&client, &KexInit::server_default()),
            Err(SshError::NoMatch)
        );
    }

    #[test]
    fn parse_rejects_wrong_message_code() {
        let mut payload = KexInit::server_default().build_payload(&[0u8; 16]);
        payload[0] = crate::SSH_MSG_NEWKEYS; // not KEXINIT
        assert_eq!(KexInit::parse_payload(&payload), Err(SshError::Unexpected));
    }

    #[test]
    fn parse_rejects_truncated_payload() {
        let payload = KexInit::server_default().build_payload(&[0u8; 16]);
        // Cut inside the name-lists.
        assert!(matches!(
            KexInit::parse_payload(&payload[..20]),
            Err(SshError::NeedMoreData) | Err(SshError::Malformed)
        ));
    }
}
