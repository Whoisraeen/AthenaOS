//! A self-contained loopback of the WHOLE server — handshake through an
//! authenticated shell channel — with a simulated client, as one allocation-only
//! function that returns `Ok`/`Err` instead of `assert!`ing. The host tests call
//! it (so it is KAT-proven), and the KERNEL calls it as its R10 boot smoketest
//! (so the exact same proven path runs in-kernel, exercising `rae_crypto` under
//! the kernel's build-std soft-float config). Pure logic, never panics.

use crate::kex::{derive_key, ed25519_hostkey_blob, exchange_hash, X25519_BASEPOINT};
use crate::kex::{SSH_MSG_KEX_ECDH_INIT, SSH_MSG_KEX_ECDH_REPLY};
use crate::kexinit::KexInit;
use crate::server::{ServerSession, SessionEvent};
use crate::session::ServerHandshake;
use crate::transport::ChaChaPolyCipher;
use crate::userauth::{
    build_publickey_request, parse_publickey_request, signed_data, AuthorizedKey, PublickeyRequest,
    SERVICE_CONNECTION,
};
use crate::{parse_packet, parse_peer_ident, read_string, write_string, write_u32};
use crate::{SSH_MSG_CHANNEL_OPEN, SSH_MSG_CHANNEL_REQUEST, SSH_MSG_SERVICE_REQUEST};
use crate::{
    SSH_MSG_CHANNEL_OPEN_CONFIRMATION, SSH_MSG_CHANNEL_SUCCESS, SSH_MSG_SERVICE_ACCEPT,
    SSH_MSG_USERAUTH_SUCCESS,
};
use alloc::vec;
use rae_crypto::ed25519;
use rae_crypto::x25519::x25519;

/// Drive a full server session against a simulated client and verify every stage
/// end to end. Returns `Ok(())` on success or `Err(reason)` at the first failed
/// invariant — so a regression turns into a loud, greppable FAIL rather than a
/// panic. Deterministic (fixed keys/secrets); no I/O.
pub fn loopback(host_seed: &[u8; 32], host_pub: &[u8; 32]) -> Result<(), &'static str> {
    // ── Handshake ────────────────────────────────────────────────────────────
    let server_eph = [3u8; 32];
    let mut hs = ServerHandshake::new(*host_seed, *host_pub, server_eph, [0u8; 16]);

    let banner = hs.banner();
    let (v_s, ident_consumed) = parse_peer_ident(&banner).map_err(|_| "banner ident unparsable")?;
    let (i_s, _) =
        parse_packet(&banner[ident_consumed..]).map_err(|_| "banner KEXINIT unframed")?;

    let v_c = b"SSH-2.0-RaeSSH-loopback".to_vec();
    hs.set_client_ident(&v_c);
    let i_c = KexInit::server_default().build_payload(&[1u8; 16]);
    if !hs
        .on_payload(&i_c)
        .map_err(|_| "client KEXINIT rejected")?
        .is_empty()
    {
        return Err("KEXINIT step produced unexpected output");
    }

    let client_secret = [7u8; 32];
    let q_c = x25519(&client_secret, &X25519_BASEPOINT);
    let mut ecdh_init = vec![SSH_MSG_KEX_ECDH_INIT];
    write_string(&mut ecdh_init, &q_c);
    let reply_bytes = hs
        .on_payload(&ecdh_init)
        .map_err(|_| "ECDH_INIT rejected")?;

    let (reply, _) = parse_packet(&reply_bytes).map_err(|_| "ECDH_REPLY unframed")?;
    if reply.first() != Some(&SSH_MSG_KEX_ECDH_REPLY) {
        return Err("expected ECDH_REPLY");
    }
    // Client parses REPLY: K_S | Q_S | signature.
    let (k_s, p) = read_string(&reply, 1).map_err(|_| "REPLY K_S")?;
    let (q_s_bytes, p) = read_string(&reply, p).map_err(|_| "REPLY Q_S")?;
    let (sig_blob, _) = read_string(&reply, p).map_err(|_| "REPLY sig")?;
    if k_s != ed25519_hostkey_blob(host_pub) {
        return Err("host key blob mismatch");
    }
    let q_s: [u8; 32] = q_s_bytes.try_into().map_err(|_| "Q_S not 32 bytes")?;
    let (_algo, sp) = read_string(sig_blob, 0).map_err(|_| "sig algo")?;
    let (raw_sig, _) = read_string(sig_blob, sp).map_err(|_| "sig bytes")?;
    let sig: [u8; 64] = raw_sig.try_into().map_err(|_| "sig not 64 bytes")?;

    // Client independently computes the shared secret + exchange hash.
    let k = x25519(&client_secret, &q_s);
    let h = exchange_hash(&v_c, &v_s, &i_c, &i_s, k_s, &q_c, &q_s, &k);
    if !ed25519::verify(host_pub, &h, &sig) {
        return Err("host signature over H did not verify");
    }
    // Client NEWKEYS -> established.
    if !hs
        .on_payload(&[crate::SSH_MSG_NEWKEYS])
        .map_err(|_| "client NEWKEYS rejected")?
        .is_empty()
    {
        return Err("NEWKEYS step produced output");
    }
    if !hs.is_established() {
        return Err("handshake did not reach Established");
    }
    let keys = hs.take_keys().ok_or("no session keys after handshake")?;
    if keys.session_id != h {
        return Err("server/client session id (H) disagree");
    }

    // Client's mirror ciphers, independently derived from K and H.
    let derive = |letter: u8| -> ChaChaPolyCipher {
        let km = derive_key(&k, &h, letter, &h, 64);
        let mut a = [0u8; 64];
        a.copy_from_slice(&km);
        ChaChaPolyCipher::from_key_material(&a)
    };
    let client_c2s = derive(b'C');
    let client_s2c = derive(b'D');
    // Cross-check both directions round-trip through the sealed transport.
    let probe = keys.cipher_s2c.seal(0, b"raessh-loopback-probe", 0);
    if client_s2c.open(0, &probe).ok().as_deref() != Some(&b"raessh-loopback-probe"[..]) {
        return Err("s2c sealed traffic did not open on the client");
    }

    // ── Encrypted phase: auth + a shell channel ─────────────────────────────
    let auth_seed = [7u8; 32];
    let auth_pub = ed25519::derive_public_key(&auth_seed);
    let mut srv = ServerSession::from_handshake(keys, vec![AuthorizedKey { pubkey: auth_pub }]);

    // Client-side cipher state starting at the post-handshake sequence numbers.
    let mut ctx = 3u64; // client c2s send seq
    let mut crx = 3u64; // client s2c recv seq

    // 1) SERVICE_REQUEST("ssh-userauth") -> SERVICE_ACCEPT.
    let mut sreq = vec![SSH_MSG_SERVICE_REQUEST];
    write_string(&mut sreq, b"ssh-userauth");
    let step = srv
        .on_encrypted(&client_c2s.seal(ctx, &sreq, 0))
        .map_err(|_| "service request decrypt failed")?;
    ctx += 1;
    let accept = client_s2c
        .open(crx, &step.reply)
        .map_err(|_| "service accept open")?;
    crx += 1;
    if accept.first() != Some(&SSH_MSG_SERVICE_ACCEPT) {
        return Err("expected SERVICE_ACCEPT");
    }

    // 2) publickey auth with a real signature -> SUCCESS + Authenticated.
    let base = parse_publickey_request(&build_publickey_request(
        "raeen",
        SERVICE_CONNECTION,
        &auth_pub,
        None,
    ))
    .map_err(|_| "build auth request")?;
    let unsigned = PublickeyRequest {
        signature: None,
        ..base
    };
    let asig = ed25519::sign(&auth_seed, &signed_data(&h, &unsigned));
    let areq = build_publickey_request("raeen", SERVICE_CONNECTION, &auth_pub, Some(&asig));
    let step = srv
        .on_encrypted(&client_c2s.seal(ctx, &areq, 0))
        .map_err(|_| "userauth decrypt failed")?;
    ctx += 1;
    if step.event
        != (SessionEvent::Authenticated {
            user: alloc::string::String::from("raeen"),
        })
    {
        return Err("auth did not produce Authenticated event");
    }
    let ok = client_s2c
        .open(crx, &step.reply)
        .map_err(|_| "userauth reply open")?;
    crx += 1;
    if ok.first() != Some(&SSH_MSG_USERAUTH_SUCCESS) || !srv.is_authenticated() {
        return Err("expected USERAUTH_SUCCESS");
    }

    // 3) CHANNEL_OPEN("session") -> confirmation.
    let mut open = vec![SSH_MSG_CHANNEL_OPEN];
    write_string(&mut open, b"session");
    write_u32(&mut open, 55);
    write_u32(&mut open, 200_000);
    write_u32(&mut open, 32_768);
    let step = srv
        .on_encrypted(&client_c2s.seal(ctx, &open, 0))
        .map_err(|_| "channel open decrypt failed")?;
    ctx += 1;
    let conf = client_s2c
        .open(crx, &step.reply)
        .map_err(|_| "open confirm open")?;
    crx += 1;
    if conf.first() != Some(&SSH_MSG_CHANNEL_OPEN_CONFIRMATION) {
        return Err("expected CHANNEL_OPEN_CONFIRMATION");
    }

    // 4) CHANNEL_REQUEST("shell") -> success + ShellRequested event.
    let mut shell = vec![SSH_MSG_CHANNEL_REQUEST];
    write_u32(&mut shell, 0);
    write_string(&mut shell, b"shell");
    shell.push(1);
    let step = srv
        .on_encrypted(&client_c2s.seal(ctx, &shell, 0))
        .map_err(|_| "shell request decrypt failed")?;
    if step.event != (SessionEvent::ShellRequested { channel: 55 }) {
        return Err("shell request did not produce ShellRequested");
    }
    let succ = client_s2c
        .open(crx, &step.reply)
        .map_err(|_| "channel success open")?;
    if succ.first() != Some(&SSH_MSG_CHANNEL_SUCCESS) {
        return Err("expected CHANNEL_SUCCESS");
    }

    // 5) Server sends shell output; the client decrypts it.
    let wire = srv.channel_output(b"raeen@athenaos:~$ ");
    let out = client_s2c
        .open(crx + 1, &wire)
        .map_err(|_| "shell output open")?;
    let (_ch, data) =
        crate::connection::parse_channel_data(&out).map_err(|_| "shell data parse")?;
    if data != b"raeen@athenaos:~$ " {
        return Err("shell output payload mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_full_session_passes() {
        let host_seed = [9u8; 32];
        let host_pub = ed25519::derive_public_key(&host_seed);
        loopback(&host_seed, &host_pub).expect("loopback self-test must pass");
    }
}
