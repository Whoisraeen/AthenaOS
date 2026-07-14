//! # Formula evaluator — turning a grid back into a *spreadsheet*.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy (criterion #5, "edit my spreadsheet"):
//! a spreadsheet a switcher can only *read* is a grid of frozen numbers — the moment
//! they change a cell, every formula that depended on it is a lie. The reader in
//! [`crate`] captures Excel's cached `<v>`; this module is the recalc engine that
//! makes the cells *mean* something again. It is the difference between "view my
//! files" and "use my files".
//!
//! ## What it is — honestly scoped
//! An A1-style formula evaluator over the common subset real spreadsheets use, not a
//! 400-function Excel clone. It implements:
//!   - a **tokenizer + recursive-descent (precedence-climbing) parser** for the
//!     expression after `=`: numbers, `"strings"`, `TRUE`/`FALSE`, cell references
//!     (`A1`, `$A$1` — absolute markers are accepted and ignored, eval is the same),
//!     ranges (`A1:B3`), the operators `+ - * / ^ %` and comparisons `= <> < <= > >=`
//!     and string concat `&`, unary minus/plus, parentheses, and `NAME(arg, …)`
//!     function calls;
//!   - a **function library** (see [`call_function`]): SUM, AVERAGE, MIN, MAX, COUNT,
//!     COUNTA, IF, AND, OR, NOT, ABS, ROUND, ROUNDUP, ROUNDDOWN, INT, MOD, SQRT,
//!     POWER, LEN, LEFT, RIGHT, MID, UPPER, LOWER, TRIM, CONCATENATE, SUMIF, AVERAGEIF;
//!   - a **dependency-ordered evaluation engine** ([`evaluate`]): it builds the
//!     formula-cell dependency graph, evaluates in topological order, and marks any
//!     cell on a dependency **cycle** with `#CIRC` rather than looping forever.
//!
//! ## Coercion rules (documented, spreadsheet-conventional)
//!   - In **arithmetic** a numeric `Text` (`"12"`, `"-3.5"`, `"1e3"`) coerces to its
//!     number; a non-numeric `Text` yields `#VALUE!`. `Bool(true)=1`, `Bool(false)=0`.
//!     An `Empty` cell is `0`.
//!   - In **aggregates** (SUM/AVERAGE/…), per Excel, `Text` and `Bool` values found
//!     *inside a range* are skipped (not coerced), while a `Bool`/numeric passed as a
//!     *direct scalar argument* counts. `Empty` is skipped. COUNT counts numbers only;
//!     COUNTA counts every non-empty value.
//!   - In **concat** (`&`, CONCATENATE) every value renders to its display string
//!     (number → compact decimal, `TRUE`/`FALSE`, error → the error literal).
//!   - In **comparison** numbers compare numerically; a number always sorts before
//!     text (Excel's type order); text compares case-insensitively; `Bool` sorts after
//!     text. `=`/`<>` on mixed types follow the same ordering.
//!   - Any argument that is an **error** propagates: the result is that same error.
//!
//! ## Bounds (never-hang / never-OOM — the load-bearing safety property)
//!   - A formula string longer than [`MAX_FORMULA_LEN`] is `#VALUE!` (not parsed).
//!   - Parser/eval recursion is depth-capped at [`MAX_EXPR_DEPTH`] → `#VALUE!`,
//!     so a 10000-deep `(((…)))` cannot overflow the stack.
//!   - A single range may expand to at most [`MAX_RANGE_CELLS`] cells → `#REF!`.
//!   - [`evaluate`] processes at most [`MAX_EVAL_CELLS`] formula cells and bounds the
//!     dependency walk, so a pathological sheet is refused, not hung on.
//!
//! `#![forbid(unsafe_code)]` (inherited), `no_std` + `alloc`, never-panic: every
//! malformed input is a [`CellValue::Error`], never an `unwrap`/index panic.

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{parse_a1, CellValue, Sheet};

// ─── Bounds ─────────────────────────────────────────────────────────────────

/// Longest formula text (the part after `=`) we will parse. Past this → `#VALUE!`.
pub const MAX_FORMULA_LEN: usize = 8 * 1024;

/// Deepest expression nesting (parentheses / nested calls). Past this → `#VALUE!`.
/// The parser checks this on every descent, so a stack overflow is impossible.
pub const MAX_EXPR_DEPTH: usize = 64;

/// Largest number of cells a single range (`A1:B3`) may expand to. Past this →
/// `#REF!` (a crafted `A1:XFD1048576` would otherwise be billions of cells).
pub const MAX_RANGE_CELLS: usize = 1 << 20; // ~1M

/// Largest number of formula cells [`evaluate`] will process in one sheet.
pub const MAX_EVAL_CELLS: usize = 1 << 20;

// ─── Error literals ─────────────────────────────────────────────────────────

const E_DIV0: &str = "#DIV/0!";
const E_VALUE: &str = "#VALUE!";
const E_REF: &str = "#REF!";
const E_NAME: &str = "#NAME?";
const E_NA: &str = "#N/A";
const E_CIRC: &str = "#CIRC";

fn err(lit: &str) -> CellValue {
    CellValue::Error(String::from(lit))
}

/// Whether a [`CellValue`] is an error (used for propagation).
fn as_error(v: &CellValue) -> Option<CellValue> {
    match v {
        CellValue::Error(_) => Some(v.clone()),
        _ => None,
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Tokenizer
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String), // function name or cell ref or bool literal
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    Amp,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LParen,
    RParen,
    Comma,
    Colon,
}

/// Tokenize a formula body (no leading `=`). Returns `None` on a malformed token
/// (e.g. an unterminated string) so the caller can yield `#VALUE!` — never panics.
fn tokenize(src: &str) -> Option<Vec<Tok>> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let n = b.len();
    let mut out: Vec<Tok> = Vec::new();
    while i < n {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            b'+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            b'-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            b'*' => {
                out.push(Tok::Star);
                i += 1;
            }
            b'/' => {
                out.push(Tok::Slash);
                i += 1;
            }
            b'^' => {
                out.push(Tok::Caret);
                i += 1;
            }
            b'%' => {
                out.push(Tok::Percent);
                i += 1;
            }
            b'&' => {
                out.push(Tok::Amp);
                i += 1;
            }
            b'(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            b':' => {
                out.push(Tok::Colon);
                i += 1;
            }
            b'=' => {
                out.push(Tok::Eq);
                i += 1;
            }
            b'<' => {
                if i + 1 < n && b[i + 1] == b'>' {
                    out.push(Tok::Ne);
                    i += 2;
                } else if i + 1 < n && b[i + 1] == b'=' {
                    out.push(Tok::Le);
                    i += 2;
                } else {
                    out.push(Tok::Lt);
                    i += 1;
                }
            }
            b'>' => {
                if i + 1 < n && b[i + 1] == b'=' {
                    out.push(Tok::Ge);
                    i += 2;
                } else {
                    out.push(Tok::Gt);
                    i += 1;
                }
            }
            b'"' => {
                // String literal; "" inside is an escaped quote.
                let mut s = String::new();
                i += 1;
                loop {
                    if i >= n {
                        return None; // unterminated
                    }
                    if b[i] == b'"' {
                        if i + 1 < n && b[i + 1] == b'"' {
                            s.push('"');
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        // Advance one UTF-8 scalar safely.
                        let start = i;
                        i += 1;
                        while i < n && (b[i] & 0xC0) == 0x80 {
                            i += 1;
                        }
                        s.push_str(&src[start..i]);
                    }
                }
                out.push(Tok::Str(s));
            }
            _ => {
                if c.is_ascii_digit() || (c == b'.' && i + 1 < n && b[i + 1].is_ascii_digit()) {
                    let start = i;
                    while i < n
                        && (b[i].is_ascii_digit()
                            || b[i] == b'.'
                            || b[i] == b'e'
                            || b[i] == b'E'
                            || ((b[i] == b'+' || b[i] == b'-')
                                && i > start
                                && (b[i - 1] == b'e' || b[i - 1] == b'E')))
                    {
                        i += 1;
                    }
                    match crate::parse_f64_pub(&src[start..i]) {
                        Some(v) => out.push(Tok::Num(v)),
                        None => return None,
                    }
                } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
                    // Identifier: function name, cell ref, or bool. Allow letters,
                    // digits, '_', '.', and '$' (absolute-ref markers).
                    let start = i;
                    while i < n
                        && (b[i].is_ascii_alphanumeric()
                            || b[i] == b'_'
                            || b[i] == b'.'
                            || b[i] == b'$')
                    {
                        i += 1;
                    }
                    out.push(Tok::Ident(src[start..i].to_owned()));
                } else {
                    return None; // unknown character
                }
            }
        }
    }
    Some(out)
}

// ═════════════════════════════════════════════════════════════════════════════
// AST + parser (precedence-climbing recursive descent)
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    /// A single cell reference, resolved to 0-based (col, row).
    Cell(u32, u32),
    /// A range reference, resolved to inclusive 0-based corners.
    Range(u32, u32, u32, u32),
    /// A reference that did not resolve (out of bounds / malformed) → `#REF!`.
    BadRef,
    Unary(UnOp, alloc::boxed::Box<Expr>),
    Binary(BinOp, alloc::boxed::Box<Expr>, alloc::boxed::Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy)]
enum UnOp {
    Neg,
    Percent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    depth: usize,
}

#[derive(Debug)]
struct ParseErr;

impl<'a> Parser<'a> {
    fn new(toks: &'a [Tok]) -> Self {
        Parser {
            toks,
            pos: 0,
            depth: 0,
        }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<&Tok> {
        let t = self.toks.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn enter(&mut self) -> Result<(), ParseErr> {
        self.depth += 1;
        if self.depth > MAX_EXPR_DEPTH {
            return Err(ParseErr);
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Parse a full expression; comparisons have the lowest precedence.
    fn parse_expr(&mut self) -> Result<Expr, ParseErr> {
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseErr> {
        self.enter()?;
        let mut left = self.parse_concat()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Eq) => BinOp::Eq,
                Some(Tok::Ne) => BinOp::Ne,
                Some(Tok::Lt) => BinOp::Lt,
                Some(Tok::Le) => BinOp::Le,
                Some(Tok::Gt) => BinOp::Gt,
                Some(Tok::Ge) => BinOp::Ge,
                _ => break,
            };
            self.bump();
            let right = self.parse_concat()?;
            left = Expr::Binary(op, boxed(left), boxed(right));
        }
        self.leave();
        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<Expr, ParseErr> {
        let mut left = self.parse_add()?;
        while matches!(self.peek(), Some(Tok::Amp)) {
            self.bump();
            let right = self.parse_add()?;
            left = Expr::Binary(BinOp::Concat, boxed(left), boxed(right));
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, ParseErr> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let right = self.parse_mul()?;
            left = Expr::Binary(op, boxed(left), boxed(right));
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseErr> {
        let mut left = self.parse_pow()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                _ => break,
            };
            self.bump();
            let right = self.parse_pow()?;
            left = Expr::Binary(op, boxed(left), boxed(right));
        }
        Ok(left)
    }

    fn parse_pow(&mut self) -> Result<Expr, ParseErr> {
        // Right-associative power, binds tighter than * /.
        let left = self.parse_unary()?;
        if matches!(self.peek(), Some(Tok::Caret)) {
            self.bump();
            let right = self.parse_pow()?;
            return Ok(Expr::Binary(BinOp::Pow, boxed(left), boxed(right)));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseErr> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.bump();
                self.enter()?;
                let e = self.parse_unary()?;
                self.leave();
                Ok(Expr::Unary(UnOp::Neg, boxed(e)))
            }
            Some(Tok::Plus) => {
                self.bump();
                self.enter()?;
                let e = self.parse_unary()?;
                self.leave();
                Ok(e)
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseErr> {
        let mut e = self.parse_primary()?;
        while matches!(self.peek(), Some(Tok::Percent)) {
            self.bump();
            e = Expr::Unary(UnOp::Percent, boxed(e));
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseErr> {
        self.enter()?;
        let result = match self.bump() {
            Some(Tok::Num(v)) => Ok(Expr::Num(*v)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s.clone())),
            Some(Tok::LParen) => {
                let e = self.parse_expr()?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err(ParseErr),
                }
            }
            Some(Tok::Ident(name)) => {
                let name = name.clone();
                if matches!(self.peek(), Some(Tok::LParen)) {
                    // Function call.
                    self.bump(); // (
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            let arg = self.parse_expr()?;
                            args.push(arg);
                            match self.peek() {
                                Some(Tok::Comma) => {
                                    self.bump();
                                }
                                _ => break,
                            }
                        }
                    }
                    match self.bump() {
                        Some(Tok::RParen) => Ok(Expr::Call(ascii_upper(&name), args)),
                        _ => Err(ParseErr),
                    }
                } else {
                    // Bool literal, cell ref, or range.
                    let upper = ascii_upper(&name);
                    if upper == "TRUE" {
                        Ok(Expr::Bool(true))
                    } else if upper == "FALSE" {
                        Ok(Expr::Bool(false))
                    } else if matches!(self.peek(), Some(Tok::Colon)) {
                        // Range: ident : ident
                        self.bump(); // :
                        let end = match self.bump() {
                            Some(Tok::Ident(e)) => e.clone(),
                            _ => return Err(ParseErr),
                        };
                        Ok(make_range(&name, &end))
                    } else {
                        Ok(make_cell(&name))
                    }
                }
            }
            _ => Err(ParseErr),
        };
        self.leave();
        result
    }
}

fn boxed(e: Expr) -> alloc::boxed::Box<Expr> {
    alloc::boxed::Box::new(e)
}

fn ascii_upper(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// Resolve a single A1 token to an [`Expr::Cell`] (or [`Expr::BadRef`]).
fn make_cell(s: &str) -> Expr {
    match parse_a1(s) {
        Ok((col, row)) => Expr::Cell(col, row),
        Err(_) => Expr::BadRef,
    }
}

/// Resolve two A1 tokens to an [`Expr::Range`] (or [`Expr::BadRef`]).
fn make_range(a: &str, b: &str) -> Expr {
    match (parse_a1(a), parse_a1(b)) {
        (Ok((c1, r1)), Ok((c2, r2))) => {
            let (lo_c, hi_c) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };
            let (lo_r, hi_r) = if r1 <= r2 { (r1, r2) } else { (r2, r1) };
            Expr::Range(lo_c, lo_r, hi_c, hi_r)
        }
        _ => Expr::BadRef,
    }
}

/// Parse a formula body into an AST. `None` on malformed/over-deep input.
fn parse(src: &str) -> Option<Expr> {
    if src.len() > MAX_FORMULA_LEN {
        return None;
    }
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return None;
    }
    let mut p = Parser::new(&toks);
    let e = p.parse_expr().ok()?;
    if p.pos != toks.len() {
        return None; // trailing junk → malformed
    }
    Some(e)
}

// ═════════════════════════════════════════════════════════════════════════════
// Evaluation
// ═════════════════════════════════════════════════════════════════════════════

/// A read-only cell resolver the evaluator queries for referenced values.
trait CellSource {
    fn value_at(&self, col: u32, row: u32) -> CellValue;
}

/// Evaluate an AST against a [`CellSource`], with a recursion-depth bound.
fn eval(expr: &Expr, src: &dyn CellSource, depth: usize) -> CellValue {
    if depth > MAX_EXPR_DEPTH {
        return err(E_VALUE);
    }
    match expr {
        Expr::Num(v) => CellValue::Number(*v),
        Expr::Str(s) => CellValue::Text(s.clone()),
        Expr::Bool(b) => CellValue::Bool(*b),
        Expr::BadRef => err(E_REF),
        Expr::Cell(c, r) => src.value_at(*c, *r),
        // A bare range used in a scalar context is a #VALUE! (Excel: implicit
        // intersection isn't supported here); ranges are only valid as call args,
        // handled in Call before this point.
        Expr::Range(..) => err(E_VALUE),
        Expr::Unary(op, e) => {
            let v = eval(e, src, depth + 1);
            if let Some(er) = as_error(&v) {
                return er;
            }
            match op {
                UnOp::Neg => match to_number(&v) {
                    Ok(n) => CellValue::Number(-n),
                    Err(e) => e,
                },
                UnOp::Percent => match to_number(&v) {
                    Ok(n) => CellValue::Number(n / 100.0),
                    Err(e) => e,
                },
            }
        }
        Expr::Binary(op, a, b) => {
            let va = eval(a, src, depth + 1);
            if let Some(er) = as_error(&va) {
                return er;
            }
            let vb = eval(b, src, depth + 1);
            if let Some(er) = as_error(&vb) {
                return er;
            }
            eval_binary(*op, &va, &vb)
        }
        Expr::Call(name, args) => eval_call(name, args, src, depth),
    }
}

fn eval_binary(op: BinOp, a: &CellValue, b: &CellValue) -> CellValue {
    match op {
        BinOp::Concat => {
            let mut s = value_to_text(a);
            s.push_str(&value_to_text(b));
            CellValue::Text(s)
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => {
            let na = match to_number(a) {
                Ok(n) => n,
                Err(e) => return e,
            };
            let nb = match to_number(b) {
                Ok(n) => n,
                Err(e) => return e,
            };
            match op {
                BinOp::Add => CellValue::Number(na + nb),
                BinOp::Sub => CellValue::Number(na - nb),
                BinOp::Mul => CellValue::Number(na * nb),
                BinOp::Div => {
                    if nb == 0.0 {
                        err(E_DIV0)
                    } else {
                        CellValue::Number(na / nb)
                    }
                }
                BinOp::Pow => CellValue::Number(powf(na, nb)),
                _ => unreachable!(),
            }
        }
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord = compare_values(a, b);
            let res = match op {
                BinOp::Eq => ord == Ordering::Equal,
                BinOp::Ne => ord != Ordering::Equal,
                BinOp::Lt => ord == Ordering::Less,
                BinOp::Le => ord != Ordering::Greater,
                BinOp::Gt => ord == Ordering::Greater,
                BinOp::Ge => ord != Ordering::Less,
                _ => unreachable!(),
            };
            CellValue::Bool(res)
        }
    }
}

use core::cmp::Ordering;

/// Spreadsheet comparison ordering: numbers < text < bool (Excel's type order); within
/// a type, numerically / case-insensitively / false<true.
fn compare_values(a: &CellValue, b: &CellValue) -> Ordering {
    fn rank(v: &CellValue) -> u8 {
        match v {
            CellValue::Number(_) | CellValue::Empty => 0,
            CellValue::Text(_) => 1,
            CellValue::Bool(_) => 2,
            CellValue::Error(_) => 3,
        }
    }
    // Empty coerces to 0 for comparison against a number.
    let an = coerce_num_for_cmp(a);
    let bn = coerce_num_for_cmp(b);
    if let (Some(x), Some(y)) = (an, bn) {
        return cmp_f64(x, y);
    }
    let (ra, rb) = (rank(a), rank(b));
    if ra != rb {
        return ra.cmp(&rb);
    }
    match (a, b) {
        (CellValue::Text(x), CellValue::Text(y)) => ascii_upper(x).cmp(&ascii_upper(y)),
        (CellValue::Bool(x), CellValue::Bool(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

fn coerce_num_for_cmp(v: &CellValue) -> Option<f64> {
    match v {
        CellValue::Number(n) => Some(*n),
        CellValue::Empty => Some(0.0),
        _ => None,
    }
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    if a < b {
        Ordering::Less
    } else if a > b {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// Coerce a value to a number for arithmetic. `Text` parses if numeric, else
/// `#VALUE!`. `Bool` → 0/1. `Empty` → 0. Error propagates.
fn to_number(v: &CellValue) -> Result<f64, CellValue> {
    match v {
        CellValue::Number(n) => Ok(*n),
        CellValue::Bool(true) => Ok(1.0),
        CellValue::Bool(false) => Ok(0.0),
        CellValue::Empty => Ok(0.0),
        CellValue::Text(s) => match crate::parse_f64_pub(s.trim()) {
            Some(n) => Ok(n),
            None => Err(err(E_VALUE)),
        },
        CellValue::Error(_) => Err(v.clone()),
    }
}

/// Render a value to text for concat/text functions.
fn value_to_text(v: &CellValue) -> String {
    v.to_display_string()
}

// ─── Function library ───────────────────────────────────────────────────────

/// Flatten a call argument into a list of scalar [`CellValue`]s: a range expands to
/// each of its cells; a scalar expression is one value. Returns an error value if the
/// range is too large or a sub-expression errored (caller decides propagation).
fn expand_arg(
    expr: &Expr,
    src: &dyn CellSource,
    depth: usize,
) -> Result<Vec<CellValue>, CellValue> {
    match expr {
        Expr::Range(c1, r1, c2, r2) => {
            let w = (*c2 as usize - *c1 as usize) + 1;
            let h = (*r2 as usize - *r1 as usize) + 1;
            let total = w.saturating_mul(h);
            if total > MAX_RANGE_CELLS {
                return Err(err(E_REF));
            }
            let mut out = Vec::with_capacity(total.min(1024));
            for r in *r1..=*r2 {
                for c in *c1..=*c2 {
                    out.push(src.value_at(c, r));
                }
            }
            Ok(out)
        }
        _ => {
            let v = eval(expr, src, depth + 1);
            let mut out = Vec::new();
            out.push(v);
            Ok(out)
        }
    }
}

fn eval_call(name: &str, args: &[Expr], src: &dyn CellSource, depth: usize) -> CellValue {
    if depth > MAX_EXPR_DEPTH {
        return err(E_VALUE);
    }
    // Functions whose semantics need ranges/laziness handle args themselves; the
    // rest get eagerly-expanded scalar argument lists.
    match name {
        // ── Logical (lazy / arg-shaped) ──
        "IF" => return fn_if(args, src, depth),
        "AND" => return fn_andor(args, src, depth, true),
        "OR" => return fn_andor(args, src, depth, false),
        "NOT" => return fn_not(args, src, depth),
        "SUMIF" => return fn_sumif(args, src, depth, false),
        "AVERAGEIF" => return fn_sumif(args, src, depth, true),
        _ => {}
    }

    // Eagerly expand all args (ranges → cells). Propagate the first error found.
    let mut flat: Vec<CellValue> = Vec::new();
    for a in args {
        match expand_arg(a, src, depth) {
            Ok(mut vs) => {
                for v in vs.drain(..) {
                    if let Some(e) = as_error(&v) {
                        return e;
                    }
                    flat.push(v);
                }
            }
            Err(e) => return e,
        }
    }

    match name {
        "SUM" => num_result(sum_of(&flat)),
        "AVERAGE" => {
            let (sum, count) = sum_count_numbers(&flat);
            if count == 0 {
                err(E_DIV0)
            } else {
                CellValue::Number(sum / count as f64)
            }
        }
        "MIN" => match min_max(&flat, true) {
            Some(v) => CellValue::Number(v),
            None => CellValue::Number(0.0),
        },
        "MAX" => match min_max(&flat, false) {
            Some(v) => CellValue::Number(v),
            None => CellValue::Number(0.0),
        },
        "COUNT" => CellValue::Number(count_numbers(&flat) as f64),
        "COUNTA" => {
            let c = flat.iter().filter(|v| !v.is_empty()).count();
            CellValue::Number(c as f64)
        }
        "ABS" => unary_num(&flat, |x| Ok(abs(x))),
        "INT" => unary_num(&flat, |x| Ok(floor(x))),
        "SQRT" => unary_num(&flat, |x| {
            if x < 0.0 {
                Err(err(E_NA))
            } else {
                Ok(sqrtf(x))
            }
        }),
        "MOD" => binary_num(&flat, |a, b| {
            if b == 0.0 {
                Err(err(E_DIV0))
            } else {
                // Excel MOD follows the sign of the divisor.
                let r = a - b * floor(a / b);
                Ok(r)
            }
        }),
        "POWER" => binary_num(&flat, |a, b| Ok(powf(a, b))),
        "ROUND" => round_family(&flat, RoundMode::Half),
        "ROUNDUP" => round_family(&flat, RoundMode::Up),
        "ROUNDDOWN" => round_family(&flat, RoundMode::Down),
        "LEN" => {
            if flat.len() != 1 {
                return err(E_VALUE);
            }
            CellValue::Number(value_to_text(&flat[0]).chars().count() as f64)
        }
        "UPPER" => text1(&flat, |s| {
            s.chars().flat_map(|c| c.to_uppercase()).collect()
        }),
        "LOWER" => text1(&flat, |s| {
            s.chars().flat_map(|c| c.to_lowercase()).collect()
        }),
        "TRIM" => text1(&flat, |s| collapse_trim(s)),
        "LEFT" => fn_leftright(&flat, true),
        "RIGHT" => fn_leftright(&flat, false),
        "MID" => fn_mid(&flat),
        "CONCATENATE" => {
            let mut s = String::new();
            for v in &flat {
                s.push_str(&value_to_text(v));
            }
            CellValue::Text(s)
        }
        _ => err(E_NAME),
    }
}

fn num_result(n: f64) -> CellValue {
    CellValue::Number(n)
}

/// SUM coercion: numbers and bools count when passed; text in ranges already filtered.
/// Here `flat` is the post-expansion list; per Excel we sum numbers, and bools that
/// arrived as scalars are already `Bool` — count them as 0/1; text is skipped.
fn sum_of(flat: &[CellValue]) -> f64 {
    let mut s = 0.0;
    for v in flat {
        match v {
            CellValue::Number(n) => s += *n,
            CellValue::Bool(b) => s += if *b { 1.0 } else { 0.0 },
            _ => {}
        }
    }
    s
}

fn sum_count_numbers(flat: &[CellValue]) -> (f64, usize) {
    let mut s = 0.0;
    let mut c = 0usize;
    for v in flat {
        match v {
            CellValue::Number(n) => {
                s += *n;
                c += 1;
            }
            CellValue::Bool(b) => {
                s += if *b { 1.0 } else { 0.0 };
                c += 1;
            }
            _ => {}
        }
    }
    (s, c)
}

fn count_numbers(flat: &[CellValue]) -> usize {
    flat.iter()
        .filter(|v| matches!(v, CellValue::Number(_)))
        .count()
}

fn min_max(flat: &[CellValue], want_min: bool) -> Option<f64> {
    let mut acc: Option<f64> = None;
    for v in flat {
        let n = match v {
            CellValue::Number(n) => *n,
            CellValue::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            _ => continue,
        };
        acc = Some(match acc {
            None => n,
            Some(cur) => {
                if (want_min && n < cur) || (!want_min && n > cur) {
                    n
                } else {
                    cur
                }
            }
        });
    }
    acc
}

fn unary_num(flat: &[CellValue], f: impl Fn(f64) -> Result<f64, CellValue>) -> CellValue {
    if flat.len() != 1 {
        return err(E_VALUE);
    }
    match to_number(&flat[0]) {
        Ok(n) => match f(n) {
            Ok(r) => CellValue::Number(r),
            Err(e) => e,
        },
        Err(e) => e,
    }
}

fn binary_num(flat: &[CellValue], f: impl Fn(f64, f64) -> Result<f64, CellValue>) -> CellValue {
    if flat.len() != 2 {
        return err(E_VALUE);
    }
    let a = match to_number(&flat[0]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let b = match to_number(&flat[1]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match f(a, b) {
        Ok(r) => CellValue::Number(r),
        Err(e) => e,
    }
}

#[derive(Clone, Copy)]
enum RoundMode {
    Half,
    Up,
    Down,
}

fn round_family(flat: &[CellValue], mode: RoundMode) -> CellValue {
    // ROUND(number, [digits]); digits defaults to 0.
    if flat.is_empty() || flat.len() > 2 {
        return err(E_VALUE);
    }
    let n = match to_number(&flat[0]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let digits = if flat.len() == 2 {
        match to_number(&flat[1]) {
            Ok(d) => d,
            Err(e) => return e,
        }
    } else {
        0.0
    };
    // Bound digits to a sane window.
    let digits = if digits > 15.0 {
        15.0
    } else if digits < -15.0 {
        -15.0
    } else {
        digits
    };
    let factor = pow10(digits as i32);
    if factor == 0.0 {
        return err(E_VALUE);
    }
    let scaled = n * factor;
    let rounded = match mode {
        RoundMode::Half => round_half_away(scaled),
        RoundMode::Up => {
            // away from zero
            if scaled >= 0.0 {
                ceil(scaled)
            } else {
                floor(scaled)
            }
        }
        RoundMode::Down => {
            // toward zero
            if scaled >= 0.0 {
                floor(scaled)
            } else {
                ceil(scaled)
            }
        }
    };
    CellValue::Number(rounded / factor)
}

fn text1(flat: &[CellValue], f: impl Fn(&str) -> String) -> CellValue {
    if flat.len() != 1 {
        return err(E_VALUE);
    }
    CellValue::Text(f(&value_to_text(&flat[0])))
}

fn collapse_trim(s: &str) -> String {
    // Excel TRIM removes leading/trailing spaces and collapses internal runs to one.
    let mut out = String::new();
    let mut prev_space = false;
    for c in s.trim().chars() {
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

fn fn_leftright(flat: &[CellValue], left: bool) -> CellValue {
    if flat.is_empty() || flat.len() > 2 {
        return err(E_VALUE);
    }
    let s = value_to_text(&flat[0]);
    let count = if flat.len() == 2 {
        match to_number(&flat[1]) {
            Ok(n) => {
                if n < 0.0 {
                    return err(E_VALUE);
                }
                n as usize
            }
            Err(e) => return e,
        }
    } else {
        1
    };
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let take = count.min(n);
    let slice: String = if left {
        chars[..take].iter().collect()
    } else {
        chars[n - take..].iter().collect()
    };
    CellValue::Text(slice)
}

fn fn_mid(flat: &[CellValue]) -> CellValue {
    // MID(text, start_1based, length)
    if flat.len() != 3 {
        return err(E_VALUE);
    }
    let s = value_to_text(&flat[0]);
    let start = match to_number(&flat[1]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let len = match to_number(&flat[2]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if start < 1.0 || len < 0.0 {
        return err(E_VALUE);
    }
    let chars: Vec<char> = s.chars().collect();
    let start_idx = (start as usize).saturating_sub(1);
    if start_idx >= chars.len() {
        return CellValue::Text(String::new());
    }
    let end = (start_idx + len as usize).min(chars.len());
    let slice: String = chars[start_idx..end].iter().collect();
    CellValue::Text(slice)
}

fn fn_if(args: &[Expr], src: &dyn CellSource, depth: usize) -> CellValue {
    if args.len() < 2 || args.len() > 3 {
        return err(E_VALUE);
    }
    let cond = eval(&args[0], src, depth + 1);
    if let Some(e) = as_error(&cond) {
        return e;
    }
    let truthy = match to_bool(&cond) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if truthy {
        eval(&args[1], src, depth + 1)
    } else if args.len() == 3 {
        eval(&args[2], src, depth + 1)
    } else {
        CellValue::Bool(false)
    }
}

fn fn_andor(args: &[Expr], src: &dyn CellSource, depth: usize, is_and: bool) -> CellValue {
    if args.is_empty() {
        return err(E_VALUE);
    }
    let mut saw = false;
    let mut acc = is_and;
    for a in args {
        let vs = match expand_arg(a, src, depth) {
            Ok(v) => v,
            Err(e) => return e,
        };
        for v in &vs {
            if let Some(e) = as_error(v) {
                return e;
            }
            // Skip empty/text per Excel (logical functions ignore text/blank).
            match v {
                CellValue::Text(_) | CellValue::Empty => continue,
                _ => {}
            }
            let b = match to_bool(v) {
                Ok(b) => b,
                Err(e) => return e,
            };
            saw = true;
            acc = if is_and { acc && b } else { acc || b };
        }
    }
    if !saw {
        return err(E_VALUE);
    }
    CellValue::Bool(acc)
}

fn fn_not(args: &[Expr], src: &dyn CellSource, depth: usize) -> CellValue {
    if args.len() != 1 {
        return err(E_VALUE);
    }
    let v = eval(&args[0], src, depth + 1);
    if let Some(e) = as_error(&v) {
        return e;
    }
    match to_bool(&v) {
        Ok(b) => CellValue::Bool(!b),
        Err(e) => e,
    }
}

/// SUMIF(range, criterion, [sum_range]) / AVERAGEIF (same shape, averages).
fn fn_sumif(args: &[Expr], src: &dyn CellSource, depth: usize, average: bool) -> CellValue {
    if args.len() < 2 || args.len() > 3 {
        return err(E_VALUE);
    }
    let test = match expand_arg(&args[0], src, depth) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // Criterion is a scalar.
    let crit = eval(&args[1], src, depth + 1);
    if let Some(e) = as_error(&crit) {
        return e;
    }
    let sum_vals = if args.len() == 3 {
        match expand_arg(&args[2], src, depth) {
            Ok(v) => v,
            Err(e) => return e,
        }
    } else {
        test.clone()
    };
    let mut total = 0.0;
    let mut count = 0usize;
    let pred = make_criterion(&crit);
    let len = test.len().min(sum_vals.len());
    for i in 0..len {
        if pred(&test[i]) {
            if let CellValue::Number(n) = &sum_vals[i] {
                total += *n;
                count += 1;
            } else if let CellValue::Bool(b) = &sum_vals[i] {
                total += if *b { 1.0 } else { 0.0 };
                count += 1;
            }
        }
    }
    if average {
        if count == 0 {
            err(E_DIV0)
        } else {
            CellValue::Number(total / count as f64)
        }
    } else {
        CellValue::Number(total)
    }
}

/// Build a predicate from a criterion value. Supports a bare value (equality) or a
/// text criterion beginning with a comparison operator (`">5"`, `"<=3"`, `"<>x"`).
fn make_criterion(crit: &CellValue) -> alloc::boxed::Box<dyn Fn(&CellValue) -> bool> {
    if let CellValue::Text(s) = crit {
        let s = s.trim();
        let (op, rest) = if let Some(r) = s.strip_prefix("<=") {
            (BinOp::Le, r)
        } else if let Some(r) = s.strip_prefix(">=") {
            (BinOp::Ge, r)
        } else if let Some(r) = s.strip_prefix("<>") {
            (BinOp::Ne, r)
        } else if let Some(r) = s.strip_prefix('<') {
            (BinOp::Lt, r)
        } else if let Some(r) = s.strip_prefix('>') {
            (BinOp::Gt, r)
        } else if let Some(r) = s.strip_prefix('=') {
            (BinOp::Eq, r)
        } else {
            (BinOp::Eq, s)
        };
        let target = match crate::parse_f64_pub(rest.trim()) {
            Some(n) => CellValue::Number(n),
            None => CellValue::Text(rest.to_owned()),
        };
        return alloc::boxed::Box::new(move |v: &CellValue| {
            let ord = compare_values(v, &target);
            match op {
                BinOp::Eq => ord == Ordering::Equal,
                BinOp::Ne => ord != Ordering::Equal,
                BinOp::Lt => ord == Ordering::Less,
                BinOp::Le => ord != Ordering::Greater,
                BinOp::Gt => ord == Ordering::Greater,
                BinOp::Ge => ord != Ordering::Less,
                _ => false,
            }
        });
    }
    let target = crit.clone();
    alloc::boxed::Box::new(move |v: &CellValue| compare_values(v, &target) == Ordering::Equal)
}

/// Coerce a value to a boolean (for IF/AND/OR/NOT). Number ≠ 0 → true; text errors.
fn to_bool(v: &CellValue) -> Result<bool, CellValue> {
    match v {
        CellValue::Bool(b) => Ok(*b),
        CellValue::Number(n) => Ok(*n != 0.0),
        CellValue::Empty => Ok(false),
        CellValue::Text(s) => {
            let u = ascii_upper(s.trim());
            if u == "TRUE" {
                Ok(true)
            } else if u == "FALSE" {
                Ok(false)
            } else {
                Err(err(E_VALUE))
            }
        }
        CellValue::Error(_) => Err(v.clone()),
    }
}

// ─── no_std math helpers (soft-float kernel; bounded loops) ──────────────────

fn abs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

fn floor(x: f64) -> f64 {
    let t = trunc(x);
    if x < 0.0 && t != x {
        t - 1.0
    } else {
        t
    }
}

fn ceil(x: f64) -> f64 {
    let t = trunc(x);
    if x > 0.0 && t != x {
        t + 1.0
    } else {
        t
    }
}

fn trunc(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    if x >= 0.0 {
        if x >= 9_007_199_254_740_992.0 {
            x
        } else {
            (x as i64) as f64
        }
    } else if x <= -9_007_199_254_740_992.0 {
        x
    } else {
        (x as i64) as f64
    }
}

fn round_half_away(x: f64) -> f64 {
    if x >= 0.0 {
        floor(x + 0.5)
    } else {
        ceil(x - 0.5)
    }
}

/// 10^n for a bounded integer exponent via repeated multiply (no powi in no_std).
fn pow10(n: i32) -> f64 {
    let mut r = 1.0f64;
    let k = if n < 0 { -n } else { n }.min(308);
    let mut i = 0;
    while i < k {
        r *= 10.0;
        i += 1;
    }
    if n < 0 {
        1.0 / r
    } else {
        r
    }
}

/// Integer/fractional power: integer exponents via exponentiation-by-squaring (exact
/// for common cases like `2^10`); fractional exponents via exp(b*ln a). Bounded.
fn powf(a: f64, b: f64) -> f64 {
    if b == trunc(b) && abs(b) <= 1024.0 {
        let mut exp = b as i64;
        let neg = exp < 0;
        if neg {
            exp = -exp;
        }
        let mut result = 1.0f64;
        let mut base = a;
        while exp > 0 {
            if exp & 1 == 1 {
                result *= base;
            }
            base *= base;
            exp >>= 1;
        }
        return if neg {
            if result == 0.0 {
                f64::INFINITY
            } else {
                1.0 / result
            }
        } else {
            result
        };
    }
    if a <= 0.0 {
        return f64::NAN;
    }
    expf(b * lnf(a))
}

/// Babylonian square root (bounded iterations).
fn sqrtf(x: f64) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    let mut g = x;
    let mut i = 0;
    while i < 64 {
        let ng = 0.5 * (g + x / g);
        if abs(ng - g) <= 1e-15 * g {
            return ng;
        }
        g = ng;
        i += 1;
    }
    g
}

/// ln via atanh series on the reduced mantissa (bounded; for x > 0).
fn lnf(x: f64) -> f64 {
    // Reduce x = m * 2^k with m in [1,2) by bit tricks unavailable in no_std f64,
    // so reduce by repeated halving/doubling instead.
    let mut k = 0i32;
    let mut m = x;
    while m >= 2.0 {
        m *= 0.5;
        k += 1;
    }
    while m < 1.0 {
        m *= 2.0;
        k -= 1;
    }
    // m in [1,2): ln m via atanh series, t=(m-1)/(m+1).
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let mut term = t;
    let mut sum = 0.0;
    let mut nn = 1.0;
    let mut i = 0;
    while i < 40 {
        sum += term / nn;
        term *= t2;
        nn += 2.0;
        i += 1;
    }
    2.0 * sum + (k as f64) * core::f64::consts::LN_2
}

/// exp via Taylor series (bounded; clamps extreme inputs).
fn expf(x: f64) -> f64 {
    if x > 700.0 {
        return f64::INFINITY;
    }
    if x < -700.0 {
        return 0.0;
    }
    let mut term = 1.0f64;
    let mut sum = 1.0f64;
    let mut i = 1;
    while i < 64 {
        term *= x / (i as f64);
        sum += term;
        if abs(term) < 1e-17 * abs(sum) {
            break;
        }
        i += 1;
    }
    sum
}

// ═════════════════════════════════════════════════════════════════════════════
// Sheet-level: dependency-ordered recalc
// ═════════════════════════════════════════════════════════════════════════════

/// A snapshot of the sheet's resolved values used as the [`CellSource`] during eval.
/// Keyed by `(col, row)`; only populated cells are present.
struct ValueGrid {
    map: BTreeMap<(u32, u32), CellValue>,
}

impl ValueGrid {
    fn new() -> Self {
        ValueGrid {
            map: BTreeMap::new(),
        }
    }
    fn set(&mut self, col: u32, row: u32, v: CellValue) {
        self.map.insert((col, row), v);
    }
}

impl CellSource for ValueGrid {
    fn value_at(&self, col: u32, row: u32) -> CellValue {
        match self.map.get(&(col, row)) {
            Some(v) => v.clone(),
            None => CellValue::Empty,
        }
    }
}

/// Collect the cell/range references a parsed formula reads (for the dependency graph).
fn collect_refs(expr: &Expr, out: &mut Vec<(u32, u32)>, depth: usize) {
    if depth > MAX_EXPR_DEPTH {
        return;
    }
    match expr {
        Expr::Cell(c, r) => out.push((*c, *r)),
        Expr::Range(c1, r1, c2, r2) => {
            let w = (*c2 as usize).saturating_sub(*c1 as usize) + 1;
            let h = (*r2 as usize).saturating_sub(*r1 as usize) + 1;
            if w.saturating_mul(h) > MAX_RANGE_CELLS {
                return; // too big; eval will return #REF! anyway
            }
            for r in *r1..=*r2 {
                for c in *c1..=*c2 {
                    out.push((c, r));
                }
            }
        }
        Expr::Unary(_, e) => collect_refs(e, out, depth + 1),
        Expr::Binary(_, a, b) => {
            collect_refs(a, out, depth + 1);
            collect_refs(b, out, depth + 1);
        }
        Expr::Call(_, args) => {
            for a in args {
                collect_refs(a, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Recalculate every formula cell in `sheet` into its [`Cell::value`], in dependency
/// (topological) order. Cells on a dependency cycle receive `#CIRC`. Non-formula
/// cells are left untouched and serve as inputs.
///
/// ## Safety property (never hang)
/// The topological walk is an iterative DFS with an explicit `in_progress` set; a
/// back-edge to an in-progress node is a cycle → every node on it is marked `#CIRC`
/// and the walk continues. There is no recursion over the cell graph (only the
/// bounded per-formula expression recursion), and at most [`MAX_EVAL_CELLS`] formula
/// cells are processed, so no input can hang or overflow the stack.
pub fn evaluate(sheet: &mut Sheet) {
    // 1. Parse every formula cell; index formulas by position.
    //    `parsed[(col,row)] = Some(Expr)` or `None` (malformed → #VALUE!).
    let mut formulas: BTreeMap<(u32, u32), Option<Expr>> = BTreeMap::new();
    // The non-formula (literal) inputs seed the value grid.
    let mut grid = ValueGrid::new();

    let mut formula_count = 0usize;
    for cell in &sheet.cells {
        let key = (cell.col, cell.row);
        if let Some(ftext) = &cell.formula {
            if formula_count >= MAX_EVAL_CELLS {
                continue;
            }
            formula_count += 1;
            let body = ftext.strip_prefix('=').unwrap_or(ftext);
            let parsed = parse(body);
            formulas.insert(key, parsed);
        } else {
            grid.set(cell.col, cell.row, cell.value.clone());
        }
    }

    if formulas.is_empty() {
        return;
    }

    // 2. Precompute each formula's dependency list (only deps that are themselves
    //    formula cells matter for ordering; literal deps are already in `grid`).
    let mut deps: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for (key, parsed) in &formulas {
        let mut refs: Vec<(u32, u32)> = Vec::new();
        if let Some(expr) = parsed {
            collect_refs(expr, &mut refs, 0);
        }
        // Keep only deps that are themselves formula cells (literal deps are already
        // in `grid`). A self-reference (`r == key`) is intentionally RETAINED: it is a
        // direct cycle the DFS back-edge check must flag as #CIRC, not silently drop.
        refs.retain(|r| formulas.contains_key(r));
        refs.sort_unstable();
        refs.dedup();
        deps.insert(*key, refs);
    }

    // 3. Iterative DFS topological evaluation with cycle detection.
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unseen,
        InProgress,
        Done,
    }
    let mut state: BTreeMap<(u32, u32), State> = BTreeMap::new();
    for key in formulas.keys() {
        state.insert(*key, State::Unseen);
    }

    let keys: Vec<(u32, u32)> = formulas.keys().cloned().collect();
    // Each stack frame: (cell, next-dependency-index).
    let mut stack: Vec<((u32, u32), usize)> = Vec::new();

    for start in keys {
        if state.get(&start) != Some(&State::Unseen) {
            continue;
        }
        stack.push((start, 0));
        state.insert(start, State::InProgress);

        while let Some(&(node, idx)) = stack.last() {
            let node_deps = deps.get(&node);
            let dep = node_deps.and_then(|d| d.get(idx)).cloned();
            match dep {
                Some(d) => {
                    // Advance this frame's cursor first.
                    if let Some(top) = stack.last_mut() {
                        top.1 += 1;
                    }
                    match state.get(&d).cloned() {
                        Some(State::Unseen) => {
                            state.insert(d, State::InProgress);
                            stack.push((d, 0));
                        }
                        Some(State::InProgress) => {
                            // Back-edge → cycle. Mark every InProgress node on the
                            // current stack as #CIRC and finalize them so we never
                            // revisit (and never loop).
                            for &(c, _) in &stack {
                                if state.get(&c) == Some(&State::InProgress) {
                                    grid.set(c.0, c.1, err(E_CIRC));
                                }
                            }
                            // Do not pop here; popping happens as frames complete
                            // below, but their values are now pinned to #CIRC.
                        }
                        _ => { /* Done — already evaluated */ }
                    }
                }
                None => {
                    // All deps processed → evaluate this node now (unless already
                    // pinned to #CIRC by a cycle).
                    let already_circ = matches!(
                        grid.value_at(node.0, node.1),
                        CellValue::Error(ref s) if s == E_CIRC
                    );
                    if !already_circ {
                        let v = match formulas.get(&node) {
                            Some(Some(expr)) => eval(expr, &grid, 0),
                            _ => err(E_VALUE), // malformed formula
                        };
                        grid.set(node.0, node.1, v);
                    }
                    state.insert(node, State::Done);
                    stack.pop();
                }
            }
        }
    }

    // 4. Write the computed values back into the sheet's formula cells.
    for cell in &mut sheet.cells {
        if cell.formula.is_some() {
            if let Some(v) = grid.map.get(&(cell.col, cell.row)) {
                cell.value = v.clone();
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Single-formula entry (testing + ad-hoc eval against a sheet's literal cells)
// ═════════════════════════════════════════════════════════════════════════════

/// Evaluate one formula string against `sheet`'s *current* cell values (literals and
/// any previously-computed formula results). Accepts an optional leading `=`. Cell
/// references resolve through `sheet`; this does **not** recurse into other formula
/// cells' formulas (call [`evaluate`] first for full dependency-ordered recalc).
/// Never panics; a malformed formula returns `#VALUE!`/`#NAME?` as a [`CellValue`].
pub fn eval_formula(sheet: &Sheet, formula: &str) -> CellValue {
    let body = formula.strip_prefix('=').unwrap_or(formula);
    let expr = match parse(body) {
        Some(e) => e,
        None => return err(E_VALUE),
    };
    let src = SheetSource { sheet };
    eval(&expr, &src, 0)
}

/// A [`CellSource`] reading directly from a [`Sheet`]'s stored cell values.
struct SheetSource<'a> {
    sheet: &'a Sheet,
}

impl<'a> CellSource for SheetSource<'a> {
    fn value_at(&self, col: u32, row: u32) -> CellValue {
        match self.sheet.cell(col, row) {
            Some(v) => v.clone(),
            None => CellValue::Empty,
        }
    }
}

#[cfg(test)]
#[path = "formula_tests.rs"]
mod formula_tests;
