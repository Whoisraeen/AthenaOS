//! # RaeJs core built-in library.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy (criterion #5 — "the web browser is the
//! universal app runtime; PWAs that feel native"): a tree-walking interpreter is only
//! useful to a page if the globals real scripts reach for exist. This module installs the
//! synchronous core that interactive snippets depend on, all `no_std`+`alloc`,
//! never-panic.
//!
//! ## Implemented
//! - `console.log` / `.error` / `.warn` / `.info` / `.debug` — captured into the
//!   interpreter buffer ([`Interpreter::take_console_output`]).
//! - `Math` — `abs floor ceil round trunc sign sqrt cbrt pow exp log log2 log10 min max
//!   sin cos tan random` (deterministic seeded) + `PI E LN2 LN10 SQRT2`.
//! - `JSON` — `parse` (hand-rolled, dep-free) + `stringify`.
//! - `Object` — `keys values entries assign freeze create getPrototypeOf
//!   defineProperty(simple) fromEntries`.
//! - `Array` — constructor + `isArray of from`; prototype `push pop shift unshift slice
//!   splice indexOf lastIndexOf includes join concat map filter reduce reduceRight
//!   forEach find findIndex some every sort reverse flat fill` + `length`.
//! - `String` prototype — `charAt charCodeAt codePointAt at slice substring substr indexOf
//!   lastIndexOf includes split replace(simple) replaceAll toUpperCase toLowerCase trim
//!   trimStart trimEnd repeat startsWith endsWith padStart padEnd concat`.
//! - `Number` — `parseInt parseFloat isNaN isFinite isInteger`; prototype `toFixed
//!   toString(radix)`; statics `MAX_SAFE_INTEGER MIN_SAFE_INTEGER EPSILON POSITIVE/
//!   NEGATIVE_INFINITY NaN`.
//! - Error constructors — `Error TypeError RangeError ReferenceError SyntaxError`.
//! - Globals — `parseInt parseFloat isNaN isFinite String Number Boolean Array
//!   globalThis NaN Infinity undefined`.
//!
//! ## Deferred (documented)
//! `Promise`/async, `WeakMap`/`WeakSet`, `Proxy`/`Reflect`, getters/setters as live
//! accessors, locale-aware methods, BigInt, typed arrays, and the DOM (a later slice).
//! `RegExp` execution + the regex-taking `String.prototype` methods (`match`/`matchAll`/
//! `replace`/`replaceAll`/`split`/`search`) now run via [`crate::builtins_regexp`] (reusing
//! the `rae_regex` engine); their honest flag/feature scope is documented there.

use crate::interp::{
    number_to_string, to_boolean, to_int32, to_uint32, ErrorKind, Interpreter, JsValue,
    RuntimeError,
};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

type R = Result<JsValue, RuntimeError>;

fn type_err(msg: &str) -> RuntimeError {
    RuntimeError::new_pub(ErrorKind::TypeError, msg)
}

fn range_err(msg: &str) -> RuntimeError {
    RuntimeError::new_pub(ErrorKind::RangeError, msg)
}

// ─── installer ───────────────────────────────────────────────────────────────

/// Install the global object + the core library into a fresh interpreter.
pub(crate) fn install(it: &mut Interpreter) {
    install_console(it);
    install_math(it);
    install_json(it);
    install_object(it);
    install_array(it);
    install_string_ctor(it);
    install_number(it);
    install_errors(it);
    install_globals(it);
    crate::builtins_collections::install(it);
    crate::builtins_regexp::install(it);
    crate::builtins_async::install(it);
}

fn make_namespace(it: &Interpreter, entries: &[(&str, JsValue)]) -> JsValue {
    let ns = it.new_object();
    for (k, v) in entries {
        let _ = it.set_property_raw(&ns, k, v.clone());
    }
    ns
}

// ─── console ─────────────────────────────────────────────────────────────────

fn install_console(it: &mut Interpreter) {
    let log = it.native("log", console_log);
    let console = make_namespace(
        it,
        &[
            ("log", log.clone()),
            ("info", log.clone()),
            ("debug", log.clone()),
            ("error", it.native("error", console_log)),
            ("warn", it.native("warn", console_log)),
        ],
    );
    it.define_global("console", console);
}

fn console_log(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    let mut parts = Vec::with_capacity(args.len());
    for a in args {
        parts.push(display_for_console(it, a)?);
    }
    it.push_console(parts.join(" "));
    Ok(JsValue::Undefined)
}

/// console renders strings without quotes but nested values inside arrays/objects with
/// JSON-ish quoting; this matches Node's `console.log` closely enough for tests.
fn display_for_console(it: &mut Interpreter, v: &JsValue) -> Result<String, RuntimeError> {
    match v {
        JsValue::String(s) => Ok(s.to_string()),
        JsValue::Array(a) => {
            let items = a.borrow().items.clone();
            let mut parts = Vec::new();
            for x in &items {
                parts.push(inspect(it, x)?);
            }
            Ok(format!("[ {} ]", parts.join(", ")))
        }
        JsValue::Object(_) => inspect(it, v),
        other => it.to_string(other),
    }
}

fn inspect(it: &mut Interpreter, v: &JsValue) -> Result<String, RuntimeError> {
    match v {
        JsValue::String(s) => Ok(format!("'{}'", s)),
        JsValue::Object(o) => {
            let keys: Vec<String> = o.borrow().props.iter().map(|(k, _)| k.clone()).collect();
            let mut parts = Vec::new();
            for k in keys {
                let val = it.get_property(v, &k)?;
                parts.push(format!("{}: {}", k, inspect(it, &val)?));
            }
            if parts.is_empty() {
                Ok("{}".to_string())
            } else {
                Ok(format!("{{ {} }}", parts.join(", ")))
            }
        }
        JsValue::Array(a) => {
            let items = a.borrow().items.clone();
            let mut parts = Vec::new();
            for x in &items {
                parts.push(inspect(it, x)?);
            }
            Ok(format!("[ {} ]", parts.join(", ")))
        }
        other => it.to_string(other),
    }
}

// ─── Math ────────────────────────────────────────────────────────────────────

fn install_math(it: &mut Interpreter) {
    macro_rules! m1 {
        ($name:literal, $f:expr) => {{
            fn wrapper(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
                let x = arg_num(it, a, 0)?;
                let g: fn(f64) -> f64 = $f;
                Ok(JsValue::Number(g(x)))
            }
            it.native($name, wrapper)
        }};
    }
    let math = make_namespace(
        it,
        &[
            ("PI", JsValue::Number(core::f64::consts::PI)),
            ("E", JsValue::Number(core::f64::consts::E)),
            ("LN2", JsValue::Number(core::f64::consts::LN_2)),
            ("LN10", JsValue::Number(core::f64::consts::LN_10)),
            ("SQRT2", JsValue::Number(core::f64::consts::SQRT_2)),
            ("abs", m1!("abs", mathfn::fabs)),
            ("floor", m1!("floor", mathfn::floor)),
            ("ceil", m1!("ceil", mathfn::ceil)),
            ("round", m1!("round", mathfn::round)),
            ("trunc", m1!("trunc", mathfn::trunc)),
            (
                "sign",
                m1!("sign", |x: f64| if x.is_nan() {
                    f64::NAN
                } else if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    x
                }),
            ),
            ("sqrt", m1!("sqrt", mathfn::sqrt)),
            ("cbrt", m1!("cbrt", mathfn::cbrt)),
            ("exp", m1!("exp", mathfn::exp)),
            ("log", m1!("log", mathfn::ln)),
            (
                "log2",
                m1!("log2", |x| mathfn::ln(x) / core::f64::consts::LN_2),
            ),
            (
                "log10",
                m1!("log10", |x| mathfn::ln(x) / core::f64::consts::LN_10),
            ),
            ("sin", m1!("sin", mathfn::sin)),
            ("cos", m1!("cos", mathfn::cos)),
            ("tan", m1!("tan", |x| mathfn::sin(x) / mathfn::cos(x))),
            ("pow", it.native("pow", math_pow)),
            ("max", it.native("max", math_max)),
            ("min", it.native("min", math_min)),
            ("random", it.native("random", math_random)),
            ("hypot", it.native("hypot", math_hypot)),
            ("clz32", it.native("clz32", math_clz32)),
        ],
    );
    it.define_global("Math", math);
}

/// `Math.clz32(x)` — count leading zero bits of `x` as an unsigned 32-bit integer
/// (`ToUint32`); `clz32(0)` is 32.
fn math_clz32(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let x = to_uint32(arg_num(it, a, 0)?);
    Ok(JsValue::Number(x.leading_zeros() as f64))
}

fn math_pow(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    Ok(JsValue::Number(mathfn::powf(
        arg_num(it, a, 0)?,
        arg_num(it, a, 1)?,
    )))
}
fn math_hypot(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let mut sum = 0.0;
    for v in a {
        let n = it.to_number(v)?;
        sum += n * n;
    }
    Ok(JsValue::Number(mathfn::sqrt(sum)))
}

// ── Pure-f64 math primitives (no_std core lacks f64::floor/sqrt/etc; no libm dep). ──
// Harvested from the proven rae_calc implementations (Newton sqrt, Taylor sin/cos,
// atanh-series ln, range-reduced exp) — bounded iteration counts, never-panic.
pub(crate) mod mathfn {
    const LN2: f64 = 0.693_147_180_559_945_3;

    #[inline]
    pub fn fabs(x: f64) -> f64 {
        if x < 0.0 {
            -x
        } else {
            x
        }
    }

    pub fn floor(x: f64) -> f64 {
        if !x.is_finite() {
            return x;
        }
        let t = x as i64 as f64;
        if t > x {
            t - 1.0
        } else {
            t
        }
    }

    pub fn ceil(x: f64) -> f64 {
        -floor(-x)
    }

    pub fn trunc(x: f64) -> f64 {
        if !x.is_finite() {
            return x;
        }
        x as i64 as f64
    }

    pub fn round(x: f64) -> f64 {
        // JS Math.round: round half UP (toward +Inf).
        floor(x + 0.5)
    }

    pub fn sqrt(x: f64) -> f64 {
        if x < 0.0 {
            return f64::NAN;
        }
        if x == 0.0 || !x.is_finite() {
            return x;
        }
        let mut g = if x >= 1.0 { x } else { 1.0 };
        let mut i = 0;
        while i < 64 {
            let next = 0.5 * (g + x / g);
            if fabs(next - g) <= 1e-15 * fabs(next) {
                return next;
            }
            g = next;
            i += 1;
        }
        g
    }

    pub fn cbrt(x: f64) -> f64 {
        if x == 0.0 || !x.is_finite() {
            return x;
        }
        let neg = x < 0.0;
        let a = fabs(x);
        let mut g = a;
        let mut i = 0;
        while i < 80 {
            let next = (2.0 * g + a / (g * g)) / 3.0;
            if fabs(next - g) <= 1e-15 * fabs(next) {
                g = next;
                break;
            }
            g = next;
            i += 1;
        }
        if neg {
            -g
        } else {
            g
        }
    }

    pub fn ln(x: f64) -> f64 {
        if x < 0.0 {
            return f64::NAN;
        }
        if x == 0.0 {
            return f64::NEG_INFINITY;
        }
        if !x.is_finite() {
            return x;
        }
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
        let t = (m - 1.0) / (m + 1.0);
        let t2 = t * t;
        let mut term = t;
        let mut sum = 0.0;
        let mut denom = 1.0;
        let mut i = 0;
        while i < 60 {
            sum += term / denom;
            term *= t2;
            denom += 2.0;
            i += 1;
        }
        2.0 * sum + (k as f64) * LN2
    }

    pub fn exp(x: f64) -> f64 {
        if x == 0.0 {
            return 1.0;
        }
        if x == f64::NEG_INFINITY {
            return 0.0;
        }
        if x == f64::INFINITY {
            return f64::INFINITY;
        }
        if x.is_nan() {
            return f64::NAN;
        }
        let k = (x / LN2 + if x >= 0.0 { 0.5 } else { -0.5 }) as i64;
        let r = x - (k as f64) * LN2;
        let mut term = 1.0;
        let mut sum = 1.0;
        let mut nn = 1.0;
        let mut i = 1;
        while i < 40 {
            term *= r / nn;
            sum += term;
            nn += 1.0;
            i += 1;
        }
        let mut result = sum;
        if k >= 0 {
            for _ in 0..k {
                result *= 2.0;
            }
        } else {
            for _ in 0..(-k) {
                result *= 0.5;
            }
        }
        result
    }

    pub fn powf(base: f64, exp_: f64) -> f64 {
        if exp_ == 0.0 {
            return 1.0;
        }
        if base.is_nan() || exp_.is_nan() {
            return f64::NAN;
        }
        // Integer exponent path (exact-ish, fast).
        if fabs(exp_) < 1024.0 && exp_ == (exp_ as i64 as f64) {
            let neg = exp_ < 0.0;
            let mut n = fabs(exp_) as u64;
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
        if base < 0.0 {
            return f64::NAN;
        }
        if base == 0.0 {
            return 0.0;
        }
        exp(exp_ * ln(base))
    }

    fn reduce_pi(x: f64) -> f64 {
        const TWO_PI: f64 = 2.0 * core::f64::consts::PI;
        let k = floor(x / TWO_PI + 0.5);
        x - k * TWO_PI
    }

    pub fn sin(x: f64) -> f64 {
        if !x.is_finite() {
            return f64::NAN;
        }
        let r = reduce_pi(x);
        let r2 = r * r;
        let mut term = r;
        let mut sum = r;
        let mut n = 1.0;
        let mut i = 0;
        while i < 24 {
            term = -term * r2 / ((2.0 * n) * (2.0 * n + 1.0));
            sum += term;
            n += 1.0;
            i += 1;
        }
        sum
    }

    pub fn cos(x: f64) -> f64 {
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
}
fn math_max(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let mut m = f64::NEG_INFINITY;
    for v in a {
        let n = it.to_number(v)?;
        if n.is_nan() {
            return Ok(JsValue::Number(f64::NAN));
        }
        if n > m {
            m = n;
        }
    }
    Ok(JsValue::Number(m))
}
fn math_min(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let mut m = f64::INFINITY;
    for v in a {
        let n = it.to_number(v)?;
        if n.is_nan() {
            return Ok(JsValue::Number(f64::NAN));
        }
        if n < m {
            m = n;
        }
    }
    Ok(JsValue::Number(m))
}
fn math_random(it: &mut Interpreter, _t: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::Number(it.next_random()))
}

// ─── JSON ──────────────────────────────────────────────────────────────────────

fn install_json(it: &mut Interpreter) {
    let json = make_namespace(
        it,
        &[
            ("parse", it.native("parse", json_parse)),
            ("stringify", it.native("stringify", json_stringify)),
        ],
    );
    it.define_global("JSON", json);
}

fn json_parse(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let src = arg_str(it, a, 0)?;
    let mut p = JsonParser {
        bytes: src.as_bytes(),
        pos: 0,
        depth: 0,
    };
    p.skip_ws();
    let v = p.parse_value(it)?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(RuntimeError::new_pub(
            ErrorKind::SyntaxError,
            "Unexpected token in JSON",
        ));
    }
    Ok(v)
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn err(&self, msg: &str) -> RuntimeError {
        RuntimeError::new_pub(ErrorKind::SyntaxError, msg)
    }

    fn parse_value(&mut self, it: &mut Interpreter) -> R {
        self.depth += 1;
        if self.depth > 256 {
            return Err(self.err("JSON nesting too deep"));
        }
        self.skip_ws();
        let r = match self.bytes.get(self.pos) {
            Some(b'{') => self.parse_object(it),
            Some(b'[') => self.parse_array(it),
            Some(b'"') => Ok(JsValue::str(self.parse_string()?)),
            Some(b't') => self.parse_lit("true", JsValue::Bool(true)),
            Some(b'f') => self.parse_lit("false", JsValue::Bool(false)),
            Some(b'n') => self.parse_lit("null", JsValue::Null),
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.parse_number(),
            _ => Err(self.err("Unexpected token in JSON")),
        };
        self.depth -= 1;
        r
    }

    fn parse_lit(&mut self, lit: &str, val: JsValue) -> R {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(val)
        } else {
            Err(self.err("Unexpected token in JSON"))
        }
    }

    fn parse_number(&mut self) -> R {
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b'-' || b == b'+' || b == b'.' || b == b'e' || b == b'E' || b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let s = core::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("Invalid JSON number"))?;
        s.parse::<f64>()
            .map(JsValue::Number)
            .map_err(|_| self.err("Invalid JSON number"))
    }

    fn parse_string(&mut self) -> Result<String, RuntimeError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.bytes.get(self.pos) {
                None => return Err(self.err("Unterminated JSON string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.bytes.get(self.pos) {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'n') => out.push('\n'),
                        Some(b't') => out.push('\t'),
                        Some(b'r') => out.push('\r'),
                        Some(b'b') => out.push('\u{0008}'),
                        Some(b'f') => out.push('\u{000C}'),
                        Some(b'u') => {
                            let hex = self
                                .bytes
                                .get(self.pos + 1..self.pos + 5)
                                .ok_or_else(|| self.err("Bad \\u escape"))?;
                            let code = u32::from_str_radix(
                                core::str::from_utf8(hex).map_err(|_| self.err("Bad \\u"))?,
                                16,
                            )
                            .map_err(|_| self.err("Bad \\u escape"))?;
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                            self.pos += 4;
                        }
                        _ => return Err(self.err("Bad JSON escape")),
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    // Copy one UTF-8 char.
                    let rest = &self.bytes[self.pos..];
                    let s = core::str::from_utf8(rest).map_err(|_| self.err("Invalid UTF-8"))?;
                    let c = s.chars().next().ok_or_else(|| self.err("Invalid string"))?;
                    out.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn parse_array(&mut self, it: &mut Interpreter) -> R {
        self.pos += 1; // [
        let mut items = Vec::new();
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(it.new_array(items));
        }
        loop {
            let v = self.parse_value(it)?;
            items.push(v);
            self.skip_ws();
            match self.bytes.get(self.pos) {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(it.new_array(items));
                }
                _ => return Err(self.err("Expected ',' or ']' in JSON array")),
            }
        }
    }

    fn parse_object(&mut self, it: &mut Interpreter) -> R {
        self.pos += 1; // {
        let obj = it.new_object();
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(obj);
        }
        loop {
            self.skip_ws();
            if self.bytes.get(self.pos) != Some(&b'"') {
                return Err(self.err("Expected string key in JSON object"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.bytes.get(self.pos) != Some(&b':') {
                return Err(self.err("Expected ':' in JSON object"));
            }
            self.pos += 1;
            let v = self.parse_value(it)?;
            it.set_property(&obj, &key, v)?;
            self.skip_ws();
            match self.bytes.get(self.pos) {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(obj);
                }
                _ => return Err(self.err("Expected ',' or '}' in JSON object")),
            }
        }
    }
}

fn json_stringify(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let v = a.first().cloned().unwrap_or(JsValue::Undefined);
    let mut out = String::new();
    match stringify_value(it, &v, &mut out, 0)? {
        true => Ok(JsValue::str(out)),
        false => Ok(JsValue::Undefined), // undefined / function at top level
    }
}

/// Returns Ok(false) if the value serializes to nothing (undefined/function/symbol).
fn stringify_value(
    it: &mut Interpreter,
    v: &JsValue,
    out: &mut String,
    depth: usize,
) -> Result<bool, RuntimeError> {
    if depth > 512 {
        return Err(RuntimeError::new_pub(
            ErrorKind::TypeError,
            "Converting circular/too-deep structure to JSON",
        ));
    }
    match v {
        JsValue::Undefined | JsValue::Function(_) => Ok(false),
        JsValue::Null => {
            out.push_str("null");
            Ok(true)
        }
        JsValue::Bool(b) => {
            out.push_str(if *b { "true" } else { "false" });
            Ok(true)
        }
        JsValue::Number(n) => {
            if n.is_finite() {
                out.push_str(&number_to_string(*n));
            } else {
                out.push_str("null"); // NaN/Infinity → null in JSON
            }
            Ok(true)
        }
        JsValue::String(s) => {
            json_quote(s, out);
            Ok(true)
        }
        JsValue::Array(arr) => {
            let items = arr.borrow().items.clone();
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let mut tmp = String::new();
                if stringify_value(it, item, &mut tmp, depth + 1)? {
                    out.push_str(&tmp);
                } else {
                    out.push_str("null"); // undefined/function in array → null
                }
            }
            out.push(']');
            Ok(true)
        }
        JsValue::Object(_o) => {
            // Source keys via `enumerate_keys` so accessor (getter) properties are
            // serialized too; `get_property` below invokes the getter for the value.
            let keys: Vec<String> = it.enumerate_keys(v);
            out.push('{');
            let mut first = true;
            for k in keys {
                let val = it.get_property(v, &k)?;
                let mut tmp = String::new();
                if stringify_value(it, &val, &mut tmp, depth + 1)? {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    json_quote(&k, out);
                    out.push(':');
                    out.push_str(&tmp);
                }
            }
            out.push('}');
            Ok(true)
        }
    }
}

fn json_quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ─── Object ──────────────────────────────────────────────────────────────────

fn install_object(it: &mut Interpreter) {
    let obj_ctor = it.native("Object", object_ctor);
    it.set_func_static(&obj_ctor, "keys", it.native("keys", object_keys));
    it.set_func_static(&obj_ctor, "values", it.native("values", object_values));
    it.set_func_static(&obj_ctor, "entries", it.native("entries", object_entries));
    it.set_func_static(&obj_ctor, "assign", it.native("assign", object_assign));
    it.set_func_static(&obj_ctor, "freeze", it.native("freeze", object_freeze));
    it.set_func_static(&obj_ctor, "create", it.native("create", object_create));
    it.set_func_static(
        &obj_ctor,
        "getOwnPropertyNames",
        it.native("getOwnPropertyNames", object_get_own_property_names),
    );
    it.set_func_static(
        &obj_ctor,
        "defineProperty",
        it.native("defineProperty", object_define_property),
    );
    it.set_func_static(
        &obj_ctor,
        "getPrototypeOf",
        it.native("getPrototypeOf", object_get_proto),
    );
    it.set_func_static(
        &obj_ctor,
        "fromEntries",
        it.native("fromEntries", object_from_entries),
    );
    it.define_global("Object", obj_ctor);
}

fn object_ctor(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    match a.first() {
        Some(JsValue::Object(_)) | Some(JsValue::Array(_)) => Ok(a[0].clone()),
        _ => Ok(it.new_object()),
    }
}
fn object_keys(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let v = a.first().cloned().unwrap_or(JsValue::Undefined);
    let keys = it
        .enumerate_keys(&v)
        .into_iter()
        .map(JsValue::str)
        .collect();
    Ok(it.new_array(keys))
}
fn object_values(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let v = a.first().cloned().unwrap_or(JsValue::Undefined);
    let mut out = Vec::new();
    for k in it.enumerate_keys(&v) {
        out.push(it.get_property(&v, &k)?);
    }
    Ok(it.new_array(out))
}
fn object_entries(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let v = a.first().cloned().unwrap_or(JsValue::Undefined);
    let mut out = Vec::new();
    for k in it.enumerate_keys(&v) {
        let val = it.get_property(&v, &k)?;
        out.push(it.new_array(vec![JsValue::str(k), val]));
    }
    Ok(it.new_array(out))
}
fn object_assign(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let target = a.first().cloned().unwrap_or_else(|| it.new_object());
    for src in a.iter().skip(1) {
        for k in it.enumerate_keys(src) {
            let val = it.get_property(src, &k)?;
            it.set_property(&target, &k, val)?;
        }
    }
    Ok(target)
}
fn object_get_own_property_names(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    // Own string keys (data + accessor), like Object.keys but conceptually
    // including non-enumerable — our model treats all own props the same, so
    // this returns the same set as enumerate_keys.
    let v = a.first().cloned().unwrap_or(JsValue::Undefined);
    let names: Vec<JsValue> = it
        .enumerate_keys(&v)
        .into_iter()
        .map(JsValue::str)
        .collect();
    Ok(it.new_array(names))
}
fn object_define_property(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    // Object.defineProperty(obj, key, descriptor): a `get`/`set` descriptor
    // installs a live accessor (the infra added for object-literal getters);
    // otherwise the `value` is stored as a plain data property. Used by every
    // transpiled CommonJS module (`Object.defineProperty(exports,"__esModule",
    // {value:true})`) and framework reactivity.
    let obj = a.first().cloned().unwrap_or(JsValue::Undefined);
    let key = it.to_string(&a.get(1).cloned().unwrap_or(JsValue::Undefined))?;
    let desc = a.get(2).cloned().unwrap_or(JsValue::Undefined);
    if let JsValue::Object(o) = &obj {
        let getter = it.get_property(&desc, "get")?;
        let setter = it.get_property(&desc, "set")?;
        let has_get = matches!(getter, JsValue::Function(_));
        let has_set = matches!(setter, JsValue::Function(_));
        if has_get || has_set {
            o.borrow_mut().define_accessor(
                &key,
                if has_get { Some(getter) } else { None },
                if has_set { Some(setter) } else { None },
            );
        } else {
            let value = it.get_property(&desc, "value")?;
            it.set_property_raw(&obj, &key, value)?;
        }
    }
    Ok(obj)
}
fn object_freeze(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    if let Some(JsValue::Object(o)) = a.first() {
        o.borrow_mut().frozen = true;
    }
    let _ = it;
    Ok(a.first().cloned().unwrap_or(JsValue::Undefined))
}
fn object_create(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let obj = it.new_object();
    if let Some(proto) = a.first() {
        if let JsValue::Object(o) = &obj {
            o.borrow_mut().proto = match proto {
                JsValue::Null => None,
                other => Some(other.clone()),
            };
        }
    }
    Ok(obj)
}
fn object_get_proto(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let v = a.first().cloned().unwrap_or(JsValue::Undefined);
    Ok(it.get_prototype(&v).unwrap_or(JsValue::Null))
}
fn object_from_entries(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let obj = it.new_object();
    let v = a.first().cloned().unwrap_or(JsValue::Undefined);
    for pair in it.iterate(&v)? {
        let k = it.get_property(&pair, "0")?;
        let val = it.get_property(&pair, "1")?;
        let ks = it.to_string(&k)?;
        it.set_property(&obj, &ks, val)?;
    }
    Ok(obj)
}

// ─── Array ───────────────────────────────────────────────────────────────────

fn install_array(it: &mut Interpreter) {
    let arr_ctor = it.native("Array", array_ctor);
    it.set_func_static(&arr_ctor, "isArray", it.native("isArray", array_is_array));
    it.set_func_static(&arr_ctor, "of", it.native("of", array_of));
    it.set_func_static(&arr_ctor, "from", it.native("from", array_from));
    it.define_global("Array", arr_ctor);
}

fn array_ctor(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    if a.len() == 1 {
        if let JsValue::Number(n) = a[0] {
            if n.is_finite() && n >= 0.0 && mathfn::trunc(n) == n {
                let len = n as usize;
                if len > crate::interp::MAX_ARRAY_LEN {
                    return Err(RuntimeError::new_pub(
                        ErrorKind::RangeError,
                        "Invalid array length",
                    ));
                }
                return Ok(it.new_array(vec![JsValue::Undefined; len]));
            }
            return Err(RuntimeError::new_pub(
                ErrorKind::RangeError,
                "Invalid array length",
            ));
        }
    }
    Ok(it.new_array(a.to_vec()))
}
fn array_is_array(_it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    Ok(JsValue::Bool(matches!(a.first(), Some(JsValue::Array(_)))))
}
fn array_of(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    Ok(it.new_array(a.to_vec()))
}
fn array_from(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let v = a.first().cloned().unwrap_or(JsValue::Undefined);
    let items = it.iterate(&v)?;
    if let Some(f) = a.get(1) {
        if matches!(f, JsValue::Function(_)) {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.into_iter().enumerate() {
                out.push(it.call_function(
                    f,
                    &JsValue::Undefined,
                    &[item, JsValue::Number(i as f64)],
                )?);
            }
            return Ok(it.new_array(out));
        }
    }
    Ok(it.new_array(items))
}

/// Dispatch an `Array.prototype.<method>` access into a bound native.
pub(crate) fn array_property(it: &Interpreter, key: &str) -> JsValue {
    let f: Option<crate::interp::NativeFn> = match key {
        "push" => Some(arr_push),
        "pop" => Some(arr_pop),
        "shift" => Some(arr_shift),
        "unshift" => Some(arr_unshift),
        "slice" => Some(arr_slice),
        "splice" => Some(arr_splice),
        "at" => Some(arr_at),
        "indexOf" => Some(arr_index_of),
        "lastIndexOf" => Some(arr_last_index_of),
        "includes" => Some(arr_includes),
        "join" => Some(arr_join),
        "concat" => Some(arr_concat),
        "map" => Some(arr_map),
        "filter" => Some(arr_filter),
        "reduce" => Some(arr_reduce),
        "reduceRight" => Some(arr_reduce_right),
        "forEach" => Some(arr_for_each),
        "find" => Some(arr_find),
        "findIndex" => Some(arr_find_index),
        "findLast" => Some(arr_find_last),
        "findLastIndex" => Some(arr_find_last_index),
        "some" => Some(arr_some),
        "every" => Some(arr_every),
        "sort" => Some(arr_sort),
        "reverse" => Some(arr_reverse),
        "flat" => Some(arr_flat),
        "flatMap" => Some(arr_flat_map),
        "fill" => Some(arr_fill),
        "copyWithin" => Some(arr_copy_within),
        "keys" => Some(arr_keys),
        "values" => Some(arr_values),
        "entries" => Some(arr_entries),
        "toString" => Some(arr_to_string),
        _ => None,
    };
    match f {
        Some(nf) => it.native(key, nf),
        None => object_property(it, key),
    }
}

/// `Object.prototype` instance-method fallback, consulted by `get_property` after
/// own-prop + prototype-chain lookup miss. Only `hasOwnProperty` is resolved here
/// (the overwhelmingly common one); `toString`/`valueOf` are deliberately NOT
/// surfaced so the interpreter's default `[object Object]` stringification path is
/// left untouched.
pub(crate) fn object_property(it: &Interpreter, key: &str) -> JsValue {
    match key {
        "hasOwnProperty" => it.native("hasOwnProperty", obj_has_own_property),
        _ => JsValue::Undefined,
    }
}

/// `obj.hasOwnProperty(k)` — true iff `k` is an OWN (not inherited) property of
/// `this`. Covers plain objects, arrays (numeric index in range, `length`, and
/// expando props), and strings (index in range, `length`).
fn obj_has_own_property(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let key = it.to_string(a.first().unwrap_or(&JsValue::Undefined))?;
    let has = match this {
        JsValue::Object(o) => o.borrow().get_own(key.as_str()).is_some(),
        JsValue::Array(arr) => {
            if key == "length" {
                true
            } else if let Ok(idx) = key.parse::<usize>() {
                idx < arr.borrow().items.len()
            } else {
                arr.borrow().props.iter().any(|(k, _)| k.as_str() == key)
            }
        }
        JsValue::String(s) => {
            key == "length"
                || key
                    .parse::<usize>()
                    .map_or(false, |i| i < s.chars().count())
        }
        _ => false,
    };
    Ok(JsValue::Bool(has))
}

fn this_array(
    this: &JsValue,
) -> Result<Rc<core::cell::RefCell<crate::interp::JsArray>>, RuntimeError> {
    match this {
        JsValue::Array(a) => Ok(a.clone()),
        _ => Err(type_err("Array.prototype method called on non-array")),
    }
}

fn arr_push(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    {
        let mut b = arr.borrow_mut();
        if b.items.len() + a.len() > crate::interp::MAX_ARRAY_LEN {
            return Err(RuntimeError::new_pub(
                ErrorKind::RangeError,
                "array length budget exceeded",
            ));
        }
        b.items.extend_from_slice(a);
    }
    let _ = it;
    let len = arr.borrow().items.len();
    Ok(JsValue::Number(len as f64))
}
fn arr_pop(_it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let popped = arr.borrow_mut().items.pop();
    Ok(popped.unwrap_or(JsValue::Undefined))
}
fn arr_shift(_it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let mut b = arr.borrow_mut();
    if b.items.is_empty() {
        Ok(JsValue::Undefined)
    } else {
        Ok(b.items.remove(0))
    }
}
fn arr_unshift(_it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let mut b = arr.borrow_mut();
    for (i, v) in a.iter().enumerate() {
        b.items.insert(i, v.clone());
    }
    Ok(JsValue::Number(b.items.len() as f64))
}
fn norm_index(idx: f64, len: usize) -> usize {
    if idx < 0.0 {
        let r = len as f64 + idx;
        if r < 0.0 {
            0
        } else {
            r as usize
        }
    } else {
        (idx as usize).min(len)
    }
}
fn arr_slice(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let items = arr.borrow().items.clone();
    let len = items.len();
    let start = match a.first() {
        Some(v) => norm_index(it.to_number(v)?, len),
        None => 0,
    };
    let end = match a.get(1) {
        Some(JsValue::Undefined) | None => len,
        Some(v) => norm_index(it.to_number(v)?, len),
    };
    let slice = if start < end {
        items[start..end].to_vec()
    } else {
        Vec::new()
    };
    Ok(it.new_array(slice))
}
fn arr_splice(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let len = arr.borrow().items.len();
    let start = match a.first() {
        Some(v) => norm_index(it.to_number(v)?, len),
        None => 0,
    };
    let delete_count = match a.get(1) {
        Some(v) => {
            let n = it.to_number(v)?;
            if n < 0.0 {
                0
            } else {
                (n as usize).min(len - start)
            }
        }
        None => len - start,
    };
    let removed: Vec<JsValue>;
    {
        let mut b = arr.borrow_mut();
        removed = b
            .items
            .splice(start..start + delete_count, a.iter().skip(2).cloned())
            .collect();
    }
    Ok(it.new_array(removed))
}
fn arr_at(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    // Array.prototype.at(index): negative indexes count from the end; an
    // out-of-range index yields `undefined` (ES2022 relative indexing).
    let arr = this_array(this)?;
    let items = arr.borrow().items.clone();
    let len = items.len() as i64;
    let mut idx = a
        .first()
        .map(|v| it.to_number(v))
        .transpose()?
        .unwrap_or(0.0) as i64;
    if idx < 0 {
        idx += len;
    }
    if idx < 0 || idx >= len {
        return Ok(JsValue::Undefined);
    }
    Ok(items[idx as usize].clone())
}
fn arr_index_of(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let target = a.first().cloned().unwrap_or(JsValue::Undefined);
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate() {
        if it.strict_eq(v, &target) {
            return Ok(JsValue::Number(i as f64));
        }
    }
    Ok(JsValue::Number(-1.0))
}
fn arr_last_index_of(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let target = a.first().cloned().unwrap_or(JsValue::Undefined);
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate().rev() {
        if it.strict_eq(v, &target) {
            return Ok(JsValue::Number(i as f64));
        }
    }
    Ok(JsValue::Number(-1.0))
}
fn arr_includes(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let target = a.first().cloned().unwrap_or(JsValue::Undefined);
    let items = arr.borrow().items.clone();
    Ok(JsValue::Bool(
        items.iter().any(|v| it.strict_eq(v, &target)),
    ))
}
fn arr_join(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let sep = match a.first() {
        Some(JsValue::Undefined) | None => ",".to_string(),
        Some(v) => it.to_string(v)?,
    };
    let items = arr.borrow().items.clone();
    let mut parts = Vec::with_capacity(items.len());
    for v in &items {
        parts.push(match v {
            JsValue::Undefined | JsValue::Null => String::new(),
            other => it.to_string(other)?,
        });
    }
    Ok(JsValue::str(parts.join(&sep)))
}
fn arr_to_string(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    arr_join(it, this, &[])
}
fn arr_concat(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let mut out = arr.borrow().items.clone();
    for v in a {
        match v {
            JsValue::Array(other) => out.extend(other.borrow().items.iter().cloned()),
            other => out.push(other.clone()),
        }
    }
    Ok(it.new_array(out))
}
fn arr_map(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("map callback required"))?;
    let items = arr.borrow().items.clone();
    let mut out = Vec::with_capacity(items.len());
    for (i, v) in items.iter().enumerate() {
        out.push(it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?);
    }
    Ok(it.new_array(out))
}
fn arr_filter(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("filter callback required"))?;
    let items = arr.borrow().items.clone();
    let mut out = Vec::new();
    for (i, v) in items.iter().enumerate() {
        let keep = it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
        if to_boolean(&keep) {
            out.push(v.clone());
        }
    }
    Ok(it.new_array(out))
}
fn arr_reduce(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("reduce callback required"))?;
    let items = arr.borrow().items.clone();
    let mut acc;
    let mut start = 0;
    if a.len() >= 2 {
        acc = a[1].clone();
    } else if items.is_empty() {
        return Err(type_err("Reduce of empty array with no initial value"));
    } else {
        acc = items[0].clone();
        start = 1;
    }
    for (i, v) in items.iter().enumerate().skip(start) {
        acc = it.call_function(
            &f,
            &JsValue::Undefined,
            &[acc, v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
    }
    Ok(acc)
}
fn arr_reduce_right(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("reduceRight callback required"))?;
    let items = arr.borrow().items.clone();
    let n = items.len();
    let mut acc;
    let mut start = 0usize; // count from the end
    if a.len() >= 2 {
        acc = a[1].clone();
    } else if items.is_empty() {
        return Err(type_err("Reduce of empty array with no initial value"));
    } else {
        acc = items[n - 1].clone();
        start = 1;
    }
    for k in start..n {
        let i = n - 1 - k;
        acc = it.call_function(
            &f,
            &JsValue::Undefined,
            &[
                acc,
                items[i].clone(),
                JsValue::Number(i as f64),
                this.clone(),
            ],
        )?;
    }
    Ok(acc)
}
fn arr_for_each(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("forEach callback required"))?;
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate() {
        it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
    }
    Ok(JsValue::Undefined)
}
fn arr_find(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("find callback required"))?;
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate() {
        let r = it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
        if to_boolean(&r) {
            return Ok(v.clone());
        }
    }
    Ok(JsValue::Undefined)
}
fn arr_find_index(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("findIndex callback required"))?;
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate() {
        let r = it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
        if to_boolean(&r) {
            return Ok(JsValue::Number(i as f64));
        }
    }
    Ok(JsValue::Number(-1.0))
}
fn arr_find_last(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("findLast callback required"))?;
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate().rev() {
        let r = it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
        if to_boolean(&r) {
            return Ok(v.clone());
        }
    }
    Ok(JsValue::Undefined)
}
fn arr_find_last_index(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("findLastIndex callback required"))?;
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate().rev() {
        let r = it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
        if to_boolean(&r) {
            return Ok(JsValue::Number(i as f64));
        }
    }
    Ok(JsValue::Number(-1.0))
}
/// `Array.prototype.flatMap` -- map each element, then flatten the result exactly
/// ONE level (array results spread, non-arrays appended). Bounded: one step per
/// element + the output capped at `MAX_ARRAY_LEN`, so a hostile callback can't
/// hang or grow memory unboundedly.
fn arr_flat_map(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("flatMap callback required"))?;
    let items = arr.borrow().items.clone();
    let mut out = Vec::new();
    for (i, v) in items.iter().enumerate() {
        it.charge_step()?;
        let mapped = it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
        let pushed = match mapped {
            JsValue::Array(inner) => inner.borrow().items.clone(),
            other => {
                let mut single = Vec::with_capacity(1);
                single.push(other);
                single
            }
        };
        for elem in pushed {
            if out.len() >= crate::interp::MAX_ARRAY_LEN {
                return Err(RuntimeError::new_pub(
                    ErrorKind::RangeError,
                    "flatMap result exceeds maximum array length",
                ));
            }
            out.push(elem);
        }
    }
    Ok(it.new_array(out))
}
fn arr_some(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("some callback required"))?;
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate() {
        let r = it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
        if to_boolean(&r) {
            return Ok(JsValue::Bool(true));
        }
    }
    Ok(JsValue::Bool(false))
}
fn arr_every(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let f = a
        .first()
        .cloned()
        .ok_or_else(|| type_err("every callback required"))?;
    let items = arr.borrow().items.clone();
    for (i, v) in items.iter().enumerate() {
        let r = it.call_function(
            &f,
            &JsValue::Undefined,
            &[v.clone(), JsValue::Number(i as f64), this.clone()],
        )?;
        if !to_boolean(&r) {
            return Ok(JsValue::Bool(false));
        }
    }
    Ok(JsValue::Bool(true))
}
fn arr_sort(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let cmp = a.first().cloned();
    let mut items = arr.borrow().items.clone();
    // Insertion sort so we can run a fallible JS comparator without panicking.
    let n = items.len();
    for i in 1..n {
        let mut j = i;
        while j > 0 {
            let order = match &cmp {
                Some(f) if matches!(f, JsValue::Function(_)) => {
                    let r = it.call_function(
                        f,
                        &JsValue::Undefined,
                        &[items[j - 1].clone(), items[j].clone()],
                    )?;
                    it.to_number(&r)?
                }
                _ => {
                    // Default (no comparator): compare as strings. This branch does NOT go
                    // through `call_function` (which ticks), so its O(n²) comparisons would
                    // otherwise never charge the step budget — `new Array(big).sort()` would
                    // hang. Charge one step per comparison so MAX_STEPS bounds it (RangeError).
                    it.charge_step()?;
                    let sa = it.to_string(&items[j - 1])?;
                    let sb = it.to_string(&items[j])?;
                    if sa <= sb {
                        -1.0
                    } else {
                        1.0
                    }
                }
            };
            if order > 0.0 {
                items.swap(j - 1, j);
                j -= 1;
            } else {
                break;
            }
        }
    }
    arr.borrow_mut().items = items;
    Ok(this.clone())
}
fn arr_reverse(_it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    arr.borrow_mut().items.reverse();
    Ok(this.clone())
}
/// Hard cap on how deep `flat` will descend natively, independent of the user-supplied
/// depth (`flat(Infinity)` → `i64::MAX`). Mirrors JSON's 512 recursion guard: a cyclic
/// array (`let a=[]; a.push(a); a.flat(Infinity)`) would otherwise overflow the host stack.
/// Beyond this depth, nested arrays are emitted as-is (bounded, never a crash).
const FLAT_MAX_NATIVE_DEPTH: i64 = 512;

fn arr_flat(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let depth = match a.first() {
        // NaN → 0 (spec ToIntegerOrInfinity). A huge/Infinity depth is clamped to the
        // native cap so a cyclic/deep array can never overflow the host stack.
        Some(v) => {
            let n = it.to_number(v)?;
            if n.is_nan() {
                0
            } else if n >= FLAT_MAX_NATIVE_DEPTH as f64 {
                FLAT_MAX_NATIVE_DEPTH
            } else if n <= 0.0 {
                0
            } else {
                n as i64
            }
        }
        None => 1,
    };
    let items = arr.borrow().items.clone();
    let mut out = Vec::new();
    flatten(it, &items, depth, &mut out)?;
    Ok(it.new_array(out))
}

/// Flatten `items` to `depth` levels into `out`. Bounded three ways so it can never hang or
/// overflow the host stack on a hostile (huge/cyclic) array:
/// 1. native recursion is capped at [`FLAT_MAX_NATIVE_DEPTH`] (`depth` is pre-clamped to it
///    by the caller, so `flat(Infinity)` on a cycle stops descending and emits the nested
///    array as a value instead of recursing forever);
/// 2. one step is charged per element (`it.charge_step()?`) so an enormous array hits
///    MAX_STEPS → `RangeError`;
/// 3. the output length is bounded by [`MAX_ARRAY_LEN`] (RangeError past it).
fn flatten(
    it: &mut Interpreter,
    items: &[JsValue],
    depth: i64,
    out: &mut Vec<JsValue>,
) -> Result<(), RuntimeError> {
    for v in items {
        it.charge_step()?;
        if out.len() >= crate::interp::MAX_ARRAY_LEN {
            return Err(RuntimeError::new_pub(
                ErrorKind::RangeError,
                "flat result exceeds maximum array length",
            ));
        }
        match v {
            JsValue::Array(a) if depth > 0 => {
                let inner = a.borrow().items.clone();
                flatten(it, &inner, depth - 1, out)?;
            }
            other => out.push(other.clone()),
        }
    }
    Ok(())
}
fn arr_fill(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let val = a.first().cloned().unwrap_or(JsValue::Undefined);
    let len = arr.borrow().items.len();
    let start = match a.get(1) {
        Some(v) => norm_index(it.to_number(v)?, len),
        None => 0,
    };
    let end = match a.get(2) {
        Some(JsValue::Undefined) | None => len,
        Some(v) => norm_index(it.to_number(v)?, len),
    };
    let mut b = arr.borrow_mut();
    for i in start..end {
        b.items[i] = val.clone();
    }
    Ok(this.clone())
}
/// `Array.prototype.copyWithin(target, start?, end?)` — shallow-copy the slice
/// `[start, end)` to position `target` within the SAME array (negatives index from
/// the end, per the spec). Source and target may overlap, so the source range is
/// snapshotted first (memmove semantics). Returns the array.
fn arr_copy_within(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let len = arr.borrow().items.len();
    let target = match a.first() {
        Some(JsValue::Undefined) | None => 0,
        Some(v) => norm_index(it.to_number(v)?, len),
    };
    let start = match a.get(1) {
        Some(JsValue::Undefined) | None => 0,
        Some(v) => norm_index(it.to_number(v)?, len),
    };
    let end = match a.get(2) {
        Some(JsValue::Undefined) | None => len,
        Some(v) => norm_index(it.to_number(v)?, len),
    };
    let count = end.saturating_sub(start).min(len.saturating_sub(target));
    if count > 0 {
        let mut b = arr.borrow_mut();
        let src: alloc::vec::Vec<JsValue> = b.items[start..start + count].to_vec();
        for (k, v) in src.into_iter().enumerate() {
            b.items[target + k] = v;
        }
    }
    Ok(this.clone())
}
fn arr_keys(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let n = arr.borrow().items.len();
    Ok(it.new_array((0..n).map(|i| JsValue::Number(i as f64)).collect()))
}
fn arr_values(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let arr = this_array(this)?;
    let items = arr.borrow().items.clone();
    Ok(it.new_array(items))
}
fn arr_entries(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    // `[a,b].entries()` → an iterable of `[index, value]` pairs (here a dense
    // array of pairs, matching how `keys()`/`values()` are modeled as spreadable
    // arrays rather than lazy iterator objects).
    let arr = this_array(this)?;
    let items = arr.borrow().items.clone();
    let pairs: Vec<JsValue> = items
        .into_iter()
        .enumerate()
        .map(|(i, v)| it.new_array(alloc::vec![JsValue::Number(i as f64), v]))
        .collect();
    Ok(it.new_array(pairs))
}

// ─── String ──────────────────────────────────────────────────────────────────

fn install_string_ctor(it: &mut Interpreter) {
    let s = it.native("String", string_ctor);
    it.set_func_static(
        &s,
        "fromCharCode",
        it.native("fromCharCode", string_from_char_code),
    );
    it.set_func_static(
        &s,
        "fromCodePoint",
        it.native("fromCodePoint", string_from_code_point),
    );
    it.define_global("String", s);
}
fn string_ctor(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    match a.first() {
        Some(v) => Ok(JsValue::str(it.to_string(v)?)),
        None => Ok(JsValue::str("")),
    }
}
fn string_from_char_code(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let mut out = String::new();
    for v in a {
        let code = it.to_number(v)? as u32;
        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
    }
    Ok(JsValue::str(out))
}
/// `String.fromCodePoint(...cps)` — build a string from full Unicode code points.
/// Spec-correct: a non-integer, out-of-range (> 0x10FFFF), or lone-surrogate code
/// point throws a `RangeError`.
fn string_from_code_point(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let mut out = String::new();
    for v in a {
        let n = it.to_number(v)?;
        if n < 0.0 || n > 0x10_FFFF as f64 || mathfn::trunc(n) != n {
            return Err(range_err("Invalid code point"));
        }
        match char::from_u32(n as u32) {
            Some(c) => out.push(c),
            None => return Err(range_err("Invalid code point")), // lone surrogate
        }
    }
    Ok(JsValue::str(out))
}

/// Dispatch a `String.prototype.<method>` access. The bound receiver is the string
/// primitive itself (each native re-reads `this`).
pub(crate) fn string_property(it: &Interpreter, key: &str) -> JsValue {
    let f: Option<crate::interp::NativeFn> = match key {
        "charAt" => Some(str_char_at),
        "charCodeAt" => Some(str_char_code_at),
        "codePointAt" => Some(str_char_code_at),
        "at" => Some(str_at),
        "slice" => Some(str_slice),
        "substring" => Some(str_substring),
        "substr" => Some(str_substr),
        "indexOf" => Some(str_index_of),
        "lastIndexOf" => Some(str_last_index_of),
        "includes" => Some(str_includes),
        "startsWith" => Some(str_starts_with),
        "endsWith" => Some(str_ends_with),
        "split" => Some(str_split),
        "replace" => Some(str_replace),
        "replaceAll" => Some(str_replace_all),
        "match" => Some(str_match),
        "matchAll" => Some(str_match_all),
        "search" => Some(str_search),
        "toUpperCase" => Some(str_to_upper),
        "toLowerCase" => Some(str_to_lower),
        "trim" => Some(str_trim),
        "trimStart" => Some(str_trim_start),
        "trimEnd" => Some(str_trim_end),
        "repeat" => Some(str_repeat),
        "padStart" => Some(str_pad_start),
        "padEnd" => Some(str_pad_end),
        "concat" => Some(str_concat),
        "localeCompare" => Some(str_locale_compare),
        "toString" => Some(str_to_string),
        "valueOf" => Some(str_to_string),
        _ => None,
    };
    match f {
        Some(nf) => it.native(key, nf),
        None => object_property(it, key),
    }
}

fn this_string(it: &mut Interpreter, this: &JsValue) -> Result<String, RuntimeError> {
    it.to_string(this)
}
fn str_to_string(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::str(this_string(it, this)?))
}
/// `str.localeCompare(that)` — returns -1/0/1. Without ICU collation data this uses
/// code-point order (the same fallback V8-without-intl uses), which is correct for
/// ASCII and deterministic for the rest.
fn str_locale_compare(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let other = match a.first() {
        Some(v) => it.to_string(v)?,
        None => String::from("undefined"),
    };
    let ord = match s.as_str().cmp(other.as_str()) {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Equal => 0.0,
        core::cmp::Ordering::Greater => 1.0,
    };
    Ok(JsValue::Number(ord))
}
fn str_char_at(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let idx = a
        .first()
        .map(|v| it.to_number(v))
        .transpose()?
        .unwrap_or(0.0);
    if idx < 0.0 {
        return Ok(JsValue::str(""));
    }
    Ok(match s.chars().nth(idx as usize) {
        Some(c) => JsValue::str(c.to_string()),
        None => JsValue::str(""),
    })
}
fn str_char_code_at(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let idx = a
        .first()
        .map(|v| it.to_number(v))
        .transpose()?
        .unwrap_or(0.0);
    if idx < 0.0 {
        return Ok(JsValue::Number(f64::NAN));
    }
    Ok(match s.chars().nth(idx as usize) {
        Some(c) => JsValue::Number(c as u32 as f64),
        None => JsValue::Number(f64::NAN),
    })
}
fn str_at(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let mut idx = a
        .first()
        .map(|v| it.to_number(v))
        .transpose()?
        .unwrap_or(0.0) as i64;
    if idx < 0 {
        idx += len;
    }
    if idx < 0 || idx >= len {
        return Ok(JsValue::Undefined);
    }
    Ok(JsValue::str(chars[idx as usize].to_string()))
}
fn str_slice(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let start = match a.first() {
        Some(v) => norm_index(it.to_number(v)?, len),
        None => 0,
    };
    let end = match a.get(1) {
        Some(JsValue::Undefined) | None => len,
        Some(v) => norm_index(it.to_number(v)?, len),
    };
    let out: String = if start < end {
        chars[start..end].iter().collect()
    } else {
        String::new()
    };
    Ok(JsValue::str(out))
}
fn str_substring(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let clamp = |n: f64| -> usize {
        if n.is_nan() || n < 0.0 {
            0
        } else {
            (n as usize).min(len)
        }
    };
    let mut start = match a.first() {
        Some(v) => clamp(it.to_number(v)?),
        None => 0,
    };
    let mut end = match a.get(1) {
        Some(JsValue::Undefined) | None => len,
        Some(v) => clamp(it.to_number(v)?),
    };
    if start > end {
        core::mem::swap(&mut start, &mut end);
    }
    Ok(JsValue::str(chars[start..end].iter().collect::<String>()))
}
fn str_substr(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let start = match a.first() {
        Some(v) => norm_index(it.to_number(v)?, len),
        None => 0,
    };
    let count = match a.get(1) {
        Some(v) => (it.to_number(v)?.max(0.0) as usize).min(len - start),
        None => len - start,
    };
    Ok(JsValue::str(
        chars[start..start + count].iter().collect::<String>(),
    ))
}
fn str_index_of(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let needle = arg_str(it, a, 0)?;
    Ok(JsValue::Number(match char_index_of(&s, &needle) {
        Some(i) => i as f64,
        None => -1.0,
    }))
}
fn str_last_index_of(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let needle = arg_str(it, a, 0)?;
    // Char-index of the last occurrence.
    let mut found: Option<usize> = None;
    let chars: Vec<char> = s.chars().collect();
    let nchars: Vec<char> = needle.chars().collect();
    if nchars.is_empty() {
        return Ok(JsValue::Number(chars.len() as f64));
    }
    if chars.len() >= nchars.len() {
        for i in 0..=(chars.len() - nchars.len()) {
            if chars[i..i + nchars.len()] == nchars[..] {
                found = Some(i);
            }
        }
    }
    Ok(JsValue::Number(found.map(|i| i as f64).unwrap_or(-1.0)))
}
fn char_index_of(haystack: &str, needle: &str) -> Option<usize> {
    let h: Vec<char> = haystack.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() {
        return Some(0);
    }
    if h.len() < n.len() {
        return None;
    }
    for i in 0..=(h.len() - n.len()) {
        if h[i..i + n.len()] == n[..] {
            return Some(i);
        }
    }
    None
}
fn str_includes(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let needle = arg_str(it, a, 0)?;
    Ok(JsValue::Bool(s.contains(&needle)))
}
fn str_starts_with(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let needle = arg_str(it, a, 0)?;
    Ok(JsValue::Bool(s.starts_with(&needle)))
}
fn str_ends_with(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let needle = arg_str(it, a, 0)?;
    Ok(JsValue::Bool(s.ends_with(&needle)))
}
fn str_split(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    // No separator → the whole string as a single element (JS).
    if matches!(a.first(), Some(JsValue::Undefined) | None) {
        return Ok(it.new_array(vec![JsValue::str(s)]));
    }
    // Accepts a RegExp or a plain string (treated literally) — delegated to the regex path.
    crate::builtins_regexp::string_split(it, &s, a.first())
}
fn str_replace(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    // Accepts a RegExp or a plain string; a function replacer is supported. First-match
    // (or all matches if the RegExp has the `g` flag), matching JS.
    crate::builtins_regexp::string_replace(it, &s, a, false)
}
fn str_replace_all(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    crate::builtins_regexp::string_replace(it, &s, a, true)
}
fn str_match(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    crate::builtins_regexp::string_match(it, &s, a.first())
}
fn str_match_all(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    crate::builtins_regexp::string_match_all(it, &s, a.first())
}
fn str_search(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    crate::builtins_regexp::string_search(it, &s, a.first())
}
fn str_to_upper(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::str(this_string(it, this)?.to_uppercase()))
}
fn str_to_lower(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::str(this_string(it, this)?.to_lowercase()))
}
fn str_trim(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::str(this_string(it, this)?.trim().to_string()))
}
fn str_trim_start(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::str(
        this_string(it, this)?.trim_start().to_string(),
    ))
}
fn str_trim_end(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::str(this_string(it, this)?.trim_end().to_string()))
}
fn str_repeat(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let s = this_string(it, this)?;
    let n = it.to_number(a.first().unwrap_or(&JsValue::Number(0.0)))?;
    if n < 0.0 || !n.is_finite() {
        return Err(RuntimeError::new_pub(
            ErrorKind::RangeError,
            "Invalid count value",
        ));
    }
    let count = n as usize;
    if s.len().saturating_mul(count) > 64 * 1024 * 1024 {
        return Err(RuntimeError::new_pub(
            ErrorKind::RangeError,
            "repeat result too large",
        ));
    }
    Ok(JsValue::str(s.repeat(count)))
}
fn str_pad_start(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    pad(it, this, a, true)
}
fn str_pad_end(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    pad(it, this, a, false)
}
fn pad(it: &mut Interpreter, this: &JsValue, a: &[JsValue], start: bool) -> R {
    let s = this_string(it, this)?;
    let target = it.to_number(a.first().unwrap_or(&JsValue::Number(0.0)))? as usize;
    let padstr = match a.get(1) {
        Some(JsValue::Undefined) | None => " ".to_string(),
        Some(v) => it.to_string(v)?,
    };
    let cur = s.chars().count();
    if cur >= target || padstr.is_empty() || target > 1024 * 1024 {
        return Ok(JsValue::str(s));
    }
    let need = target - cur;
    let mut fill = String::new();
    while fill.chars().count() < need {
        fill.push_str(&padstr);
    }
    let fill: String = fill.chars().take(need).collect();
    Ok(JsValue::str(if start {
        format!("{}{}", fill, s)
    } else {
        format!("{}{}", s, fill)
    }))
}
fn str_concat(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let mut s = this_string(it, this)?;
    for v in a {
        s.push_str(&it.to_string(v)?);
    }
    Ok(JsValue::str(s))
}

// ─── Number ──────────────────────────────────────────────────────────────────

fn install_number(it: &mut Interpreter) {
    let n = it.native("Number", number_ctor);
    it.set_func_static(&n, "isNaN", it.native("isNaN", number_is_nan));
    it.set_func_static(&n, "isFinite", it.native("isFinite", number_is_finite));
    it.set_func_static(&n, "isInteger", it.native("isInteger", number_is_integer));
    it.set_func_static(
        &n,
        "isSafeInteger",
        it.native("isSafeInteger", number_is_safe_integer),
    );
    it.set_func_static(&n, "parseInt", it.native("parseInt", global_parse_int));
    it.set_func_static(
        &n,
        "parseFloat",
        it.native("parseFloat", global_parse_float),
    );
    it.set_func_static(&n, "MAX_SAFE_INTEGER", JsValue::Number(9007199254740991.0));
    it.set_func_static(&n, "MIN_SAFE_INTEGER", JsValue::Number(-9007199254740991.0));
    it.set_func_static(&n, "MAX_VALUE", JsValue::Number(f64::MAX));
    it.set_func_static(&n, "MIN_VALUE", JsValue::Number(f64::MIN_POSITIVE));
    it.set_func_static(&n, "EPSILON", JsValue::Number(f64::EPSILON));
    it.set_func_static(&n, "POSITIVE_INFINITY", JsValue::Number(f64::INFINITY));
    it.set_func_static(&n, "NEGATIVE_INFINITY", JsValue::Number(f64::NEG_INFINITY));
    it.set_func_static(&n, "NaN", JsValue::Number(f64::NAN));
    it.define_global("Number", n);
}
fn number_ctor(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    match a.first() {
        Some(v) => Ok(JsValue::Number(it.to_number(v)?)),
        None => Ok(JsValue::Number(0.0)),
    }
}
fn number_is_nan(_it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    Ok(JsValue::Bool(
        matches!(a.first(), Some(JsValue::Number(n)) if n.is_nan()),
    ))
}
fn number_is_finite(_it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    Ok(JsValue::Bool(
        matches!(a.first(), Some(JsValue::Number(n)) if n.is_finite()),
    ))
}
fn number_is_integer(_it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    Ok(JsValue::Bool(
        matches!(a.first(), Some(JsValue::Number(n)) if n.is_finite() && mathfn::trunc(*n) == *n),
    ))
}
fn number_is_safe_integer(_it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    // An integer exactly representable as an f64: |n| <= Number.MAX_SAFE_INTEGER (2^53-1).
    Ok(JsValue::Bool(matches!(
        a.first(),
        Some(JsValue::Number(n))
            if n.is_finite() && mathfn::trunc(*n) == *n && n.abs() <= 9_007_199_254_740_991.0
    )))
}

/// Dispatch a `Number.prototype.<method>` access (`toFixed`, `toString(radix)`).
pub(crate) fn number_property(it: &Interpreter, key: &str) -> JsValue {
    let f: Option<crate::interp::NativeFn> = match key {
        "toFixed" => Some(num_to_fixed),
        "toString" => Some(num_to_string),
        "valueOf" => Some(num_value_of),
        "toPrecision" => Some(num_to_fixed),
        _ => None,
    };
    match f {
        Some(nf) => it.native(key, nf),
        None => JsValue::Undefined,
    }
}
fn num_value_of(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::Number(it.to_number(this)?))
}
fn num_to_fixed(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let n = it.to_number(this)?;
    let digits = a
        .first()
        .map(|v| it.to_number(v))
        .transpose()?
        .unwrap_or(0.0) as usize;
    if digits > 100 {
        return Err(RuntimeError::new_pub(
            ErrorKind::RangeError,
            "toFixed digits out of range",
        ));
    }
    Ok(JsValue::str(format!("{:.*}", digits, n)))
}
fn num_to_string(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let n = it.to_number(this)?;
    let radix = a
        .first()
        .map(|v| it.to_number(v))
        .transpose()?
        .unwrap_or(10.0) as u32;
    if radix == 10 || radix == 0 {
        return Ok(JsValue::str(number_to_string(n)));
    }
    if !(2..=36).contains(&radix) {
        return Err(RuntimeError::new_pub(
            ErrorKind::RangeError,
            "radix must be between 2 and 36",
        ));
    }
    Ok(JsValue::str(int_to_radix(n, radix)))
}
fn int_to_radix(n: f64, radix: u32) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if !n.is_finite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let neg = n < 0.0;
    let mut int = mathfn::trunc(mathfn::fabs(n)) as u64;
    if int == 0 {
        return "0".to_string();
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf: Vec<u8> = Vec::new();
    while int > 0 {
        buf.push(digits[(int % radix as u64) as usize]);
        int /= radix as u64;
    }
    if neg {
        buf.push(b'-');
    }
    buf.reverse();
    String::from_utf8(buf).unwrap_or_default()
}

// ─── Error constructors ────────────────────────────────────────────────────────

fn install_errors(it: &mut Interpreter) {
    for kind in [
        ErrorKind::Error,
        ErrorKind::TypeError,
        ErrorKind::RangeError,
        ErrorKind::ReferenceError,
        ErrorKind::SyntaxError,
    ] {
        let name = kind.name();
        let ctor = it.native_error_ctor(kind);
        it.define_global(name, ctor);
    }
}

// ─── Globals ───────────────────────────────────────────────────────────────────

fn install_globals(it: &mut Interpreter) {
    let pi = it.native("parseInt", global_parse_int);
    it.define_global("parseInt", pi);
    let pf = it.native("parseFloat", global_parse_float);
    it.define_global("parseFloat", pf);
    let inan = it.native("isNaN", global_is_nan);
    it.define_global("isNaN", inan);
    let ifin = it.native("isFinite", global_is_finite);
    it.define_global("isFinite", ifin);
    let euc = it.native("encodeURIComponent", global_encode_uri_component);
    it.define_global("encodeURIComponent", euc);
    let duc = it.native("decodeURIComponent", global_decode_uri_component);
    it.define_global("decodeURIComponent", duc);
    let eu = it.native("encodeURI", global_encode_uri);
    it.define_global("encodeURI", eu);
    let du = it.native("decodeURI", global_decode_uri);
    it.define_global("decodeURI", du);
    let bool_ctor = it.native("Boolean", boolean_ctor);
    it.define_global("Boolean", bool_ctor);
    it.define_global("NaN", JsValue::Number(f64::NAN));
    it.define_global("Infinity", JsValue::Number(f64::INFINITY));
    it.define_global("undefined", JsValue::Undefined);
    let gt = it.global_this_value();
    it.define_global("globalThis", gt);
}

fn boolean_ctor(_it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    Ok(JsValue::Bool(to_boolean(
        a.first().unwrap_or(&JsValue::Undefined),
    )))
}
fn global_is_nan(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let n = it.to_number(a.first().unwrap_or(&JsValue::Undefined))?;
    Ok(JsValue::Bool(n.is_nan()))
}
fn global_is_finite(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let n = it.to_number(a.first().unwrap_or(&JsValue::Undefined))?;
    Ok(JsValue::Bool(n.is_finite()))
}
fn global_parse_int(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let s = arg_str(it, a, 0)?;
    let radix = match a.get(1) {
        Some(JsValue::Undefined) | None => 0u32,
        Some(v) => it.to_number(v)? as u32,
    };
    Ok(JsValue::Number(parse_int(&s, radix)))
}
fn global_parse_float(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let s = arg_str(it, a, 0)?;
    Ok(JsValue::Number(parse_float(&s)))
}

// ── URI encode/decode (RFC 3986 percent-encoding over UTF-8 bytes) ─────────────
fn uri_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
        )
}
fn uri_reserved(b: u8) -> bool {
    matches!(
        b,
        b';' | b'/' | b'?' | b':' | b'@' | b'&' | b'=' | b'+' | b'$' | b',' | b'#'
    )
}
fn uri_hex(n: u8) -> u8 {
    let n = n & 0xF;
    if n < 10 {
        b'0' + n
    } else {
        b'A' + n - 10
    }
}
fn uri_hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
/// Percent-encode every byte not in the unreserved set (plus the reserved set
/// when `keep_reserved`, i.e. encodeURI vs encodeURIComponent). Operates on the
/// string's UTF-8 bytes, matching JS semantics for non-ASCII.
fn uri_encode(s: &str, keep_reserved: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if uri_unreserved(b) || (keep_reserved && uri_reserved(b)) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(uri_hex(b >> 4) as char);
            out.push(uri_hex(b & 0xF) as char);
        }
    }
    out
}
/// Decode `%XX` sequences back to bytes, then to UTF-8. `keep_reserved` leaves a
/// reserved-char `%XX` encoded (decodeURI vs decodeURIComponent). `Err` on a
/// malformed `%` sequence or non-UTF-8 result (the caller raises an error — JS
/// throws URIError here; rae_js has no URIError variant so it uses Error).
fn uri_decode(s: &str, keep_reserved: bool) -> Result<String, ()> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(());
            }
            let hi = uri_hex_val(bytes[i + 1]).ok_or(())?;
            let lo = uri_hex_val(bytes[i + 2]).ok_or(())?;
            let byte = (hi << 4) | lo;
            if keep_reserved && uri_reserved(byte) {
                out.push(b'%');
                out.push(bytes[i + 1]);
                out.push(bytes[i + 2]);
            } else {
                out.push(byte);
            }
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    alloc::string::String::from_utf8(out).map_err(|_| ())
}
fn global_encode_uri_component(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let s = arg_str(it, a, 0)?;
    Ok(JsValue::str(uri_encode(&s, false)))
}
fn global_encode_uri(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let s = arg_str(it, a, 0)?;
    Ok(JsValue::str(uri_encode(&s, true)))
}
fn global_decode_uri_component(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let s = arg_str(it, a, 0)?;
    uri_decode(&s, false)
        .map(JsValue::str)
        .map_err(|_| RuntimeError::new_pub(ErrorKind::Error, "URI malformed"))
}
fn global_decode_uri(it: &mut Interpreter, _t: &JsValue, a: &[JsValue]) -> R {
    let s = arg_str(it, a, 0)?;
    uri_decode(&s, true)
        .map(JsValue::str)
        .map_err(|_| RuntimeError::new_pub(ErrorKind::Error, "URI malformed"))
}

fn parse_int(s: &str, mut radix: u32) -> f64 {
    let t = s.trim_start();
    let bytes = t.as_bytes();
    let mut i = 0;
    let mut sign = 1.0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        if bytes[i] == b'-' {
            sign = -1.0;
        }
        i += 1;
    }
    if (radix == 0 || radix == 16)
        && i + 1 < bytes.len()
        && bytes[i] == b'0'
        && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X')
    {
        i += 2;
        radix = 16;
    }
    if radix == 0 {
        radix = 10;
    }
    if !(2..=36).contains(&radix) {
        return f64::NAN;
    }
    let start = i;
    let mut acc: f64 = 0.0;
    while i < bytes.len() {
        let c = bytes[i];
        let digit = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'z' => (c - b'a' + 10) as u32,
            b'A'..=b'Z' => (c - b'A' + 10) as u32,
            _ => break,
        };
        if digit >= radix {
            break;
        }
        acc = acc * radix as f64 + digit as f64;
        i += 1;
    }
    if i == start {
        return f64::NAN;
    }
    sign * acc
}
fn parse_float(s: &str) -> f64 {
    let t = s.trim_start();
    // Take the longest valid float prefix.
    let bytes = t.as_bytes();
    let mut end = 0;
    let mut seen_dot = false;
    let mut seen_e = false;
    let mut seen_digit = false;
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
        end = i;
    }
    if t[i..].starts_with("Infinity") {
        return if t.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    while i < bytes.len() {
        match bytes[i] {
            b'0'..=b'9' => {
                seen_digit = true;
                end = i + 1;
            }
            b'.' if !seen_dot && !seen_e => {
                seen_dot = true;
                end = i + 1;
            }
            b'e' | b'E' if !seen_e && seen_digit => {
                seen_e = true;
                if i + 1 < bytes.len() && (bytes[i + 1] == b'+' || bytes[i + 1] == b'-') {
                    i += 1;
                }
                end = i + 1;
            }
            _ => break,
        }
        i += 1;
    }
    if !seen_digit {
        return f64::NAN;
    }
    t[..end].parse::<f64>().unwrap_or(f64::NAN)
}

// ─── function prototype (call/apply/bind) ──────────────────────────────────────

/// Dispatch `Function.prototype.<method>` (`call`, `apply`, `bind`).
pub(crate) fn function_property(it: &Interpreter, key: &str) -> JsValue {
    let f: Option<crate::interp::NativeFn> = match key {
        "call" => Some(fn_call),
        "apply" => Some(fn_apply),
        "bind" => Some(fn_bind),
        "toString" => Some(fn_to_string),
        _ => None,
    };
    match f {
        Some(nf) => it.native(key, nf),
        None => JsValue::Undefined,
    }
}
fn fn_call(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let new_this = a.first().cloned().unwrap_or(JsValue::Undefined);
    let args: Vec<JsValue> = a.iter().skip(1).cloned().collect();
    it.call_function(this, &new_this, &args)
}
fn fn_apply(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let new_this = a.first().cloned().unwrap_or(JsValue::Undefined);
    let args = match a.get(1) {
        Some(JsValue::Array(arr)) => arr.borrow().items.clone(),
        Some(JsValue::Undefined) | None => Vec::new(),
        Some(other) => it.iterate(other)?,
    };
    it.call_function(this, &new_this, &args)
}
fn fn_bind(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    // A bound function is implemented as a native closure capturing target + this + args.
    // Since NativeFn is a plain fn pointer (no captures), we model bind by storing the
    // binding on a fresh object and dispatching through a thunk — but to keep it simple
    // and dep-free, bind returns the original function with `this` ignored is wrong; so we
    // build a small wrapper object. Documented limitation: bind captures `this` only when
    // the bound fn is called via the returned value's own call. We use a closure object.
    let bound_this = a.first().cloned().unwrap_or(JsValue::Undefined);
    let pre_args: Vec<JsValue> = a.iter().skip(1).cloned().collect();
    Ok(it.make_bound_function(this.clone(), bound_this, pre_args))
}
fn fn_to_string(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    it.to_string(this).map(JsValue::str)
}

// ─── argument helpers ──────────────────────────────────────────────────────────

fn arg_num(it: &mut Interpreter, a: &[JsValue], i: usize) -> Result<f64, RuntimeError> {
    it.to_number(a.get(i).unwrap_or(&JsValue::Undefined))
}
fn arg_str(it: &mut Interpreter, a: &[JsValue], i: usize) -> Result<String, RuntimeError> {
    it.to_string(a.get(i).unwrap_or(&JsValue::Undefined))
}

// to_int32 is re-exported for any future bitwise builtin; suppress unused-import noise.
#[allow(unused_imports)]
use to_int32 as _to_int32;
