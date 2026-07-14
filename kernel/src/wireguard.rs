//! WireGuard userspace primitive — Concept §AthNet:
//!
//! > "built-in WireGuard, QUIC priority, gaming traffic shaping."
//!
//! WireGuard is "userspace VPN" only in the sense that the canonical
//! Linux implementation runs as a kernel module. AthenaOS keeps the
//! crypto-state-machine kernel-side because the firewall + traffic
//! shaper need to see plaintext to apply QoS, and we already have a
//! kernel-side crypto module (`crate::crypto`) with the Curve25519 +
//! ChaCha20-Poly1305 + Blake2s primitives WireGuard needs. The on-wire
//! protocol parsing (handshake init/response/cookie/data) lives here;
//! the actual UDP send/recv funnels through `crate::net`.
//!
//! This first pass exposes the **tunnel registry** + a stat surface so
//! Settings → Network → VPN can list tunnels, toggle them, and show
//! transfer counters. The handshake state machine is stubbed; what
//! exists today is the bookkeeping. When the userspace daemon plus the
//! crypto wiring land, the only addition is the actual handshake
//! transitions inside `Tunnel::tick()`.
//!
//! ## Syscalls (81-84)
//!
//! | nr | name           | rdi/rsi/rdx/r10                                              | rax |
//! |----|----------------|--------------------------------------------------------------|----|
//! | 81 | WG_LIST        | rdi=out_ptr, rsi=out_cap (WgTunnelAbi entries)               | count |
//! | 82 | WG_ADD         | rdi=name_ptr, rsi=name_len, rdx=endpoint_packed, r10=key_ptr | id  |
//! | 83 | WG_REMOVE      | rdi=tunnel_id                                                | 0/err |
//! | 84 | WG_STATS       | rdi=tunnel_id, rsi=out_ptr (WgStatsAbi)                      | bytes |

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ── Tunnel model ───────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    Idle = 0,
    Initiated = 1,
    Confirmed = 2,
    Rekeying = 3,
    Failed = 4,
}

#[derive(Debug, Clone)]
struct Tunnel {
    id: u64,
    name: String,
    /// Remote IPv4 packed as (ip << 16) | port — keeps the syscall ABI fits-
    /// in-registers and lets v6 sneak in later via a flag bit if we want.
    endpoint: u64,
    peer_static: [u8; 32],
    local_static: [u8; 32],
    /// Optional Noise IKpsk2 pre-shared key. All-zero = "no PSK", which is a
    /// valid WireGuard configuration (the PSK is an optional post-quantum /
    /// defense-in-depth layer, still mixed via KDF3 with a zero key).
    preshared_key: [u8; 32],
    state: HandshakeState,
    /// Counters in bytes; updated by data-packet path once wired.
    tx_bytes: u64,
    rx_bytes: u64,
    last_handshake_tsc: u64,

    // Handshake state (Noise IKpsk2)
    chaining_key: [u8; 32],
    handshake_hash: [u8; 32],
    ephemeral_priv: [u8; 32],
    ephemeral_pub: [u8; 32],
    send_key: [u8; 32],
    recv_key: [u8; 32],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WgTunnelAbi {
    pub version: u32, // = 1
    pub id: u64,
    pub state: u32,
    pub endpoint: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub last_handshake_tsc: u64,
    pub pubkey: [u8; 32],
    pub name: [u8; 24],
}

impl WgTunnelAbi {
    /// Serialize into the exact repr(C) byte layout for a SMAP-safe copy_to_user.
    /// Non-obvious padding: `id`(u64) forces 4 pad bytes after `version`@0, and
    /// `endpoint`(u64) forces 4 pad bytes after `state`@16. Layout:
    ///   version@0, pad@4, id@8, state@16, pad@20, endpoint@24, tx@32, rx@40,
    ///   last_handshake@48, pubkey@56, name@88 → total 112.
    fn to_le_bytes(&self) -> [u8; 112] {
        debug_assert_eq!(core::mem::size_of::<WgTunnelAbi>(), 112);
        let mut b = [0u8; 112];
        b[0..4].copy_from_slice(&self.version.to_le_bytes());
        b[8..16].copy_from_slice(&self.id.to_le_bytes());
        b[16..20].copy_from_slice(&self.state.to_le_bytes());
        b[24..32].copy_from_slice(&self.endpoint.to_le_bytes());
        b[32..40].copy_from_slice(&self.tx_bytes.to_le_bytes());
        b[40..48].copy_from_slice(&self.rx_bytes.to_le_bytes());
        b[48..56].copy_from_slice(&self.last_handshake_tsc.to_le_bytes());
        b[56..88].copy_from_slice(&self.pubkey);
        b[88..112].copy_from_slice(&self.name);
        b
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WgStatsAbi {
    pub version: u32,
    pub state: u32,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub last_handshake_tsc: u64,
    pub peer_count: u32,
    pub _reserved: u32,
}

impl WgStatsAbi {
    /// Serialize into the exact repr(C) byte layout for a SMAP-safe copy_to_user.
    /// No internal padding (two u32 pack to 8 before the u64 run). Layout:
    ///   version@0, state@4, tx@8, rx@16, last_handshake@24, peer_count@32,
    ///   _reserved@36 → total 40.
    fn to_le_bytes(&self) -> [u8; 40] {
        debug_assert_eq!(core::mem::size_of::<WgStatsAbi>(), 40);
        let mut b = [0u8; 40];
        b[0..4].copy_from_slice(&self.version.to_le_bytes());
        b[4..8].copy_from_slice(&self.state.to_le_bytes());
        b[8..16].copy_from_slice(&self.tx_bytes.to_le_bytes());
        b[16..24].copy_from_slice(&self.rx_bytes.to_le_bytes());
        b[24..32].copy_from_slice(&self.last_handshake_tsc.to_le_bytes());
        b[32..36].copy_from_slice(&self.peer_count.to_le_bytes());
        b[36..40].copy_from_slice(&self._reserved.to_le_bytes());
        b
    }
}

struct Registry {
    tunnels: BTreeMap<u64, Tunnel>,
    handshakes_attempted: u64,
    handshakes_succeeded: u64,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static REG: Mutex<Option<Registry>> = Mutex::new(None);

// ── Boot init ──────────────────────────────────────────────────────────

pub fn init() {
    *REG.lock() = Some(Registry {
        tunnels: BTreeMap::new(),
        handshakes_attempted: 0,
        handshakes_succeeded: 0,
    });
    crate::serial_println!(
        "[ OK ] WireGuard: tunnel registry ready (kernel-side state machine, userspace UDP)",
    );
}

// ── Public API ─────────────────────────────────────────────────────────

pub fn add_tunnel(name: &str, endpoint: u64, pubkey: [u8; 32]) -> u64 {
    // BUG-41 fix: never deploy a tunnel with a known (all-zero) static private
    // key — that lets any observer derive the transport keys. Generate a fresh
    // X25519 private key from the CSPRNG (X25519 clamps internally, so 32 random
    // bytes is a valid scalar). MasterChecklist Phase 3 swaps this for a key
    // loaded from sealed storage / AthGuard TPM quote.
    let mut local_static = [0u8; 32];
    if crate::crypto::getrandom(&mut local_static).is_err() {
        crate::serial_println!(
            "[wireguard] WARNING: entropy unavailable — new tunnel static key is NOT secure"
        );
    }
    // No PSK by default (all-zero is valid WireGuard); the userspace daemon can
    // supply one later via a dedicated syscall.
    add_tunnel_full(name, endpoint, pubkey, local_static, [0u8; 32])
}

/// Register a tunnel with fully-specified key material. Splitting this out lets
/// the boot self-test drive both ends of a Noise IKpsk2 handshake with known
/// keys (initiator static private + peer static public + PSK) instead of the
/// randomly-generated static key `add_tunnel` produces.
fn add_tunnel_full(
    name: &str,
    endpoint: u64,
    peer_static: [u8; 32],
    local_static: [u8; 32],
    preshared_key: [u8; 32],
) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let t = Tunnel {
        id,
        name: String::from(name),
        endpoint,
        peer_static,
        local_static,
        preshared_key,
        state: HandshakeState::Idle,
        tx_bytes: 0,
        rx_bytes: 0,
        last_handshake_tsc: 0,
        chaining_key: [0; 32],
        handshake_hash: [0; 32],
        ephemeral_priv: [0; 32],
        ephemeral_pub: [0; 32],
        send_key: [0; 32],
        recv_key: [0; 32],
    };
    let mut g = REG.lock();
    if let Some(r) = g.as_mut() {
        r.tunnels.insert(id, t);
    }
    crate::serial_println!(
        "[wireguard] tunnel #{} \"{}\" registered (endpoint=0x{:x})",
        id,
        name,
        endpoint,
    );
    id
}

// ── Noise IKpsk2 handshake driver ──────────────────────────────────────
//
// WireGuard's protocol is the Noise Protocol Framework's IKpsk2 pattern:
//
//   Initiator → Responder:  Initiation message (148 bytes)
//                           - sender_index, ephemeral, static (encrypted),
//                             timestamp (encrypted), MAC1, MAC2
//   Responder → Initiator:  Response message (92 bytes)
//                           - sender_index, receiver_index, ephemeral,
//                             empty (encrypted), MAC1, MAC2
//   then keepalive + data packets (transport phase)
//
// The actual mixing uses Blake2s (real, in crypto.rs) + ChaCha20-Poly1305
// (real, in crypto.rs) + X25519 (real, in crypto.rs). The state
// machine below walks the transitions. See `kernelchecklist.md` §M-D.

fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ── Noise IKpsk2 primitives (conformant WireGuard v1) ──────────────────
//
// Protocol name: Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s. All symmetric-state
// mixing below follows the WireGuard whitepaper §5.4 and the Linux kernel
// reference `noise.c` (mix_hash BEFORE the ephemeral KDF, KDFn built from
// HMAC-BLAKE2s with the 0x1/0x2/0x3 chaining bytes — NOT a raw hkdf_extract).

const CONSTRUCTION: &[u8] = b"Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
const IDENTIFIER: &[u8] = b"WireGuard v1 zx2c4 Jason@zx2c4.com";
const LABEL_MAC1: &[u8] = b"mac1----";

/// BLAKE2s-256 hash of a single slice — the Noise `HASH()` primitive.
fn noise_hash(input: &[u8]) -> [u8; 32] {
    use crate::crypto::{Blake2s256Context, HashAlgorithm};
    let mut h = Blake2s256Context::new();
    h.update(input);
    let mut r = [0u8; 32];
    h.finalize(&mut r);
    r
}

/// Noise `MixHash`: h ← HASH(h ‖ data).
fn mix_hash(h: &[u8; 32], data: &[u8]) -> [u8; 32] {
    use crate::crypto::{Blake2s256Context, HashAlgorithm};
    let mut ctx = Blake2s256Context::new();
    ctx.update(h);
    ctx.update(data);
    let mut out = [0u8; 32];
    ctx.finalize(&mut out);
    out
}

/// Raw HMAC-BLAKE2s(key, msg) — the building block of every KDFn below.
fn hmac_blake2s(key: &[u8], msg: &[u8]) -> [u8; 32] {
    use crate::crypto::HmacContext;
    let hmac = HmacContext::new_blake2s(key);
    let mut out = [0u8; 32];
    hmac.compute(msg, &mut out);
    out
}

/// WireGuard KDF1: returns τ₁ = HMAC(HMAC(key, input), 0x1).
fn kdf1(key: &[u8; 32], input: &[u8]) -> [u8; 32] {
    let t0 = hmac_blake2s(key, input);
    hmac_blake2s(&t0, &[0x1])
}

/// WireGuard KDF2: τ₁ = HMAC(t0,0x1); τ₂ = HMAC(t0, τ₁‖0x2).
fn kdf2(key: &[u8; 32], input: &[u8]) -> ([u8; 32], [u8; 32]) {
    let t0 = hmac_blake2s(key, input);
    let t1 = hmac_blake2s(&t0, &[0x1]);
    let mut buf = [0u8; 33];
    buf[..32].copy_from_slice(&t1);
    buf[32] = 0x2;
    let t2 = hmac_blake2s(&t0, &buf);
    (t1, t2)
}

/// WireGuard KDF3: τ₁,τ₂ as KDF2 plus τ₃ = HMAC(t0, τ₂‖0x3).
fn kdf3(key: &[u8; 32], input: &[u8]) -> ([u8; 32], [u8; 32], [u8; 32]) {
    let t0 = hmac_blake2s(key, input);
    let t1 = hmac_blake2s(&t0, &[0x1]);
    let mut b2 = [0u8; 33];
    b2[..32].copy_from_slice(&t1);
    b2[32] = 0x2;
    let t2 = hmac_blake2s(&t0, &b2);
    let mut b3 = [0u8; 33];
    b3[..32].copy_from_slice(&t2);
    b3[32] = 0x3;
    let t3 = hmac_blake2s(&t0, &b3);
    (t1, t2, t3)
}

/// WireGuard nonce for a handshake AEAD: 4 zero bytes ‖ LE64(counter). Every
/// handshake message uses counter 0, so this is all-zero, but the helper keeps
/// the transport path honest when it lands.
fn wg_nonce(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..12].copy_from_slice(&counter.to_le_bytes());
    n
}

/// AEAD(key, counter, plaintext, aad) → ciphertext ‖ 16-byte tag.
fn aead_encrypt(key: &[u8; 32], counter: u64, plaintext: &[u8], aad: &[u8]) -> alloc::vec::Vec<u8> {
    use crate::crypto::ChaCha20Poly1305;
    let aead = ChaCha20Poly1305::new(key);
    let nonce = wg_nonce(counter);
    let mut out = alloc::vec![0u8; plaintext.len() + 16];
    let (ct, tag) = out.split_at_mut(plaintext.len());
    let mut tag_arr = [0u8; 16];
    // ChaCha20-Poly1305 encrypt is infallible here (fixed 32-byte key); the
    // Result only ever carries key-length errors we can't produce.
    aead.encrypt(&nonce, aad, plaintext, ct, &mut tag_arr)
        .expect("chacha20poly1305 encrypt");
    tag.copy_from_slice(&tag_arr);
    out
}

/// AEAD-decrypt-and-verify. Returns Err("AEAD auth failed") on ANY tag
/// mismatch — this is the fail-closed authenticator of the whole handshake.
fn aead_decrypt(
    key: &[u8; 32],
    counter: u64,
    data: &[u8],
    aad: &[u8],
) -> Result<alloc::vec::Vec<u8>, &'static str> {
    use crate::crypto::ChaCha20Poly1305;
    if data.len() < 16 {
        return Err("AEAD ciphertext too short");
    }
    let (ct, tag) = data.split_at(data.len() - 16);
    let mut tag_arr = [0u8; 16];
    tag_arr.copy_from_slice(tag);
    let aead = ChaCha20Poly1305::new(key);
    let nonce = wg_nonce(counter);
    let mut pt = alloc::vec![0u8; ct.len()];
    aead.decrypt(&nonce, aad, ct, &tag_arr, &mut pt)
        .map_err(|_| "AEAD auth failed")?;
    Ok(pt)
}

/// X25519 DH — small wrapper so the handshake reads like the whitepaper.
fn dh(private: &[u8; 32], peer_public: &[u8; 32]) -> Result<[u8; 32], &'static str> {
    use crate::crypto::X25519Context;
    let ctx = X25519Context::with_private_key(*private);
    let mut out = [0u8; 32];
    ctx.compute_shared_secret(peer_public, &mut out)
        .map_err(|_| "DH failed")?;
    Ok(out)
}

/// X25519 public key for a private scalar.
fn dh_public(private: &[u8; 32]) -> [u8; 32] {
    use crate::crypto::X25519Context;
    *X25519Context::with_private_key(*private).public_key_bytes()
}

/// mac1 = Keyed-BLAKE2s(key = HASH(LABEL_MAC1 ‖ receiver_static_pub),
/// msg = message bytes preceding the mac1 field), 16-byte output.
fn compute_mac1(receiver_static_pub: &[u8; 32], msg_prefix: &[u8]) -> [u8; 16] {
    use crate::crypto::{Blake2s256Context, HashAlgorithm};
    let mut kctx = Blake2s256Context::new();
    kctx.update(LABEL_MAC1);
    kctx.update(receiver_static_pub);
    let mut key = [0u8; 32];
    kctx.finalize(&mut key);
    let mut m = Blake2s256Context::new_keyed(&key, 16);
    m.update(msg_prefix);
    let mut out = [0u8; 16];
    m.finalize(&mut out);
    out
}

fn macs_equal(a: &[u8], b: &[u8; 16]) -> bool {
    if a.len() != 16 {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..16 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Compute the shared initial (chaining_key, hash) both peers start from.
/// `responder_static_pub` is mixed into the hash by BOTH sides (it is always
/// the responder's static public key, per Noise IK's pre-message).
fn handshake_initial(responder_static_pub: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    use crate::crypto::{Blake2s256Context, HashAlgorithm};
    let chaining_key = noise_hash(CONSTRUCTION);
    let hash = {
        let mut c = Blake2s256Context::new();
        c.update(&chaining_key);
        c.update(IDENTIFIER);
        let mut o = [0u8; 32];
        c.finalize(&mut o);
        o
    };
    let hash = mix_hash(&hash, responder_static_pub);
    (chaining_key, hash)
}

/// TAI64N timestamp (12 bytes: TAI64 seconds ‖ nanoseconds). We do not have a
/// synchronized wall clock during early boot, so this derives a monotonically
/// increasing value from the TSC. A real peer only requires the timestamp be
/// strictly greater than the last one it accepted (anti-replay); interop with
/// upstream WireGuard therefore needs a wall-clock source wired here (caveat
/// noted honestly — this is not yet real Unix time).
fn tai64n_now() -> [u8; 12] {
    let mut ts = [0u8; 12];
    // TAI64 label base 2^62 + (approx) seconds; use the TSC high word as a
    // stand-in "seconds" and low word as nanoseconds so successive calls differ.
    let t = rdtsc();
    let secs: u64 = 0x4000_0000_0000_0000 | (t >> 30);
    let nanos: u32 = (t & 0x3FFF_FFFF) as u32;
    ts[0..8].copy_from_slice(&secs.to_be_bytes());
    ts[8..12].copy_from_slice(&nanos.to_be_bytes());
    ts
}

/// Drive the initiation step for a tunnel: build a fully-formed, cryptographic
/// 148-byte WireGuard Initiation message (encrypted static + timestamp + mac1)
/// and stash the initiator's running Noise state so `handle_response` can
/// finish the handshake. Userspace performs the actual UDP send.
pub fn drive_initiation(tunnel_id: u64) -> Result<[u8; 148], &'static str> {
    let mut g = REG.lock();
    let r = g.as_mut().ok_or("registry not initialized")?;
    let t = r.tunnels.get_mut(&tunnel_id).ok_or("no such tunnel")?;

    if t.state != HandshakeState::Idle && t.state != HandshakeState::Failed {
        return Err("handshake already in progress");
    }

    let responder_static = t.peer_static;
    let initiator_static_pub = dh_public(&t.local_static);

    // Noise symmetric state init (mixes the responder's static public key).
    let (mut chaining_key, mut hash) = handshake_initial(&responder_static);

    // Ephemeral keypair.
    use crate::crypto::getrandom;
    let mut eph_priv = [0u8; 32];
    getrandom(&mut eph_priv).map_err(|_| "entropy exhaustion")?;
    let eph_pub = dh_public(&eph_priv);

    let mut msg = [0u8; 148];
    msg[0] = 0x01; // type = Initiation
                   // msg[1..4] reserved = 0
    let sender_index: u32 = (tunnel_id as u32) ^ 0x1000_0000;
    msg[4..8].copy_from_slice(&sender_index.to_le_bytes());
    msg[8..40].copy_from_slice(&eph_pub);

    // mix_hash(ephemeral) BEFORE KDF1 (matches the Linux ref message_ephemeral).
    hash = mix_hash(&hash, &eph_pub);
    chaining_key = kdf1(&chaining_key, &eph_pub);

    // encrypted_static = AEAD(k, 0, S_pub_i, hash) where (C,k)=KDF2(C, DH(e_i,S_r)).
    let es = dh(&eph_priv, &responder_static)?;
    let (ck2, key) = kdf2(&chaining_key, &es);
    chaining_key = ck2;
    let enc_static = aead_encrypt(&key, 0, &initiator_static_pub, &hash);
    msg[40..88].copy_from_slice(&enc_static);
    hash = mix_hash(&hash, &enc_static);

    // encrypted_timestamp = AEAD(k, 0, TAI64N, hash) where (C,k)=KDF2(C, DH(S_i,S_r)).
    let ts_ss = dh(&t.local_static, &responder_static)?;
    let (ck3, key_ts) = kdf2(&chaining_key, &ts_ss);
    chaining_key = ck3;
    let timestamp = tai64n_now();
    let enc_ts = aead_encrypt(&key_ts, 0, &timestamp, &hash);
    msg[88..116].copy_from_slice(&enc_ts);
    hash = mix_hash(&hash, &enc_ts);

    // mac1 over bytes [0,116); mac2 = 0 (no cookie).
    let mac1 = compute_mac1(&responder_static, &msg[0..116]);
    msg[116..132].copy_from_slice(&mac1);
    // msg[132..148] mac2 stays zero.

    // Persist initiator state for handle_response.
    t.ephemeral_priv = eph_priv;
    t.ephemeral_pub = eph_pub;
    t.chaining_key = chaining_key;
    t.handshake_hash = hash;
    t.state = HandshakeState::Initiated;
    r.handshakes_attempted += 1;
    crate::serial_println!(
        "[wireguard] tunnel #{} drive_initiation → Initiated (148B: enc-static+timestamp+mac1)",
        tunnel_id,
    );
    Ok(msg)
}

/// Result of consuming an initiation on the responder side: the 92-byte
/// response to send back plus the derived transport keys (from the responder's
/// point of view, so `recv_key` decrypts what the initiator sends).
pub struct ResponderHandshake {
    pub response: [u8; 92],
    pub send_key: [u8; 32],
    pub recv_key: [u8; 32],
    /// The initiator static public key recovered (and authenticated) from the
    /// initiation's encrypted_static field.
    pub peer_static_pub: [u8; 32],
}

/// Responder half of the Noise IKpsk2 handshake. Verifies the initiation's
/// mac1 and both AEAD authenticators, then produces the response message and
/// transport keys. This exists so the handshake can be proven end-to-end
/// in-kernel without an external peer (same precedent as the TLS 1.3 loopback).
///
/// `resp_ephemeral_priv` is injected so the flow is deterministic under test;
/// pass a fresh random scalar in any real use.
pub fn responder_consume_initiation(
    responder_static_priv: &[u8; 32],
    preshared_key: &[u8; 32],
    init_msg: &[u8],
    resp_ephemeral_priv: &[u8; 32],
    responder_sender_index: u32,
) -> Result<ResponderHandshake, &'static str> {
    if init_msg.len() != 148 {
        return Err("initiation must be 148 bytes");
    }
    if init_msg[0] != 0x01 {
        return Err("not an Initiation message");
    }
    let responder_static_pub = dh_public(responder_static_priv);

    // mac1 covers bytes [0,116). Reject early on mismatch (DoS mitigation).
    let want_mac1 = compute_mac1(&responder_static_pub, &init_msg[0..116]);
    if !macs_equal(&init_msg[116..132], &want_mac1) {
        return Err("initiation mac1 invalid");
    }

    let (mut chaining_key, mut hash) = handshake_initial(&responder_static_pub);

    let mut eph_pub_i = [0u8; 32];
    eph_pub_i.copy_from_slice(&init_msg[8..40]);
    hash = mix_hash(&hash, &eph_pub_i);
    chaining_key = kdf1(&chaining_key, &eph_pub_i);

    // Decrypt static: (C,k)=KDF2(C, DH(S_r, e_i)).
    let es = dh(responder_static_priv, &eph_pub_i)?;
    let (ck2, key) = kdf2(&chaining_key, &es);
    chaining_key = ck2;
    let pt_static = aead_decrypt(&key, 0, &init_msg[40..88], &hash)?;
    if pt_static.len() != 32 {
        return Err("decrypted static wrong length");
    }
    let mut peer_static_pub = [0u8; 32];
    peer_static_pub.copy_from_slice(&pt_static);
    hash = mix_hash(&hash, &init_msg[40..88]);

    // Decrypt timestamp: (C,k)=KDF2(C, DH(S_r, S_i)).
    let ts_ss = dh(responder_static_priv, &peer_static_pub)?;
    let (ck3, key_ts) = kdf2(&chaining_key, &ts_ss);
    chaining_key = ck3;
    let _timestamp = aead_decrypt(&key_ts, 0, &init_msg[88..116], &hash)?;
    hash = mix_hash(&hash, &init_msg[88..116]);

    // ── Build the response ──────────────────────────────────────────────
    let eph_pub_r = dh_public(resp_ephemeral_priv);
    let receiver_index = u32::from_le_bytes(init_msg[4..8].try_into().unwrap());

    let mut resp = [0u8; 92];
    resp[0] = 0x02; // type = Response
    resp[4..8].copy_from_slice(&responder_sender_index.to_le_bytes());
    resp[8..12].copy_from_slice(&receiver_index.to_le_bytes());
    resp[12..44].copy_from_slice(&eph_pub_r);

    hash = mix_hash(&hash, &eph_pub_r);
    chaining_key = kdf1(&chaining_key, &eph_pub_r);
    // ee: DH(e_r, e_i)
    let ee = dh(resp_ephemeral_priv, &eph_pub_i)?;
    chaining_key = kdf1(&chaining_key, &ee);
    // se: DH(e_r, S_i)
    let se = dh(resp_ephemeral_priv, &peer_static_pub)?;
    chaining_key = kdf1(&chaining_key, &se);
    // psk: (C, τ, k) = KDF3(C, psk); mix_hash(τ)
    let (ck4, tau, key_nothing) = kdf3(&chaining_key, preshared_key);
    chaining_key = ck4;
    hash = mix_hash(&hash, &tau);
    // encrypted_nothing = AEAD(k, 0, empty, hash)
    let enc_nothing = aead_encrypt(&key_nothing, 0, &[], &hash);
    resp[44..60].copy_from_slice(&enc_nothing);
    // The terminal mix_hash(hash, encrypted_nothing) is omitted: the responder
    // derives its transport keys from `chaining_key` alone, and the handshake
    // hash is not carried into the transport phase.

    // mac1 over [0,60) keyed by HASH(LABEL_MAC1 ‖ S_pub_i).
    let mac1 = compute_mac1(&peer_static_pub, &resp[0..60]);
    resp[60..76].copy_from_slice(&mac1);
    // resp[76..92] mac2 stays zero.

    // Transport keys: responder derives (recv, send) = KDF2(C, ε).
    let (recv_key, send_key) = kdf2(&chaining_key, &[]);

    Ok(ResponderHandshake {
        response: resp,
        send_key,
        recv_key,
        peer_static_pub,
    })
}

/// Handle a Response message arrival (initiator side). Completes the Noise
/// mixing over the responder ephemeral + both DHs + the PSK, then AEAD-decrypts
/// and VERIFIES `encrypted_nothing`. ONLY on tag success are transport keys
/// derived and the tunnel moved to Confirmed. Any authenticator failure returns
/// Err and leaves the tunnel unauthenticated (fail-closed).
pub fn handle_response(tunnel_id: u64, response: &[u8]) -> Result<(), &'static str> {
    if response.len() != 92 {
        return Err("response must be 92 bytes");
    }
    if response[0] != 0x02 {
        return Err("not a Response message");
    }
    let mut g = REG.lock();
    let r = g.as_mut().ok_or("registry not initialized")?;
    let t = r.tunnels.get_mut(&tunnel_id).ok_or("no such tunnel")?;

    if t.state != HandshakeState::Initiated {
        return Err("not in Initiated state");
    }

    // Verify mac1 (keyed by our OWN static public key) before touching crypto.
    let initiator_static_pub = dh_public(&t.local_static);
    let want_mac1 = compute_mac1(&initiator_static_pub, &response[0..60]);
    if !macs_equal(&response[60..76], &want_mac1) {
        return Err("response mac1 invalid");
    }

    let mut peer_ephemeral = [0u8; 32];
    peer_ephemeral.copy_from_slice(&response[12..44]);

    let mut chaining_key = t.chaining_key;
    let mut hash = t.handshake_hash;

    hash = mix_hash(&hash, &peer_ephemeral);
    chaining_key = kdf1(&chaining_key, &peer_ephemeral);
    // ee: DH(e_i, e_r)
    let ee = dh(&t.ephemeral_priv, &peer_ephemeral)?;
    chaining_key = kdf1(&chaining_key, &ee);
    // se: DH(S_i, e_r)
    let se = dh(&t.local_static, &peer_ephemeral)?;
    chaining_key = kdf1(&chaining_key, &se);
    // psk: (C, τ, k) = KDF3(C, psk); mix_hash(τ)
    let (ck4, tau, key_nothing) = kdf3(&chaining_key, &t.preshared_key);
    chaining_key = ck4;
    hash = mix_hash(&hash, &tau);

    // AUTHENTICATOR: decrypt+verify encrypted_nothing (bytes 44..60), AAD=hash.
    // This is the ONLY thing that proves the peer knows the static+psk secrets.
    let pt = aead_decrypt(&key_nothing, 0, &response[44..60], &hash)?;
    if !pt.is_empty() {
        return Err("encrypted_nothing not empty");
    }

    // Authenticated — derive transport keys and confirm.
    let (send_key, recv_key) = kdf2(&chaining_key, &[]);
    t.send_key = send_key;
    t.recv_key = recv_key;
    t.state = HandshakeState::Confirmed;
    t.last_handshake_tsc = rdtsc();
    r.handshakes_succeeded += 1;
    crate::serial_println!(
        "[wireguard] tunnel #{} handle_response → Confirmed (encrypted_nothing verified; transport keys derived)",
        tunnel_id,
    );
    Ok(())
}

/// Snapshot a tunnel's (send_key, recv_key, state) — used by the boot
/// self-test to compare both ends of the loopback handshake.
fn tunnel_snapshot(id: u64) -> Option<([u8; 32], [u8; 32], HandshakeState)> {
    let g = REG.lock();
    let t = g.as_ref()?.tunnels.get(&id)?;
    Some((t.send_key, t.recv_key, t.state))
}

/// Tear down handshake state explicitly (rekey trigger or cookie reply).
pub fn reset_to_idle(tunnel_id: u64) -> Result<(), &'static str> {
    let mut g = REG.lock();
    let r = g.as_mut().ok_or("registry not initialized")?;
    let t = r.tunnels.get_mut(&tunnel_id).ok_or("no such tunnel")?;
    t.state = HandshakeState::Idle;
    Ok(())
}

pub fn remove_tunnel(id: u64) -> u64 {
    let mut g = REG.lock();
    let r = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    match r.tunnels.remove(&id) {
        Some(_) => 0,
        None => ERR_NO_SUCH,
    }
}

pub fn stats_for(id: u64) -> Option<WgStatsAbi> {
    let g = REG.lock();
    g.as_ref()?.tunnels.get(&id).map(|t| WgStatsAbi {
        version: 1,
        state: t.state as u32,
        tx_bytes: t.tx_bytes,
        rx_bytes: t.rx_bytes,
        last_handshake_tsc: t.last_handshake_tsc,
        peer_count: 1,
        _reserved: 0,
    })
}

// ── Error codes ────────────────────────────────────────────────────────

pub const ERR_NOT_INIT: u64 = 0xFFFF_FFFF_FFFF_FA01;
pub const ERR_NO_SUCH: u64 = 0xFFFF_FFFF_FFFF_FA02;
pub const ERR_BAD_USER: u64 = 0xFFFF_FFFF_FFFF_FA03;

// ── Syscalls ───────────────────────────────────────────────────────────

pub const SYS_WG_LIST: u64 = 81;
pub const SYS_WG_ADD: u64 = 82;
pub const SYS_WG_REMOVE: u64 = 83;
pub const SYS_WG_STATS: u64 = 84;

const WG_TUNNEL_ABI: usize = core::mem::size_of::<WgTunnelAbi>();
const WG_STATS_ABI: usize = core::mem::size_of::<WgStatsAbi>();

pub fn sys_list(out_ptr: u64, out_cap: u64, validate_w: impl Fn(u64, u64, bool) -> bool) -> u64 {
    if out_cap > 0 && !validate_w(out_ptr, out_cap, true) {
        return 0;
    }
    let g = REG.lock();
    let r = match g.as_ref() {
        Some(r) => r,
        None => return 0,
    };
    let max = (out_cap as usize) / WG_TUNNEL_ABI;
    let n = r.tunnels.len().min(max);
    // Assemble all entries kernel-side, then one SMAP-safe copy through the
    // uaccess/extable chokepoint (was per-entry raw write_unaligned to user).
    let mut buf = alloc::vec::Vec::with_capacity(n * WG_TUNNEL_ABI);
    for (id, t) in r.tunnels.iter().take(n) {
        let mut name = [0u8; 24];
        let nb = t.name.as_bytes();
        let len = nb.len().min(24);
        name[..len].copy_from_slice(&nb[..len]);
        let abi = WgTunnelAbi {
            version: 1,
            id: *id,
            state: t.state as u32,
            endpoint: t.endpoint,
            tx_bytes: t.tx_bytes,
            rx_bytes: t.rx_bytes,
            last_handshake_tsc: t.last_handshake_tsc,
            pubkey: t.peer_static,
            name,
        };
        buf.extend_from_slice(&abi.to_le_bytes());
    }
    if crate::uaccess::copy_to_user(out_ptr, &buf).is_err() {
        return 0;
    }
    n as u64
}

pub fn sys_add(
    name_ptr: u64,
    name_len: u64,
    endpoint: u64,
    key_ptr: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if !validate_r(name_ptr, name_len, false) {
        return ERR_BAD_USER;
    }
    if !validate_r(key_ptr, 32, false) {
        return ERR_BAD_USER;
    }
    // SMAP-safe reads through the uaccess/extable chokepoint (was raw
    // copy_nonoverlapping from user pointers).
    let name_buf = match crate::uaccess::copy_from_user(name_ptr, name_len as usize) {
        Ok(b) => b,
        Err(()) => return ERR_BAD_USER,
    };
    let name = String::from_utf8(name_buf).unwrap_or_default();
    let mut pk = [0u8; 32];
    if crate::uaccess::copy_from_user_into(key_ptr, &mut pk).is_err() {
        return ERR_BAD_USER;
    }
    add_tunnel(&name, endpoint, pk)
}

pub fn sys_remove(id: u64) -> u64 {
    remove_tunnel(id)
}

pub fn sys_stats(
    id: u64,
    out_ptr: u64,
    out_cap: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if out_cap < WG_STATS_ABI as u64 {
        return u64::MAX;
    }
    if !validate_w(out_ptr, WG_STATS_ABI as u64, true) {
        return u64::MAX;
    }
    let abi = match stats_for(id) {
        Some(a) => a,
        None => return u64::MAX,
    };
    // SMAP-safe copy through the uaccess/extable chokepoint (was raw
    // write_unaligned to the user pointer).
    if crate::uaccess::copy_to_user(out_ptr, &abi.to_le_bytes()).is_err() {
        return u64::MAX;
    }
    WG_STATS_ABI as u64
}

// ── /proc/raeen/wireguard ──────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = REG.lock();
    let r = match g.as_ref() {
        Some(r) => r,
        None => return String::from("# wireguard not initialized\n"),
    };
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "# AthenaOS WireGuard ({} tunnels, {}/{} successful handshakes)\n",
        r.tunnels.len(),
        r.handshakes_succeeded,
        r.handshakes_attempted,
    ));
    for (id, t) in &r.tunnels {
        out.push_str(&alloc::format!(
            "#{:<3} \"{}\" state={:?} endpoint=0x{:x} tx={} rx={}\n",
            id,
            t.name,
            t.state,
            t.endpoint,
            t.tx_bytes,
            t.rx_bytes,
        ));
    }
    out
}

// ── Boot smoketest ─────────────────────────────────────────────────────

pub fn run_boot_smoketest() {
    // Register a placeholder tunnel so the Settings → Network → VPN panel
    // shows something the moment a user opens it. Real entries replace
    // this once the userspace `raevpn` daemon connects.
    let pk = [0xC0u8; 32];
    let id = add_tunnel("example-vpn-tokyo", 0x52_54_00_02_0001_5800, pk);
    crate::serial_println!(
        "[wireguard] smoketest tunnel id={} registered (kernel ABI + counters wired)",
        id,
    );

    // ── Full Noise IKpsk2 handshake proof (in-kernel initiator ↔ responder) ──
    //
    // Internal-consistency proof (NOT external interop): we run BOTH ends of a
    // real WireGuard handshake with fresh X25519 static keys + a non-zero PSK.
    // PASS requires (a) the initiator reaches Confirmed, (b) both ends derive
    // MATCHING transport keys (initiator.send == responder.recv and vice-versa),
    // and (c) a one-byte tamper of `encrypted_nothing` is REJECTED (stays
    // unauthenticated). Same precedent as the TLS 1.3 in-kernel loopback.
    let (confirmed, keys_match, tamper_rejected) = handshake_loopback_selftest();

    if confirmed && keys_match && tamper_rejected {
        crate::serial_println!(
            "[wireguard] smoketest: handshake=confirmed keys_match=true tamper_rejected=true -> PASS"
        );
    } else {
        crate::serial_println!(
            "[wireguard] smoketest: handshake=confirmed={} keys_match={} tamper_rejected={} -> FAIL",
            confirmed,
            keys_match,
            tamper_rejected
        );
    }
}

/// Run a complete initiator↔responder WireGuard handshake in-kernel and report
/// (confirmed, keys_match, tamper_rejected). Any crypto/entropy failure yields
/// `false` in the relevant slot so the smoketest can print FAIL.
fn handshake_loopback_selftest() -> (bool, bool, bool) {
    use crate::crypto::getrandom;

    // Fresh static keypairs + PSK. X25519 clamps internally, so 32 random bytes
    // is a valid scalar; a non-zero PSK exercises the KDF3 mixing path.
    let mut i_priv = [0u8; 32];
    let mut r_priv = [0u8; 32];
    let mut psk = [0u8; 32];
    let mut resp_eph = [0u8; 32];
    if getrandom(&mut i_priv).is_err()
        || getrandom(&mut r_priv).is_err()
        || getrandom(&mut psk).is_err()
        || getrandom(&mut resp_eph).is_err()
    {
        crate::serial_println!("[wireguard] smoketest: entropy unavailable");
        return (false, false, false);
    }
    let r_pub = dh_public(&r_priv);
    let i_pub = dh_public(&i_priv);

    // ── Happy path ──────────────────────────────────────────────────────
    let id = add_tunnel_full("selftest-loopback", 0, r_pub, i_priv, psk);
    let init = match drive_initiation(id) {
        Ok(m) => m,
        Err(e) => {
            crate::serial_println!("[wireguard] smoketest: drive_initiation: {}", e);
            return (false, false, false);
        }
    };
    let resp = match responder_consume_initiation(&r_priv, &psk, &init, &resp_eph, 0xABCD_0001) {
        Ok(rh) => rh,
        Err(e) => {
            crate::serial_println!("[wireguard] smoketest: responder: {}", e);
            return (false, false, false);
        }
    };
    // Responder must recover the initiator's true static public key.
    if resp.peer_static_pub != i_pub {
        crate::serial_println!("[wireguard] smoketest: responder recovered wrong static key");
        return (false, false, false);
    }
    if let Err(e) = handle_response(id, &resp.response) {
        crate::serial_println!("[wireguard] smoketest: handle_response: {}", e);
        return (false, false, false);
    }

    let confirmed;
    let keys_match;
    match tunnel_snapshot(id) {
        Some((send, recv, state)) => {
            confirmed = state == HandshakeState::Confirmed;
            // Initiator.send decrypts on responder.recv and vice-versa.
            keys_match = send == resp.recv_key && recv == resp.send_key;
        }
        None => return (false, false, false),
    }

    // ── Tamper path: flip one byte of encrypted_nothing → must be rejected ──
    let id2 = add_tunnel_full("selftest-tamper", 0, r_pub, i_priv, psk);
    let mut resp_eph2 = [0u8; 32];
    let _ = getrandom(&mut resp_eph2);
    let tamper_rejected = match drive_initiation(id2) {
        Ok(init2) => {
            match responder_consume_initiation(&r_priv, &psk, &init2, &resp_eph2, 0xABCD_0002) {
                Ok(mut rh2) => {
                    rh2.response[44] ^= 0x01; // corrupt encrypted_nothing
                    let rejected = handle_response(id2, &rh2.response).is_err();
                    // And the tunnel must NOT have reached Confirmed.
                    let not_confirmed = match tunnel_snapshot(id2) {
                        Some((_, _, st)) => st != HandshakeState::Confirmed,
                        None => false,
                    };
                    rejected && not_confirmed
                }
                Err(_) => false,
            }
        }
        Err(_) => false,
    };

    (confirmed, keys_match, tamper_rejected)
}

// ── Peer keepalive + rekeying timers ─────────────────────────────────────────
// MasterChecklist Phase 10: "Real X25519 so WireGuard handshake is cryptographically valid."
// MasterChecklist Phase 10: "Built-in WireGuard `raevpn` userspace daemon."
//
// WireGuard protocol (per whitepaper §6.5):
//   - Rekey interval:      REKEY_AFTER_TIME = 180s (initiate new handshake)
//   - Keepalive interval:  KEEPALIVE_TIMEOUT = 10s (send empty data packet to keep NAT alive)
//   - Reject after:        REJECT_AFTER_TIME = 180s + REKEY_ATTEMPT_TIME (discard if no rekey)
//
// Reference: boringtun `src/noise/timers.rs` — exact implementation of these timers.

const REKEY_AFTER_SECS: u64 = 180;
const KEEPALIVE_SECS: u64 = 10;
const REKEY_ATTEMPT_SECS: u64 = 90;

/// Called from the network timer tick (every second or so). Walks all tunnels
/// and issues keepalive packets or triggers rekeying as needed.
pub fn on_timer_tick(now_secs: u64) {
    let guard = REG.lock();
    let Some(reg) = guard.as_ref() else {
        return;
    };

    for tunnel in reg.tunnels.values() {
        // Only service tunnels that have completed at least one handshake.
        if tunnel.last_handshake_tsc == 0 {
            continue;
        }
        // Use now_secs modulo as a simple periodic trigger.
        // (In a full implementation, track session_started_at_secs per tunnel.)
        let age = now_secs; // placeholder: treat all tunnels as having been up since boot

        // Keepalive: send empty transport packet to keep NAT mapping alive.
        if age % KEEPALIVE_SECS == 0 && age > 0 {
            crate::serial_println!("[wireguard] tunnel #{} keepalive at {}s", tunnel.id, age);
            // TODO: actually send empty transport packet via the UDP/net path.
            // MasterChecklist Phase 10: raevpn daemon wraps this.
        }

        // Rekey: initiate new handshake before the session expires.
        if age >= REKEY_AFTER_SECS {
            crate::serial_println!(
                "[wireguard] tunnel #{} REKEY triggered at {}s (REKEY_AFTER_TIME={}s)",
                tunnel.id,
                age,
                REKEY_AFTER_SECS
            );
            // MasterChecklist Phase 10: drive_initiation() starts the new handshake.
            // In a full impl: call drive_initiation(tunnel.id) and send the result.
        }
    }
}

/// Wire on_timer_tick into the network poll loop.
/// Called from `net::poll_full()` with the current wall-clock second.
pub fn tick(now_secs: u64) {
    on_timer_tick(now_secs);
}
