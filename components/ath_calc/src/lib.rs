//! AthenaOS Calculator math engine — *"apps that just work"* (LEGACY_GAMING_CONCEPT.md
//! §Three User Experiences). A real expression evaluator powering the bundled
//! Calculator app: operator precedence, parentheses, unary minus, decimals,
//! percent, and the `sqrt`/`pow` niceties — all in pure `f64`, no panics on bad
//! input.
//!
//! Why this is its own crate: `apps/calculator` is a `#![no_std] #![no_main]`
//! bin that links athkit's `#[panic_handler]`, so `cargo test` inside it trips
//! the duplicate `panic_impl` lang-item gotcha (project memory: no-std-workspace
//! host-test hazard). Factoring the math out into a zero-dep `no_std` lib that
//! toggles to `std` under `cfg(test)` (the `ath_tokens` pattern) gives us a
//! clean, FAIL-able proof: `cargo test -p ath_calc`.
//!
//! ## Grammar (recursive descent, standard precedence)
//! ```text
//! expr    := term  (('+' | '-') term)*
//! term    := unary (('*' | '/') unary)*
//! unary   := ('-' | '+')* postfix
//! postfix := primary ('%' | '!')*
//! primary := NUMBER | CONST
//!          | '(' expr ')'
//!          | fn1 '(' expr ')'          // sin cos tan asin acos atan
//!          |                           // ln log/log10 log2 exp sqrt
//!          |                           // abs floor ceil round fact
//!          | 'pow' '(' expr ',' expr ')'
//! ```
//! Constants: `pi`, `e`. Percent is postfix "divide by 100" (`50%` → `0.5`);
//! `!` is postfix factorial (`5!` → `120`). All scientific functions are pure
//! `f64` series/CORDIC-free approximations (Taylor + range reduction) — NO
//! `libm`, never a panic or infinite loop on a domain error (`ln(-1)`,
//! `asin(2)` → [`CalcError::NotFinite`]).
//!
//! ## Programmer mode (separate integer/bitwise path — [`eval_int`])
//! A distinct `i64` grammar: `+ - * / %` plus `& | ^ ~ << >>`, with hex (`0x`),
//! binary (`0b`), and octal (`0o`) literals. C-like precedence, wrapping
//! arithmetic (no panic).
//!
//! Pure logic, host-KAT'd (`cargo test -p ath_calc`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// Why evaluation failed. Every bad input maps to one of these — the evaluator
/// never panics, so the UI can show a calm "Error" instead of crashing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcError {
    /// Malformed input: unexpected character, dangling operator, unbalanced
    /// parens, empty expression, a second decimal point in one number, etc.
    Parse,
    /// Division (or modulo) by zero.
    DivByZero,
    /// Result is not finite (NaN / ±Inf), e.g. `sqrt(-1)`.
    NotFinite,
}

// ── Tokenizer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
    Comma,
    Pow,
    /// A named scientific function (`sin`, `ln`, `floor`, `fact`, …).
    Func(Fn1),
    /// A constant: `pi` or `e`.
    Const(f64),
    /// Postfix factorial `!`.
    Bang,
}

/// Single-argument scientific functions exposed in the f64 grammar. Each maps to
/// a pure-`f64` approximation below (no `libm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fn1 {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Ln,
    Log10,
    Log2,
    Exp,
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Round,
    Fact,
}

/// Hand-rolled tokenizer over ASCII bytes. Returns `Parse` on any unknown byte
/// or malformed number (no `f64::from_str` — that pulls in core::num parsing we
/// keep explicit and panic-free).
fn tokenize(expr: &str) -> Result<[Option<Tok>; MAX_TOKENS], CalcError> {
    let bytes = expr.as_bytes();
    let mut out: [Option<Tok>; MAX_TOKENS] = [None; MAX_TOKENS];
    let mut n = 0usize;
    let mut i = 0usize;

    macro_rules! push {
        ($t:expr) => {{
            if n >= MAX_TOKENS {
                return Err(CalcError::Parse);
            }
            out[n] = Some($t);
            n += 1;
        }};
    }

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            b'+' => {
                push!(Tok::Plus);
                i += 1;
            }
            b'-' => {
                push!(Tok::Minus);
                i += 1;
            }
            b'*' => {
                push!(Tok::Star);
                i += 1;
            }
            b'/' => {
                push!(Tok::Slash);
                i += 1;
            }
            b'%' => {
                push!(Tok::Percent);
                i += 1;
            }
            b'(' => {
                push!(Tok::LParen);
                i += 1;
            }
            b')' => {
                push!(Tok::RParen);
                i += 1;
            }
            b',' => {
                push!(Tok::Comma);
                i += 1;
            }
            b'!' => {
                push!(Tok::Bang);
                i += 1;
            }
            b'0'..=b'9' | b'.' => {
                let (val, next) = parse_number(bytes, i)?;
                push!(Tok::Num(val));
                i = next;
            }
            b'a'..=b'z' | b'A'..=b'Z' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                // Case-insensitive name match without allocating: lowercase into
                // a fixed scratch buffer (names are short).
                let mut name = [0u8; 8];
                let raw = &bytes[start..i];
                if raw.len() > name.len() {
                    return Err(CalcError::Parse);
                }
                for (d, &b) in name.iter_mut().zip(raw.iter()) {
                    *d = b.to_ascii_lowercase();
                }
                let lname = &name[..raw.len()];
                match lname {
                    b"sqrt" => push!(Tok::Func(Fn1::Sqrt)),
                    b"pow" => push!(Tok::Pow),
                    b"sin" => push!(Tok::Func(Fn1::Sin)),
                    b"cos" => push!(Tok::Func(Fn1::Cos)),
                    b"tan" => push!(Tok::Func(Fn1::Tan)),
                    b"asin" => push!(Tok::Func(Fn1::Asin)),
                    b"acos" => push!(Tok::Func(Fn1::Acos)),
                    b"atan" => push!(Tok::Func(Fn1::Atan)),
                    b"ln" => push!(Tok::Func(Fn1::Ln)),
                    b"log" | b"log10" => push!(Tok::Func(Fn1::Log10)),
                    b"log2" => push!(Tok::Func(Fn1::Log2)),
                    b"exp" => push!(Tok::Func(Fn1::Exp)),
                    b"abs" => push!(Tok::Func(Fn1::Abs)),
                    b"floor" => push!(Tok::Func(Fn1::Floor)),
                    b"ceil" => push!(Tok::Func(Fn1::Ceil)),
                    b"round" => push!(Tok::Func(Fn1::Round)),
                    b"fact" => push!(Tok::Func(Fn1::Fact)),
                    b"pi" => push!(Tok::Const(PI)),
                    b"e" => push!(Tok::Const(E)),
                    _ => return Err(CalcError::Parse),
                }
            }
            _ => return Err(CalcError::Parse),
        }
    }

    Ok(out)
}

/// Parse a non-negative decimal number starting at `start`. Returns the value
/// and the index of the first byte past the number. Rejects a second `.`.
fn parse_number(bytes: &[u8], start: usize) -> Result<(f64, usize), CalcError> {
    let mut i = start;
    let mut int_part: f64 = 0.0;
    let mut saw_digit = false;

    while i < bytes.len() && bytes[i].is_ascii_digit() {
        int_part = int_part * 10.0 + (bytes[i] - b'0') as f64;
        saw_digit = true;
        i += 1;
    }

    let mut value = int_part;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let mut frac: f64 = 0.0;
        let mut scale: f64 = 0.1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            frac += (bytes[i] - b'0') as f64 * scale;
            scale *= 0.1;
            saw_digit = true;
            i += 1;
        }
        value += frac;
    }

    if !saw_digit {
        // A bare "." with no digits.
        return Err(CalcError::Parse);
    }
    Ok((value, i))
}

const MAX_TOKENS: usize = 256;

/// π and e to full `f64` precision (the grammar's `pi` / `e` constants).
const PI: f64 = 3.141_592_653_589_793;
const E: f64 = 2.718_281_828_459_045;

// ── Parser / evaluator (recursive descent) ───────────────────────────────────

struct Parser<'a> {
    toks: &'a [Option<Tok>; MAX_TOKENS],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<Tok> {
        if self.pos < MAX_TOKENS {
            self.toks[self.pos]
        } else {
            None
        }
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.peek();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, want: Tok) -> Result<(), CalcError> {
        match self.next() {
            Some(t) if t == want => Ok(()),
            _ => Err(CalcError::Parse),
        }
    }

    // expr := term (('+' | '-') term)*
    fn expr(&mut self) -> Result<f64, CalcError> {
        let mut acc = self.term()?;
        loop {
            match self.peek() {
                Some(Tok::Plus) => {
                    self.pos += 1;
                    acc += self.term()?;
                }
                Some(Tok::Minus) => {
                    self.pos += 1;
                    acc -= self.term()?;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    // term := unary (('*' | '/') unary)*
    fn term(&mut self) -> Result<f64, CalcError> {
        let mut acc = self.unary()?;
        loop {
            match self.peek() {
                Some(Tok::Star) => {
                    self.pos += 1;
                    acc *= self.unary()?;
                }
                Some(Tok::Slash) => {
                    self.pos += 1;
                    let rhs = self.unary()?;
                    if rhs == 0.0 {
                        return Err(CalcError::DivByZero);
                    }
                    acc /= rhs;
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    // unary := ('-' | '+')* postfix
    fn unary(&mut self) -> Result<f64, CalcError> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some(Tok::Plus) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.postfix(),
        }
    }

    // postfix := primary ('%' | '!')*
    fn postfix(&mut self) -> Result<f64, CalcError> {
        let mut v = self.primary()?;
        loop {
            match self.peek() {
                Some(Tok::Percent) => {
                    self.pos += 1;
                    v /= 100.0;
                }
                Some(Tok::Bang) => {
                    self.pos += 1;
                    v = factorial(v)?;
                }
                _ => break,
            }
        }
        Ok(v)
    }

    // primary := NUMBER | CONST | '(' expr ')' | fn1 '(' expr ')' | pow '(' a , b ')'
    fn primary(&mut self) -> Result<f64, CalcError> {
        match self.next() {
            Some(Tok::Num(v)) => Ok(v),
            Some(Tok::Const(v)) => Ok(v),
            Some(Tok::LParen) => {
                let v = self.expr()?;
                self.expect(Tok::RParen)?;
                Ok(v)
            }
            Some(Tok::Func(f)) => {
                self.expect(Tok::LParen)?;
                let a = self.expr()?;
                self.expect(Tok::RParen)?;
                finite(apply_fn1(f, a)?)
            }
            Some(Tok::Pow) => {
                self.expect(Tok::LParen)?;
                let base = self.expr()?;
                self.expect(Tok::Comma)?;
                let exp = self.expr()?;
                self.expect(Tok::RParen)?;
                finite(powf(base, exp))
            }
            _ => Err(CalcError::Parse),
        }
    }
}

/// Dispatch a single-argument scientific function. Domain errors (e.g.
/// `asin(2)`, `ln(-1)`) return a non-finite value (caught by [`finite`]) or a
/// [`CalcError`]; never a panic or infinite loop.
fn apply_fn1(f: Fn1, x: f64) -> Result<f64, CalcError> {
    let r = match f {
        Fn1::Sin => sin_approx(x),
        Fn1::Cos => cos_approx(x),
        Fn1::Tan => {
            let c = cos_approx(x);
            if fabs(c) < 1e-300 {
                f64::NAN
            } else {
                sin_approx(x) / c
            }
        }
        Fn1::Asin => asin_approx(x),
        Fn1::Acos => {
            // acos(x) = pi/2 - asin(x); NaN domain propagates.
            let a = asin_approx(x);
            if a.is_finite() {
                PI / 2.0 - a
            } else {
                f64::NAN
            }
        }
        Fn1::Atan => atan_approx(x),
        Fn1::Ln => ln_approx(x),
        Fn1::Log10 => {
            const LN10: f64 = 2.302_585_092_994_046;
            let l = ln_approx(x);
            if l.is_finite() {
                l / LN10
            } else {
                f64::NAN
            }
        }
        Fn1::Log2 => {
            const LN2: f64 = 0.693_147_180_559_945_3;
            let l = ln_approx(x);
            if l.is_finite() {
                l / LN2
            } else {
                f64::NAN
            }
        }
        Fn1::Exp => exp_approx(x),
        Fn1::Sqrt => sqrt(x),
        Fn1::Abs => fabs(x),
        Fn1::Floor => floor(x),
        Fn1::Ceil => -floor(-x),
        Fn1::Round => floor(x + 0.5),
        Fn1::Fact => return factorial(x),
    };
    Ok(r)
}

/// Integer factorial over a non-negative integer `f64`. Non-integers and
/// negatives are a domain error; large inputs (>170, overflow `f64`) yield
/// `NotFinite`. Bounded loop — never spins.
fn factorial(x: f64) -> Result<f64, CalcError> {
    if x < 0.0 || x != (x as i64 as f64) {
        return Err(CalcError::NotFinite);
    }
    let n = x as i64;
    if n > 170 {
        // 171! overflows f64.
        return Err(CalcError::NotFinite);
    }
    let mut acc = 1.0f64;
    let mut i = 2i64;
    while i <= n {
        acc *= i as f64;
        i += 1;
    }
    Ok(acc)
}

// ── Scientific approximations (no libm; bounded iterations, never-panic) ──────

/// sin(x) via range reduction to [-π, π] then a bounded Taylor series.
fn sin_approx(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let r = reduce_pi(x);
    // Taylor: x - x^3/3! + x^5/5! - ... (converges fast on |r| <= π).
    let r2 = r * r;
    let mut term = r;
    let mut sum = r;
    let mut n = 1.0;
    let mut i = 0;
    while i < 24 {
        // next term = -term * r^2 / ((2n)(2n+1))
        term = -term * r2 / ((2.0 * n) * (2.0 * n + 1.0));
        sum += term;
        n += 1.0;
        i += 1;
    }
    sum
}

/// cos(x) = sin(x + π/2), reusing the reduced Taylor series.
fn cos_approx(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let r = reduce_pi(x);
    let r2 = r * r;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut n = 1.0;
    let mut i = 0;
    while i < 24 {
        term = -term * r2 / ((2.0 * n - 1.0) * (2.0 * n));
        sum += term;
        n += 1.0;
        i += 1;
    }
    sum
}

/// Reduce an angle to [-π, π] by subtracting whole multiples of 2π. Bounded:
/// the reduction count is derived directly, no unbounded loop.
fn reduce_pi(x: f64) -> f64 {
    const TWO_PI: f64 = 2.0 * PI;
    // k = round(x / 2π)
    let k = floor(x / TWO_PI + 0.5);
    x - k * TWO_PI
}

/// atan(x) via argument reduction (atan(x)=π/2-atan(1/x) for |x|>1) and the
/// Euler-accelerated series, so convergence stays fast for all finite x.
fn atan_approx(x: f64) -> f64 {
    if !x.is_finite() {
        // atan(±inf) = ±π/2
        return if x > 0.0 { PI / 2.0 } else { -PI / 2.0 };
    }
    let neg = x < 0.0;
    let mut a = fabs(x);
    let mut offset = 0.0;
    let mut invert = false;
    if a > 1.0 {
        a = 1.0 / a;
        invert = true;
        offset = PI / 2.0;
    }
    // Euler series: atan(a) = (a/(1+a^2)) * sum_n prod ... — use the classic
    // accelerated form: z = a^2/(1+a^2); atan = (a/(1+a^2)) * Σ (2^2n (n!)^2 /
    // (2n+1)!) z^n. Bounded.
    let denom = 1.0 + a * a;
    let z = a * a / denom;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut n = 1.0;
    let mut i = 0;
    while i < 40 {
        // ratio between successive coefficients of the accelerated series:
        // c_n/c_{n-1} = 2n / (2n+1)
        term *= (2.0 * n) / (2.0 * n + 1.0) * z;
        sum += term;
        n += 1.0;
        i += 1;
    }
    let base = a / denom * sum;
    let mut result = if invert { offset - base } else { base };
    if neg {
        result = -result;
    }
    result
}

/// asin(x) on [-1,1] via asin(x)=atan(x/sqrt(1-x^2)); endpoints handled exactly.
/// Out-of-domain (|x|>1) → NaN.
fn asin_approx(x: f64) -> f64 {
    if x > 1.0 || x < -1.0 || !x.is_finite() {
        return f64::NAN;
    }
    if x == 1.0 {
        return PI / 2.0;
    }
    if x == -1.0 {
        return -PI / 2.0;
    }
    let d = sqrt(1.0 - x * x);
    if d == 0.0 {
        return if x > 0.0 { PI / 2.0 } else { -PI / 2.0 };
    }
    atan_approx(x / d)
}

fn finite(v: f64) -> Result<f64, CalcError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CalcError::NotFinite)
    }
}

/// Pure-`f64` square root via Newton-Raphson (no `libm`, no `f64::sqrt`
/// intrinsic — keeps the crate transcendental-free for `no_std`). Negative
/// inputs return NaN, surfaced as [`CalcError::NotFinite`].
/// `f64::abs` is a `std` intrinsic (unavailable in `no_std`/`core`); this is the
/// pure replacement so the crate stays `libm`-free.
#[inline]
fn fabs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

fn sqrt(x: f64) -> f64 {
    if x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return 0.0;
    }
    // Seed near x; converges quadratically in a few iterations for our range.
    let mut g = if x >= 1.0 { x } else { 1.0 };
    let mut i = 0;
    while i < 64 {
        let next = 0.5 * (g + x / g);
        if fabs(next - g) <= 1e-12 * fabs(next) {
            return next;
        }
        g = next;
        i += 1;
    }
    g
}

/// Pure-`f64` power. Integer exponents use exponentiation-by-squaring (exact-ish
/// and fast); non-integer exponents fall back to `exp(y * ln(x))` via local
/// series so the crate stays `libm`-free.
fn powf(base: f64, exp: f64) -> f64 {
    if exp == 0.0 {
        return 1.0;
    }
    // Integer exponent path (covers the common calculator use: pow(2,10)).
    if fabs(exp) < 1024.0 && exp == (exp as i64 as f64) {
        let neg = exp < 0.0;
        let mut n = fabs(exp) as u64;
        let mut b = base;
        let mut acc = 1.0;
        while n > 0 {
            if n & 1 == 1 {
                acc *= b;
            }
            b *= b;
            n >>= 1;
        }
        return if neg {
            if acc == 0.0 {
                f64::INFINITY
            } else {
                1.0 / acc
            }
        } else {
            acc
        };
    }
    // Fractional exponent: exp(exp * ln(base)). Negative base is undefined here.
    if base <= 0.0 {
        return f64::NAN;
    }
    exp_approx(exp * ln_approx(base))
}

/// Natural log via the `atanh` series, with range reduction by powers of 2 so
/// the series argument stays well inside its convergence radius.
fn ln_approx(x: f64) -> f64 {
    if x <= 0.0 {
        return f64::NAN;
    }
    // Reduce x to [2/3, 4/3) by factoring powers of two; ln2 added back.
    const LN2: f64 = 0.693_147_180_559_945_3;
    let mut m = x;
    let mut k: i32 = 0;
    while m > 4.0 / 3.0 {
        m *= 0.5;
        k += 1;
    }
    while m < 2.0 / 3.0 {
        m *= 2.0;
        k -= 1;
    }
    // ln(m) = 2*atanh((m-1)/(m+1)) = 2*(t + t^3/3 + t^5/5 + ...)
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let mut term = t;
    let mut sum = 0.0;
    let mut denom = 1.0;
    let mut i = 0;
    while i < 40 {
        sum += term / denom;
        term *= t2;
        denom += 2.0;
        i += 1;
    }
    2.0 * sum + (k as f64) * LN2
}

/// e^x via range reduction (x = k*ln2 + r) and a Taylor series on the small r.
fn exp_approx(x: f64) -> f64 {
    const LN2: f64 = 0.693_147_180_559_945_3;
    if x == 0.0 {
        return 1.0;
    }
    // k = round(x / ln2); r = x - k*ln2  (|r| <= ln2/2)
    let k = (x / LN2 + if x >= 0.0 { 0.5 } else { -0.5 }) as i64;
    let r = x - (k as f64) * LN2;
    // e^r via Taylor.
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut nn = 1.0;
    let mut i = 1;
    while i < 30 {
        term *= r / nn;
        sum += term;
        nn += 1.0;
        i += 1;
    }
    // multiply by 2^k
    let mut result = sum;
    if k >= 0 {
        let mut j = 0;
        while j < k {
            result *= 2.0;
            j += 1;
        }
    } else {
        let mut j = 0;
        while j < -k {
            result *= 0.5;
            j += 1;
        }
    }
    result
}

/// Evaluate a full arithmetic expression and return the result.
///
/// Supports `+ - * /`, parentheses, decimal points, unary +/-, postfix `%`
/// (divide-by-100), and the `sqrt(x)` / `pow(base, exp)` functions. Never
/// panics: every malformed input or arithmetic fault returns a [`CalcError`].
///
/// ```
/// assert_eq!(ath_calc::eval("2+3*4"), Ok(14.0));
/// assert_eq!(ath_calc::eval("(2+3)*4"), Ok(20.0));
/// assert!(ath_calc::eval("5/0").is_err());
/// ```
pub fn eval(expr: &str) -> Result<f64, CalcError> {
    let toks = tokenize(expr)?;
    // Empty expression is malformed (not 0).
    if toks[0].is_none() {
        return Err(CalcError::Parse);
    }
    let mut p = Parser {
        toks: &toks,
        pos: 0,
    };
    let v = p.expr()?;
    // Trailing tokens (e.g. "2 3", "1)2") => malformed.
    if p.peek().is_some() {
        return Err(CalcError::Parse);
    }
    finite(v)
}

// ── Programmer mode: integer / bitwise evaluator (distinct from the f64 path) ─
//
// A separate i64 grammar so Programmer mode is exact and bit-accurate. Supports
// `+ - * / %` plus the bitwise operators `& | ^ ~ << >>`, with hex (`0x`), binary
// (`0b`), and octal (`0o`) literals. Precedence (lowest→highest), C-like:
//   `|`  →  `^`  →  `&`  →  `<< >>`  →  `+ -`  →  `* / %`  →  unary `- ~`  →  primary

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ITok {
    Num(i64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    And,
    Or,
    Xor,
    Not,
    Shl,
    Shr,
    LParen,
    RParen,
}

fn itokenize(expr: &str) -> Result<[Option<ITok>; MAX_TOKENS], CalcError> {
    let bytes = expr.as_bytes();
    let mut out: [Option<ITok>; MAX_TOKENS] = [None; MAX_TOKENS];
    let mut n = 0usize;
    let mut i = 0usize;

    macro_rules! push {
        ($t:expr) => {{
            if n >= MAX_TOKENS {
                return Err(CalcError::Parse);
            }
            out[n] = Some($t);
            n += 1;
        }};
    }

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'+' => {
                push!(ITok::Plus);
                i += 1;
            }
            b'-' => {
                push!(ITok::Minus);
                i += 1;
            }
            b'*' => {
                push!(ITok::Star);
                i += 1;
            }
            b'/' => {
                push!(ITok::Slash);
                i += 1;
            }
            b'%' => {
                push!(ITok::Percent);
                i += 1;
            }
            b'&' => {
                push!(ITok::And);
                i += 1;
            }
            b'|' => {
                push!(ITok::Or);
                i += 1;
            }
            b'^' => {
                push!(ITok::Xor);
                i += 1;
            }
            b'~' => {
                push!(ITok::Not);
                i += 1;
            }
            b'(' => {
                push!(ITok::LParen);
                i += 1;
            }
            b')' => {
                push!(ITok::RParen);
                i += 1;
            }
            b'<' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'<' {
                    push!(ITok::Shl);
                    i += 2;
                } else {
                    return Err(CalcError::Parse);
                }
            }
            b'>' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                    push!(ITok::Shr);
                    i += 2;
                } else {
                    return Err(CalcError::Parse);
                }
            }
            b'0'..=b'9' => {
                let (val, next) = parse_int_literal(bytes, i)?;
                push!(ITok::Num(val));
                i = next;
            }
            _ => return Err(CalcError::Parse),
        }
    }
    Ok(out)
}

/// Parse a non-negative integer literal: `0x..` (hex), `0b..` (binary), `0o..`
/// (octal), or decimal. Overflow of `i64` → `Parse`. Never panics.
fn parse_int_literal(bytes: &[u8], start: usize) -> Result<(i64, usize), CalcError> {
    let mut i = start;
    // Radix prefixes.
    if bytes[i] == b'0' && i + 1 < bytes.len() {
        let (radix, skip) = match bytes[i + 1] {
            b'x' | b'X' => (16u64, 2),
            b'b' | b'B' => (2u64, 2),
            b'o' | b'O' => (8u64, 2),
            _ => (0u64, 0),
        };
        if radix != 0 {
            i += skip;
            let digit_start = i;
            let mut acc: u64 = 0;
            while i < bytes.len() {
                let d = match bytes[i] {
                    b'0'..=b'9' => (bytes[i] - b'0') as u64,
                    b'a'..=b'f' => (bytes[i] - b'a' + 10) as u64,
                    b'A'..=b'F' => (bytes[i] - b'A' + 10) as u64,
                    _ => break,
                };
                if d >= radix {
                    break;
                }
                acc = acc
                    .checked_mul(radix)
                    .and_then(|v| v.checked_add(d))
                    .ok_or(CalcError::Parse)?;
                i += 1;
            }
            if i == digit_start {
                return Err(CalcError::Parse); // "0x" with no digits
            }
            return Ok((acc as i64, i));
        }
    }
    // Decimal.
    let mut acc: u64 = 0;
    let dstart = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        let d = (bytes[i] - b'0') as u64;
        acc = acc
            .checked_mul(10)
            .and_then(|v| v.checked_add(d))
            .ok_or(CalcError::Parse)?;
        i += 1;
    }
    if i == dstart {
        return Err(CalcError::Parse);
    }
    Ok((acc as i64, i))
}

struct IParser<'a> {
    toks: &'a [Option<ITok>; MAX_TOKENS],
    pos: usize,
}

impl<'a> IParser<'a> {
    fn peek(&self) -> Option<ITok> {
        if self.pos < MAX_TOKENS {
            self.toks[self.pos]
        } else {
            None
        }
    }
    fn expect(&mut self, want: ITok) -> Result<(), CalcError> {
        match self.peek() {
            Some(t) if t == want => {
                self.pos += 1;
                Ok(())
            }
            _ => Err(CalcError::Parse),
        }
    }

    // bit_or := bit_xor ('|' bit_xor)*
    fn bit_or(&mut self) -> Result<i64, CalcError> {
        let mut acc = self.bit_xor()?;
        while let Some(ITok::Or) = self.peek() {
            self.pos += 1;
            acc |= self.bit_xor()?;
        }
        Ok(acc)
    }
    // bit_xor := bit_and ('^' bit_and)*
    fn bit_xor(&mut self) -> Result<i64, CalcError> {
        let mut acc = self.bit_and()?;
        while let Some(ITok::Xor) = self.peek() {
            self.pos += 1;
            acc ^= self.bit_and()?;
        }
        Ok(acc)
    }
    // bit_and := shift ('&' shift)*
    fn bit_and(&mut self) -> Result<i64, CalcError> {
        let mut acc = self.shift()?;
        while let Some(ITok::And) = self.peek() {
            self.pos += 1;
            acc &= self.shift()?;
        }
        Ok(acc)
    }
    // shift := add (('<<' | '>>') add)*
    fn shift(&mut self) -> Result<i64, CalcError> {
        let mut acc = self.add()?;
        loop {
            match self.peek() {
                Some(ITok::Shl) => {
                    self.pos += 1;
                    let r = self.add()?;
                    acc = if (0..64).contains(&r) {
                        acc.wrapping_shl(r as u32)
                    } else {
                        0
                    };
                }
                Some(ITok::Shr) => {
                    self.pos += 1;
                    let r = self.add()?;
                    acc = if (0..64).contains(&r) {
                        acc.wrapping_shr(r as u32)
                    } else {
                        0
                    };
                }
                _ => break,
            }
        }
        Ok(acc)
    }
    // add := mul (('+' | '-') mul)*
    fn add(&mut self) -> Result<i64, CalcError> {
        let mut acc = self.mul()?;
        loop {
            match self.peek() {
                Some(ITok::Plus) => {
                    self.pos += 1;
                    acc = acc.wrapping_add(self.mul()?);
                }
                Some(ITok::Minus) => {
                    self.pos += 1;
                    acc = acc.wrapping_sub(self.mul()?);
                }
                _ => break,
            }
        }
        Ok(acc)
    }
    // mul := unary (('*' | '/' | '%') unary)*
    fn mul(&mut self) -> Result<i64, CalcError> {
        let mut acc = self.unary()?;
        loop {
            match self.peek() {
                Some(ITok::Star) => {
                    self.pos += 1;
                    acc = acc.wrapping_mul(self.unary()?);
                }
                Some(ITok::Slash) => {
                    self.pos += 1;
                    let r = self.unary()?;
                    if r == 0 {
                        return Err(CalcError::DivByZero);
                    }
                    acc = acc.wrapping_div(r);
                }
                Some(ITok::Percent) => {
                    self.pos += 1;
                    let r = self.unary()?;
                    if r == 0 {
                        return Err(CalcError::DivByZero);
                    }
                    acc = acc.wrapping_rem(r);
                }
                _ => break,
            }
        }
        Ok(acc)
    }
    // unary := ('-' | '~')* primary
    fn unary(&mut self) -> Result<i64, CalcError> {
        match self.peek() {
            Some(ITok::Minus) => {
                self.pos += 1;
                Ok(self.unary()?.wrapping_neg())
            }
            Some(ITok::Not) => {
                self.pos += 1;
                Ok(!self.unary()?)
            }
            Some(ITok::Plus) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.primary(),
        }
    }
    // primary := NUMBER | '(' bit_or ')'
    fn primary(&mut self) -> Result<i64, CalcError> {
        match self.peek() {
            Some(ITok::Num(v)) => {
                self.pos += 1;
                Ok(v)
            }
            Some(ITok::LParen) => {
                self.pos += 1;
                let v = self.bit_or()?;
                self.expect(ITok::RParen)?;
                Ok(v)
            }
            _ => Err(CalcError::Parse),
        }
    }
}

/// Evaluate an integer/bitwise expression for Programmer mode and return the
/// signed 64-bit result. Supports `+ - * / %` and `& | ^ ~ << >>`, with hex
/// (`0x`), binary (`0b`), and octal (`0o`) literals. Arithmetic wraps (two's
/// complement) rather than panicking; division/modulo by zero is
/// [`CalcError::DivByZero`]; malformed input is [`CalcError::Parse`].
///
/// ```
/// assert_eq!(ath_calc::eval_int("0xFF & 0x0F"), Ok(0x0F));
/// assert_eq!(ath_calc::eval_int("1 << 8"), Ok(256));
/// assert_eq!(ath_calc::eval_int("0b1010 | 0b0101"), Ok(0x0F));
/// ```
pub fn eval_int(expr: &str) -> Result<i64, CalcError> {
    let toks = itokenize(expr)?;
    if toks[0].is_none() {
        return Err(CalcError::Parse);
    }
    let mut p = IParser {
        toks: &toks,
        pos: 0,
    };
    let v = p.bit_or()?;
    if p.peek().is_some() {
        return Err(CalcError::Parse);
    }
    Ok(v)
}

// ── Button-driven calculator state machine ───────────────────────────────────

/// Maximum characters in the live entry buffer.
const ENTRY_CAP: usize = 31;

/// A standard immediate-execute desktop calculator (the Windows/macOS "Standard"
/// mode), driven by button/key events. It keeps a text entry buffer plus a
/// pending binary operation and an accumulator, so pressing `2 + 3 =` yields 5
/// and chaining `+ 4 =` continues from 8. Use [`eval`] directly when you have a
/// full expression string instead.
#[derive(Debug, Clone)]
pub struct Calculator {
    entry: [u8; ENTRY_CAP],
    entry_len: usize,
    accumulator: f64,
    pending: PendingOp,
    /// True when the next digit starts a brand-new entry (after `=`/operator).
    fresh: bool,
    /// True when the last evaluation errored; display shows "Error".
    error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingOp {
    None,
    Add,
    Sub,
    Mul,
    Div,
}

impl Default for Calculator {
    fn default() -> Self {
        Self::new()
    }
}

impl Calculator {
    /// A fresh calculator showing `0`.
    pub fn new() -> Self {
        let mut c = Self {
            entry: [0u8; ENTRY_CAP],
            entry_len: 1,
            accumulator: 0.0,
            pending: PendingOp::None,
            fresh: true,
            error: false,
        };
        c.entry[0] = b'0';
        c
    }

    /// The current display string (entry buffer, or "Error").
    pub fn display(&self) -> &str {
        if self.error {
            return "Error";
        }
        // Bytes are always ASCII digits / '.' / '-', so utf8 is total.
        core::str::from_utf8(&self.entry[..self.entry_len]).unwrap_or("0")
    }

    /// Current entry parsed as `f64` (0.0 if empty/lone sign).
    fn entry_value(&self) -> f64 {
        match eval(self.display()) {
            Ok(v) => v,
            Err(_) => 0.0,
        }
    }

    /// Append a digit `0..=9`.
    pub fn input_digit(&mut self, d: u8) {
        if d > 9 {
            return;
        }
        self.clear_error_if_any();
        if self.fresh {
            self.entry_len = 0;
            self.fresh = false;
        }
        // Replace a lone "0" entry.
        if self.entry_len == 1 && self.entry[0] == b'0' {
            self.entry_len = 0;
        }
        if self.entry_len < ENTRY_CAP {
            self.entry[self.entry_len] = b'0' + d;
            self.entry_len += 1;
        }
        if self.entry_len == 0 {
            self.entry[0] = b'0';
            self.entry_len = 1;
        }
    }

    /// Add a decimal point (ignored if the entry already has one).
    pub fn input_decimal(&mut self) {
        self.clear_error_if_any();
        if self.fresh {
            self.entry[0] = b'0';
            self.entry_len = 1;
            self.fresh = false;
        }
        if self.entry[..self.entry_len].contains(&b'.') {
            return;
        }
        if self.entry_len < ENTRY_CAP {
            self.entry[self.entry_len] = b'.';
            self.entry_len += 1;
        }
    }

    /// Apply a pending binary operator (immediate-execute semantics).
    pub fn apply_op(&mut self, op: char) {
        let new = match op {
            '+' => PendingOp::Add,
            '-' => PendingOp::Sub,
            '*' => PendingOp::Mul,
            '/' => PendingOp::Div,
            _ => return,
        };
        if self.error {
            return;
        }
        let cur = self.entry_value();
        if self.pending == PendingOp::None || self.fresh {
            self.accumulator = cur;
        } else {
            self.fold(cur);
        }
        self.pending = new;
        self.fresh = true;
    }

    /// `=` — fold the pending op and show the result.
    pub fn equals(&mut self) {
        if self.error {
            return;
        }
        let cur = self.entry_value();
        if self.pending != PendingOp::None {
            self.fold(cur);
        } else {
            self.accumulator = cur;
        }
        self.pending = PendingOp::None;
        self.set_entry(self.accumulator);
        self.fresh = true;
    }

    fn fold(&mut self, rhs: f64) {
        let r = match self.pending {
            PendingOp::Add => self.accumulator + rhs,
            PendingOp::Sub => self.accumulator - rhs,
            PendingOp::Mul => self.accumulator * rhs,
            PendingOp::Div => {
                if rhs == 0.0 {
                    self.error = true;
                    self.accumulator = 0.0;
                    return;
                }
                self.accumulator / rhs
            }
            PendingOp::None => rhs,
        };
        if r.is_finite() {
            self.accumulator = r;
        } else {
            self.error = true;
            self.accumulator = 0.0;
        }
    }

    /// `%` — interpret the current entry as a percentage of the accumulator
    /// when an op is pending (Windows behavior: `200 + 10 % =` → 220), else as
    /// "divide entry by 100".
    pub fn percent(&mut self) {
        if self.error {
            return;
        }
        let cur = self.entry_value();
        let result = if self.pending != PendingOp::None {
            self.accumulator * cur / 100.0
        } else {
            cur / 100.0
        };
        self.set_entry(result);
        self.fresh = false;
    }

    /// Toggle the sign of the current entry.
    pub fn negate(&mut self) {
        if self.error {
            return;
        }
        let v = self.entry_value();
        self.set_entry(-v);
        self.fresh = false;
    }

    /// Backspace one character of the entry.
    pub fn backspace(&mut self) {
        if self.error {
            self.clear();
            return;
        }
        if self.fresh {
            return;
        }
        if self.entry_len > 1 {
            self.entry_len -= 1;
            // Don't leave a lone "-".
            if self.entry_len == 1 && self.entry[0] == b'-' {
                self.entry[0] = b'0';
            }
        } else {
            self.entry[0] = b'0';
            self.entry_len = 1;
        }
    }

    /// `C` — full reset.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    fn clear_error_if_any(&mut self) {
        if self.error {
            self.clear();
        }
    }

    /// Format `v` into the entry buffer (trims trailing zeros, no exponent).
    fn set_entry(&mut self, v: f64) {
        self.entry_len = 0;
        if !v.is_finite() {
            self.error = true;
            return;
        }
        let mut value = v;
        if value == 0.0 {
            // normalize -0.0
            value = 0.0;
        }
        let neg = value < 0.0;
        if neg {
            value = -value;
        }

        // Round to 10 decimal places to avoid float noise like 0.30000000004.
        // Integer part.
        let int_part = floor(value) as u128;
        let mut frac = value - int_part as f64;

        let mut buf = [0u8; ENTRY_CAP];
        let mut len = 0usize;

        if neg {
            buf[len] = b'-';
            len += 1;
        }

        // Write integer digits.
        len += write_uint(&mut buf[len..], int_part);

        // Fractional part, up to 10 digits, trailing zeros trimmed.
        let mut frac_digits = [0u8; 10];
        let mut fd = 0usize;
        let mut i = 0;
        while i < 10 && frac > 0.0 {
            frac *= 10.0;
            let d = floor(frac) as u8;
            frac -= d as f64;
            frac_digits[fd] = b'0' + (d % 10);
            fd += 1;
            i += 1;
        }
        // Trim trailing zeros.
        while fd > 0 && frac_digits[fd - 1] == b'0' {
            fd -= 1;
        }
        if fd > 0 && len + 1 + fd <= ENTRY_CAP {
            buf[len] = b'.';
            len += 1;
            for j in 0..fd {
                buf[len] = frac_digits[j];
                len += 1;
            }
        }

        let copy = len.min(ENTRY_CAP);
        self.entry[..copy].copy_from_slice(&buf[..copy]);
        self.entry_len = copy.max(1);
        if copy == 0 {
            self.entry[0] = b'0';
        }
    }
}

/// `floor` without `libm`: truncate toward negative infinity for our finite,
/// in-range values.
fn floor(x: f64) -> f64 {
    let t = x as i128 as f64;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

/// Write a base-10 unsigned integer into `buf`, returning the byte count.
fn write_uint(buf: &mut [u8], mut v: u128) -> usize {
    if buf.is_empty() {
        return 0;
    }
    if v == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 40];
    let mut n = 0;
    while v > 0 && n < tmp.len() {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let count = n.min(buf.len());
    for j in 0..count {
        buf[j] = tmp[n - 1 - j];
    }
    count
}

// ── Host KATs (the FAIL-able proof: `cargo test -p ath_calc`) ────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn precedence_mul_before_add() {
        assert_eq!(eval("2+3*4"), Ok(14.0));
    }

    #[test]
    fn parentheses_override_precedence() {
        assert_eq!(eval("(2+3)*4"), Ok(20.0));
    }

    #[test]
    fn float_division() {
        // 10/4 must be 2.5, NOT integer-truncated 2 (the bug this crate fixes).
        assert_eq!(eval("10/4"), Ok(2.5));
        // Negative invariant: the integer-only engine would have returned 2.0.
        assert_ne!(eval("10/4"), Ok(2.0));
    }

    #[test]
    fn unary_minus() {
        assert_eq!(eval("-5+2"), Ok(-3.0));
        assert_eq!(eval("3*-2"), Ok(-6.0));
        assert_eq!(eval("--4"), Ok(4.0));
    }

    #[test]
    fn percent_is_divide_by_100() {
        assert_eq!(eval("50%"), Ok(0.5));
        assert_eq!(eval("200*10%"), Ok(20.0)); // 200 * 0.1
    }

    #[test]
    fn divide_by_zero_is_err_not_panic() {
        assert_eq!(eval("5/0"), Err(CalcError::DivByZero));
        assert_eq!(eval("1/(2-2)"), Err(CalcError::DivByZero));
    }

    #[test]
    fn malformed_input_is_err_not_panic() {
        // These must all return Err and must NOT panic.
        assert_eq!(eval("1++2"), Ok(3.0)); // "1 + (+2)" — valid unary plus
        assert!(eval("1+").is_err());
        assert!(eval("(1+2").is_err());
        assert!(eval("1+2)").is_err());
        assert!(eval("").is_err());
        assert!(eval("   ").is_err());
        assert!(eval("abc").is_err());
        assert!(eval("1.2.3").is_err());
        assert!(eval("2 3").is_err());
        assert!(eval("*5").is_err());
    }

    #[test]
    fn decimals_parse() {
        assert!(approx(eval("0.1+0.2").unwrap(), 0.3));
        assert_eq!(eval("3.14"), Ok(3.14));
        assert_eq!(eval(".5"), Ok(0.5));
    }

    #[test]
    fn functions() {
        assert_eq!(eval("sqrt(16)"), Ok(4.0));
        assert!(approx(eval("sqrt(2)").unwrap(), 1.414_213_562_373_095_1));
        assert_eq!(eval("pow(2,10)"), Ok(1024.0));
        assert!(approx(eval("pow(9,0.5)").unwrap(), 3.0));
        assert!(eval("sqrt(-1)").is_err()); // NotFinite, not a panic
        assert!(eval("pow(2)").is_err()); // missing arg
    }

    #[test]
    fn whitespace_tolerated() {
        assert_eq!(eval("  2 +  3 * 4 "), Ok(14.0));
    }

    // ── Scientific KATs ─────────────────────────────────────────────────────

    fn approx_t(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn trig_basics() {
        assert!(approx_t(eval("sin(0)").unwrap(), 0.0, 1e-9));
        assert!(approx_t(eval("cos(0)").unwrap(), 1.0, 1e-9));
        // sin(pi/2) ≈ 1 — the headline range-reduction proof.
        assert!(approx_t(eval("sin(pi/2)").unwrap(), 1.0, 1e-9));
        assert!(approx_t(eval("cos(pi)").unwrap(), -1.0, 1e-9));
        assert!(approx_t(eval("tan(0)").unwrap(), 0.0, 1e-9));
        // If trig range-reduction were broken (e.g. no reduce_pi), sin(pi/2)
        // would NOT be ~1. This guard flips first.
        assert_ne!(eval("sin(pi/2)").unwrap() < 0.5, true);
        // Large argument: sin(100π + π/2) = sin(π/2) = 1. With NO range
        // reduction the bounded Taylor series diverges wildly at arg≈314, so
        // this assert is the load-bearing proof that reduce_pi runs.
        assert!(approx_t(eval("sin(100*pi + pi/2)").unwrap(), 1.0, 1e-6));
        assert!(approx_t(eval("cos(100*pi)").unwrap(), 1.0, 1e-6));
    }

    #[test]
    fn inverse_trig() {
        assert!(approx_t(
            eval("asin(1)").unwrap(),
            std::f64::consts::FRAC_PI_2,
            1e-9
        ));
        assert!(approx_t(
            eval("acos(0)").unwrap(),
            std::f64::consts::FRAC_PI_2,
            1e-9
        ));
        assert!(approx_t(
            eval("atan(1)").unwrap(),
            std::f64::consts::FRAC_PI_4,
            1e-9
        ));
        // Round trip: sin(asin(0.3)) ≈ 0.3
        assert!(approx_t(eval("sin(asin(0.3))").unwrap(), 0.3, 1e-8));
    }

    #[test]
    fn logs_and_exp() {
        assert!(approx_t(eval("ln(e)").unwrap(), 1.0, 1e-9));
        assert!(approx_t(eval("exp(0)").unwrap(), 1.0, 1e-12));
        assert!(approx_t(eval("exp(1)").unwrap(), std::f64::consts::E, 1e-9));
        assert!(approx_t(eval("log10(1000)").unwrap(), 3.0, 1e-9));
        assert!(approx_t(eval("log2(1024)").unwrap(), 10.0, 1e-9));
        // ln(e) must NOT be 0 (a broken series would collapse).
        assert_ne!(eval("ln(e)"), Ok(0.0));
    }

    #[test]
    fn factorial_and_rounding() {
        assert_eq!(eval("5!"), Ok(120.0));
        assert_eq!(eval("fact(5)"), Ok(120.0));
        assert_eq!(eval("0!"), Ok(1.0));
        assert_eq!(eval("floor(3.7)"), Ok(3.0));
        assert_eq!(eval("ceil(3.2)"), Ok(4.0));
        assert_eq!(eval("round(2.5)"), Ok(3.0));
        assert_eq!(eval("abs(-9)"), Ok(9.0));
        assert!(approx_t(eval("pi").unwrap(), std::f64::consts::PI, 1e-12));
    }

    #[test]
    fn pow_caret_via_pow_fn() {
        // 2^10 through pow().
        assert_eq!(eval("pow(2,10)"), Ok(1024.0));
        assert_ne!(eval("pow(2,10)"), Ok(20.0));
    }

    #[test]
    fn science_domain_errors_no_panic() {
        // Must Err (not panic, not infinite-loop).
        assert!(eval("ln(-1)").is_err());
        assert!(eval("ln(0)").is_err());
        assert!(eval("sqrt(-1)").is_err());
        assert!(eval("asin(2)").is_err());
        assert!(eval("acos(2)").is_err());
        assert!(eval("(-2)!").is_err()); // factorial of negative
        assert!(eval("2.5!").is_err()); // factorial of non-integer
                                        // Huge factorial → NotFinite, not a hang.
        assert!(eval("200!").is_err());
    }

    // ── Programmer / bitwise KATs ───────────────────────────────────────────

    #[test]
    fn bitwise_basics() {
        assert_eq!(eval_int("0xFF & 0x0F"), Ok(0x0F));
        assert_eq!(eval_int("1 << 8"), Ok(256));
        assert_eq!(eval_int("0b1010 | 0b0101"), Ok(0x0F));
        assert_eq!(eval_int("0xF0 ^ 0x0F"), Ok(0xFF));
        assert_eq!(eval_int("0o17"), Ok(15));
        assert_eq!(eval_int("~0"), Ok(-1));
        assert_eq!(eval_int("256 >> 4"), Ok(16));
        // Precedence: + binds tighter than <<.
        assert_eq!(eval_int("1 + 1 << 4"), Ok(2 << 4));
        // Negative guard: 1<<8 is 256, NOT 9.
        assert_ne!(eval_int("1 << 8"), Ok(9));
    }

    #[test]
    fn bitwise_arithmetic_and_parens() {
        assert_eq!(eval_int("(2 + 3) * 4"), Ok(20));
        assert_eq!(eval_int("10 / 3"), Ok(3)); // integer division
        assert_eq!(eval_int("10 % 3"), Ok(1));
        assert_eq!(eval_int("0xFF"), Ok(255));
    }

    #[test]
    fn bitwise_errors_no_panic() {
        assert_eq!(eval_int("5 / 0"), Err(CalcError::DivByZero));
        assert_eq!(eval_int("5 % 0"), Err(CalcError::DivByZero));
        assert!(eval_int("0x").is_err());
        assert!(eval_int("0xZZ").is_err());
        assert!(eval_int("1 <").is_err());
        assert!(eval_int("1 +").is_err());
        assert!(eval_int("").is_err());
        assert!(eval_int("3.5").is_err()); // no floats in programmer mode
    }

    // ── Calculator state-machine KATs ───────────────────────────────────────

    #[test]
    fn calc_immediate_execute_chain() {
        let mut c = Calculator::new();
        assert_eq!(c.display(), "0");
        c.input_digit(2);
        c.apply_op('+');
        c.input_digit(3);
        c.equals();
        assert_eq!(c.display(), "5");
        // Chain: continue from 5.
        c.apply_op('*');
        c.input_digit(4);
        c.equals();
        assert_eq!(c.display(), "20");
    }

    #[test]
    fn calc_decimal_and_float_result() {
        let mut c = Calculator::new();
        c.input_digit(1);
        c.input_digit(0);
        c.apply_op('/');
        c.input_digit(4);
        c.equals();
        assert_eq!(c.display(), "2.5");
    }

    #[test]
    fn calc_decimal_button() {
        let mut c = Calculator::new();
        c.input_digit(3);
        c.input_decimal();
        c.input_decimal(); // second dot ignored
        c.input_digit(1);
        c.input_digit(4);
        assert_eq!(c.display(), "3.14");
    }

    #[test]
    fn calc_divide_by_zero_shows_error_not_panic() {
        let mut c = Calculator::new();
        c.input_digit(5);
        c.apply_op('/');
        c.input_digit(0);
        c.equals();
        assert_eq!(c.display(), "Error");
        // A digit after error starts clean.
        c.input_digit(7);
        assert_eq!(c.display(), "7");
    }

    #[test]
    fn calc_negate_and_backspace() {
        let mut c = Calculator::new();
        c.input_digit(4);
        c.input_digit(2);
        c.negate();
        assert_eq!(c.display(), "-42");
        c.backspace();
        assert_eq!(c.display(), "-4");
        c.backspace();
        assert_eq!(c.display(), "0");
    }

    #[test]
    fn calc_percent() {
        let mut c = Calculator::new();
        c.input_digit(5);
        c.input_digit(0);
        c.percent();
        assert_eq!(c.display(), "0.5");
    }

    #[test]
    fn calc_clear_resets() {
        let mut c = Calculator::new();
        c.input_digit(9);
        c.apply_op('+');
        c.input_digit(9);
        c.clear();
        assert_eq!(c.display(), "0");
    }
}
