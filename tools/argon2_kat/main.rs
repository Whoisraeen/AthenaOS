// Host harness for the SHARED `rae_crypto` crate — the exact code the kernel
// (FDE key derivation) and raeid (account password hashing) use. Runs the
// RFC 9106 §5.3 Argon2id and RFC 7693 BLAKE2b known-answer tests on the host,
// which is how we validate the primitive while the QEMU kernel build is
// embargoed. `rae_crypto` also carries these as `#[cfg(test)]` unit tests
// (`cargo test -p rae_crypto`); this binary is the standalone, dependency-free
// way to eyeball the vectors.
//
//   cargo run --release --manifest-path tools/argon2_kat/Cargo.toml

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn main() {
    let mut fail = 0;

    // BLAKE2b-512("abc") — RFC 7693 Appendix A.
    let got = rae_crypto::blake2b(64, b"abc");
    let want = "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1\
                7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923";
    println!("BLAKE2b-512(\"abc\") = {}", hex(&got));
    if hex(&got) == want {
        println!("  -> PASS");
    } else {
        println!("  -> FAIL (want {})", want);
        fail += 1;
    }

    // RFC 9106 §5.3 Argon2id: v19, m=32 KiB, t=3, p=4, password=32x01,
    // salt=16x02, secret=8x03, ad=12x04, 32-byte tag.
    let mut tag = [0u8; 32];
    rae_crypto::argon2id_full(&[0x01; 32], &[0x02; 16], &[0x03; 8], &[0x04; 12], 3, 32, 4, &mut tag);
    let want = "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659";
    println!("Argon2id RFC9106 tag = {}", hex(&tag));
    if hex(&tag) == want {
        println!("  -> PASS");
    } else {
        println!("  -> FAIL (want {})", want);
        fail += 1;
    }

    if fail == 0 {
        println!("\nALL KATs PASS");
        std::process::exit(0);
    } else {
        println!("\n{} KAT(s) FAILED", fail);
        std::process::exit(1);
    }
}
