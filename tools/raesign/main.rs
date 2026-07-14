// raesign — Ed25519 (RFC 8032) detached-signature tool for RaeenOS secure boot,
// atomic-update verification, and app-bundle code signing. Built on the shared
// `rae_crypto` Ed25519 so signer and on-device verifier are the same code.
//
// Subcommands:
//   raesign selftest
//       Run the RFC 8032 §7.1 known-answer test (proves the tool's crypto).
//   raesign keygen <passphrase>
//       Derive a signing key from a passphrase via Argon2id (deterministic,
//       memory-hard). Prints seed + public key as hex. Reproducible — no key
//       file to leak; for production use an HSM instead of a passphrase.
//   raesign pubkey <seed-hex>
//       Print the public key (hex) for a 32-byte seed.
//   raesign sign <in-file> <seed-hex> <out-sig>
//       Write a 64-byte detached signature of <in-file> to <out-sig>.
//   raesign verify <in-file> <sig-file> <pubkey-hex>
//       Verify <in-file> against the 64-byte <sig-file> under <pubkey-hex>.
//       Exit 0 = valid, 1 = invalid/forged, 2 = usage/IO error.

use std::process::exit;

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err("hex string has odd length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("bad hex: {e}")))
        .collect()
}

fn read_seed(hex: &str) -> [u8; 32] {
    let v = hex_decode(hex).unwrap_or_else(|e| fail(&format!("seed: {e}")));
    if v.len() != 32 {
        fail("seed must be 32 bytes (64 hex chars)");
    }
    let mut s = [0u8; 32];
    s.copy_from_slice(&v);
    s
}

fn fail(msg: &str) -> ! {
    eprintln!("raesign: {msg}");
    exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }
    match args[1].as_str() {
        "selftest" => cmd_selftest(),
        "keygen" if args.len() == 3 => cmd_keygen(&args[2]),
        "pubkey" if args.len() == 3 => cmd_pubkey(&args[2]),
        "sign" if args.len() == 5 => cmd_sign(&args[2], &args[3], &args[4]),
        "verify" if args.len() == 5 => cmd_verify(&args[2], &args[3], &args[4]),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         raesign selftest\n  \
         raesign keygen <passphrase>\n  \
         raesign pubkey <seed-hex>\n  \
         raesign sign <in-file> <seed-hex> <out-sig>\n  \
         raesign verify <in-file> <sig-file> <pubkey-hex>"
    );
    exit(2);
}

fn cmd_selftest() {
    // RFC 8032 §7.1 Test 2.
    let seed = read_seed("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb");
    let msg = [0x72u8];
    let expect_pk = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";
    let expect_sig = "92a009a9f0d4cab8720e820b5f6425\
                      40a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00";
    let pk = rae_crypto::ed25519::derive_public_key(&seed);
    let sig = rae_crypto::ed25519::sign(&seed, &msg);
    let pk_ok = hex_encode(&pk) == expect_pk;
    let sig_ok = hex_encode(&sig) == expect_sig;
    let verify_ok = rae_crypto::ed25519::verify(&pk, &msg, &sig);
    let forge_rejected = !rae_crypto::ed25519::verify(&pk, b"forged", &sig);
    println!("pubkey   = {} -> {}", hex_encode(&pk), pass(pk_ok));
    println!("sign     = {}... -> {}", &hex_encode(&sig)[..32], pass(sig_ok));
    println!("verify   -> {}", pass(verify_ok));
    println!("forgery  rejected -> {}", pass(forge_rejected));
    if pk_ok && sig_ok && verify_ok && forge_rejected {
        println!("\nRFC 8032 KAT: PASS");
        exit(0);
    }
    println!("\nRFC 8032 KAT: FAIL");
    exit(1);
}

fn pass(ok: bool) -> &'static str {
    if ok {
        "PASS"
    } else {
        "FAIL"
    }
}

fn cmd_keygen(passphrase: &str) {
    // Deterministic key from a passphrase via Argon2id (memory-hard). The salt
    // is fixed + domain-separated so the same passphrase always yields the same
    // signing key — reproducible, nothing on disk to leak.
    let salt = b"raesign-keygen-1";
    let mut seed = [0u8; 32];
    rae_crypto::argon2id_derive(passphrase.as_bytes(), salt, 3, 8_192, 1, &mut seed);
    let pk = rae_crypto::ed25519::derive_public_key(&seed);
    println!("seed   = {}", hex_encode(&seed));
    println!("pubkey = {}", hex_encode(&pk));
    eprintln!("note: keep the passphrase/seed secret; embed only the pubkey in the verifier.");
}

fn cmd_pubkey(seed_hex: &str) {
    let seed = read_seed(seed_hex);
    let pk = rae_crypto::ed25519::derive_public_key(&seed);
    println!("{}", hex_encode(&pk));
}

fn cmd_sign(in_file: &str, seed_hex: &str, out_sig: &str) {
    let seed = read_seed(seed_hex);
    let data = std::fs::read(in_file).unwrap_or_else(|e| fail(&format!("read {in_file}: {e}")));
    let sig = rae_crypto::ed25519::sign(&seed, &data);
    std::fs::write(out_sig, sig).unwrap_or_else(|e| fail(&format!("write {out_sig}: {e}")));
    eprintln!(
        "signed {} ({} bytes) -> {} (64-byte Ed25519 detached signature)",
        in_file,
        data.len(),
        out_sig
    );
    println!("{}", hex_encode(&sig));
}

fn cmd_verify(in_file: &str, sig_file: &str, pubkey_hex: &str) {
    let pkv = hex_decode(pubkey_hex).unwrap_or_else(|e| fail(&format!("pubkey: {e}")));
    if pkv.len() != 32 {
        fail("pubkey must be 32 bytes (64 hex chars)");
    }
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&pkv);

    let data = std::fs::read(in_file).unwrap_or_else(|e| fail(&format!("read {in_file}: {e}")));
    let sigv = std::fs::read(sig_file).unwrap_or_else(|e| fail(&format!("read {sig_file}: {e}")));
    if sigv.len() != 64 {
        fail("signature file must be exactly 64 bytes");
    }
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&sigv);

    if rae_crypto::ed25519::verify(&pk, &data, &sig) {
        println!("OK: {} signature is valid", in_file);
        exit(0);
    } else {
        println!("FAIL: {} signature is INVALID", in_file);
        exit(1);
    }
}
