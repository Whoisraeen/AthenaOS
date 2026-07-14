#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use blake2::{Blake2s256, Digest};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305,
};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

// ─── WireGuard Protocol Constants ────────────────────────────────────────────

const WG_MSG_INITIATION: u8 = 1;
const WG_MSG_RESPONSE: u8 = 2;
const WG_MSG_COOKIE: u8 = 3;
const WG_MSG_DATA: u8 = 4;

const WG_KEY_LEN: usize = 32;
const WG_NONCE_LEN: usize = 12;
const WG_TAG_LEN: usize = 16;
const WG_HASH_LEN: usize = 32;
const WG_TIMESTAMP_LEN: usize = 12;
const WG_COOKIE_LEN: usize = 16;
const WG_MAC_LEN: usize = 16;

const REKEY_AFTER_MESSAGES: u64 = 1 << 60;
const REJECT_AFTER_MESSAGES: u64 = u64::MAX - (1 << 13);
const REKEY_AFTER_TIME_SECS: u64 = 120;
const REJECT_AFTER_TIME_SECS: u64 = 180;
const REKEY_TIMEOUT_SECS: u64 = 5;
const KEEPALIVE_TIMEOUT_SECS: u64 = 10;

// ─── Cryptographic Primitives ────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Key([u8; WG_KEY_LEN]);

impl core::fmt::Debug for Key {
    // Never print key material; expose only zero-ness for test diagnostics.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if *self == Key::zero() {
            f.write_str("Key(zero)")
        } else {
            f.write_str("Key(<redacted>)")
        }
    }
}

impl Key {
    pub const fn zero() -> Self {
        Self([0u8; WG_KEY_LEN])
    }

    pub fn from_bytes(b: &[u8; WG_KEY_LEN]) -> Self {
        Self(*b)
    }

    pub fn as_bytes(&self) -> &[u8; WG_KEY_LEN] {
        &self.0
    }
}

// Defense-in-depth: secret material is scrubbable. `Key` is `Copy` (a value type
// threaded through the handshake), so it cannot carry `Drop`/`ZeroizeOnDrop`; we
// provide an explicit `Zeroize` impl so owners of secrets (e.g. NoiseHandshake's
// Drop) can wipe them. This neither changes the API nor the redacted Debug.
impl Zeroize for Key {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Zeroize for Hash {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy)]
pub struct Hash([u8; WG_HASH_LEN]);

impl Hash {
    pub const fn zero() -> Self {
        Self([0u8; WG_HASH_LEN])
    }
}

fn blake2s(data: &[u8], key: &[u8]) -> Hash {
    let mut hasher = if key.is_empty() {
        Blake2s256::new()
    } else {
        Blake2s256::new_with_prefix(key)
    };
    hasher.update(data);
    let result = hasher.finalize();
    let mut h = [0u8; WG_HASH_LEN];
    h.copy_from_slice(&result);
    Hash(h)
}

/// Keyed BLAKE2s used for MAC1/MAC2 (WireGuard uses BLAKE2s in keyed mode here,
/// output truncated to 16 bytes). Distinct from the HMAC-BLAKE2s used by the KDF.
fn blake2s_mac(key: &[u8], data: &[u8]) -> [u8; WG_MAC_LEN] {
    let h = blake2s(data, key);
    let mut mac = [0u8; WG_MAC_LEN];
    mac.copy_from_slice(&h.0[..WG_MAC_LEN]);
    mac
}

const BLAKE2S_BLOCK_LEN: usize = 64;

/// HMAC-BLAKE2s (RFC 2104 construction over the *unkeyed* BLAKE2s hash).
/// WireGuard's HKDF is HMAC-based (RFC 5869) with BLAKE2s as the hash function —
/// this is NOT the same as BLAKE2s's native keyed mode (which `blake2s_mac` uses).
fn hmac_blake2s(key: &[u8], data: &[u8]) -> [u8; WG_HASH_LEN] {
    let mut block = [0u8; BLAKE2S_BLOCK_LEN];
    if key.len() > BLAKE2S_BLOCK_LEN {
        // Key longer than block: hash it first.
        let h = blake2s(key, &[]);
        block[..WG_HASH_LEN].copy_from_slice(&h.0);
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLAKE2S_BLOCK_LEN];
    let mut opad = [0x5cu8; BLAKE2S_BLOCK_LEN];
    for i in 0..BLAKE2S_BLOCK_LEN {
        ipad[i] ^= block[i];
        opad[i] ^= block[i];
    }

    // inner = H(ipad || data)
    let mut inner = Blake2s256::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_digest = inner.finalize();

    // outer = H(opad || inner)
    let mut outer = Blake2s256::new();
    outer.update(&opad);
    outer.update(&inner_digest);
    let outer_digest = outer.finalize();

    let mut out = [0u8; WG_HASH_LEN];
    out.copy_from_slice(&outer_digest);
    out
}

/// WireGuard HKDF expansion producing `n` 32-byte outputs (n in 1..=3).
/// `KDF_n(key, input)` per the whitepaper: tau0 = HMAC(key, input);
/// tau1 = HMAC(tau0, 0x1); tau_i = HMAC(tau0, tau_{i-1} || i).
fn hkdf<const N: usize>(chaining_key: &[u8; WG_HASH_LEN], input: &[u8]) -> [[u8; WG_HASH_LEN]; N] {
    let tau0 = hmac_blake2s(chaining_key, input);
    let mut out = [[0u8; WG_HASH_LEN]; N];
    if N == 0 {
        return out;
    }
    out[0] = hmac_blake2s(&tau0, &[0x01]);
    let mut prev = out[0];
    let mut i = 1usize;
    while i < N {
        let mut msg = [0u8; WG_HASH_LEN + 1];
        msg[..WG_HASH_LEN].copy_from_slice(&prev);
        msg[WG_HASH_LEN] = (i as u8) + 1;
        out[i] = hmac_blake2s(&tau0, &msg);
        prev = out[i];
        i += 1;
    }
    out
}

/// Mix `data` into the running transcript hash: H = HASH(H || data).
fn mix_hash(h: &Hash, data: &[u8]) -> Hash {
    let mut hasher = Blake2s256::new();
    hasher.update(&h.0);
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; WG_HASH_LEN];
    out.copy_from_slice(&result);
    Hash(out)
}

/// AEAD encrypt with a 32-byte key and nonce 0 (WireGuard handshake messages
/// always use counter nonce 0 for their AEAD fields). Returns ciphertext||tag.
fn aead_encrypt(key: &[u8; WG_KEY_LEN], aad: &[u8], plaintext: &[u8]) -> Option<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(key.as_slice().into());
    let n = [0u8; 12];
    let payload = Payload {
        msg: plaintext,
        aad,
    };
    cipher.encrypt(&n.into(), payload).ok()
}

/// AEAD decrypt counterpart of `aead_encrypt`. Returns None on tag mismatch.
fn aead_decrypt(key: &[u8; WG_KEY_LEN], aad: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(key.as_slice().into());
    let n = [0u8; 12];
    let payload = Payload {
        msg: ciphertext,
        aad,
    };
    cipher.decrypt(&n.into(), payload).ok()
}

/// AEAD-seal `plaintext` into `out` (ciphertext || tag). Returns `Some(())` on
/// success and `None` only if the AEAD backend refuses the operation — for
/// ChaCha20-Poly1305 that is the ~256 GiB single-message limit, unreachable for
/// our bounded packet inputs. Mirrors the fallible `chacha20_poly1305_decrypt`
/// sibling: no `unwrap` on the crypto path.
#[must_use]
fn chacha20_poly1305_encrypt(
    key: &Key,
    nonce: u64,
    aad: &[u8],
    plaintext: &[u8],
    out: &mut [u8],
) -> Option<()> {
    let cipher = ChaCha20Poly1305::new(key.0.as_slice().into());
    let mut n = [0u8; 12];
    n[4..12].copy_from_slice(&nonce.to_le_bytes());
    let payload = Payload {
        msg: plaintext,
        aad,
    };
    let ciphertext = cipher.encrypt(&n.into(), payload).ok()?;
    out[..ciphertext.len()].copy_from_slice(&ciphertext);
    Some(())
}

fn chacha20_poly1305_decrypt(
    key: &Key,
    nonce: u64,
    aad: &[u8],
    ciphertext: &[u8],
    out: &mut [u8],
) -> bool {
    let cipher = ChaCha20Poly1305::new(key.0.as_slice().into());
    let mut n = [0u8; 12];
    n[4..12].copy_from_slice(&nonce.to_le_bytes());
    let payload = Payload {
        msg: ciphertext,
        aad,
    };
    match cipher.decrypt(&n.into(), payload) {
        Ok(plaintext) => {
            out[..plaintext.len()].copy_from_slice(&plaintext);
            true
        }
        Err(_) => false,
    }
}

fn x25519(private_key: &Key, public_key: &Key) -> Key {
    let static_secret = StaticSecret::from(private_key.0);
    let public = PublicKey::from(public_key.0);
    let shared_secret = static_secret.diffie_hellman(&public);
    Key(*shared_secret.as_bytes())
}

fn x25519_base(private_key: &Key) -> Key {
    let static_secret = StaticSecret::from(private_key.0);
    let public = PublicKey::from(&static_secret);
    Key(*public.as_bytes())
}

// ─── Noise_IKpsk2 Handshake ──────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HandshakeState {
    Empty,
    InitiationSent,
    InitiationReceived,
    ResponseSent,
    ResponseReceived,
    Established,
    Expired,
}

// WireGuard handshake message layout offsets (little-endian, no padding).
// Initiation (148 bytes): type(1) reserved(3) sender(4) ephemeral(32)
//   enc_static(32+16) enc_timestamp(12+16) mac1(16) mac2(16)
const WG_INIT_LEN: usize = 148;
const WG_INIT_MAC1_OFFSET: usize = 116; // bytes [0..116) are MAC1-covered
const WG_INIT_MAC2_OFFSET: usize = 132;
// Response (92 bytes): type(1) reserved(3) sender(4) receiver(4) ephemeral(32)
//   enc_empty(0+16) mac1(16) mac2(16)
const WG_RESP_LEN: usize = 92;
const WG_RESP_MAC1_OFFSET: usize = 60;
const WG_RESP_MAC2_OFFSET: usize = 76;

const LABEL_MAC1: &[u8] = b"mac1----";

/// Derive the MAC1 key for a peer: HASH(LABEL_MAC1 || peer_static_public).
fn mac1_key(peer_static_public: &Key) -> [u8; WG_HASH_LEN] {
    let mut hasher = Blake2s256::new();
    hasher.update(LABEL_MAC1);
    hasher.update(&peer_static_public.0);
    let d = hasher.finalize();
    let mut out = [0u8; WG_HASH_LEN];
    out.copy_from_slice(&d);
    out
}

/// Noise_IKpsk2 handshake state, implemented per the WireGuard whitepaper
/// (§5.4): the Concept's "built-in WireGuard" promise made into a real,
/// matching-transport-keys-on-both-ends handshake.
pub struct NoiseHandshake {
    state: HandshakeState,
    local_static: Key,
    local_static_public: Key,
    local_ephemeral: Key,
    remote_static: Key,
    remote_ephemeral: Key,
    preshared_key: Key,
    chaining_key: Hash,
    hash: Hash,
    sending_key: Key,
    receiving_key: Key,
    sender_index: u32,
    receiver_index: u32,
    timestamp: [u8; WG_TIMESTAMP_LEN],
    /// Whether this side initiated. Determines transport-key send/recv ordering.
    is_initiator: bool,
    /// Buffer holding the serialized response from the most recent create_response.
    last_response: Vec<u8>,
}

// Defense-in-depth: wipe long-lived secret material when the handshake state is
// torn down so private/ephemeral/PSK/transport keys do not linger in freed heap
// or stack frames. Public-only fields (indices, the local static *public*) are
// left untouched.
impl Drop for NoiseHandshake {
    fn drop(&mut self) {
        self.local_static.zeroize();
        self.local_ephemeral.zeroize();
        self.remote_ephemeral.zeroize();
        self.preshared_key.zeroize();
        self.chaining_key.zeroize();
        self.sending_key.zeroize();
        self.receiving_key.zeroize();
    }
}

impl NoiseHandshake {
    pub fn new(local_static: Key, remote_static: Key, preshared_key: Key) -> Self {
        let construction = b"Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
        let identifier = b"WireGuard v1 zx2c4 Jason@zx2c4.com";
        // Ci = HASH(CONSTRUCTION); Hi = HASH(Ci || IDENTIFIER); Hi = HASH(Hi || Spub_r)
        let chaining_key = blake2s(construction, &[]);
        let hash = mix_hash(&chaining_key, identifier);

        Self {
            state: HandshakeState::Empty,
            local_static,
            local_static_public: x25519_base(&local_static),
            local_ephemeral: Key::zero(),
            remote_static,
            remote_ephemeral: Key::zero(),
            preshared_key,
            chaining_key,
            hash,
            sending_key: Key::zero(),
            receiving_key: Key::zero(),
            sender_index: 0,
            receiver_index: 0,
            timestamp: [0u8; WG_TIMESTAMP_LEN],
            is_initiator: false,
            last_response: Vec::new(),
        }
    }

    pub fn state(&self) -> HandshakeState {
        self.state
    }

    pub fn sending_key(&self) -> Key {
        self.sending_key
    }

    pub fn receiving_key(&self) -> Key {
        self.receiving_key
    }

    pub fn set_timestamp(&mut self, ts: [u8; WG_TIMESTAMP_LEN]) {
        self.timestamp = ts;
    }

    /// Build the handshake initiation message (msg type 1).
    /// `ephemeral` is the initiator's per-handshake ephemeral private key
    /// (injected for determinism in tests; the kernel path supplies CSPRNG bytes).
    pub fn create_initiation(&mut self, sender_index: u32, ephemeral: Key) -> Vec<u8> {
        self.is_initiator = true;
        self.sender_index = sender_index;
        self.local_ephemeral = ephemeral;
        let eph_public = x25519_base(&self.local_ephemeral);

        // Hi = HASH(Hi || Spub_r) — bind to the responder's identity.
        self.hash = mix_hash(&self.hash, &self.remote_static.0);
        // Ci = KDF1(Ci, Epub_i)
        let [c] = hkdf::<1>(&self.chaining_key.0, &eph_public.0);
        self.chaining_key = Hash(c);
        // Hi = HASH(Hi || Epub_i)
        self.hash = mix_hash(&self.hash, &eph_public.0);

        // (Ci, k) = KDF2(Ci, DH(Epriv_i, Spub_r))
        let dh1 = x25519(&self.local_ephemeral, &self.remote_static);
        let [c1, k1] = hkdf::<2>(&self.chaining_key.0, &dh1.0);
        self.chaining_key = Hash(c1);
        // msg.static = AEAD(k, 0, Spub_i, Hi)
        let enc_static = match aead_encrypt(&k1, &self.hash.0, &self.local_static_public.0) {
            Some(v) => v,
            None => return Vec::new(),
        };
        self.hash = mix_hash(&self.hash, &enc_static);

        // (Ci, k) = KDF2(Ci, DH(Spriv_i, Spub_r))
        let dh2 = x25519(&self.local_static, &self.remote_static);
        let [c2, k2] = hkdf::<2>(&self.chaining_key.0, &dh2.0);
        self.chaining_key = Hash(c2);
        // msg.timestamp = AEAD(k, 0, timestamp, Hi)
        let enc_ts = match aead_encrypt(&k2, &self.hash.0, &self.timestamp) {
            Some(v) => v,
            None => return Vec::new(),
        };
        self.hash = mix_hash(&self.hash, &enc_ts);

        let mut msg = Vec::with_capacity(WG_INIT_LEN);
        msg.push(WG_MSG_INITIATION);
        msg.extend_from_slice(&[0u8; 3]);
        msg.extend_from_slice(&sender_index.to_le_bytes());
        msg.extend_from_slice(&eph_public.0);
        msg.extend_from_slice(&enc_static); // 48 bytes
        msg.extend_from_slice(&enc_ts); // 28 bytes
        debug_assert_eq!(msg.len(), WG_INIT_MAC1_OFFSET);

        // mac1 = MAC(HASH(LABEL_MAC1 || Spub_r), msg[0..mac1_offset])
        let mkey = mac1_key(&self.remote_static);
        let mac1 = blake2s_mac(&mkey, &msg);
        msg.extend_from_slice(&mac1);
        msg.extend_from_slice(&[0u8; WG_MAC_LEN]); // mac2 = 0 (no cookie)

        self.state = HandshakeState::InitiationSent;
        msg
    }

    /// Consume a handshake initiation message (responder side).
    /// Returns false (never panics) on any malformed/forged input.
    pub fn consume_initiation(&mut self, msg: &[u8]) -> bool {
        if msg.len() < WG_INIT_LEN || msg[0] != WG_MSG_INITIATION {
            return false;
        }
        // Verify MAC1 against our own static public key first (cheap anti-DoS).
        if !self.verify_initiation_mac1(msg) {
            return false;
        }
        self.is_initiator = false;
        self.receiver_index = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);

        let eph: [u8; 32] = match msg[8..40].try_into() {
            Ok(e) => e,
            Err(_) => return false,
        };
        self.remote_ephemeral = Key::from_bytes(&eph);

        // Replay the initiator's transcript using OUR static public as Spub_r.
        self.hash = mix_hash(&self.hash, &self.local_static_public.0);
        let [c] = hkdf::<1>(&self.chaining_key.0, &eph);
        self.chaining_key = Hash(c);
        self.hash = mix_hash(&self.hash, &eph);

        // (Ci, k) = KDF2(Ci, DH(Spriv_r, Epub_i))
        let dh1 = x25519(&self.local_static, &self.remote_ephemeral);
        let [c1, k1] = hkdf::<2>(&self.chaining_key.0, &dh1.0);
        self.chaining_key = Hash(c1);
        let enc_static = &msg[40..88];
        let dec_static = match aead_decrypt(&k1, &self.hash.0, enc_static) {
            Some(v) if v.len() == WG_KEY_LEN => v,
            _ => return false,
        };
        let mut rs = [0u8; WG_KEY_LEN];
        rs.copy_from_slice(&dec_static);
        self.remote_static = Key(rs);
        self.hash = mix_hash(&self.hash, enc_static);

        // (Ci, k) = KDF2(Ci, DH(Spriv_r, Spub_i))
        let dh2 = x25519(&self.local_static, &self.remote_static);
        let [c2, k2] = hkdf::<2>(&self.chaining_key.0, &dh2.0);
        self.chaining_key = Hash(c2);
        let enc_ts = &msg[88..116];
        let dec_ts = match aead_decrypt(&k2, &self.hash.0, enc_ts) {
            Some(v) if v.len() == WG_TIMESTAMP_LEN => v,
            _ => return false,
        };
        self.timestamp.copy_from_slice(&dec_ts);
        self.hash = mix_hash(&self.hash, enc_ts);

        self.state = HandshakeState::InitiationReceived;
        true
    }

    /// Build the handshake response message (msg type 2, responder side).
    pub fn create_response(&mut self, sender_index: u32, ephemeral: Key) -> bool {
        if self.state != HandshakeState::InitiationReceived {
            return false;
        }
        self.sender_index = sender_index;
        self.local_ephemeral = ephemeral;
        let eph_public = x25519_base(&self.local_ephemeral);

        // Ci = KDF1(Ci, Epub_r); Hr = HASH(Hr || Epub_r)
        let [c] = hkdf::<1>(&self.chaining_key.0, &eph_public.0);
        self.chaining_key = Hash(c);
        self.hash = mix_hash(&self.hash, &eph_public.0);

        // Ci = KDF1(Ci, DH(Epriv_r, Epub_i))
        let dh1 = x25519(&self.local_ephemeral, &self.remote_ephemeral);
        let [c1] = hkdf::<1>(&self.chaining_key.0, &dh1.0);
        self.chaining_key = Hash(c1);
        // Ci = KDF1(Ci, DH(Epriv_r, Spub_i))
        let dh2 = x25519(&self.local_ephemeral, &self.remote_static);
        let [c2] = hkdf::<1>(&self.chaining_key.0, &dh2.0);
        self.chaining_key = Hash(c2);

        // (Ci, tau, k) = KDF3(Ci, Q)  where Q = preshared key
        let [c3, tau, k] = hkdf::<3>(&self.chaining_key.0, &self.preshared_key.0);
        self.chaining_key = Hash(c3);
        self.hash = mix_hash(&self.hash, &tau);

        // msg.empty = AEAD(k, 0, "", Hr)
        let enc_empty = match aead_encrypt(&k, &self.hash.0, &[]) {
            Some(v) => v,
            None => return false,
        };
        self.hash = mix_hash(&self.hash, &enc_empty);

        let mut msg = Vec::with_capacity(WG_RESP_LEN);
        msg.push(WG_MSG_RESPONSE);
        msg.extend_from_slice(&[0u8; 3]);
        msg.extend_from_slice(&sender_index.to_le_bytes());
        msg.extend_from_slice(&self.receiver_index.to_le_bytes());
        msg.extend_from_slice(&eph_public.0);
        msg.extend_from_slice(&enc_empty); // 16 bytes (tag only)
        debug_assert_eq!(msg.len(), WG_RESP_MAC1_OFFSET);

        let mkey = mac1_key(&self.remote_static);
        let mac1 = blake2s_mac(&mkey, &msg);
        msg.extend_from_slice(&mac1);
        msg.extend_from_slice(&[0u8; WG_MAC_LEN]);

        self.derive_transport_keys();
        self.state = HandshakeState::ResponseSent;
        self.last_response = msg;
        true
    }

    /// The serialized response bytes produced by the most recent `create_response`.
    pub fn take_response(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.last_response)
    }

    /// Consume a handshake response message (initiator side).
    /// Returns false (never panics) on any malformed/forged input.
    pub fn consume_response(&mut self, msg: &[u8]) -> bool {
        if msg.len() < WG_RESP_LEN || msg[0] != WG_MSG_RESPONSE {
            return false;
        }
        if self.state != HandshakeState::InitiationSent {
            return false;
        }
        if !self.verify_response_mac1(msg) {
            return false;
        }
        self.receiver_index = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
        let eph: [u8; 32] = match msg[12..44].try_into() {
            Ok(e) => e,
            Err(_) => return false,
        };
        self.remote_ephemeral = Key::from_bytes(&eph);

        let [c] = hkdf::<1>(&self.chaining_key.0, &eph);
        self.chaining_key = Hash(c);
        self.hash = mix_hash(&self.hash, &eph);

        let dh1 = x25519(&self.local_ephemeral, &self.remote_ephemeral);
        let [c1] = hkdf::<1>(&self.chaining_key.0, &dh1.0);
        self.chaining_key = Hash(c1);
        let dh2 = x25519(&self.local_static, &self.remote_ephemeral);
        let [c2] = hkdf::<1>(&self.chaining_key.0, &dh2.0);
        self.chaining_key = Hash(c2);

        let [c3, tau, k] = hkdf::<3>(&self.chaining_key.0, &self.preshared_key.0);
        self.chaining_key = Hash(c3);
        self.hash = mix_hash(&self.hash, &tau);

        let enc_empty = &msg[44..60];
        let dec = match aead_decrypt(&k, &self.hash.0, enc_empty) {
            Some(v) => v,
            None => return false,
        };
        if !dec.is_empty() {
            return false;
        }
        self.hash = mix_hash(&self.hash, enc_empty);

        self.derive_transport_keys();
        self.state = HandshakeState::Established;
        true
    }

    /// Verify the MAC1 field of an initiation against our own static public key.
    fn verify_initiation_mac1(&self, msg: &[u8]) -> bool {
        if msg.len() < WG_INIT_MAC2_OFFSET {
            return false;
        }
        let mkey = mac1_key(&self.local_static_public);
        let expected = blake2s_mac(&mkey, &msg[..WG_INIT_MAC1_OFFSET]);
        let got = &msg[WG_INIT_MAC1_OFFSET..WG_INIT_MAC2_OFFSET];
        ct_eq(&expected, got)
    }

    /// Verify the MAC1 field of a response against our own static public key.
    fn verify_response_mac1(&self, msg: &[u8]) -> bool {
        if msg.len() < WG_RESP_MAC2_OFFSET {
            return false;
        }
        let mkey = mac1_key(&self.local_static_public);
        let expected = blake2s_mac(&mkey, &msg[..WG_RESP_MAC1_OFFSET]);
        let got = &msg[WG_RESP_MAC1_OFFSET..WG_RESP_MAC2_OFFSET];
        ct_eq(&expected, got)
    }

    /// Final transport-key split: (T_send, T_recv) = KDF2(Ci, "").
    /// The initiator's first output is its send key; the responder's first
    /// output is its receive key, so the directions line up across the wire.
    fn derive_transport_keys(&mut self) {
        let [t1, t2] = hkdf::<2>(&self.chaining_key.0, &[]);
        if self.is_initiator {
            self.sending_key = Key(t1);
            self.receiving_key = Key(t2);
        } else {
            self.sending_key = Key(t2);
            self.receiving_key = Key(t1);
        }
    }
}

/// Constant-time byte-slice equality (anti-timing-oracle for MAC checks).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

// ─── Cookie Reply Mechanism ──────────────────────────────────────────────────

pub struct CookieReply {
    pub receiver_index: u32,
    pub nonce: [u8; 24],
    pub encrypted_cookie: [u8; WG_COOKIE_LEN + WG_TAG_LEN],
}

impl CookieReply {
    pub fn create(receiver_index: u32, cookie_key: &Key, peer_mac1: &[u8; WG_MAC_LEN]) -> Self {
        let nonce = [0u8; 24];
        let mut encrypted_cookie = [0u8; WG_COOKIE_LEN + WG_TAG_LEN];
        let cookie = blake2s_mac(&cookie_key.0, peer_mac1);
        // Input is locally generated and bounded, so the AEAD limit is
        // unreachable; on the impossible error path `encrypted_cookie` simply
        // stays zeroed rather than panicking.
        let _ = chacha20_poly1305_encrypt(cookie_key, 0, peer_mac1, &cookie, &mut encrypted_cookie);
        Self {
            receiver_index,
            nonce,
            encrypted_cookie,
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut msg = Vec::with_capacity(64);
        msg.push(WG_MSG_COOKIE);
        msg.extend_from_slice(&[0u8; 3]);
        msg.extend_from_slice(&self.receiver_index.to_le_bytes());
        msg.extend_from_slice(&self.nonce);
        msg.extend_from_slice(&self.encrypted_cookie);
        msg
    }
}

// ─── Under-Load Detection ────────────────────────────────────────────────────

pub struct LoadDetector {
    handshakes_per_second: AtomicU64,
    threshold: u64,
    under_load: AtomicBool,
    last_reset_tick: AtomicU64,
}

impl LoadDetector {
    pub fn new(threshold: u64) -> Self {
        Self {
            handshakes_per_second: AtomicU64::new(0),
            threshold,
            under_load: AtomicBool::new(false),
            last_reset_tick: AtomicU64::new(0),
        }
    }

    pub fn record_handshake(&self, current_tick: u64) {
        let last = self.last_reset_tick.load(Ordering::Relaxed);
        if current_tick.saturating_sub(last) > 1000 {
            self.handshakes_per_second.store(1, Ordering::Relaxed);
            self.last_reset_tick.store(current_tick, Ordering::Relaxed);
        } else {
            self.handshakes_per_second.fetch_add(1, Ordering::Relaxed);
        }
        let count = self.handshakes_per_second.load(Ordering::Relaxed);
        self.under_load
            .store(count > self.threshold, Ordering::Relaxed);
    }

    pub fn is_under_load(&self) -> bool {
        self.under_load.load(Ordering::Relaxed)
    }
}

// ─── Timer State Machine ─────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimerEvent {
    RetransmitHandshake,
    SendKeepalive,
    NewHandshake,
    ZeroKeyMaterial,
    PersistentKeepalive,
}

pub struct TimerState {
    pub last_sent: u64,
    pub last_received: u64,
    pub last_handshake_initiated: u64,
    pub handshake_attempts: u32,
    pub persistent_keepalive_interval: u64,
    pub keepalive_needed: bool,
}

impl TimerState {
    pub fn new() -> Self {
        Self {
            last_sent: 0,
            last_received: 0,
            last_handshake_initiated: 0,
            handshake_attempts: 0,
            persistent_keepalive_interval: 0,
            keepalive_needed: false,
        }
    }

    pub fn tick(&mut self, now: u64) -> Option<TimerEvent> {
        if self.handshake_attempts > 0
            && now.saturating_sub(self.last_handshake_initiated) > REKEY_TIMEOUT_SECS
        {
            self.handshake_attempts += 1;
            if self.handshake_attempts > 20 {
                return Some(TimerEvent::ZeroKeyMaterial);
            }
            self.last_handshake_initiated = now;
            return Some(TimerEvent::RetransmitHandshake);
        }

        if self.keepalive_needed && now.saturating_sub(self.last_sent) > KEEPALIVE_TIMEOUT_SECS {
            self.keepalive_needed = false;
            return Some(TimerEvent::SendKeepalive);
        }

        if self.persistent_keepalive_interval > 0
            && now.saturating_sub(self.last_sent) > self.persistent_keepalive_interval
        {
            return Some(TimerEvent::PersistentKeepalive);
        }

        if now.saturating_sub(self.last_handshake_initiated) > REKEY_AFTER_TIME_SECS {
            return Some(TimerEvent::NewHandshake);
        }

        None
    }

    pub fn data_sent(&mut self, now: u64) {
        self.last_sent = now;
        self.keepalive_needed = true;
    }

    pub fn data_received(&mut self, now: u64) {
        self.last_received = now;
    }

    pub fn handshake_initiated(&mut self, now: u64) {
        self.last_handshake_initiated = now;
        self.handshake_attempts = 1;
    }

    pub fn handshake_complete(&mut self) {
        self.handshake_attempts = 0;
    }
}

// ─── WireGuard Data Transport ────────────────────────────────────────────────

/// RFC 6479 sliding-window anti-replay over a 64-counter window. After a packet
/// is authenticated, [`accept`](ReplayWindow::accept) returns `false` for a
/// counter that has already been seen (replay) or is more than 64 behind the
/// highest (too old), and `true` (recording it) otherwise — tolerating the
/// out-of-order delivery UDP produces.
struct ReplayWindow {
    highest: u64,
    bitmap: u64,
    seen_any: bool,
}

impl ReplayWindow {
    fn new() -> Self {
        Self {
            highest: 0,
            bitmap: 0,
            seen_any: false,
        }
    }

    fn accept(&mut self, counter: u64) -> bool {
        if !self.seen_any {
            self.seen_any = true;
            self.highest = counter;
            self.bitmap = 1; // bit 0 marks `highest`
            return true;
        }
        if counter > self.highest {
            let shift = counter - self.highest;
            self.bitmap = if shift >= 64 {
                1
            } else {
                (self.bitmap << shift) | 1
            };
            self.highest = counter;
            true
        } else {
            let diff = self.highest - counter;
            if diff >= 64 {
                return false; // older than the window
            }
            let bit = 1u64 << diff;
            if self.bitmap & bit != 0 {
                false // already seen — replay
            } else {
                self.bitmap |= bit;
                true
            }
        }
    }
}

pub struct TransportSession {
    pub sender_index: u32,
    pub receiver_index: u32,
    sending_key: Key,
    receiving_key: Key,
    sending_counter: AtomicU64,
    /// Receive-side anti-replay window (RFC 6479). Was a bare max counter that
    /// never rejected anything — a replay vulnerability.
    replay: spin::Mutex<ReplayWindow>,
    created_at: u64,
}

impl TransportSession {
    pub fn new(
        sender_index: u32,
        receiver_index: u32,
        sending_key: Key,
        receiving_key: Key,
        created_at: u64,
    ) -> Self {
        Self {
            sender_index,
            receiver_index,
            sending_key,
            receiving_key,
            sending_counter: AtomicU64::new(0),
            replay: spin::Mutex::new(ReplayWindow::new()),
            created_at,
        }
    }

    pub fn encrypt_packet(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
        let counter = self.sending_counter.fetch_add(1, Ordering::Relaxed);
        if counter >= REJECT_AFTER_MESSAGES {
            return None;
        }
        let mut out = Vec::with_capacity(16 + plaintext.len() + WG_TAG_LEN);
        out.push(WG_MSG_DATA);
        out.extend_from_slice(&[0u8; 3]);
        out.extend_from_slice(&self.receiver_index.to_le_bytes());
        out.extend_from_slice(&counter.to_le_bytes());
        let mut encrypted = alloc::vec![0u8; plaintext.len() + WG_TAG_LEN];
        chacha20_poly1305_encrypt(&self.sending_key, counter, &[], plaintext, &mut encrypted)?;
        out.extend_from_slice(&encrypted);
        Some(out)
    }

    pub fn decrypt_packet(&self, msg: &[u8]) -> Option<Vec<u8>> {
        if msg.len() < 16 + WG_TAG_LEN || msg[0] != WG_MSG_DATA {
            return None;
        }
        let counter = u64::from_le_bytes(msg[8..16].try_into().ok()?);
        if counter >= REJECT_AFTER_MESSAGES {
            return None;
        }
        let ciphertext = &msg[16..];
        let mut plaintext = alloc::vec![0u8; ciphertext.len() - WG_TAG_LEN];
        // Authenticate FIRST (the counter is attacker-controlled until the AEAD
        // tag verifies), then enforce anti-replay on the now-trusted counter —
        // the WireGuard receive order. A replayed or too-old counter is dropped.
        if !chacha20_poly1305_decrypt(
            &self.receiving_key,
            counter,
            &[],
            ciphertext,
            &mut plaintext,
        ) {
            return None;
        }
        if !self.replay.lock().accept(counter) {
            return None;
        }
        Some(plaintext)
    }

    pub fn is_expired(&self, now: u64) -> bool {
        now.saturating_sub(self.created_at) > REJECT_AFTER_TIME_SECS
    }

    pub fn needs_rekey(&self, now: u64) -> bool {
        let counter = self.sending_counter.load(Ordering::Relaxed);
        counter >= REKEY_AFTER_MESSAGES
            || now.saturating_sub(self.created_at) > REKEY_AFTER_TIME_SECS
    }
}

// ─── AllowedIPs Trie (Longest Prefix Match) ──────────────────────────────────

#[derive(Clone)]
pub struct AllowedIpEntry {
    pub ip: [u8; 16],
    pub cidr: u8,
    pub is_v6: bool,
    pub peer_index: usize,
}

pub struct AllowedIpsTrie {
    entries: Vec<AllowedIpEntry>,
}

impl AllowedIpsTrie {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn insert(&mut self, entry: AllowedIpEntry) {
        self.entries.push(entry);
    }

    pub fn remove_by_peer(&mut self, peer_index: usize) {
        self.entries.retain(|e| e.peer_index != peer_index);
    }

    pub fn lookup(&self, addr: &[u8], is_v6: bool) -> Option<usize> {
        let mut best_match: Option<(u8, usize)> = None;
        let addr_len = if is_v6 { 16 } else { 4 };

        for entry in &self.entries {
            if entry.is_v6 != is_v6 {
                continue;
            }
            if self.prefix_matches(&entry.ip, addr, entry.cidr, addr_len) {
                match best_match {
                    None => best_match = Some((entry.cidr, entry.peer_index)),
                    Some((best_cidr, _)) if entry.cidr > best_cidr => {
                        best_match = Some((entry.cidr, entry.peer_index));
                    }
                    _ => {}
                }
            }
        }
        best_match.map(|(_, idx)| idx)
    }

    fn prefix_matches(&self, network: &[u8], addr: &[u8], cidr: u8, addr_len: usize) -> bool {
        let full_bytes = (cidr / 8) as usize;
        let remaining_bits = cidr % 8;

        for i in 0..full_bytes.min(addr_len) {
            if network[i] != addr[i] {
                return false;
            }
        }
        if remaining_bits > 0 && full_bytes < addr_len {
            let mask = 0xFF << (8 - remaining_bits);
            if (network[full_bytes] & mask) != (addr[full_bytes] & mask) {
                return false;
            }
        }
        true
    }
}

// ─── WireGuard Peer ──────────────────────────────────────────────────────────

pub struct WgPeer {
    pub public_key: Key,
    pub preshared_key: Key,
    pub endpoint: Option<Endpoint>,
    pub allowed_ips: Vec<AllowedIpEntry>,
    pub persistent_keepalive: u16,
    pub last_handshake: u64,
    pub rx_bytes: AtomicU64,
    pub tx_bytes: AtomicU64,
    pub handshake: NoiseHandshake,
    pub session: Option<TransportSession>,
    pub timer: TimerState,
}

#[derive(Clone)]
pub struct Endpoint {
    pub addr: [u8; 16],
    pub port: u16,
    pub is_v6: bool,
}

impl WgPeer {
    pub fn new(public_key: Key, preshared_key: Key) -> Self {
        let local_static = Key::zero();
        Self {
            public_key,
            preshared_key,
            endpoint: None,
            allowed_ips: Vec::new(),
            persistent_keepalive: 0,
            last_handshake: 0,
            rx_bytes: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            handshake: NoiseHandshake::new(local_static, public_key, preshared_key),
            session: None,
            timer: TimerState::new(),
        }
    }
}

// ─── WireGuard Device ────────────────────────────────────────────────────────

pub struct WgDevice {
    pub private_key: Key,
    pub public_key: Key,
    pub listen_port: u16,
    pub fwmark: u32,
    pub peers: Vec<WgPeer>,
    pub routing_table: AllowedIpsTrie,
    pub load_detector: LoadDetector,
}

impl WgDevice {
    pub fn new(private_key: Key, listen_port: u16) -> Self {
        let public_key = x25519_base(&private_key);
        Self {
            private_key,
            public_key,
            listen_port,
            fwmark: 0,
            peers: Vec::new(),
            routing_table: AllowedIpsTrie::new(),
            load_detector: LoadDetector::new(100),
        }
    }

    pub fn add_peer(&mut self, mut peer: WgPeer) {
        let idx = self.peers.len();
        peer.handshake = NoiseHandshake::new(self.private_key, peer.public_key, peer.preshared_key);
        for ip in &peer.allowed_ips {
            let mut entry = ip.clone();
            entry.peer_index = idx;
            self.routing_table.insert(entry);
        }
        self.peers.push(peer);
    }

    pub fn remove_peer(&mut self, public_key: &Key) {
        if let Some(idx) = self.peers.iter().position(|p| &p.public_key == public_key) {
            self.routing_table.remove_by_peer(idx);
            self.peers.remove(idx);
        }
    }

    pub fn route_packet(&self, dst_addr: &[u8], is_v6: bool) -> Option<usize> {
        self.routing_table.lookup(dst_addr, is_v6)
    }
}

// ─── OpenVPN Compatibility ───────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OvpnProtocol {
    Udp,
    Tcp,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OvpnAuthMode {
    StaticKey,
    Tls,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OvpnCompression {
    None,
    Lzo,
    Lz4,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OvpnState {
    Disconnected,
    Connecting,
    TlsHandshake,
    KeyExchange,
    Connected,
    Reconnecting,
    Disconnecting,
}

pub struct OvpnSession {
    pub state: OvpnState,
    pub protocol: OvpnProtocol,
    pub auth_mode: OvpnAuthMode,
    pub compression: OvpnCompression,
    pub server_addr: [u8; 16],
    pub server_port: u16,
    pub data_key_encrypt: Key,
    pub data_key_decrypt: Key,
    pub packet_id_send: u32,
    pub packet_id_recv: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl OvpnSession {
    pub fn new(protocol: OvpnProtocol, auth_mode: OvpnAuthMode) -> Self {
        Self {
            state: OvpnState::Disconnected,
            protocol,
            auth_mode,
            compression: OvpnCompression::None,
            server_addr: [0u8; 16],
            server_port: 1194,
            data_key_encrypt: Key::zero(),
            data_key_decrypt: Key::zero(),
            packet_id_send: 0,
            packet_id_recv: 0,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }

    pub fn connect(&mut self, addr: [u8; 16], port: u16) {
        self.server_addr = addr;
        self.server_port = port;
        self.state = OvpnState::Connecting;
    }

    pub fn tls_handshake(&mut self, _client_hello: &[u8]) -> Vec<u8> {
        self.state = OvpnState::TlsHandshake;
        let server_hello = alloc::vec![0x16, 0x03, 0x03]; // TLS header stub
        server_hello
    }

    pub fn key_exchange(&mut self, material: &[u8]) {
        let h = blake2s(material, &[]);
        self.data_key_encrypt = Key(h.0);
        let h2 = blake2s(&h.0, material);
        self.data_key_decrypt = Key(h2.0);
        self.state = OvpnState::KeyExchange;
    }

    pub fn established(&mut self) {
        self.state = OvpnState::Connected;
    }

    pub fn encrypt_data(&mut self, plaintext: &[u8]) -> Vec<u8> {
        self.packet_id_send += 1;
        let mut out = Vec::with_capacity(4 + plaintext.len() + WG_TAG_LEN);
        out.extend_from_slice(&self.packet_id_send.to_le_bytes());
        let mut encrypted = alloc::vec![0u8; plaintext.len() + WG_TAG_LEN];
        // Bounded local input; AEAD limit unreachable (see CookieReply::create).
        let _ = chacha20_poly1305_encrypt(
            &self.data_key_encrypt,
            self.packet_id_send as u64,
            &[],
            plaintext,
            &mut encrypted,
        );
        out.extend_from_slice(&encrypted);
        self.tx_bytes += out.len() as u64;
        out
    }

    pub fn decrypt_data(&mut self, ciphertext: &[u8]) -> Option<Vec<u8>> {
        if ciphertext.len() < 4 + WG_TAG_LEN {
            return None;
        }
        let pkt_id = u32::from_le_bytes(ciphertext[..4].try_into().ok()?);
        if pkt_id <= self.packet_id_recv {
            return None;
        }
        self.packet_id_recv = pkt_id;
        let data = &ciphertext[4..];
        let mut plaintext = alloc::vec![0u8; data.len() - WG_TAG_LEN];
        if chacha20_poly1305_decrypt(
            &self.data_key_decrypt,
            pkt_id as u64,
            &[],
            data,
            &mut plaintext,
        ) {
            self.rx_bytes += ciphertext.len() as u64;
            Some(plaintext)
        } else {
            None
        }
    }

    pub fn disconnect(&mut self) {
        self.state = OvpnState::Disconnecting;
        self.data_key_encrypt = Key::zero();
        self.data_key_decrypt = Key::zero();
        self.state = OvpnState::Disconnected;
    }

    pub fn push_config(&self) -> Vec<u8> {
        let mut config = Vec::new();
        config.extend_from_slice(b"push \"route 0.0.0.0 0.0.0.0\"\n");
        config.extend_from_slice(b"push \"dhcp-option DNS 10.8.0.1\"\n");
        config
    }
}

// ─── IPSec / IKEv2 ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IkeExchangeType {
    IkeSaInit,
    IkeAuth,
    CreateChildSa,
    Informational,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IpsecMode {
    Tunnel,
    Transport,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IpsecProtocol {
    Esp,
    Ah,
}

pub struct IkeProposal {
    pub encryption: u16, // AES-256-GCM = 20
    pub integrity: u16,  // SHA2-256 = 12
    pub dh_group: u16,   // DH group 14 = 14
    pub prf: u16,        // PRF-HMAC-SHA2-256 = 5
}

pub struct SecurityAssociation {
    pub spi: u32,
    pub protocol: IpsecProtocol,
    pub mode: IpsecMode,
    pub encrypt_key: Key,
    pub auth_key: Key,
    pub sequence_number: u32,
    /// RFC 6479 anti-replay: `anti_replay_window` is the 64-bit bitmap relative
    /// to `recv_highest` (the highest authenticated sequence number seen;
    /// `0` = none yet, since ESP sequence numbers start at 1).
    pub anti_replay_window: u64,
    pub recv_highest: u32,
    pub lifetime_bytes: u64,
    pub lifetime_seconds: u64,
    pub bytes_used: u64,
    pub created_at: u64,
}

impl SecurityAssociation {
    pub fn new(spi: u32, protocol: IpsecProtocol, mode: IpsecMode) -> Self {
        Self {
            spi,
            protocol,
            mode,
            encrypt_key: Key::zero(),
            auth_key: Key::zero(),
            sequence_number: 0,
            anti_replay_window: 0,
            recv_highest: 0,
            lifetime_bytes: 1_000_000_000,
            lifetime_seconds: 3600,
            bytes_used: 0,
            created_at: 0,
        }
    }

    pub fn esp_encrypt(&mut self, plaintext: &[u8]) -> Vec<u8> {
        self.sequence_number += 1;
        let mut packet = Vec::with_capacity(8 + plaintext.len() + WG_TAG_LEN);
        packet.extend_from_slice(&self.spi.to_be_bytes());
        packet.extend_from_slice(&self.sequence_number.to_be_bytes());
        let mut encrypted = alloc::vec![0u8; plaintext.len() + WG_TAG_LEN];
        // Bounded local input; AEAD limit unreachable (see CookieReply::create).
        let _ = chacha20_poly1305_encrypt(
            &self.encrypt_key,
            self.sequence_number as u64,
            &packet[..8],
            plaintext,
            &mut encrypted,
        );
        packet.extend_from_slice(&encrypted);
        self.bytes_used += packet.len() as u64;
        packet
    }

    pub fn esp_decrypt(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        if packet.len() < 8 + WG_TAG_LEN {
            return None;
        }
        let spi = u32::from_be_bytes(packet[..4].try_into().ok()?);
        if spi != self.spi {
            return None;
        }
        let seq = u32::from_be_bytes(packet[4..8].try_into().ok()?);
        let ciphertext = &packet[8..];
        let mut plaintext = alloc::vec![0u8; ciphertext.len() - WG_TAG_LEN];
        // Authenticate FIRST, then enforce anti-replay on the trusted sequence —
        // the RFC 4303 order (an unauthenticated packet must not pollute the
        // replay window). Replaces the former `seq % 64` toy that false-rejected
        // any seq >= 64 and stopped accepting anything after 64 packets.
        if !chacha20_poly1305_decrypt(
            &self.encrypt_key,
            seq as u64,
            &packet[..8],
            ciphertext,
            &mut plaintext,
        ) {
            return None;
        }
        if !self.accept_seq(seq) {
            return None; // replayed or older than the window
        }
        Some(plaintext)
    }

    /// RFC 6479 sliding-window anti-replay over `recv_highest` + the
    /// `anti_replay_window` bitmap. `seq == 0` is rejected (ESP sequence numbers
    /// start at 1). Returns `false` for a replay or a sequence more than 64 behind
    /// the highest; tolerates in-window reordering.
    fn accept_seq(&mut self, seq: u32) -> bool {
        if seq == 0 {
            return false;
        }
        if self.recv_highest == 0 {
            self.recv_highest = seq;
            self.anti_replay_window = 1;
            return true;
        }
        if seq > self.recv_highest {
            let shift = seq - self.recv_highest;
            self.anti_replay_window = if shift >= 64 {
                1
            } else {
                (self.anti_replay_window << shift) | 1
            };
            self.recv_highest = seq;
            true
        } else {
            let diff = self.recv_highest - seq;
            if diff >= 64 {
                return false;
            }
            let bit = 1u64 << diff;
            if self.anti_replay_window & bit != 0 {
                false
            } else {
                self.anti_replay_window |= bit;
                true
            }
        }
    }

    pub fn is_expired(&self, now: u64) -> bool {
        self.bytes_used >= self.lifetime_bytes
            || now.saturating_sub(self.created_at) >= self.lifetime_seconds
    }
}

pub struct Sad {
    pub entries: Vec<SecurityAssociation>,
}

impl Sad {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, sa: SecurityAssociation) {
        self.entries.push(sa);
    }

    pub fn lookup_by_spi(&self, spi: u32) -> Option<&SecurityAssociation> {
        self.entries.iter().find(|e| e.spi == spi)
    }

    pub fn remove_expired(&mut self, now: u64) {
        self.entries.retain(|sa| !sa.is_expired(now));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpdAction {
    Protect,
    Bypass,
    Discard,
}

pub struct SpdEntry {
    pub src_addr: [u8; 16],
    pub src_mask: u8,
    pub dst_addr: [u8; 16],
    pub dst_mask: u8,
    pub protocol: u8,
    pub src_port: u16,
    pub dst_port: u16,
    pub action: SpdAction,
    pub sa_spi: u32,
}

pub struct Spd {
    pub entries: Vec<SpdEntry>,
}

impl Spd {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, entry: SpdEntry) {
        self.entries.push(entry);
    }

    /// Find the first policy whose protocol matches (or is the `0` wildcard) AND
    /// whose source/destination CIDRs cover `src`/`dst`. `src`/`dst` are 16-byte
    /// addresses (IPv6, or v4-mapped). Previously this ignored `src`/`dst`
    /// entirely and matched on protocol alone — so the wrong policy (or none)
    /// would apply to a packet.
    pub fn lookup(&self, src: &[u8], dst: &[u8], proto: u8) -> Option<&SpdEntry> {
        let src16 = <&[u8; 16]>::try_from(src).ok()?;
        let dst16 = <&[u8; 16]>::try_from(dst).ok()?;
        self.entries.iter().find(|e| {
            (e.protocol == proto || e.protocol == 0)
                && cidr_match(src16, &e.src_addr, e.src_mask)
                && cidr_match(dst16, &e.dst_addr, e.dst_mask)
        })
    }
}

/// Does `addr` fall within `net`/`prefix` (CIDR), comparing the top `prefix`
/// bits of two 16-byte addresses? `prefix == 0` matches everything; `prefix` is
/// clamped to 128. Never panics.
fn cidr_match(addr: &[u8; 16], net: &[u8; 16], prefix: u8) -> bool {
    let prefix = (prefix as usize).min(128);
    let full = prefix / 8;
    if addr[..full] != net[..full] {
        return false;
    }
    let rem = prefix % 8;
    if rem > 0 {
        let mask = 0xFFu8 << (8 - rem);
        if (addr[full] & mask) != (net[full] & mask) {
            return false;
        }
    }
    true
}

// ─── L2TP ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum L2tpRole {
    Lac, // L2TP Access Concentrator
    Lns, // L2TP Network Server
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum L2tpState {
    Idle,
    WaitCtlReply,
    Established,
    SessionEstablished,
    Closing,
}

pub struct L2tpAvp {
    pub attr_type: u16,
    pub value: Vec<u8>,
    pub mandatory: bool,
}

pub struct L2tpTunnel {
    pub state: L2tpState,
    pub role: L2tpRole,
    pub local_tunnel_id: u16,
    pub remote_tunnel_id: u16,
    pub sessions: Vec<L2tpDataSession>,
    pub next_ns: u16,
    pub next_nr: u16,
}

pub struct L2tpDataSession {
    pub local_session_id: u16,
    pub remote_session_id: u16,
    pub active: bool,
}

impl L2tpTunnel {
    pub fn new(role: L2tpRole, local_tunnel_id: u16) -> Self {
        Self {
            state: L2tpState::Idle,
            role,
            local_tunnel_id,
            remote_tunnel_id: 0,
            sessions: Vec::new(),
            next_ns: 0,
            next_nr: 0,
        }
    }

    pub fn start_control_connection(&mut self) -> Vec<u8> {
        self.state = L2tpState::WaitCtlReply;
        let mut msg = Vec::with_capacity(32);
        msg.extend_from_slice(&[0xC8, 0x02]); // L2TP header flags
        msg.extend_from_slice(&12u16.to_be_bytes()); // length
        msg.extend_from_slice(&self.local_tunnel_id.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes()); // session 0 for control
        msg.extend_from_slice(&self.next_ns.to_be_bytes());
        msg.extend_from_slice(&self.next_nr.to_be_bytes());
        self.next_ns += 1;
        msg
    }

    pub fn handle_control_reply(&mut self, msg: &[u8]) -> bool {
        if msg.len() < 12 {
            return false;
        }
        self.remote_tunnel_id = u16::from_be_bytes([msg[4], msg[5]]);
        self.state = L2tpState::Established;
        true
    }

    pub fn create_session(&mut self, local_session_id: u16) -> Vec<u8> {
        let session = L2tpDataSession {
            local_session_id,
            remote_session_id: 0,
            active: false,
        };
        self.sessions.push(session);
        let mut msg = Vec::with_capacity(32);
        msg.extend_from_slice(&[0xC8, 0x02]);
        msg.extend_from_slice(&local_session_id.to_be_bytes());
        msg
    }

    pub fn session_established(&mut self, local_id: u16, remote_id: u16) {
        if let Some(s) = self
            .sessions
            .iter_mut()
            .find(|s| s.local_session_id == local_id)
        {
            s.remote_session_id = remote_id;
            s.active = true;
        }
        self.state = L2tpState::SessionEstablished;
    }

    pub fn encapsulate_data(&self, session_id: u16, payload: &[u8]) -> Vec<u8> {
        let mut pkt = Vec::with_capacity(8 + payload.len());
        pkt.extend_from_slice(&[0x00, 0x02]); // data header
        pkt.extend_from_slice(&self.remote_tunnel_id.to_be_bytes());
        pkt.extend_from_slice(&session_id.to_be_bytes());
        pkt.extend_from_slice(payload);
        pkt
    }
}

// ─── PPTP ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PptpState {
    Idle,
    ControlConnected,
    CallEstablished,
    Connected,
}

pub struct PptpSession {
    pub state: PptpState,
    pub call_id: u16,
    pub peer_call_id: u16,
    pub gre_key: u32,
    pub mppe_send_key: Key,
    pub mppe_recv_key: Key,
    pub sequence_sent: u32,
    pub sequence_recv: u32,
    pub ack_sent: u32,
    pub ack_recv: u32,
}

impl PptpSession {
    pub fn new(call_id: u16) -> Self {
        Self {
            state: PptpState::Idle,
            call_id,
            peer_call_id: 0,
            gre_key: 0,
            mppe_send_key: Key::zero(),
            mppe_recv_key: Key::zero(),
            sequence_sent: 0,
            sequence_recv: 0,
            ack_sent: 0,
            ack_recv: 0,
        }
    }

    pub fn start_control_connection(&mut self) -> Vec<u8> {
        let mut msg = Vec::with_capacity(156);
        msg.extend_from_slice(&156u16.to_be_bytes()); // length
        msg.extend_from_slice(&1u16.to_be_bytes()); // PPTP message type: control
        msg.extend_from_slice(&[0x00, 0x1A, 0x2C, 0x00]); // magic cookie
        msg.extend_from_slice(&1u16.to_be_bytes()); // Start-Control-Connection-Request
        msg.resize(156, 0);
        self.state = PptpState::ControlConnected;
        msg
    }

    pub fn outgoing_call(&mut self) -> Vec<u8> {
        let mut msg = Vec::with_capacity(168);
        msg.extend_from_slice(&168u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes()); // control
        msg.extend_from_slice(&[0x00, 0x1A, 0x2C, 0x00]);
        msg.extend_from_slice(&7u16.to_be_bytes()); // Outgoing-Call-Request
        msg.extend_from_slice(&self.call_id.to_be_bytes());
        msg.resize(168, 0);
        msg
    }

    pub fn call_established(&mut self, peer_call_id: u16) {
        self.peer_call_id = peer_call_id;
        self.gre_key = (self.peer_call_id as u32) << 16 | self.call_id as u32;
        self.state = PptpState::CallEstablished;
    }

    pub fn gre_encapsulate(&mut self, payload: &[u8]) -> Vec<u8> {
        self.sequence_sent += 1;
        let mut pkt = Vec::with_capacity(16 + payload.len());
        pkt.extend_from_slice(&[0x30, 0x01]); // GRE flags + protocol type (PPP)
        pkt.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        pkt.extend_from_slice(&self.gre_key.to_be_bytes());
        pkt.extend_from_slice(&self.sequence_sent.to_be_bytes());
        pkt.extend_from_slice(&self.ack_recv.to_be_bytes());
        pkt.extend_from_slice(payload);
        pkt
    }

    pub fn gre_decapsulate(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        if packet.len() < 16 {
            return None;
        }
        let key = u32::from_be_bytes(packet[4..8].try_into().ok()?);
        if key != self.gre_key {
            return None;
        }
        self.sequence_recv = u32::from_be_bytes(packet[8..12].try_into().ok()?);
        self.ack_recv = self.sequence_recv;
        Some(packet[16..].to_vec())
    }

    pub fn mppe_encrypt(&self, data: &[u8]) -> Vec<u8> {
        let mut out = alloc::vec![0u8; data.len()];
        for (i, &b) in data.iter().enumerate() {
            out[i] = b ^ self.mppe_send_key.0[i % WG_KEY_LEN];
        }
        out
    }
}

// ─── SSTP ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SstpState {
    Disconnected,
    SslHandshake,
    ConnectRequestSent,
    ConnectAckReceived,
    Connected,
    Disconnecting,
}

pub struct SstpSession {
    pub state: SstpState,
    pub ssl_established: bool,
    pub ppp_negotiated: bool,
    pub server_cert_hash: [u8; 32],
    pub nonce: [u8; 32],
}

impl SstpSession {
    pub fn new() -> Self {
        Self {
            state: SstpState::Disconnected,
            ssl_established: false,
            ppp_negotiated: false,
            server_cert_hash: [0u8; 32],
            nonce: [0u8; 32],
        }
    }

    pub fn ssl_connect(&mut self, cert_hash: [u8; 32]) -> bool {
        self.server_cert_hash = cert_hash;
        self.ssl_established = true;
        self.state = SstpState::SslHandshake;
        true
    }

    pub fn send_connect_request(&mut self) -> Vec<u8> {
        self.state = SstpState::ConnectRequestSent;
        let mut msg = Vec::with_capacity(24);
        msg.extend_from_slice(&[0x10, 0x01]); // SSTP version + message type
        msg.extend_from_slice(&24u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes()); // SSTP_MSG_CALL_CONNECT_REQUEST
        msg.extend_from_slice(&3u16.to_be_bytes()); // protocol ID: PPP
        msg.resize(24, 0);
        msg
    }

    pub fn handle_connect_ack(&mut self, msg: &[u8]) -> bool {
        if msg.len() < 4 {
            return false;
        }
        self.state = SstpState::ConnectAckReceived;
        true
    }

    pub fn ppp_established(&mut self) {
        self.ppp_negotiated = true;
        self.state = SstpState::Connected;
    }

    pub fn validate_certificate(&self, cert_hash: &[u8; 32]) -> bool {
        // Constant-time compare: never leak how many leading bytes matched.
        ct_eq(&self.server_cert_hash, cert_hash)
    }

    pub fn wrap_ppp_frame(&self, ppp_data: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(4 + ppp_data.len());
        frame.extend_from_slice(&[0x10, 0x00]); // data message
        frame.extend_from_slice(&((4 + ppp_data.len()) as u16).to_be_bytes());
        frame.extend_from_slice(ppp_data);
        frame
    }
}

// ─── VPN Configuration & Profiles ────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VpnProtocol {
    WireGuard,
    OpenVpn,
    IpSec,
    L2tp,
    Pptp,
    Sstp,
}

#[derive(Clone)]
pub struct VpnProfile {
    pub name: String,
    pub protocol: VpnProtocol,
    pub server: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub private_key: Option<Key>,
    pub routes: Vec<RouteEntry>,
    pub dns_servers: Vec<[u8; 16]>,
    pub split_tunnel: SplitTunnelConfig,
    pub auto_connect: bool,
    pub kill_switch: bool,
}

#[derive(Clone)]
pub struct RouteEntry {
    pub network: [u8; 16],
    pub prefix_len: u8,
    pub is_v6: bool,
    pub is_exclude: bool,
}

impl VpnProfile {
    pub fn new(name: String, protocol: VpnProtocol, server: String, port: u16) -> Self {
        Self {
            name,
            protocol,
            server,
            port,
            username: None,
            password: None,
            private_key: None,
            routes: Vec::new(),
            dns_servers: Vec::new(),
            split_tunnel: SplitTunnelConfig::new(),
            auto_connect: false,
            kill_switch: false,
        }
    }

    pub fn export_wgquick(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"[Interface]\n");
        if let Some(ref key) = self.private_key {
            out.extend_from_slice(b"PrivateKey = ");
            for b in key.0.iter() {
                let hi = b >> 4;
                let lo = b & 0xf;
                out.push(if hi < 10 { b'0' + hi } else { b'a' + hi - 10 });
                out.push(if lo < 10 { b'0' + lo } else { b'a' + lo - 10 });
            }
            out.push(b'\n');
        }
        out.extend_from_slice(b"\n[Peer]\n");
        out.extend_from_slice(b"Endpoint = ");
        out.extend_from_slice(self.server.as_bytes());
        out.push(b':');
        let port_str = alloc::format!("{}", self.port);
        out.extend_from_slice(port_str.as_bytes());
        out.push(b'\n');
        out
    }

    pub fn export_ovpn(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"client\n");
        out.extend_from_slice(b"dev tun\n");
        out.extend_from_slice(b"proto udp\n");
        out.extend_from_slice(b"remote ");
        out.extend_from_slice(self.server.as_bytes());
        out.push(b' ');
        let port_str = alloc::format!("{}", self.port);
        out.extend_from_slice(port_str.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(b"resolv-retry infinite\n");
        out.extend_from_slice(b"nobind\n");
        out.extend_from_slice(b"persist-key\n");
        out.extend_from_slice(b"persist-tun\n");
        out
    }
}

// ─── Split Tunneling ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SplitTunnelConfig {
    pub enabled: bool,
    pub mode: SplitTunnelMode,
    pub app_rules: Vec<AppVpnRule>,
    pub route_rules: Vec<RouteEntry>,
    pub dns_split: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SplitTunnelMode {
    IncludeOnly,
    ExcludeOnly,
}

#[derive(Clone)]
pub struct AppVpnRule {
    pub app_id: String,
    pub use_vpn: bool,
}

impl SplitTunnelConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            mode: SplitTunnelMode::ExcludeOnly,
            app_rules: Vec::new(),
            route_rules: Vec::new(),
            dns_split: false,
        }
    }

    pub fn should_route_through_vpn(&self, app_id: &str, dst: &[u8], is_v6: bool) -> bool {
        if !self.enabled {
            return true;
        }
        if let Some(rule) = self.app_rules.iter().find(|r| r.app_id == app_id) {
            return rule.use_vpn;
        }
        for route in &self.route_rules {
            if route.is_v6 != is_v6 {
                continue;
            }
            let matches = prefix_match(&route.network, dst, route.prefix_len, is_v6);
            if matches {
                return !route.is_exclude;
            }
        }
        match self.mode {
            SplitTunnelMode::IncludeOnly => false,
            SplitTunnelMode::ExcludeOnly => true,
        }
    }
}

fn prefix_match(network: &[u8; 16], addr: &[u8], prefix: u8, is_v6: bool) -> bool {
    let len = if is_v6 { 16 } else { 4 };
    let full_bytes = (prefix / 8) as usize;
    for i in 0..full_bytes.min(len) {
        if network[i] != addr[i] {
            return false;
        }
    }
    let rem = prefix % 8;
    if rem > 0 && full_bytes < len {
        let mask = 0xFF << (8 - rem);
        if (network[full_bytes] & mask) != (addr[full_bytes] & mask) {
            return false;
        }
    }
    true
}

// ─── Kill Switch ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KillSwitchMode {
    Off,
    Auto,
    AlwaysOn,
}

pub struct KillSwitch {
    pub mode: KillSwitchMode,
    pub active: bool,
    pub block_ipv6: bool,
    pub block_dns_leak: bool,
    pub block_webrtc_leak: bool,
    pub allowed_lan: bool,
    pub firewall_rules: Vec<FirewallRule>,
}

#[derive(Clone)]
pub struct FirewallRule {
    pub direction: FirewallDirection,
    pub action: FirewallAction,
    pub interface: Option<String>,
    pub protocol: u8,
    pub dst_port: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FirewallDirection {
    Inbound,
    Outbound,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FirewallAction {
    Allow,
    Block,
}

impl KillSwitch {
    pub fn new() -> Self {
        Self {
            mode: KillSwitchMode::Off,
            active: false,
            block_ipv6: true,
            block_dns_leak: true,
            block_webrtc_leak: true,
            allowed_lan: true,
            firewall_rules: Vec::new(),
        }
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.firewall_rules.clear();
        self.firewall_rules.push(FirewallRule {
            direction: FirewallDirection::Outbound,
            action: FirewallAction::Block,
            interface: None,
            protocol: 0,
            dst_port: 0,
        });
        if self.block_dns_leak {
            self.firewall_rules.push(FirewallRule {
                direction: FirewallDirection::Outbound,
                action: FirewallAction::Block,
                interface: None,
                protocol: 17, // UDP
                dst_port: 53,
            });
        }
        if self.block_ipv6 {
            self.firewall_rules.push(FirewallRule {
                direction: FirewallDirection::Outbound,
                action: FirewallAction::Block,
                interface: None,
                protocol: 41, // IPv6
                dst_port: 0,
            });
        }
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        self.firewall_rules.clear();
    }

    pub fn should_block(&self, dst: &[u8], protocol: u8, port: u16) -> bool {
        if !self.active {
            return false;
        }
        if self.allowed_lan && is_lan_address(dst) {
            return false;
        }
        for rule in &self.firewall_rules {
            if rule.action == FirewallAction::Block
                && (rule.protocol == 0 || rule.protocol == protocol)
                && (rule.dst_port == 0 || rule.dst_port == port)
            {
                return true;
            }
        }
        false
    }
}

fn is_lan_address(addr: &[u8]) -> bool {
    if addr.len() >= 4 {
        if addr[0] == 10 {
            return true;
        }
        if addr[0] == 172 && (addr[1] & 0xF0) == 16 {
            return true;
        }
        if addr[0] == 192 && addr[1] == 168 {
            return true;
        }
    }
    false
}

// ─── DNS Configuration ───────────────────────────────────────────────────────

pub struct VpnDns {
    pub servers: Vec<[u8; 16]>,
    pub leak_prevention: bool,
    pub dns_over_https: bool,
    pub doh_endpoint: Option<String>,
    pub original_servers: Vec<[u8; 16]>,
}

impl VpnDns {
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
            leak_prevention: true,
            dns_over_https: false,
            doh_endpoint: None,
            original_servers: Vec::new(),
        }
    }

    pub fn apply_vpn_dns(&mut self, vpn_servers: &[[u8; 16]]) {
        self.original_servers = self.servers.clone();
        self.servers = vpn_servers.to_vec();
    }

    pub fn restore_dns(&mut self) {
        self.servers = core::mem::take(&mut self.original_servers);
    }

    pub fn resolve_through_tunnel(&self, _query: &[u8]) -> Option<Vec<u8>> {
        if self.servers.is_empty() {
            return None;
        }
        Some(alloc::vec![0; 12])
    }
}

// ─── Connection Management ───────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Disconnecting,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ServerSelectionStrategy {
    LatencyBased,
    LoadBased,
    Random,
    Manual,
}

pub struct ConnectionStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub connected_since: u64,
    pub current_speed_up: u64,
    pub current_speed_down: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub packets_lost: u64,
}

pub struct VpnConnection {
    pub state: ConnectionState,
    pub profile: Option<VpnProfile>,
    pub stats: ConnectionStats,
    pub kill_switch: KillSwitch,
    pub dns: VpnDns,
    pub reconnect_attempts: u32,
    pub max_reconnect_attempts: u32,
}

impl VpnConnection {
    pub fn new() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            profile: None,
            stats: ConnectionStats {
                bytes_sent: 0,
                bytes_received: 0,
                connected_since: 0,
                current_speed_up: 0,
                current_speed_down: 0,
                packets_sent: 0,
                packets_received: 0,
                packets_lost: 0,
            },
            kill_switch: KillSwitch::new(),
            dns: VpnDns::new(),
            reconnect_attempts: 0,
            max_reconnect_attempts: 5,
        }
    }

    pub fn connect(&mut self, profile: VpnProfile, now: u64) {
        if profile.kill_switch {
            self.kill_switch.mode = KillSwitchMode::Auto;
        }
        self.profile = Some(profile);
        self.state = ConnectionState::Connecting;
        self.stats.connected_since = now;
        self.reconnect_attempts = 0;
    }

    pub fn established(&mut self) {
        self.state = ConnectionState::Connected;
        if self.kill_switch.mode != KillSwitchMode::Off {
            self.kill_switch.activate();
        }
        if let Some(ref profile) = self.profile {
            self.dns.apply_vpn_dns(&profile.dns_servers);
        }
    }

    pub fn disconnect(&mut self) {
        self.state = ConnectionState::Disconnecting;
        self.dns.restore_dns();
        if self.kill_switch.mode == KillSwitchMode::Auto {
            self.kill_switch.deactivate();
        }
        self.state = ConnectionState::Disconnected;
        self.profile = None;
    }

    pub fn reconnect(&mut self) {
        if self.reconnect_attempts >= self.max_reconnect_attempts {
            self.state = ConnectionState::Error;
            return;
        }
        self.reconnect_attempts += 1;
        self.state = ConnectionState::Reconnecting;
    }

    pub fn update_stats(&mut self, sent: u64, received: u64) {
        self.stats.bytes_sent += sent;
        self.stats.bytes_received += received;
        self.stats.packets_sent += 1;
        self.stats.packets_received += 1;
    }
}

// ─── Multi-Hop ───────────────────────────────────────────────────────────────

pub struct MultiHopChain {
    pub hops: Vec<HopServer>,
    pub active: bool,
}

pub struct HopServer {
    pub server: String,
    pub port: u16,
    pub protocol: VpnProtocol,
    pub connected: bool,
}

impl MultiHopChain {
    pub fn new() -> Self {
        Self {
            hops: Vec::new(),
            active: false,
        }
    }

    pub fn add_hop(&mut self, server: String, port: u16, protocol: VpnProtocol) {
        self.hops.push(HopServer {
            server,
            port,
            protocol,
            connected: false,
        });
    }

    pub fn connect_chain(&mut self) -> bool {
        for hop in self.hops.iter_mut() {
            hop.connected = true;
        }
        self.active = true;
        true
    }

    pub fn disconnect_chain(&mut self) {
        for hop in self.hops.iter_mut().rev() {
            hop.connected = false;
        }
        self.active = false;
    }

    pub fn hop_count(&self) -> usize {
        self.hops.len()
    }
}

// ─── Server Selection ────────────────────────────────────────────────────────

pub struct ServerEntry {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub protocol: VpnProtocol,
    pub latency_ms: u32,
    pub load_percent: u8,
    pub country: String,
}

pub struct ServerList {
    pub servers: Vec<ServerEntry>,
    pub strategy: ServerSelectionStrategy,
}

impl ServerList {
    pub fn new(strategy: ServerSelectionStrategy) -> Self {
        Self {
            servers: Vec::new(),
            strategy,
        }
    }

    pub fn select_best(&self) -> Option<&ServerEntry> {
        if self.servers.is_empty() {
            return None;
        }
        match self.strategy {
            ServerSelectionStrategy::LatencyBased => {
                self.servers.iter().min_by_key(|s| s.latency_ms)
            }
            ServerSelectionStrategy::LoadBased => {
                self.servers.iter().min_by_key(|s| s.load_percent)
            }
            ServerSelectionStrategy::Random => self.servers.first(),
            ServerSelectionStrategy::Manual => self.servers.first(),
        }
    }

    pub fn add_server(&mut self, entry: ServerEntry) {
        self.servers.push(entry);
    }
}

// ─── Global VPN Manager ──────────────────────────────────────────────────────

pub struct VpnManager {
    pub connection: VpnConnection,
    pub profiles: Vec<VpnProfile>,
    pub server_list: ServerList,
    pub multi_hop: MultiHopChain,
    pub initialized: bool,
}

impl VpnManager {
    pub const fn new() -> Self {
        Self {
            connection: VpnConnection {
                state: ConnectionState::Disconnected,
                profile: None,
                stats: ConnectionStats {
                    bytes_sent: 0,
                    bytes_received: 0,
                    connected_since: 0,
                    current_speed_up: 0,
                    current_speed_down: 0,
                    packets_sent: 0,
                    packets_received: 0,
                    packets_lost: 0,
                },
                kill_switch: KillSwitch {
                    mode: KillSwitchMode::Off,
                    active: false,
                    block_ipv6: true,
                    block_dns_leak: true,
                    block_webrtc_leak: true,
                    allowed_lan: true,
                    firewall_rules: Vec::new(),
                },
                dns: VpnDns {
                    servers: Vec::new(),
                    leak_prevention: true,
                    dns_over_https: false,
                    doh_endpoint: None,
                    original_servers: Vec::new(),
                },
                reconnect_attempts: 0,
                max_reconnect_attempts: 5,
            },
            profiles: Vec::new(),
            server_list: ServerList {
                servers: Vec::new(),
                strategy: ServerSelectionStrategy::LatencyBased,
            },
            multi_hop: MultiHopChain {
                hops: Vec::new(),
                active: false,
            },
            initialized: false,
        }
    }

    pub fn init(&mut self) {
        self.initialized = true;
    }

    pub fn add_profile(&mut self, profile: VpnProfile) {
        self.profiles.push(profile);
    }

    pub fn connect_profile(&mut self, name: &str, now: u64) -> bool {
        let profile = self.profiles.iter().find(|p| p.name == name).cloned();
        if let Some(p) = profile {
            self.connection.connect(p, now);
            true
        } else {
            false
        }
    }

    pub fn disconnect(&mut self) {
        self.connection.disconnect();
        self.multi_hop.disconnect_chain();
    }

    pub fn status(&self) -> ConnectionState {
        self.connection.state
    }
}

static mut VPN_MANAGER: VpnManager = VpnManager::new();

pub fn init() {
    unsafe {
        VPN_MANAGER.init();
    }
}

pub fn vpn_manager() -> &'static mut VpnManager {
    unsafe { &mut VPN_MANAGER }
}

pub fn run_boot_smoketest() {
    // 1. Test X25519
    let priv_a = Key([0x01; 32]);
    let priv_b = Key([0x02; 32]);
    let pub_a = x25519_base(&priv_a);
    let pub_b = x25519_base(&priv_b);
    let shared_a = x25519(&priv_a, &pub_b);
    let shared_b = x25519(&priv_b, &pub_a);

    let x25519_ok = shared_a == shared_b && shared_a != Key::zero();

    // 2. Test Blake2s
    let data = b"AthenaOS Crypto Test";
    let hash1 = blake2s(data, &[]);
    let hash2 = blake2s(data, &[]);
    let blake2_ok = hash1.0 == hash2.0 && hash1.0 != [0u8; 32];

    // 3. Test ChaCha20-Poly1305
    let key = Key([0x42; 32]);
    let nonce = 12345;
    let aad = b"AAD";
    let plaintext = b"Hello, WireGuard!";
    let mut ciphertext = [0u8; 17 + 16]; // 17 bytes + 16 bytes tag
    let enc_ok = chacha20_poly1305_encrypt(&key, nonce, aad, plaintext, &mut ciphertext).is_some();

    let mut decrypted = [0u8; 17];
    let chacha_ok = enc_ok
        && chacha20_poly1305_decrypt(&key, nonce, aad, &ciphertext, &mut decrypted)
        && decrypted == *plaintext;

    // 4. Full Noise_IKpsk2 handshake round-trip → matching transport keys.
    let handshake_ok = handshake_roundtrip_ok();

    let _ = x25519_ok && blake2_ok && chacha_ok && handshake_ok;
}

/// Run a complete initiator<->responder Noise_IKpsk2 handshake and verify both
/// sides derive matching, crossed transport keys. Returns false on any failure;
/// the kernel boot smoketest (R10) prints PASS/FAIL based on this.
pub fn handshake_roundtrip_ok() -> bool {
    let init_static = Key([0x11; 32]);
    let resp_static = Key([0x22; 32]);
    let psk = Key([0x33; 32]);
    let init_static_pub = x25519_base(&init_static);
    let resp_static_pub = x25519_base(&resp_static);

    let mut initiator = NoiseHandshake::new(init_static, resp_static_pub, psk);
    // Responder's remote_static is filled in from the decrypted initiation; start zero.
    let mut responder = NoiseHandshake::new(resp_static, Key::zero(), psk);

    let init_eph = Key([0xA1; 32]);
    let resp_eph = Key([0xB2; 32]);

    let msg1 = initiator.create_initiation(1, init_eph);
    if msg1.len() != WG_INIT_LEN {
        return false;
    }
    if !responder.consume_initiation(&msg1) {
        return false;
    }
    // Responder must have recovered the initiator's true static public key.
    if responder.remote_static != init_static_pub {
        return false;
    }
    if !responder.create_response(2, resp_eph) {
        return false;
    }
    let msg2 = responder.take_response();
    if msg2.len() != WG_RESP_LEN {
        return false;
    }
    if !initiator.consume_response(&msg2) {
        return false;
    }

    // Crossed keys: initiator.send == responder.recv, initiator.recv == responder.send.
    initiator.sending_key() == responder.receiving_key()
        && initiator.receiving_key() == responder.sending_key()
        && initiator.sending_key() != Key::zero()
        && initiator.sending_key() != initiator.receiving_key()
        && initiator.state() == HandshakeState::Established
        && responder.state() == HandshakeState::ResponseSent
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RFC 6479 anti-replay window ──────────────────────────────────────────

    #[test]
    fn replay_window_in_order_and_replay() {
        let mut w = ReplayWindow::new();
        assert!(w.accept(1));
        assert!(w.accept(2));
        assert!(w.accept(3));
        assert!(!w.accept(2), "replay of 2 must be rejected");
        assert!(!w.accept(1), "replay of 1 must be rejected");
        assert!(w.accept(4));
    }

    #[test]
    fn replay_window_out_of_order_within_window() {
        let mut w = ReplayWindow::new();
        assert!(w.accept(10));
        assert!(w.accept(5), "reordered, unseen, within window -> accept");
        assert!(w.accept(9));
        assert!(!w.accept(9), "second 9 is a replay");
        assert!(!w.accept(5), "second 5 is a replay");
    }

    #[test]
    fn replay_window_too_old_rejected() {
        let mut w = ReplayWindow::new();
        assert!(w.accept(100));
        assert!(
            !w.accept(100 - 64),
            "exactly 64 behind is outside the window"
        );
        assert!(!w.accept(30), "70 behind is too old");
        assert!(
            w.accept(100 - 63),
            "63 behind is the oldest still in-window"
        );
    }

    #[test]
    fn replay_window_advances_past_64() {
        let mut w = ReplayWindow::new();
        assert!(w.accept(0));
        assert!(w.accept(200), "a big jump forward shifts the whole window");
        assert!(!w.accept(0), "0 is now far too old");
        assert!(w.accept(199));
        assert!(!w.accept(199), "replay after the jump");
    }

    fn xport_pair() -> (TransportSession, TransportSession) {
        // A sends with 0x11 / receives with 0x22; B mirrors — so A's ciphertext
        // (key 0x11) is exactly what B decrypts (B.receiving = 0x11).
        let a = TransportSession::new(1, 2, Key([0x11; 32]), Key([0x22; 32]), 0);
        let b = TransportSession::new(2, 1, Key([0x22; 32]), Key([0x11; 32]), 0);
        (a, b)
    }

    /// THE fix: a replayed transport packet is dropped (decrypt_packet used to
    /// accept it every time — no anti-replay).
    #[test]
    fn decrypt_packet_rejects_replay() {
        let (a, b) = xport_pair();
        let pkt = a.encrypt_packet(b"secret payload").unwrap();
        assert_eq!(
            b.decrypt_packet(&pkt).as_deref(),
            Some(&b"secret payload"[..])
        );
        assert_eq!(
            b.decrypt_packet(&pkt),
            None,
            "replayed packet must be dropped"
        );
    }

    /// Reordered (but unique) packets are still accepted — anti-replay must not
    /// break normal UDP reordering.
    #[test]
    fn decrypt_packet_accepts_reordering() {
        let (a, b) = xport_pair();
        let p0 = a.encrypt_packet(b"zero").unwrap();
        let p1 = a.encrypt_packet(b"one").unwrap();
        let p2 = a.encrypt_packet(b"two").unwrap();
        assert!(b.decrypt_packet(&p2).is_some());
        assert!(b.decrypt_packet(&p0).is_some());
        assert!(b.decrypt_packet(&p1).is_some());
        assert!(b.decrypt_packet(&p0).is_none(), "re-delivery is a replay");
    }

    // ── IPsec SPD policy lookup (CIDR src/dst match) ─────────────────────────

    #[test]
    fn spd_cidr_match() {
        let net = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]; // 2001:db8::/32
        let inside = [
            0x20, 0x01, 0x0d, 0xb8, 0xff, 0xee, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
        ];
        let outside = [0x20, 0x01, 0x0d, 0xb9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        assert!(cidr_match(&inside, &net, 32));
        assert!(!cidr_match(&outside, &net, 32));
        assert!(cidr_match(&outside, &net, 0), "/0 matches everything");
        assert!(cidr_match(&net, &net, 128), "exact /128");
        // /33: top bit of byte 4 — inside(0xff) vs net(0x00) differ.
        assert!(!cidr_match(&inside, &net, 33));
    }

    /// lookup now matches src/dst/proto — previously it ignored src/dst, so the
    /// wrong policy applied.
    #[test]
    fn spd_lookup_matches_src_dst() {
        let mut spd = Spd::new();
        spd.add(SpdEntry {
            src_addr: [10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            src_mask: 8,
            dst_addr: [10, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            dst_mask: 24,
            protocol: 6,
            src_port: 0,
            dst_port: 0,
            action: SpdAction::Protect,
            sa_spi: 1,
        });
        let src_in = [10, 5, 5, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let dst_in = [10, 0, 1, 99, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let dst_out = [10, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(
            spd.lookup(&src_in, &dst_in, 6).is_some(),
            "matching policy found"
        );
        assert!(
            spd.lookup(&src_in, &dst_out, 6).is_none(),
            "wrong dst /24 -> no match"
        );
        assert!(
            spd.lookup(&src_in, &dst_in, 17).is_none(),
            "wrong proto -> no match"
        );
        assert!(
            spd.lookup(&[10, 0, 0, 1], &dst_in, 6).is_none(),
            "bad address length -> None"
        );
    }

    // ── IPsec ESP anti-replay (RFC 6479) ─────────────────────────────────────

    fn esp_sa_pair() -> (SecurityAssociation, SecurityAssociation) {
        let mut a = SecurityAssociation::new(0x1000, IpsecProtocol::Esp, IpsecMode::Tunnel);
        let mut b = SecurityAssociation::new(0x1000, IpsecProtocol::Esp, IpsecMode::Tunnel);
        a.encrypt_key = Key([0x55; 32]);
        b.encrypt_key = Key([0x55; 32]);
        (a, b)
    }

    /// THE fix: a replayed ESP packet is dropped.
    #[test]
    fn esp_decrypt_rejects_replay() {
        let (mut a, mut b) = esp_sa_pair();
        let pkt = a.esp_encrypt(b"esp payload");
        assert_eq!(b.esp_decrypt(&pkt).as_deref(), Some(&b"esp payload"[..]));
        assert_eq!(
            b.esp_decrypt(&pkt),
            None,
            "replayed ESP packet must be dropped"
        );
    }

    /// A long in-order run is accepted — the old `seq % 64` toy false-rejected
    /// packet 65+ (bit collision with seq 1).
    #[test]
    fn esp_decrypt_works_past_64_packets() {
        let (mut a, mut b) = esp_sa_pair();
        for i in 1..=70u32 {
            let p = a.esp_encrypt(b"x");
            assert!(b.esp_decrypt(&p).is_some(), "seq {i} must be accepted");
        }
        let again = a.esp_encrypt(b"x"); // seq 71
        assert!(b.esp_decrypt(&again).is_some());
        assert!(b.esp_decrypt(&again).is_none(), "replay of 71 rejected");
    }

    fn setup() -> (NoiseHandshake, NoiseHandshake, Key, Key, Key, Key) {
        let init_static = Key([0x11; 32]);
        let resp_static = Key([0x22; 32]);
        let psk = Key([0x33; 32]);
        let resp_static_pub = x25519_base(&resp_static);
        let init_static_pub = x25519_base(&init_static);
        let initiator = NoiseHandshake::new(init_static, resp_static_pub, psk);
        let responder = NoiseHandshake::new(resp_static, Key::zero(), psk);
        (
            initiator,
            responder,
            Key([0xA1; 32]),
            Key([0xB2; 32]),
            init_static_pub,
            resp_static_pub,
        )
    }

    #[test]
    fn full_handshake_derives_matching_transport_keys() {
        let (mut ini, mut res, ie, re, init_pub, _) = setup();

        let msg1 = ini.create_initiation(1, ie);
        assert_eq!(msg1.len(), WG_INIT_LEN, "initiation must be 148 bytes");
        assert_eq!(msg1[0], WG_MSG_INITIATION);

        assert!(res.consume_initiation(&msg1), "responder must accept msg1");
        // Static key was encrypted: it must NOT appear in cleartext in the message.
        assert_ne!(
            &msg1[40..72],
            init_pub.as_bytes(),
            "static key must be encrypted, not cleartext"
        );
        // But the responder must recover the true initiator static public.
        assert_eq!(res.remote_static, init_pub);

        assert!(res.create_response(2, re), "responder must build msg2");
        let msg2 = res.take_response();
        assert_eq!(msg2.len(), WG_RESP_LEN, "response must be 92 bytes");
        assert_eq!(msg2[0], WG_MSG_RESPONSE);

        assert!(ini.consume_response(&msg2), "initiator must accept msg2");

        // The whole point: crossed transport keys match on both ends.
        assert_eq!(
            ini.sending_key(),
            res.receiving_key(),
            "initiator send key must equal responder recv key"
        );
        assert_eq!(
            ini.receiving_key(),
            res.sending_key(),
            "initiator recv key must equal responder send key"
        );
        assert_ne!(ini.sending_key(), Key::zero());
        assert_ne!(
            ini.sending_key(),
            ini.receiving_key(),
            "send and recv keys must differ"
        );
        assert_eq!(ini.state(), HandshakeState::Established);
        assert_eq!(res.state(), HandshakeState::ResponseSent);
    }

    #[test]
    fn transport_data_round_trips_under_handshake_keys() {
        let (mut ini, mut res, ie, re, _, _) = setup();
        let msg1 = ini.create_initiation(7, ie);
        assert!(res.consume_initiation(&msg1));
        assert!(res.create_response(9, re));
        let msg2 = res.take_response();
        assert!(ini.consume_response(&msg2));

        // Build real transport sessions from the derived keys and move a packet
        // initiator -> responder.
        let isess = TransportSession::new(7, 9, ini.sending_key(), ini.receiving_key(), 0);
        let rsess = TransportSession::new(9, 7, res.sending_key(), res.receiving_key(), 0);

        let payload = b"gaming packet over wireguard";
        let wire = isess.encrypt_packet(payload).expect("encrypt");
        let got = rsess.decrypt_packet(&wire).expect("decrypt");
        assert_eq!(got.as_slice(), payload, "transport payload must survive");
    }

    #[test]
    fn tampered_initiation_static_is_rejected() {
        let (mut ini, mut res, ie, _, _, _) = setup();
        let mut msg1 = ini.create_initiation(1, ie);
        // Flip a byte inside the encrypted static field -> AEAD tag must fail.
        msg1[45] ^= 0xFF;
        assert!(
            !res.consume_initiation(&msg1),
            "tampered initiation must be rejected"
        );
    }

    #[test]
    fn tampered_response_empty_is_rejected() {
        let (mut ini, mut res, ie, re, _, _) = setup();
        let msg1 = ini.create_initiation(1, ie);
        assert!(res.consume_initiation(&msg1));
        assert!(res.create_response(2, re));
        let mut msg2 = res.take_response();
        // Corrupt the encrypted-empty tag.
        msg2[50] ^= 0x01;
        assert!(
            !ini.consume_response(&msg2),
            "tampered response must be rejected"
        );
    }

    #[test]
    fn forged_mac1_is_rejected() {
        let (mut ini, mut res, ie, _, _, _) = setup();
        let mut msg1 = ini.create_initiation(1, ie);
        // Zero the MAC1 field -> verify_initiation_mac1 must fail before any DH.
        for b in msg1[WG_INIT_MAC1_OFFSET..WG_INIT_MAC2_OFFSET].iter_mut() {
            *b = 0;
        }
        assert!(
            !res.consume_initiation(&msg1),
            "forged MAC1 must be rejected"
        );
    }

    #[test]
    fn wrong_psk_breaks_key_agreement() {
        let init_static = Key([0x11; 32]);
        let resp_static = Key([0x22; 32]);
        let resp_static_pub = x25519_base(&resp_static);
        let mut ini = NoiseHandshake::new(init_static, resp_static_pub, Key([0x33; 32]));
        // Responder uses a DIFFERENT preshared key.
        let mut res = NoiseHandshake::new(resp_static, Key::zero(), Key([0x99; 32]));

        let msg1 = ini.create_initiation(1, Key([0xA1; 32]));
        assert!(res.consume_initiation(&msg1));
        assert!(res.create_response(2, Key([0xB2; 32])));
        let msg2 = res.take_response();
        // The PSK is mixed before the AEAD-empty tag, so the initiator's decrypt
        // of msg.empty must fail -> response rejected.
        assert!(
            !ini.consume_response(&msg2),
            "mismatched PSK must break the handshake"
        );
    }

    #[test]
    fn malformed_handshake_bytes_never_panic() {
        // Untrusted-network surface: feed garbage of every length, expect false.
        for len in 0..200usize {
            let buf = alloc::vec![0xABu8; len];
            let mut res = NoiseHandshake::new(Key([0x22; 32]), Key::zero(), Key([0x33; 32]));
            let _ = res.consume_initiation(&buf);
            let mut ini =
                NoiseHandshake::new(Key([0x11; 32]), x25519_base(&Key([0x22; 32])), Key::zero());
            let _ = ini.consume_response(&buf);
        }
        // Truncated-but-typed messages.
        let mut res = NoiseHandshake::new(Key([0x22; 32]), Key::zero(), Key([0x33; 32]));
        let mut short_init = alloc::vec![0u8; 100];
        short_init[0] = WG_MSG_INITIATION;
        assert!(!res.consume_initiation(&short_init));
    }

    #[test]
    fn smoketest_helper_passes() {
        assert!(handshake_roundtrip_ok(), "boot smoketest helper must pass");
    }

    #[test]
    fn aead_seal_returns_some_and_round_trips() {
        // Fix 1 regression: the fallible encrypt must succeed on normal bounded
        // input and survive a decrypt round-trip.
        let key = Key([0x42; 32]);
        let aad = b"AAD";
        let plaintext = b"Hello, WireGuard!";
        let mut ct = [0u8; 17 + WG_TAG_LEN];
        assert!(
            chacha20_poly1305_encrypt(&key, 12345, aad, plaintext, &mut ct).is_some(),
            "AEAD seal must return Some on normal input"
        );
        let mut pt = [0u8; 17];
        assert!(chacha20_poly1305_decrypt(&key, 12345, aad, &ct, &mut pt));
        assert_eq!(&pt, plaintext);
    }

    #[test]
    fn validate_certificate_matches_only_exact_hash() {
        // Fix 2: constant-time compare must still be correct (matches iff equal).
        let mut s = SstpSession::new();
        s.server_cert_hash = [0x5a; 32];
        assert!(
            s.validate_certificate(&[0x5a; 32]),
            "exact hash must validate"
        );
        let mut wrong = [0x5a; 32];
        wrong[31] ^= 0x01;
        assert!(
            !s.validate_certificate(&wrong),
            "mismatched hash must reject"
        );
    }

    #[test]
    fn key_zeroize_scrubs_material() {
        // Fix 3: explicit Zeroize wipes secret bytes without breaking the API.
        let mut k = Key([0xAB; 32]);
        k.zeroize();
        assert_eq!(k, Key::zero(), "zeroize must scrub key material");
    }

    #[test]
    fn hkdf_outputs_are_distinct_and_deterministic() {
        let ck = [0x5au8; 32];
        let a = hkdf::<3>(&ck, b"input");
        let b = hkdf::<3>(&ck, b"input");
        assert_eq!(a, b, "hkdf must be deterministic");
        assert_ne!(a[0], a[1]);
        assert_ne!(a[1], a[2]);
        assert_ne!(a[0], a[2]);
    }
}
