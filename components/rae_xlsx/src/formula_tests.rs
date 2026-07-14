// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite for the formula evaluator — `cargo test -p rae_xlsx`.
// FAIL-able by construction: every assert checks an exact computed value.
//
// Under `#[cfg(test)]` the crate is `std`, so `Vec`/`String`/`vec!` are in the
// prelude — no `use std::` / `extern crate std` (the architecture gate §R7 bans
// those lines in a no_std crate).
// ════════════════════════════════════════════════════════════════════════════

use crate::formula::{eval_formula, evaluate};
use crate::{Cell, CellValue, Sheet};

// ─── Sheet construction helpers (crate-private fields reachable from this child
// module) ────────────────────────────────────────────────────────────────────

fn empty_sheet() -> Sheet {
    Sheet::default()
}

fn set_literal(sheet: &mut Sheet, a1: &str, v: CellValue) {
    let (col, row) = crate::parse_a1(a1).expect("test a1");
    let _ = crate::push_cell(
        sheet,
        Cell {
            col,
            row,
            value: v,
            formula: None,
        },
    );
}

fn set_formula(sheet: &mut Sheet, a1: &str, f: &str) {
    let (col, row) = crate::parse_a1(a1).expect("test a1");
    let _ = crate::push_cell(
        sheet,
        Cell {
            col,
            row,
            value: CellValue::Empty,
            formula: Some(String::from(f)),
        },
    );
}

fn num(n: f64) -> CellValue {
    CellValue::Number(n)
}
fn txt(s: &str) -> CellValue {
    CellValue::Text(String::from(s))
}

/// Compare a Number result to an expected value within a small epsilon.
fn assert_num(got: &CellValue, expected: f64) {
    match got {
        CellValue::Number(n) => {
            assert!((n - expected).abs() < 1e-9, "expected {expected}, got {n}");
        }
        other => panic!("expected Number({expected}), got {other:?}"),
    }
}

fn assert_err(got: &CellValue, lit: &str) {
    match got {
        CellValue::Error(s) => assert_eq!(s, lit, "wrong error literal"),
        other => panic!("expected Error({lit}), got {other:?}"),
    }
}

fn ev(f: &str) -> CellValue {
    eval_formula(&empty_sheet(), f)
}

// ─── Arithmetic + precedence ─────────────────────────────────────────────────

#[test]
fn arithmetic_precedence() {
    assert_num(&ev("=1+2*3"), 7.0); // * before +
    assert_num(&ev("=(1+2)*3"), 9.0); // parens
    assert_num(&ev("=2^10"), 1024.0); // power
    assert_num(&ev("=2^3^2"), 512.0); // right-assoc: 2^(3^2)=2^9
    assert_num(&ev("=10-4-3"), 3.0); // left-assoc subtraction
    assert_num(&ev("=20/4/5"), 1.0); // left-assoc division
    assert_num(&ev("=-3^2"), 9.0); // unary minus binds looser than ^ here: -(3^2)? Excel: (-3)^2=9
    assert_num(&ev("=50%"), 0.5); // percent postfix
    assert_num(&ev("=10+5%"), 10.05); // 10 + 0.05
}

#[test]
fn arithmetic_fails_loudly() {
    // FAIL-able guard: a wrong precedence would make this 9, not 7.
    let r = ev("=1+2*3");
    assert!(matches!(r, CellValue::Number(n) if (n - 7.0).abs() < 1e-9));
    assert!(!matches!(r, CellValue::Number(n) if (n - 9.0).abs() < 1e-9));
}

#[test]
fn division_by_zero() {
    assert_err(&ev("=10/0"), "#DIV/0!");
    assert_err(&ev("=1/(2-2)"), "#DIV/0!");
}

#[test]
fn unary_and_signs() {
    assert_num(&ev("=-5"), -5.0);
    assert_num(&ev("=--5"), 5.0);
    assert_num(&ev("=+7"), 7.0);
    assert_num(&ev("=3*-2"), -6.0);
}

// ─── Comparisons + booleans ──────────────────────────────────────────────────

#[test]
fn comparisons() {
    assert_eq!(ev("=3<>4"), CellValue::Bool(true));
    assert_eq!(ev("=3=3"), CellValue::Bool(true));
    assert_eq!(ev("=3<4"), CellValue::Bool(true));
    assert_eq!(ev("=4<=4"), CellValue::Bool(true));
    assert_eq!(ev("=5>2"), CellValue::Bool(true));
    assert_eq!(ev("=5>=6"), CellValue::Bool(false));
    assert_eq!(ev("=2>3"), CellValue::Bool(false));
}

#[test]
fn boolean_literals_and_logic() {
    assert_eq!(ev("=TRUE"), CellValue::Bool(true));
    assert_eq!(ev("=FALSE"), CellValue::Bool(false));
    assert_eq!(ev("=AND(TRUE,TRUE)"), CellValue::Bool(true));
    assert_eq!(ev("=AND(TRUE,FALSE)"), CellValue::Bool(false));
    assert_eq!(ev("=OR(FALSE,TRUE)"), CellValue::Bool(true));
    assert_eq!(ev("=OR(FALSE,FALSE)"), CellValue::Bool(false));
    assert_eq!(ev("=NOT(TRUE)"), CellValue::Bool(false));
    assert_eq!(ev("=NOT(FALSE)"), CellValue::Bool(true));
}

// ─── Aggregates over ranges ──────────────────────────────────────────────────

fn sheet_a1_a3_123() -> Sheet {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(1.0));
    set_literal(&mut s, "A2", num(2.0));
    set_literal(&mut s, "A3", num(3.0));
    s
}

#[test]
fn sum_range() {
    let s = sheet_a1_a3_123();
    assert_num(&eval_formula(&s, "=SUM(A1:A3)"), 6.0);
    // FAIL-able: an off-by-one range would give 3 or 5.
    assert!(
        !matches!(eval_formula(&s, "=SUM(A1:A3)"), CellValue::Number(n) if (n - 5.0).abs() < 1e-9)
    );
}

#[test]
fn average_min_max_count() {
    let s = sheet_a1_a3_123();
    assert_num(&eval_formula(&s, "=AVERAGE(A1:A3)"), 2.0);
    assert_num(&eval_formula(&s, "=MAX(A1:A3)"), 3.0);
    assert_num(&eval_formula(&s, "=MIN(A1:A3)"), 1.0);
    assert_num(&eval_formula(&s, "=COUNT(A1:A3)"), 3.0);
    assert_num(&eval_formula(&s, "=COUNTA(A1:A3)"), 3.0);
}

#[test]
fn sum_mixed_args() {
    let s = sheet_a1_a3_123();
    // Scalar + range mixing.
    assert_num(&eval_formula(&s, "=SUM(A1:A3,10)"), 16.0);
    assert_num(&eval_formula(&s, "=SUM(A1,A2,A3)"), 6.0);
}

#[test]
fn empty_cells_in_sum_are_zero() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(5.0));
    // A2 omitted (empty), A3 present.
    set_literal(&mut s, "A3", num(7.0));
    assert_num(&eval_formula(&s, "=SUM(A1:A3)"), 12.0);
    assert_num(&eval_formula(&s, "=COUNT(A1:A3)"), 2.0); // empty not counted
}

#[test]
fn text_in_range_skipped_by_sum() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(1.0));
    set_literal(&mut s, "A2", txt("hello"));
    set_literal(&mut s, "A3", num(3.0));
    assert_num(&eval_formula(&s, "=SUM(A1:A3)"), 4.0);
    assert_num(&eval_formula(&s, "=COUNT(A1:A3)"), 2.0);
    assert_num(&eval_formula(&s, "=COUNTA(A1:A3)"), 3.0);
}

// ─── IF + conditionals ───────────────────────────────────────────────────────

#[test]
fn if_function() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(5.0));
    assert_eq!(eval_formula(&s, "=IF(A1>2,\"big\",\"small\")"), txt("big"));
    set_literal(&mut s, "B1", num(1.0));
    assert_eq!(
        eval_formula(&s, "=IF(B1>2,\"big\",\"small\")"),
        txt("small")
    );
    // IF with numeric branches.
    assert_num(&eval_formula(&s, "=IF(A1>2,100,200)"), 100.0);
}

#[test]
fn sumif_averageif() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(1.0));
    set_literal(&mut s, "A2", num(5.0));
    set_literal(&mut s, "A3", num(9.0));
    assert_num(&eval_formula(&s, "=SUMIF(A1:A3,\">4\")"), 14.0);
    assert_num(&eval_formula(&s, "=AVERAGEIF(A1:A3,\">4\")"), 7.0);
    assert_num(&eval_formula(&s, "=SUMIF(A1:A3,\"<=5\")"), 6.0);
}

// ─── Math functions ──────────────────────────────────────────────────────────

#[test]
fn math_functions() {
    assert_num(&ev("=ABS(-5)"), 5.0);
    assert_num(&ev("=ABS(5)"), 5.0);
    assert_num(&ev("=ROUND(3.14159,2)"), 3.14);
    assert_num(&ev("=ROUND(2.5,0)"), 3.0); // half away from zero
    assert_num(&ev("=ROUNDUP(2.1,0)"), 3.0);
    assert_num(&ev("=ROUNDDOWN(2.9,0)"), 2.0);
    assert_num(&ev("=INT(3.9)"), 3.0);
    assert_num(&ev("=INT(-3.1)"), -4.0); // floor
    assert_num(&ev("=MOD(10,3)"), 1.0);
    assert_num(&ev("=MOD(-10,3)"), 2.0); // sign of divisor
    assert_num(&ev("=SQRT(16)"), 4.0);
    assert_num(&ev("=POWER(2,10)"), 1024.0);
    assert_num(&ev("=POWER(9,0.5)"), 3.0); // fractional exponent
}

#[test]
fn sqrt_negative_is_error() {
    assert_err(&ev("=SQRT(-1)"), "#N/A");
}

// ─── Text functions ──────────────────────────────────────────────────────────

#[test]
fn text_functions() {
    assert_eq!(ev("=LEFT(\"hello\",2)"), txt("he"));
    assert_eq!(ev("=RIGHT(\"hello\",3)"), txt("llo"));
    assert_eq!(ev("=MID(\"hello\",2,3)"), txt("ell"));
    assert_num(&ev("=LEN(\"hello\")"), 5.0);
    assert_eq!(ev("=UPPER(\"abc\")"), txt("ABC"));
    assert_eq!(ev("=LOWER(\"ABC\")"), txt("abc"));
    assert_eq!(ev("=TRIM(\"  a  b  \")"), txt("a b"));
}

#[test]
fn concat_and_amp() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(1.0));
    set_literal(&mut s, "B1", txt("x"));
    assert_eq!(eval_formula(&s, "=A1&B1"), txt("1x"));
    assert_eq!(eval_formula(&s, "=CONCATENATE(A1,\"x\")"), txt("1x"));
    assert_eq!(ev("=\"a\"&\"b\"&\"c\""), txt("abc"));
    // number renders compactly in concat
    assert_eq!(ev("=\"v=\"&2.5"), txt("v=2.5"));
}

// ─── Cell references resolve ─────────────────────────────────────────────────

#[test]
fn cell_refs_and_absolute() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(10.0));
    set_literal(&mut s, "B2", num(3.0));
    assert_num(&eval_formula(&s, "=A1+B2*3"), 19.0);
    // Absolute markers parse the same.
    assert_num(&eval_formula(&s, "=$A$1+$B$2"), 13.0);
}

// ─── Dependency-ordered recalc (the load-bearing engine test) ────────────────

#[test]
fn dependency_chain_order() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(1.0));
    set_formula(&mut s, "B1", "=A1+1");
    set_formula(&mut s, "C1", "=B1+1");
    // Deliberately insert C1 BEFORE B1 was resolved — evaluate() must topo-order.
    evaluate(&mut s);
    assert_eq!(s.cell(0, 0), Some(&num(1.0))); // A1
    assert_eq!(s.cell(1, 0), Some(&num(2.0))); // B1
    assert_eq!(s.cell(2, 0), Some(&num(3.0))); // C1
}

#[test]
fn dependency_chain_reverse_insertion() {
    // Insert C1 first, then B1, then A1 — order independence proof.
    let mut s = empty_sheet();
    set_formula(&mut s, "C1", "=B1+1");
    set_formula(&mut s, "B1", "=A1+1");
    set_literal(&mut s, "A1", num(10.0));
    evaluate(&mut s);
    assert_eq!(s.cell(2, 0), Some(&num(12.0))); // C1
    assert_eq!(s.cell(1, 0), Some(&num(11.0))); // B1
}

#[test]
fn evaluate_sum_of_formula_cells() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(2.0));
    set_formula(&mut s, "A2", "=A1*2"); // 4
    set_formula(&mut s, "A3", "=A2+A1"); // 6
    set_formula(&mut s, "A4", "=SUM(A1:A3)"); // 2+4+6 = 12
    evaluate(&mut s);
    assert_eq!(s.cell(0, 1), Some(&num(4.0)));
    assert_eq!(s.cell(0, 2), Some(&num(6.0)));
    assert_eq!(s.cell(0, 3), Some(&num(12.0)));
}

// ─── Cycle detection — the never-hang safety assert ──────────────────────────

#[test]
fn direct_cycle_is_circ_no_hang() {
    let mut s = empty_sheet();
    set_formula(&mut s, "A1", "=B1");
    set_formula(&mut s, "B1", "=A1");
    evaluate(&mut s); // MUST return (no infinite loop)
    assert_err(s.cell(0, 0).unwrap(), "#CIRC");
    assert_err(s.cell(1, 0).unwrap(), "#CIRC");
}

#[test]
fn self_cycle_is_circ() {
    let mut s = empty_sheet();
    set_formula(&mut s, "A1", "=A1+1");
    evaluate(&mut s);
    assert_err(s.cell(0, 0).unwrap(), "#CIRC");
}

#[test]
fn longer_cycle_is_circ() {
    let mut s = empty_sheet();
    set_formula(&mut s, "A1", "=B1");
    set_formula(&mut s, "B1", "=C1");
    set_formula(&mut s, "C1", "=A1");
    evaluate(&mut s);
    assert_err(s.cell(0, 0).unwrap(), "#CIRC");
    assert_err(s.cell(1, 0).unwrap(), "#CIRC");
    assert_err(s.cell(2, 0).unwrap(), "#CIRC");
}

#[test]
fn cycle_does_not_poison_independent_cells() {
    let mut s = empty_sheet();
    set_literal(&mut s, "D1", num(5.0));
    set_formula(&mut s, "D2", "=D1*2"); // independent, must compute = 10
    set_formula(&mut s, "A1", "=B1"); // cycle
    set_formula(&mut s, "B1", "=A1");
    evaluate(&mut s);
    assert_eq!(s.cell(3, 1), Some(&num(10.0))); // D2 fine
    assert_err(s.cell(0, 0).unwrap(), "#CIRC");
}

// ─── Error values + propagation ──────────────────────────────────────────────

#[test]
fn unknown_function_is_name_error() {
    assert_err(&ev("=FLORBLE(1,2)"), "#NAME?");
    assert_err(&ev("=NOTAREALFN()"), "#NAME?");
}

#[test]
fn value_error_for_text_arithmetic() {
    // "=1+\"abc\"" — non-numeric text in arithmetic → #VALUE!.
    assert_err(&ev("=1+\"abc\""), "#VALUE!");
}

#[test]
fn numeric_string_coerces_in_arithmetic() {
    // A numeric string DOES coerce (documented rule).
    assert_num(&ev("=1+\"2\""), 3.0);
    assert_num(&ev("=\"10\"*\"2\""), 20.0);
}

#[test]
fn error_propagates_through_operators() {
    assert_err(&ev("=10/0+1"), "#DIV/0!");
    assert_err(&ev("=SUM(10/0,5)"), "#DIV/0!");
    assert_err(&ev("=1+SQRT(-1)"), "#N/A");
}

#[test]
fn ref_error_for_bad_ref() {
    // A range too large to expand → #REF!.
    assert_err(&ev("=SUM(A1:XFD1048576)"), "#REF!");
}

// ─── Malformed input → graceful error, never panic ───────────────────────────

#[test]
fn malformed_formulas_dont_panic() {
    assert_err(&ev("=1+"), "#VALUE!");
    assert_err(&ev("=(1+2"), "#VALUE!"); // unbalanced paren
    assert_err(&ev("=*5"), "#VALUE!");
    assert_err(&ev("=\"unterminated"), "#VALUE!");
    assert_err(&ev("="), "#VALUE!");
    assert_err(&ev("=,"), "#VALUE!");
    assert_err(&ev("=SUM("), "#VALUE!");
}

// ─── Bounded: depth + size guards (never stack-overflow / hang) ───────────────

#[test]
fn deep_nesting_is_bounded() {
    // 10000-deep parenthesization must return an error, not overflow the stack.
    let mut f = String::from("=");
    for _ in 0..10_000 {
        f.push('(');
    }
    f.push('1');
    for _ in 0..10_000 {
        f.push(')');
    }
    let r = eval_formula(&empty_sheet(), &f);
    assert!(
        matches!(r, CellValue::Error(_)),
        "expected bounded error, got {r:?}"
    );
}

#[test]
fn over_long_formula_is_bounded() {
    let mut f = String::from("=1");
    for _ in 0..(crate::MAX_FORMULA_LEN + 100) {
        f.push_str("+1");
    }
    let r = eval_formula(&empty_sheet(), &f);
    assert_err(&r, "#VALUE!");
}

#[test]
fn giant_range_is_bounded() {
    // Past MAX_RANGE_CELLS → #REF!, not a multi-billion-cell hang.
    let r = ev("=SUM(A1:XFD1048576)");
    assert_err(&r, "#REF!");
}

// ─── Seeded fuzz: never panic over arbitrary formula strings ──────────────────

#[test]
fn seeded_fuzz_never_panics() {
    // A tiny xorshift PRNG seeds pseudo-random formula strings from a token alphabet;
    // every result must be a CellValue (no panic), proving the never-panic posture.
    let alphabet = b"=+-*/^%&()<>,:.0123456789ABCDEF\"$ SUMIF";
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let s = empty_sheet();
    for _ in 0..4000 {
        let len = (next() % 40) as usize;
        let mut f = String::from("=");
        for _ in 0..len {
            let idx = (next() as usize) % alphabet.len();
            f.push(alphabet[idx] as char);
        }
        // Must not panic; we don't care what the value is.
        let _ = eval_formula(&s, &f);
    }
}

#[test]
fn fuzz_with_references_against_real_sheet() {
    let s = sheet_a1_a3_123();
    let alphabet = b"=+-*ABC123():,SUM";
    let mut state: u64 = 0xDEAD_BEEF_CAFE_F00D;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..4000 {
        let len = (next() % 30) as usize;
        let mut f = String::from("=");
        for _ in 0..len {
            let idx = (next() as usize) % alphabet.len();
            f.push(alphabet[idx] as char);
        }
        let _ = eval_formula(&s, &f);
    }
}

// ─── evaluate() over a sheet with no formulas is a no-op ─────────────────────

#[test]
fn evaluate_no_formulas_noop() {
    let mut s = sheet_a1_a3_123();
    evaluate(&mut s);
    assert_eq!(s.cell(0, 0), Some(&num(1.0)));
    assert_eq!(s.cell(0, 2), Some(&num(3.0)));
}

// ─── Malformed formula in a cell during evaluate → #VALUE!, not panic ────────

#[test]
fn evaluate_malformed_cell_formula() {
    let mut s = empty_sheet();
    set_literal(&mut s, "A1", num(1.0));
    set_formula(&mut s, "A2", "=A1+"); // malformed
    evaluate(&mut s);
    assert_err(s.cell(0, 1).unwrap(), "#VALUE!");
}
