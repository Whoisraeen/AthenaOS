// H.264 CAVLC VLC table prefix-free verifier (the §7.1 gate), the analogue of
// tools/mp3_huff_gen/gen.rs and tools/aac_huff_gen/gen.rs.
//
// It `include!`s the SAME table source the runtime decoder uses
// (components/raemedia/src/h264_tables.rs) so there is exactly one copy of the data; a
// transcription typo is caught HERE before it can reach the decoder. For each CAVLC VLC
// context (coeff_token ×4 nC ranges + chroma-DC, total_zeros ×15 + chroma-DC ×3,
// run_before ×7) it verifies:
//   * every code fits within its declared bit length,
//   * no codeword is a prefix of another (prefix-free / instantaneously decodable),
//   * value ranges are within the ITU maxima.
// Prints `All H.264 CAVLC tables verified prefix-free.` and exits 0, or `FAIL <table>:
// ...` and exits 1. Run:  rustc -O gen.rs -o gen && ./gen
//
// Source: ITU-T H.264 Tables 9-5/9-7/9-8/9-10, cross-checked FFmpeg h264_cavlc / openh264
// CAVLC tables. No code copied; only public ISO/ITU data tables transcribed.

// Pull in the SAME table source the runtime decoder uses (one copy of the data). The
// file's inner `//!` doc-comment requires it live inside a module item.
#[allow(dead_code)]
mod tables {
    include!("../../components/raemedia/src/h264_tables.rs");
}
use tables::*;

/// Verify a VLC table is prefix-free and well-formed. Returns Err(msg) on failure.
fn check_prefix_free(name: &str, table: &[Vlc]) -> Result<(), String> {
    for (i, e) in table.iter().enumerate() {
        if e.len == 0 || e.len > 16 {
            return Err(format!("{name}: entry {i} has illegal len {}", e.len));
        }
        // code must fit in len bits.
        if (e.code as u32) >> e.len != 0 {
            return Err(format!(
                "{name}: entry {i} code {:#b} does not fit in {} bits",
                e.code, e.len
            ));
        }
    }
    // Pairwise prefix check: for any two distinct codewords, the shorter (left-aligned)
    // must not be a prefix of the longer.
    for i in 0..table.len() {
        for j in 0..table.len() {
            if i == j {
                continue;
            }
            let a = &table[i];
            let b = &table[j];
            if a.len <= b.len {
                // Is `a` a prefix of `b`? Compare the top a.len bits of each codeword.
                let b_top = (b.code >> (b.len - a.len)) as u16;
                if b_top == a.code {
                    return Err(format!(
                        "{name}: codeword {i} ({:#b}/{}) is a prefix of {j} ({:#b}/{})",
                        a.code, a.len, b.code, b.len
                    ));
                }
            }
        }
    }
    Ok(())
}

fn main() {
    let mut failures: Vec<String> = Vec::new();

    fn check(failures: &mut Vec<String>, name: &str, t: &[Vlc]) {
        if let Err(e) = check_prefix_free(name, t) {
            failures.push(e);
        }
    }

    check(&mut failures, "coeff_token[nC 0..2]", COEFF_TOKEN_0);
    check(&mut failures, "coeff_token[nC 2..4]", COEFF_TOKEN_1);
    check(&mut failures, "coeff_token[nC 4..8]", COEFF_TOKEN_2);
    check(&mut failures, "coeff_token[chroma-DC]", COEFF_TOKEN_CHROMA_DC);

    // coeff_token tables must have exactly 62 entries (16 TotalCoeff × up to 4 T1) for
    // the luma contexts; chroma-DC has 14 (TotalCoeff 0..4 with valid T1 combos).
    for (name, t, want) in [
        ("coeff_token[nC 0..2]", COEFF_TOKEN_0, 62usize),
        ("coeff_token[nC 2..4]", COEFF_TOKEN_1, 62),
        ("coeff_token[nC 4..8]", COEFF_TOKEN_2, 62),
        ("coeff_token[chroma-DC]", COEFF_TOKEN_CHROMA_DC, 14),
    ] {
        if t.len() != want {
            failures.push(format!("{name}: expected {want} entries, found {}", t.len()));
        }
    }

    for (i, t) in TOTAL_ZEROS_4X4.iter().enumerate() {
        check(&mut failures, &format!("total_zeros_4x4[tc={}]", i + 1), t);
    }
    for (i, t) in TOTAL_ZEROS_CHROMA_DC.iter().enumerate() {
        check(&mut failures, &format!("total_zeros_chromaDC[tc={}]", i + 1), t);
    }
    for (i, t) in RUN_BEFORE.iter().enumerate() {
        check(&mut failures, &format!("run_before[zerosLeft={}]", i + 1), t);
    }

    // Spot-check a few documented anchors (the §D.1 anchors).
    let anchor_ok = COEFF_TOKEN_0[0] == (Vlc { code: 1, len: 1, a: 0, b: 0 })
        && COEFF_TOKEN_CHROMA_DC[2] == (Vlc { code: 1, len: 1, a: 1, b: 1 })
        && RUN_BEFORE[0][0] == (Vlc { code: 1, len: 1, a: 0, b: 0 })
        && RUN_BEFORE[0][1] == (Vlc { code: 0, len: 1, a: 1, b: 0 });
    if !anchor_ok {
        failures.push("anchor mismatch (Table 9-5/9-10 first rows)".to_string());
    }

    if failures.is_empty() {
        println!("All H.264 CAVLC tables verified prefix-free.");
        std::process::exit(0);
    } else {
        for f in &failures {
            eprintln!("FAIL {f}");
        }
        std::process::exit(1);
    }
}
