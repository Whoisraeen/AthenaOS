//! RaeCrypto — shared `#![no_std]` cryptographic primitives.
//!
//! One source of truth for the primitives that more than one slice needs, so
//! nobody rolls their own (the account-password path in `athid` previously
//! shipped a homebrew non-cryptographic "mixing" function; FDE in the kernel
//! shipped an iterated-HMAC placeholder for Argon2id). Both now call here.
//!
//! Contents:
//!   - BLAKE2b (RFC 7693), arbitrary 1..=64-byte digest.
//!   - Argon2id (RFC 9106): the full memory-hard password/key-derivation
//!     function — BLAKE2b core, the H' variable-length hash, BlaMka
//!     compression over 1 KiB blocks, multi-lane reference-indexed fill, and
//!     Argon2id split addressing (data-independent on pass 0 / slices 0-1,
//!     data-dependent thereafter).
//!
//! Validated against the RFC 9106 §5.3 / RFC 7693 known-answer vectors by the
//! host harness in `tools/argon2_kat/` and, in the kernel, fail-closed in
//! `encryption::run_boot_smoketest`.
//!
//! Cost note: `argon2id_full` allocates `m_kib` × 1 KiB blocks. The kernel heap
//! is ~32 MiB, so callers there must keep `m_kib` well under that (8 MiB is the
//! house default). The KAT uses 32 KiB.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// GF(2^255-19) field arithmetic shared by ed25519 + x25519 (crate-internal).
mod field25519;

/// Ed25519 (RFC 8032) signatures + SHA-512 — the shared signer/verifier for
/// secure boot, atomic-update verification, and AthGuard code signing.
pub mod ed25519;

/// X25519 (RFC 7748) Diffie-Hellman over Curve25519 — the shared key-agreement
/// primitive for secure channels (athsync, TLS, WireGuard).
pub mod x25519;

/// SHA-256 (FIPS 180-4) + HMAC-SHA256 (RFC 2104) + HKDF-SHA256 (RFC 5869) —
/// the shared hash / MAC / key-derivation family for protocol key schedules,
/// signed manifests, and integrity checks.
pub mod sha256;

/// SHA-1 (RFC 3174) — **OTP / legacy-compatibility only** (cryptographically
/// retired for collision resistance; required for HMAC-SHA-1 in HOTP/TOTP, where
/// the collision weakness does not apply). See the module docs. Never use for
/// signatures or content hashing.
pub mod sha1;

/// HMAC (RFC 2104) over the crate's hashes: `hmac_sha1` (for OTP) + `hmac_sha256`.
/// One MAC namespace shared by `ath_otp` (2FA codes) and the protocol stacks.
pub mod hmac;

/// ChaCha20-Poly1305 AEAD (RFC 8439) — the shared authenticated cipher for
/// secure channels (athsync, TLS, WireGuard). `seal` / `open`.
pub mod chacha20poly1305;

/// ECDSA over NIST P-256 + SHA-256 (= COSE **ES256**, alg -7) — verification
/// of WebAuthn/FIDO2 assertions and attestations. Wraps the vetted RustCrypto
/// `p256`/`ecdsa` crates; unblocks `athid::webauthn`'s ES256 path (the
/// algorithm every hardware security key + platform authenticator uses).
pub mod p256_ecdsa;

/// ECDSA over NIST P-384 + SHA-384 (= COSE **ES384**, alg -35) — the DNSSEC
/// algorithm-14 (`ECDSAP384SHA384`, RFC 6605) verifier that closes AthNet's
/// DNSSEC validator to algorithm-complete, and the high-assurance ES384 path
/// for COSE/WebAuthn + P-384 code-signing chains. Sibling of `p256_ecdsa`;
/// wraps the vetted RustCrypto `p384`/`ecdsa` crates + `sha2::Sha384`.
pub mod p384_ecdsa;

/// RSASSA-PKCS1-v1_5 signature VERIFICATION (RFC 8017 §8.2.2) — the shared
/// verify-only RSA (schoolbook public-key modexp `sig^e mod n`) used by the
/// kernel's DNSSEC / X.509 path and by `athid::webauthn`'s COSE **RS256**
/// (alg -257, Windows Hello's TPM platform authenticator). One implementation,
/// not two drifting copies.
pub mod rsa;

/// Measured-boot PCR bank (TPM 2.0-style SHA-256 accumulators) — the shared
/// measurement core used by the kernel's boot measurement + AthGuard's attestation.
pub mod pcr;

const BLAKE2B_IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

const BLAKE2B_SIGMA: [[usize; 16]; 12] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];

#[inline]
fn blake2b_g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

fn blake2b_compress(h: &mut [u64; 8], block: &[u8; 128], t: u128, last: bool) {
    let mut m = [0u64; 16];
    for i in 0..16 {
        m[i] = u64::from_le_bytes(block[i * 8..i * 8 + 8].try_into().unwrap());
    }
    let mut v = [0u64; 16];
    for i in 0..8 {
        v[i] = h[i];
        v[i + 8] = BLAKE2B_IV[i];
    }
    v[12] ^= t as u64;
    v[13] ^= (t >> 64) as u64;
    if last {
        v[14] ^= 0xFFFF_FFFF_FFFF_FFFF;
    }
    for r in 0..12 {
        let s = &BLAKE2B_SIGMA[r];
        blake2b_g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
        blake2b_g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
        blake2b_g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
        blake2b_g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
        blake2b_g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
        blake2b_g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
        blake2b_g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
        blake2b_g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
    }
    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
}

/// BLAKE2b (RFC 7693), no key, output length 1..=64 bytes.
pub fn blake2b(out_len: usize, input: &[u8]) -> Vec<u8> {
    let mut h = BLAKE2B_IV;
    h[0] ^= 0x0101_0000 ^ (out_len as u64);
    let mut t: u128 = 0;
    let mut offset = 0usize;
    while input.len() - offset > 128 {
        let mut block = [0u8; 128];
        block.copy_from_slice(&input[offset..offset + 128]);
        t += 128;
        blake2b_compress(&mut h, &block, t, false);
        offset += 128;
    }
    let remaining = input.len() - offset;
    let mut block = [0u8; 128];
    block[..remaining].copy_from_slice(&input[offset..]);
    t += remaining as u128;
    blake2b_compress(&mut h, &block, t, true);
    let mut out = Vec::with_capacity(64);
    for word in h.iter() {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.truncate(out_len);
    out
}

/// Argon2's variable-length hash H' (RFC 9106 §3.3). Prepends LE32(out_len).
fn blake2b_long(out_len: usize, input: &[u8]) -> Vec<u8> {
    let mut prefixed = Vec::with_capacity(4 + input.len());
    prefixed.extend_from_slice(&(out_len as u32).to_le_bytes());
    prefixed.extend_from_slice(input);
    if out_len <= 64 {
        return blake2b(out_len, &prefixed);
    }
    let mut out = Vec::with_capacity(out_len);
    let mut v = blake2b(64, &prefixed); // V1
    out.extend_from_slice(&v[..32]);
    let r = (out_len + 31) / 32 - 2; // number of full 32-byte V-blocks
    for _ in 1..r {
        v = blake2b(64, &v);
        out.extend_from_slice(&v[..32]);
    }
    let last_len = out_len - 32 * r;
    v = blake2b(last_len, &v);
    out.extend_from_slice(&v);
    out
}

/// BlaMka mixing (Argon2's GB): the BLAKE2b round with the fused 64-bit
/// multiply `a += b + 2*lo(a)*lo(b)`.
#[inline]
fn argon2_gb(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize) {
    #[inline]
    fn fma(x: u64, y: u64) -> u64 {
        let xl = x & 0xFFFF_FFFF;
        let yl = y & 0xFFFF_FFFF;
        2u64.wrapping_mul(xl).wrapping_mul(yl)
    }
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(fma(v[a], v[b]));
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]).wrapping_add(fma(v[c], v[d]));
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(fma(v[a], v[b]));
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]).wrapping_add(fma(v[c], v[d]));
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

#[inline]
fn argon2_p(v: &mut [u64; 16]) {
    argon2_gb(v, 0, 4, 8, 12);
    argon2_gb(v, 1, 5, 9, 13);
    argon2_gb(v, 2, 6, 10, 14);
    argon2_gb(v, 3, 7, 11, 15);
    argon2_gb(v, 0, 5, 10, 15);
    argon2_gb(v, 1, 6, 11, 12);
    argon2_gb(v, 2, 7, 8, 13);
    argon2_gb(v, 3, 4, 9, 14);
}

/// Argon2 compression G over two 1024-byte blocks (128 u64 each). When
/// `with_xor`, the result is XORed into `out` (pass > 0); else it overwrites.
fn argon2_fill_block(prev: &[u64; 128], refb: &[u64; 128], out: &mut [u64; 128], with_xor: bool) {
    let mut r = [0u64; 128];
    for i in 0..128 {
        r[i] = prev[i] ^ refb[i];
    }
    let mut q = r;
    for i in 0..8 {
        let mut row = [0u64; 16];
        row.copy_from_slice(&q[16 * i..16 * i + 16]);
        argon2_p(&mut row);
        q[16 * i..16 * i + 16].copy_from_slice(&row);
    }
    for i in 0..8 {
        let idx = [
            2 * i,
            2 * i + 1,
            2 * i + 16,
            2 * i + 17,
            2 * i + 32,
            2 * i + 33,
            2 * i + 48,
            2 * i + 49,
            2 * i + 64,
            2 * i + 65,
            2 * i + 80,
            2 * i + 81,
            2 * i + 96,
            2 * i + 97,
            2 * i + 112,
            2 * i + 113,
        ];
        let mut col = [0u64; 16];
        for k in 0..16 {
            col[k] = q[idx[k]];
        }
        argon2_p(&mut col);
        for k in 0..16 {
            q[idx[k]] = col[k];
        }
    }
    for i in 0..128 {
        let z = r[i] ^ q[i];
        if with_xor {
            out[i] ^= z;
        } else {
            out[i] = z;
        }
    }
}

/// Full Argon2id (RFC 9106) with secret key `K` and associated data `X`.
/// `m_kib` is memory in KiB (= number of 1 KiB blocks). Writes `out.len()`
/// bytes of tag.
#[allow(clippy::too_many_arguments)]
pub fn argon2id_full(
    password: &[u8],
    salt: &[u8],
    secret: &[u8],
    ad: &[u8],
    t_cost: u32,
    m_kib: u32,
    parallelism: u32,
    out: &mut [u8],
) {
    let tag_len = out.len();
    let p = parallelism.max(1);
    let m = m_kib.max(8 * p);
    let m_prime = 4 * p * (m / (4 * p)); // multiple of 4p
    let lane_len = m_prime / p; // q columns per lane
    let seg_len = lane_len / 4; // segment length

    // H0 = BLAKE2b-512 of the parameter pre-hash.
    let mut h0_input = Vec::new();
    h0_input.extend_from_slice(&p.to_le_bytes());
    h0_input.extend_from_slice(&(tag_len as u32).to_le_bytes());
    h0_input.extend_from_slice(&m.to_le_bytes());
    h0_input.extend_from_slice(&t_cost.to_le_bytes());
    h0_input.extend_from_slice(&0x13u32.to_le_bytes()); // version 1.3
    h0_input.extend_from_slice(&2u32.to_le_bytes()); // type = Argon2id
    h0_input.extend_from_slice(&(password.len() as u32).to_le_bytes());
    h0_input.extend_from_slice(password);
    h0_input.extend_from_slice(&(salt.len() as u32).to_le_bytes());
    h0_input.extend_from_slice(salt);
    h0_input.extend_from_slice(&(secret.len() as u32).to_le_bytes());
    h0_input.extend_from_slice(secret);
    h0_input.extend_from_slice(&(ad.len() as u32).to_le_bytes());
    h0_input.extend_from_slice(ad);
    let h0 = blake2b(64, &h0_input);

    let num_blocks = m_prime as usize;
    let mut blocks: Vec<[u64; 128]> = vec![[0u64; 128]; num_blocks];
    let bidx = |lane: u32, col: u32| -> usize { (lane * lane_len + col) as usize };
    let load_block = |dst: &mut [u64; 128], bytes: &[u8]| {
        for i in 0..128 {
            dst[i] = u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap());
        }
    };

    // First two columns of every lane from H'(1024, H0 || LE32(col) || LE32(lane)).
    for lane in 0..p {
        for col in 0..2u32 {
            let mut input = h0.clone();
            input.extend_from_slice(&col.to_le_bytes());
            input.extend_from_slice(&lane.to_le_bytes());
            let blk = blake2b_long(1024, &input);
            let mut words = [0u64; 128];
            load_block(&mut words, &blk);
            blocks[bidx(lane, col)] = words;
        }
    }

    // Fill the matrix.
    for pass in 0..t_cost {
        for slice in 0..4u32 {
            for lane in 0..p {
                // Argon2id: data-independent addressing on pass 0, slices 0 & 1.
                let data_independent = pass == 0 && slice < 2;
                let mut input_block = [0u64; 128];
                let mut address_block = [0u64; 128];
                let mut addr_counter: u64 = 0;
                if data_independent {
                    input_block[0] = pass as u64;
                    input_block[1] = lane as u64;
                    input_block[2] = slice as u64;
                    input_block[3] = m_prime as u64;
                    input_block[4] = t_cost as u64;
                    input_block[5] = 2; // Argon2id
                }

                let start_col = if pass == 0 && slice == 0 { 2 } else { 0 };
                for idx in start_col..seg_len {
                    let col = slice * seg_len + idx;
                    let prev_col = if col == 0 { lane_len - 1 } else { col - 1 };
                    let prev_block = blocks[bidx(lane, prev_col)];

                    let (j1, j2): (u64, u64);
                    if data_independent {
                        if idx % 128 == 0 {
                            addr_counter += 1;
                            input_block[6] = addr_counter;
                            let zero = [0u64; 128];
                            let mut tmp = [0u64; 128];
                            argon2_fill_block(&zero, &input_block, &mut tmp, false);
                            argon2_fill_block(&zero, &tmp, &mut address_block, false);
                        }
                        let word = address_block[(idx % 128) as usize];
                        j1 = word & 0xFFFF_FFFF;
                        j2 = word >> 32;
                    } else {
                        let word = prev_block[0];
                        j1 = word & 0xFFFF_FFFF;
                        j2 = word >> 32;
                    }

                    let ref_lane = if pass == 0 && slice == 0 {
                        lane
                    } else {
                        (j2 % p as u64) as u32
                    };
                    let same_lane = ref_lane == lane;

                    // Reference area size (RFC 9106 §3.4.1.2).
                    let ref_area_size: i64 = if pass == 0 {
                        if slice == 0 {
                            idx as i64 - 1
                        } else if same_lane {
                            (slice * seg_len + idx) as i64 - 1
                        } else {
                            (slice * seg_len) as i64 - if idx == 0 { 1 } else { 0 }
                        }
                    } else if same_lane {
                        (lane_len - seg_len + idx) as i64 - 1
                    } else {
                        (lane_len - seg_len) as i64 - if idx == 0 { 1 } else { 0 }
                    };
                    let ref_area = ref_area_size.max(0) as u64;

                    let mut rel = j1;
                    rel = (rel * rel) >> 32;
                    rel = ref_area
                        .wrapping_sub(1)
                        .wrapping_sub((ref_area * rel) >> 32);
                    let start_pos: u64 = if pass != 0 && slice != 3 {
                        ((slice + 1) * seg_len) as u64
                    } else {
                        0
                    };
                    let ref_col = ((start_pos + rel) % lane_len as u64) as u32;

                    let ref_block = blocks[bidx(ref_lane, ref_col)];
                    let with_xor = pass != 0;
                    let mut new_block = blocks[bidx(lane, col)];
                    argon2_fill_block(&prev_block, &ref_block, &mut new_block, with_xor);
                    blocks[bidx(lane, col)] = new_block;
                }
            }
        }
    }

    // Final block = XOR of the last column across all lanes; tag = H'(T, C).
    let mut final_block = blocks[bidx(0, lane_len - 1)];
    for lane in 1..p {
        let b = blocks[bidx(lane, lane_len - 1)];
        for i in 0..128 {
            final_block[i] ^= b[i];
        }
    }
    let mut final_bytes = Vec::with_capacity(1024);
    for word in final_block.iter() {
        final_bytes.extend_from_slice(&word.to_le_bytes());
    }
    let tag = blake2b_long(tag_len, &final_bytes);
    let n = core::cmp::min(out.len(), tag.len());
    out[..n].copy_from_slice(&tag[..n]);
}

/// Argon2id with no secret/associated-data — the common password/key entry
/// point. Memory-hard per the `memory_kib`/`parallelism` cost parameters.
pub fn argon2id_derive(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    memory_kib: u32,
    parallelism: u8,
    output: &mut [u8],
) {
    argon2id_full(
        password,
        salt,
        &[],
        &[],
        iterations.max(1),
        memory_kib,
        parallelism.max(1) as u32,
        output,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake2b_abc_rfc7693() {
        let got = blake2b(64, b"abc");
        let want: [u8; 64] = [
            0xba, 0x80, 0xa5, 0x3f, 0x98, 0x1c, 0x4d, 0x0d, 0x6a, 0x27, 0x97, 0xb6, 0x9f, 0x12,
            0xf6, 0xe9, 0x4c, 0x21, 0x2f, 0x14, 0x68, 0x5a, 0xc4, 0xb7, 0x4b, 0x12, 0xbb, 0x6f,
            0xdb, 0xff, 0xa2, 0xd1, 0x7d, 0x87, 0xc5, 0x39, 0x2a, 0xab, 0x79, 0x2d, 0xc2, 0x52,
            0xd5, 0xde, 0x45, 0x33, 0xcc, 0x95, 0x18, 0xd3, 0x8a, 0xa8, 0xdb, 0xf1, 0x92, 0x5a,
            0xb9, 0x23, 0x86, 0xed, 0xd4, 0x00, 0x99, 0x23,
        ];
        assert_eq!(got.as_slice(), want.as_slice());
    }

    #[test]
    fn argon2id_rfc9106_kat() {
        let mut tag = [0u8; 32];
        argon2id_full(
            &[0x01; 32],
            &[0x02; 16],
            &[0x03; 8],
            &[0x04; 12],
            3,
            32,
            4,
            &mut tag,
        );
        let want: [u8; 32] = [
            0x0d, 0x64, 0x0d, 0xf5, 0x8d, 0x78, 0x76, 0x6c, 0x08, 0xc0, 0x37, 0xa3, 0x4a, 0x8b,
            0x53, 0xc9, 0xd0, 0x1e, 0xf0, 0x45, 0x2d, 0x75, 0xb6, 0x5e, 0xb5, 0x25, 0x20, 0xe9,
            0x6b, 0x01, 0xe6, 0x59,
        ];
        assert_eq!(tag, want);
    }
}
