//! DNS over TLS (Concept §AthNet: private by default — name lookups leave
//! the machine encrypted, never as cleartext UDP-53 an ISP or coffee-shop
//! AP can read or rewrite). MasterChecklist Phase 10.2 — "DNS over TLS or
//! DNS over HTTPS".
//!
//! RFC 7858: each DNS message travels over a TLS 1.3 stream prefixed by a
//! two-octet big-endian length; multiple messages may be pipelined in one
//! stream. The DNS wire format comes from `dns.rs` (the same resolver that
//! drives UDP-53 today), the encryption from `tls.rs` (real X25519 + HKDF +
//! SHA-256 + ChaCha20-Poly1305, KAT-proven). The smoketest runs the FULL
//! exchange — handshake, framed query, server-side parse, framed answer,
//! client-side parse, plus pipelined deframing — through an in-kernel
//! client↔server loopback, so the proof is deterministic and identical on
//! QEMU and iron. Pointing the stream at a live 853 socket is the wire-up
//! follow-up (MasterChecklist Phase 10.2 — same item, iron half).

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::dns::{decode_name, DnsHeader, DnsQueryType, DnsRecord, DnsResolver};
use crate::tls::{ContentType, TlsConnection};

/// A DoT upstream: address, port 853, and the SNI/certificate name the TLS
/// session must present (RFC 7858 §4.2 — out-of-band pinned identity).
pub struct DotUpstream {
    pub addr: [u8; 4],
    pub port: u16,
    pub sni: &'static str,
}

/// Well-known public DoT resolvers, used until AthGuard network policy
/// supplies an override.
pub static UPSTREAMS: [DotUpstream; 2] = [
    DotUpstream {
        addr: [1, 1, 1, 1],
        port: 853,
        sni: "cloudflare-dns.com",
    },
    DotUpstream {
        addr: [9, 9, 9, 9],
        port: 853,
        sni: "dns.quad9.net",
    },
];

static QUERIES_FRAMED: AtomicU64 = AtomicU64::new(0);
static ANSWERS_PARSED: AtomicU64 = AtomicU64::new(0);

/// RFC 7858 §3.3 framing: two-octet big-endian length prefix per message.
pub fn frame(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.len() + 2);
    out.extend_from_slice(&(msg.len() as u16).to_be_bytes());
    out.extend_from_slice(msg);
    out
}

/// Split a decrypted TLS stream into complete DNS messages. Returns the
/// messages plus the number of bytes consumed — a trailing partial message
/// stays in the stream until more TLS records arrive.
pub fn deframe(stream: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let mut msgs = Vec::new();
    let mut off = 0usize;
    while stream.len() >= off + 2 {
        let len = u16::from_be_bytes([stream[off], stream[off + 1]]) as usize;
        if stream.len() < off + 2 + len {
            break;
        }
        msgs.push(stream[off + 2..off + 2 + len].to_vec());
        off += 2 + len;
    }
    (msgs, off)
}

/// Server half of the loopback: parse the question out of `query` and
/// answer it with a single A record (compression pointer back to the
/// question name, exactly as real resolvers respond).
fn build_a_response(query: &[u8], addr: [u8; 4]) -> Option<Vec<u8>> {
    let header = DnsHeader::parse(query)?;
    if header.qd_count != 1 {
        return None;
    }
    let mut offset = 12;
    let _name = decode_name(query, &mut offset)?;
    if offset + 4 > query.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([query[offset], query[offset + 1]]);
    let question_end = offset + 4;

    let mut buf = Vec::with_capacity(query.len() + 16);
    DnsHeader {
        id: header.id,
        flags: 0x8180, // QR + RD + RA, NoError
        qd_count: 1,
        an_count: 1,
        ns_count: 0,
        ar_count: 0,
    }
    .serialize(&mut buf);
    buf.extend_from_slice(&query[12..question_end]); // echo the question
    buf.extend_from_slice(&[0xC0, 0x0C]); // name = pointer to offset 12
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes()); // IN
    buf.extend_from_slice(&300u32.to_be_bytes()); // TTL
    buf.extend_from_slice(&4u16.to_be_bytes());
    buf.extend_from_slice(&addr);
    Some(buf)
}

pub fn init() {
    crate::serial_println!(
        "[dot] DNS-over-TLS armed (RFC 7858): upstreams {}.{}.{}.{}:{} ({}), {}.{}.{}.{}:{} ({})",
        UPSTREAMS[0].addr[0],
        UPSTREAMS[0].addr[1],
        UPSTREAMS[0].addr[2],
        UPSTREAMS[0].addr[3],
        UPSTREAMS[0].port,
        UPSTREAMS[0].sni,
        UPSTREAMS[1].addr[0],
        UPSTREAMS[1].addr[1],
        UPSTREAMS[1].addr[2],
        UPSTREAMS[1].addr[3],
        UPSTREAMS[1].port,
        UPSTREAMS[1].sni,
    );
}

/// Deterministic proof of the whole DoT exchange through an in-kernel TLS
/// loopback: handshake → framed query encrypted client→server → server
/// parses the DNS question and answers → framed answer encrypted
/// server→client → client parses the A record. Plus RFC 7858 pipelining
/// (two frames in one stream deframe to two messages).
pub fn run_boot_smoketest() {
    let mut client = TlsConnection::new_client(UPSTREAMS[0].sni);
    let mut server = TlsConnection::new_server();
    // This loopback exercises the record/key-agreement layer, not PKI — the
    // "server" is us. Opt out of the (default-on) client cert authentication so
    // the handshake completes; real outbound TLS keeps the secure default.
    client.allow_unverified = true;

    // TLS 1.3 handshake to agreed traffic keys (same key-agreement proof as
    // the tls.rs smoketest — both sides derive from one ECDHE secret).
    let ch = match client.handshake(&[]) {
        Ok(b) => b,
        Err(e) => {
            crate::serial_println!("[dot] smoketest FAIL: ClientHello -> {:?}", e);
            return;
        }
    };
    let sh_flight = match server.handshake(&ch) {
        Ok(b) => b,
        Err(e) => {
            crate::serial_println!("[dot] smoketest FAIL: server handshake -> {:?}", e);
            return;
        }
    };
    let _ = client.handshake(&sh_flight);

    let tls_keys = match (
        client.handshake.early_secret.as_ref(),
        server.handshake.early_secret.as_ref(),
    ) {
        (Some(c), Some(s)) => c == s && c.iter().any(|&b| b != 0),
        _ => false,
    };

    // Client: build the DNS query, frame it (RFC 7858), encrypt it.
    let mut resolver = DnsResolver::new();
    let query_wire = resolver.build_query("dot.athena.test", DnsQueryType::A);
    let query_id = u16::from_be_bytes([query_wire[0], query_wire[1]]);
    QUERIES_FRAMED.fetch_add(1, Ordering::Relaxed);

    let (ckey, civ) = client
        .traffic_keys
        .as_ref()
        .map(|k| (k.client_key.clone(), k.client_iv.clone()))
        .unwrap_or_default();
    let query_record = client.encrypt_record(
        ContentType::ApplicationData,
        &frame(&query_wire),
        &ckey,
        &civ,
        0,
    );

    // Server: decrypt with its read-side (client-write) key, deframe, parse.
    let (skey_c, siv_c) = server
        .traffic_keys
        .as_ref()
        .map(|k| (k.client_key.clone(), k.client_iv.clone()))
        .unwrap_or_default();
    let server_plain = server
        .decrypt_record(&query_record, &skey_c, &siv_c, 0)
        .map(|(_, p)| p)
        .unwrap_or_default();
    let (server_msgs, _) = deframe(&server_plain);
    let query_thru_tls = server_msgs.len() == 1 && server_msgs[0] == query_wire;

    // Server: answer with an A record, frame + encrypt with its write key.
    let answer_wire = server_msgs
        .first()
        .and_then(|q| build_a_response(q, [10, 0, 0, 53]))
        .unwrap_or_default();
    let (skey_s, siv_s) = server
        .traffic_keys
        .as_ref()
        .map(|k| (k.server_key.clone(), k.server_iv.clone()))
        .unwrap_or_default();
    let answer_record = server.encrypt_record(
        ContentType::ApplicationData,
        &frame(&answer_wire),
        &skey_s,
        &siv_s,
        0,
    );

    // Client: decrypt the server-write stream, deframe, parse the answer.
    let (ckey_s, civ_s) = client
        .traffic_keys
        .as_ref()
        .map(|k| (k.server_key.clone(), k.server_iv.clone()))
        .unwrap_or_default();
    let client_plain = client
        .decrypt_record(&answer_record, &ckey_s, &civ_s, 0)
        .map(|(_, p)| p)
        .unwrap_or_default();
    let (client_msgs, _) = deframe(&client_plain);
    let a_record = client_msgs
        .first()
        .and_then(|m| resolver.parse_response(m))
        .map(|r| {
            r.id == query_id
                && r.query_name == "dot.athena.test"
                && r.answers
                    .iter()
                    .any(|a| matches!(a, DnsRecord::A(addr) if *addr == [10, 0, 0, 53]))
        })
        .unwrap_or(false);
    if a_record {
        ANSWERS_PARSED.fetch_add(1, Ordering::Relaxed);
    }

    // RFC 7858 pipelining: two frames in one stream deframe to two messages
    // with nothing left over.
    let mut pipelined = frame(&query_wire);
    pipelined.extend_from_slice(&frame(&answer_wire));
    let (pipe_msgs, consumed) = deframe(&pipelined);
    let pipeline = pipe_msgs.len() == 2 && consumed == pipelined.len();

    let pass = tls_keys && query_thru_tls && a_record && pipeline;
    crate::serial_println!(
        "[dot] smoketest: tls_keys={} query_thru_tls={} a_record={} pipeline_deframe={} -> {} (RFC 7858 over TLS 1.3)",
        tls_keys,
        query_thru_tls,
        a_record,
        pipeline,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/athena/dot` — DoT transport state.
pub fn dump_text() -> String {
    let mut out = String::from("# DNS over TLS (RFC 7858)\n");
    for up in UPSTREAMS.iter() {
        out.push_str(&alloc::format!(
            "upstream: {}.{}.{}.{}:{} sni={}\n",
            up.addr[0],
            up.addr[1],
            up.addr[2],
            up.addr[3],
            up.port,
            up.sni,
        ));
    }
    out.push_str(&alloc::format!(
        "queries_framed: {}\nanswers_parsed: {}\n",
        QUERIES_FRAMED.load(Ordering::Relaxed),
        ANSWERS_PARSED.load(Ordering::Relaxed),
    ));
    out
}
