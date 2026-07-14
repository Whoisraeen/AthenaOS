//! # RaeJs `RegExp` execution — regex that actually MATCHES.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy (criterion #5 — "the web browser is the
//! universal app runtime; PWAs that feel native"): real-world page script uses regular
//! expressions *everywhere* — form/input validation (`/^\d+$/`), URL/route parsing,
//! templating, search-and-replace. The [interpreter](crate::interp) parsed a regex literal
//! `/abc/gi` into an inert object that did nothing, so any interactive page that validates
//! a field or splits a string on a pattern was dead on arrival. This module makes `RegExp`
//! and the regex-taking `String.prototype` methods *execute*, by REUSING the proven,
//! never-panic, ReDoS-safe [`ath_regex`] engine (Thompson NFA / Pike VM) — there is no
//! second regex implementation here, exactly the "harvest a proven engine, don't transplant
//! a fragile one" mindset of `docs/LINUX_DRIVER_STRATEGY.md`.
//!
//! ## Implemented (the deliverable)
//! - **`RegExp`**: a `/pat/flags` literal and `new RegExp(pat, flags)` both construct a
//!   RegExp wrapping a compiled [`ath_regex::Regex`] (compiled at construction; a bad
//!   pattern → `SyntaxError`, never a panic). `lastIndex` is a writable own property.
//! - **`RegExp.prototype`**: `.test(str)`, `.exec(str)` (match array with `.index`/`.input`
//!   + capture groups, advancing `lastIndex` for the `g`/`y` flags, `null` on no match),
//!   `.toString()` → `"/source/flags"`, and the `source`/`flags`/`global`/`ignoreCase`/
//!   `multiline`/`sticky`/`dotAll`/`unicode` accessors.
//! - **`String.prototype`**: `.match`, `.matchAll`, `.replace`, `.replaceAll`, `.split`,
//!   `.search` — each accepting **either** a RegExp **or** a plain string (a string is
//!   treated as a *literal* pattern, exactly like JS: `"a.b.c".split(".")` → 3 parts).
//!
//! ## Flag mapping — honest scope (ath_regex's flavor vs JS's)
//! ath_regex is a deliberately-bounded subset (see its crate docs): a Thompson NFA with no
//! backtracking, so backreferences, lazy quantifiers, lookaround, named groups, `\b`,
//! Unicode property escapes (`\p{…}`), and inline flags (`(?i)`) are **not** part of its
//! syntax. The JS flags map as follows:
//! - `i` (ignoreCase): ath_regex has **no** case-folding mode, so we implement `i`
//!   ourselves by a *correct* source transform — every unescaped ASCII letter `c`
//!   (including inside `[...]` classes) is expanded to the class `[cC]` before compiling.
//!   This is real, not faked, and does not disturb `\d`/`\D`/`.`/anchors. **Limitation
//!   (documented):** only ASCII letters are folded; non-ASCII case (`Σ`/`σ`) is not.
//! - `g` (global): honored — `.match`/`.replace`/`.replaceAll`/`.matchAll`/`.exec` use the
//!   all-matches path (`ath_regex::Regex::find_all`) and advance `lastIndex`.
//! - `s` (dotAll): ath_regex's `.` already excludes only `\n`; full dotAll is **not**
//!   distinguished (best-effort) — recorded on the object but does not change matching.
//! - `m` (multiline): ath_regex `^`/`$` are start/end-of-text only (no per-line) — recorded
//!   but **not** honored; documented, not silently wrong (anchors simply stay text-anchored).
//! - `y` (sticky): treated like `g` for iteration (anchored-at-lastIndex semantics are
//!   approximated via `lastIndex`); `u` (unicode): recorded, no extra unicode mode.
//! - A **function replacer** (`str.replace(re, (m, ...groups) => …)`) IS implemented (it is
//!   tractable here because we drive the match loop ourselves). String replacements support
//!   the `$&`/`$1`..`$9`/`$$` substitution subset that [`ath_regex::Regex::replace_all`]
//!   provides, plus `$&` (whole match) handled here.
//!
//! ## Never-panic / bounded (load-bearing)
//! `#![forbid(unsafe_code)]` (workspace). A malformed pattern → `SyntaxError` (caught from
//! [`ath_regex::Regex::new`]'s `Err`, never `unwrap`). A regex method on a non-string /
//! non-regex receiver → `TypeError`. Pathological input is bounded by ath_regex's own
//! linear-time guarantee plus the interpreter's step budget. No script — valid or hostile —
//! panics the host. Run the FAIL-able KATs with `cargo test -p ath_js`.

use crate::builtins_collections::Internal;
use crate::interp::{ErrorKind, Interpreter, JsValue, RuntimeError};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use ath_regex::Regex;

type R = Result<JsValue, RuntimeError>;

/// The internal state of a `RegExp` instance: its original `source`/`flags` text plus the
/// compiled program. Shared via `Rc` so it is cheap to clone out of the object's borrow.
pub struct RegExpData {
    /// The original pattern text (the `.source` accessor; `(?:)` for an empty literal).
    pub source: String,
    /// The flag string, normalized to its recognized subset, in canonical order.
    pub flags: String,
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub sticky: bool,
    pub unicode: bool,
    /// The compiled engine. Compiled once at construction.
    pub re: Regex,
}

fn syntax_err(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::new_pub(ErrorKind::SyntaxError, msg)
}
fn type_err(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::new_pub(ErrorKind::TypeError, msg)
}

// ─── Flag parsing + the case-insensitivity source transform ─────────────────────

/// Parse a JS flag string into the recognized subset, rejecting unknown/duplicate flags
/// with `SyntaxError` (matching JS). Returns the `(global, i, m, s, y, u)` booleans.
fn parse_flags(flags: &str) -> Result<(bool, bool, bool, bool, bool, bool), RuntimeError> {
    let (mut g, mut i, mut m, mut s, mut y, mut u) = (false, false, false, false, false, false);
    for c in flags.chars() {
        let slot = match c {
            'g' => &mut g,
            'i' => &mut i,
            'm' => &mut m,
            's' => &mut s,
            'y' => &mut y,
            'u' => &mut u,
            other => {
                return Err(syntax_err(format!(
                    "Invalid regular expression flag '{}'",
                    other
                )));
            }
        };
        if *slot {
            return Err(syntax_err(format!(
                "Duplicate regular expression flag '{}'",
                c
            )));
        }
        *slot = true;
    }
    Ok((g, i, m, s, y, u))
}

/// Canonicalize the flag string to JS order (`gimsuy`) over the recognized subset.
fn canonical_flags(g: bool, i: bool, m: bool, s: bool, u: bool, y: bool) -> String {
    let mut out = String::new();
    if g {
        out.push('g');
    }
    if i {
        out.push('i');
    }
    if m {
        out.push('m');
    }
    if s {
        out.push('s');
    }
    if u {
        out.push('u');
    }
    if y {
        out.push('y');
    }
    out
}

/// Rewrite `pattern` so that matching is ASCII-case-insensitive, by expanding every
/// *unescaped* ASCII letter `c` to the class member set `[cC]`, including letters that
/// already live inside a `[...]` class. Metacharacters, escapes (`\d`, `\w`, `\.`), and
/// shorthands are left untouched — this is a correctness-preserving transform, not a
/// reinterpretation of the engine's syntax. Non-ASCII letters are NOT folded (documented).
fn case_fold_source(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_class = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                // Copy the escape and its following char verbatim (no folding inside).
                out.push('\\');
                if i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            '[' if !in_class => {
                in_class = true;
                out.push('[');
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            _ if c.is_ascii_alphabetic() => {
                let lo = c.to_ascii_lowercase();
                let hi = c.to_ascii_uppercase();
                if in_class {
                    // Already inside a class: just add both case variants as members.
                    out.push(lo);
                    out.push(hi);
                } else {
                    // Wrap the single letter in its own class.
                    out.push('[');
                    out.push(lo);
                    out.push(hi);
                    out.push(']');
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    out
}

/// Compile `(source, flags)` into a [`RegExpData`], applying the `i`-flag source transform.
/// A malformed pattern → `SyntaxError` (never a panic).
fn compile(source: &str, flags: &str) -> Result<RegExpData, RuntimeError> {
    let (g, i, m, s, y, u) = parse_flags(flags)?;
    // JS: an empty literal `//` has source `(?:)`. ath_regex accepts an empty pattern.
    let effective = if i {
        case_fold_source(source)
    } else {
        source.to_string()
    };
    let re = Regex::new(&effective)
        .map_err(|e| syntax_err(format!("Invalid regular expression: /{}/: {:?}", source, e)))?;
    Ok(RegExpData {
        source: if source.is_empty() {
            "(?:)".to_string()
        } else {
            source.to_string()
        },
        flags: canonical_flags(g, i, m, s, u, y),
        global: g,
        ignore_case: i,
        multiline: m,
        dot_all: s,
        sticky: y,
        unicode: u,
        re,
    })
}

// ─── Construction + accessors (called from interp.rs) ───────────────────────────

/// Build a `RegExp` object value wrapping a compiled engine. Used by both the regex-literal
/// evaluator and `new RegExp(...)`. A bad pattern throws `SyntaxError`.
pub(crate) fn construct_regexp(it: &Interpreter, source: &str, flags: &str) -> R {
    let data = compile(source, flags)?;
    let obj = it.new_object_with_proto(it.regexp_proto_value());
    // `lastIndex` is a writable own data property (JS spec); start at 0.
    it.set_property_raw(&obj, "lastIndex", JsValue::Number(0.0))?;
    it.set_internal(&obj, Internal::RegExp(Rc::new(data)));
    Ok(obj)
}

/// Resolve a RegExp accessor (`source`/`flags`/`global`/…) from the internal slot. Returns
/// `None` for any other key (so normal prototype-chain lookup proceeds). Called by
/// `Interpreter::get_property`.
pub(crate) fn regexp_accessor(rd: &RegExpData, key: &str) -> Option<JsValue> {
    match key {
        "source" => Some(JsValue::str(rd.source.clone())),
        "flags" => Some(JsValue::str(rd.flags.clone())),
        "global" => Some(JsValue::Bool(rd.global)),
        "ignoreCase" => Some(JsValue::Bool(rd.ignore_case)),
        "multiline" => Some(JsValue::Bool(rd.multiline)),
        "dotAll" => Some(JsValue::Bool(rd.dot_all)),
        "sticky" => Some(JsValue::Bool(rd.sticky)),
        "unicode" => Some(JsValue::Bool(rd.unicode)),
        _ => None,
    }
}

/// Extract the `RegExpData` from a value if it is a RegExp object, else `None`.
fn as_regexp(it: &Interpreter, v: &JsValue) -> Option<Rc<RegExpData>> {
    match it.get_internal(v) {
        Some(Internal::RegExp(rd)) => Some(rd),
        _ => None,
    }
}

/// Coerce a String-method argument into a RegExp: if it already is one, reuse it; if it is a
/// string (or anything else), treat it as a **literal** pattern (escaping every
/// metacharacter), matching JS's `str.split("a.b")` literal semantics. `flags` lets callers
/// force a global match (e.g. `replaceAll`).
fn arg_as_regexp(
    it: &mut Interpreter,
    arg: Option<&JsValue>,
    extra_flags: &str,
) -> Result<Rc<RegExpData>, RuntimeError> {
    match arg {
        Some(v) => {
            if let Some(rd) = as_regexp(it, v) {
                if extra_flags.is_empty() {
                    return Ok(rd);
                }
                // Need a global variant of an existing RegExp: recompile with merged flags.
                let mut merged = rd.flags.clone();
                for f in extra_flags.chars() {
                    if !merged.contains(f) {
                        merged.push(f);
                    }
                }
                let data = compile(&rd.source_for_recompile(), &merged)?;
                Ok(Rc::new(data))
            } else {
                let s = it.to_string(v)?;
                let escaped = escape_literal(&s);
                let data = compile(&escaped, extra_flags)?;
                Ok(Rc::new(data))
            }
        }
        None => {
            // `str.split()` / `str.match()` with no arg: match the empty pattern.
            let data = compile("", extra_flags)?;
            Ok(Rc::new(data))
        }
    }
}

impl RegExpData {
    /// The pattern text to re-feed when recompiling (the `i`-transform is re-derived from
    /// flags by `compile`, so we hand back the ORIGINAL un-transformed source).
    fn source_for_recompile(&self) -> String {
        if self.source == "(?:)" {
            String::new()
        } else {
            self.source.clone()
        }
    }
}

/// Escape a plain string so it matches literally as a regex (every ath_regex metacharacter
/// is backslash-escaped). Used so `String.prototype` methods treat a string argument as a
/// literal pattern, like JS.
fn escape_literal(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(
            c,
            '.' | '*'
                | '+'
                | '?'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '|'
                | '^'
                | '$'
                | '\\'
                | '/'
                | '-'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// ─── Byte/char offset helpers ───────────────────────────────────────────────────

/// ath_regex reports byte offsets; the rest of the interpreter indexes strings by `char`
/// (`.length`, `s[i]`, `.charAt`). Convert a byte offset to a char index for `.index`.
fn byte_to_char_index(text: &str, byte: usize) -> usize {
    text.char_indices().take_while(|(b, _)| *b < byte).count()
}

// ─── Installer ──────────────────────────────────────────────────────────────────

/// Install `RegExp` (the constructor + its prototype) into the interpreter. Mirrors the
/// Map/Set installer in [`crate::builtins_collections`].
pub(crate) fn install(it: &mut Interpreter) {
    // Build the shared prototype carrying the prototype methods.
    let proto = it.new_object();
    let methods: &[(&str, crate::interp::NativeFn)] = &[
        ("test", regexp_test),
        ("exec", regexp_exec),
        ("toString", regexp_to_string),
    ];
    for (name, f) in methods {
        let nf = it.native(name, *f);
        let _ = it.set_property(&proto, name, nf);
    }
    it.set_regexp_proto(proto.clone());

    // The constructor: `new RegExp(pat, flags)` and `RegExp(pat, flags)`.
    let ctor = it.native("RegExp", regexp_ctor);
    if let JsValue::Function(f) = &ctor {
        *f.prototype.borrow_mut() = Some(proto.clone());
    }
    let _ = it.set_property(&proto, "constructor", ctor.clone());
    it.define_global("RegExp", ctor);
}

/// `new RegExp(pattern, flags)` / `RegExp(pattern, flags)`. A RegExp first arg copies its
/// source (+ optional new flags), matching JS.
fn regexp_ctor(it: &mut Interpreter, _this: &JsValue, a: &[JsValue]) -> R {
    let (source, default_flags) = match a.first() {
        Some(v) => {
            if let Some(rd) = as_regexp(it, v) {
                (rd.source_for_recompile(), rd.flags.clone())
            } else if matches!(v, JsValue::Undefined) {
                (String::new(), String::new())
            } else {
                (it.to_string(v)?, String::new())
            }
        }
        None => (String::new(), String::new()),
    };
    let flags = match a.get(1) {
        Some(JsValue::Undefined) | None => default_flags,
        Some(v) => it.to_string(v)?,
    };
    construct_regexp(it, &source, &flags)
}

// ─── RegExp.prototype methods ────────────────────────────────────────────────────

fn this_regexp(it: &Interpreter, this: &JsValue) -> Result<Rc<RegExpData>, RuntimeError> {
    as_regexp(it, this).ok_or_else(|| type_err("Method called on an incompatible receiver"))
}

/// `re.test(str)` → bool. Honors `lastIndex` for global/sticky regexps.
fn regexp_test(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let rd = this_regexp(it, this)?;
    let text = match a.first() {
        Some(v) => it.to_string(v)?,
        None => "undefined".to_string(),
    };
    if rd.global || rd.sticky {
        // Advance from lastIndex; reset to 0 on no match (JS semantics).
        let m = exec_from_last_index(it, this, &rd, &text)?;
        Ok(JsValue::Bool(m.is_some()))
    } else {
        Ok(JsValue::Bool(rd.re.is_match(&text)))
    }
}

/// `re.exec(str)` → match array (`[whole, g1, g2, …]` with `.index`/`.input`) or `null`.
/// Advances `lastIndex` for global/sticky regexps.
fn regexp_exec(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let rd = this_regexp(it, this)?;
    let text = match a.first() {
        Some(v) => it.to_string(v)?,
        None => "undefined".to_string(),
    };
    let m = if rd.global || rd.sticky {
        exec_from_last_index(it, this, &rd, &text)?
    } else {
        exec_once(&rd, &text, 0)
    };
    match m {
        Some((start_byte, caps)) => Ok(build_match_array(it, &text, start_byte, &caps)),
        None => Ok(JsValue::Null),
    }
}

/// `re.toString()` → `"/source/flags"`.
fn regexp_to_string(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let rd = this_regexp(it, this)?;
    Ok(JsValue::str(format!("/{}/{}", rd.source, rd.flags)))
}

/// One match attempt over `text[start_byte..]`, returning the absolute byte start and the
/// captured group substrings (group 0 + each group; `None` = non-participating).
fn exec_once(
    rd: &RegExpData,
    text: &str,
    start_byte: usize,
) -> Option<(usize, Vec<Option<String>>)> {
    if start_byte > text.len() {
        return None;
    }
    let sub = &text[start_byte..];
    let caps = rd.re.captures(sub)?;
    let whole = caps.get(0)?;
    let abs_start = start_byte + whole.start;
    let mut groups: Vec<Option<String>> = Vec::with_capacity(caps.len());
    for g in 0..caps.len() {
        groups.push(
            caps.get(g)
                .map(|m| text[start_byte + m.start..start_byte + m.end].to_string()),
        );
    }
    Some((abs_start, groups))
}

/// Match starting at the object's `lastIndex`, updating `lastIndex` afterward (global/sticky
/// semantics). Returns the match (absolute byte start + group strings) or `None`.
fn exec_from_last_index(
    it: &mut Interpreter,
    this: &JsValue,
    rd: &RegExpData,
    text: &str,
) -> Result<Option<(usize, Vec<Option<String>>)>, RuntimeError> {
    // lastIndex is a char index in JS; convert to a byte start.
    let last_char = match it.get_property(this, "lastIndex")? {
        JsValue::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
        _ => 0,
    };
    let start_byte = char_index_to_byte(text, last_char);
    match exec_once(rd, text, start_byte) {
        Some((abs_start, groups)) => {
            let whole_len = groups[0].as_ref().map(|s| s.len()).unwrap_or(0);
            let end_byte = abs_start + whole_len;
            let next_char = byte_to_char_index(text, end_byte);
            // Guarantee progress on a zero-width match.
            let next = if end_byte <= start_byte {
                next_char + 1
            } else {
                next_char
            };
            it.set_property(this, "lastIndex", JsValue::Number(next as f64))?;
            Ok(Some((abs_start, groups)))
        }
        None => {
            it.set_property(this, "lastIndex", JsValue::Number(0.0))?;
            Ok(None)
        }
    }
}

fn char_index_to_byte(text: &str, char_idx: usize) -> usize {
    match text.char_indices().nth(char_idx) {
        Some((b, _)) => b,
        None => text.len(),
    }
}

/// Build the JS exec-style match array: index 0 is the whole match, indices 1.. are groups,
/// with `.index` (char offset) and `.input` (the searched string) attached.
fn build_match_array(
    it: &Interpreter,
    text: &str,
    start_byte: usize,
    groups: &[Option<String>],
) -> JsValue {
    let items: Vec<JsValue> = groups
        .iter()
        .map(|g| match g {
            Some(s) => JsValue::str(s.clone()),
            None => JsValue::Undefined,
        })
        .collect();
    let arr = it.new_array(items);
    let _ = it.set_property_raw(
        &arr,
        "index",
        JsValue::Number(byte_to_char_index(text, start_byte) as f64),
    );
    let _ = it.set_property_raw(&arr, "input", JsValue::str(text.to_string()));
    arr
}

// ─── String.prototype regex methods (called from builtins.rs) ────────────────────

/// `str.match(re)`: with the `g` flag → array of whole-match strings (or `null` if none);
/// without `g` → an exec-style array (groups + `.index`/`.input`) or `null`.
pub(crate) fn string_match(it: &mut Interpreter, text: &str, arg: Option<&JsValue>) -> R {
    let rd = arg_as_regexp(it, arg, "")?;
    if rd.global {
        let ms = rd.re.find_all(text);
        if ms.is_empty() {
            return Ok(JsValue::Null);
        }
        let items: Vec<JsValue> = ms
            .iter()
            .map(|m| JsValue::str(text[m.start..m.end].to_string()))
            .collect();
        Ok(it.new_array(items))
    } else {
        match exec_once(&rd, text, 0) {
            Some((start_byte, groups)) => Ok(build_match_array(it, text, start_byte, &groups)),
            None => Ok(JsValue::Null),
        }
    }
}

/// `str.matchAll(re)`: an array of exec-style match arrays (one per non-overlapping match),
/// each with groups + `.index`/`.input`. (We return a materialized array; a real iterator is
/// deferred — a `for-of` over an array works identically here.)
pub(crate) fn string_match_all(it: &mut Interpreter, text: &str, arg: Option<&JsValue>) -> R {
    let rd = arg_as_regexp(it, arg, "g")?;
    let ms = rd.re.find_all(text);
    let mut out: Vec<JsValue> = Vec::with_capacity(ms.len());
    for m in ms {
        // Re-run captures at this start to recover group spans for the match array.
        if let Some((start_byte, groups)) = exec_once(&rd, text, m.start) {
            out.push(build_match_array(it, text, start_byte, &groups));
        }
    }
    Ok(it.new_array(out))
}

/// `str.search(re)` → char index of the first match, or `-1`.
pub(crate) fn string_search(it: &mut Interpreter, text: &str, arg: Option<&JsValue>) -> R {
    let rd = arg_as_regexp(it, arg, "")?;
    match rd.re.find(text) {
        Some(m) => Ok(JsValue::Number(byte_to_char_index(text, m.start) as f64)),
        None => Ok(JsValue::Number(-1.0)),
    }
}

/// `str.split(re)` → array of pieces between matches. An empty-pattern split yields the
/// individual characters (JS), and a string argument is treated literally.
pub(crate) fn string_split(it: &mut Interpreter, text: &str, arg: Option<&JsValue>) -> R {
    let rd = arg_as_regexp(it, arg, "")?;
    // Empty pattern → split into chars.
    if rd.source == "(?:)" {
        let parts: Vec<JsValue> = text.chars().map(|c| JsValue::str(c.to_string())).collect();
        return Ok(it.new_array(parts));
    }
    let ms = rd.re.find_all(text);
    let mut parts: Vec<JsValue> = Vec::new();
    let mut last = 0usize;
    for m in &ms {
        // A zero-width match at the same spot would loop; find_all already steps it.
        if m.end == m.start && m.start == last {
            continue;
        }
        parts.push(JsValue::str(text[last..m.start].to_string()));
        last = m.end;
    }
    parts.push(JsValue::str(text[last..].to_string()));
    Ok(it.new_array(parts))
}

/// `str.replace(re, repl)` / `str.replaceAll(re, repl)`. `repl` may be a string (with
/// `$&`/`$1..$9`/`$$` substitution) or a function `(match, ...groups, index, input) => …`.
/// `force_global` makes `replaceAll` replace every match even for a non-`g` pattern.
pub(crate) fn string_replace(
    it: &mut Interpreter,
    text: &str,
    args: &[JsValue],
    force_global: bool,
) -> R {
    let extra = if force_global { "g" } else { "" };
    let rd = arg_as_regexp(it, args.first(), extra)?;
    let repl = args.get(1).cloned().unwrap_or(JsValue::Undefined);
    let global = rd.global || force_global;

    // Collect the matches to replace (one for non-global, all for global).
    let matches: Vec<ath_regex::Match> = if global {
        rd.re.find_all(text)
    } else {
        rd.re.find(text).into_iter().collect()
    };
    if matches.is_empty() {
        return Ok(JsValue::str(text.to_string()));
    }

    let repl_is_fn = repl.type_of() == "function";
    let mut out = String::new();
    let mut last = 0usize;
    for m in &matches {
        out.push_str(&text[last..m.start]);
        if repl_is_fn {
            // Recover the capture groups for this match to pass to the function.
            let groups = match exec_once(&rd, text, m.start) {
                Some((_, g)) => g,
                None => vec![Some(text[m.start..m.end].to_string())],
            };
            let mut call_args: Vec<JsValue> = groups
                .iter()
                .map(|g| match g {
                    Some(s) => JsValue::str(s.clone()),
                    None => JsValue::Undefined,
                })
                .collect();
            call_args.push(JsValue::Number(byte_to_char_index(text, m.start) as f64));
            call_args.push(JsValue::str(text.to_string()));
            let result = it.call_function(&repl, &JsValue::Undefined, &call_args)?;
            out.push_str(&it.to_string(&result)?);
        } else {
            let template = it.to_string(&repl)?;
            let groups = match exec_once(&rd, text, m.start) {
                Some((_, g)) => g,
                None => vec![Some(text[m.start..m.end].to_string())],
            };
            expand_replacement(&template, &groups, &mut out);
        }
        last = m.end;
    }
    out.push_str(&text[last..]);
    Ok(JsValue::str(out))
}

/// Expand a string-replacement template, honoring `$&` (whole match), `$1`..`$9` (groups),
/// and `$$` (literal `$`). An unknown `$x` is emitted literally (JS behavior).
fn expand_replacement(template: &str, groups: &[Option<String>], out: &mut String) {
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            match next {
                '$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                '&' => {
                    if let Some(Some(whole)) = groups.first() {
                        out.push_str(whole);
                    }
                    i += 2;
                    continue;
                }
                d if d.is_ascii_digit() => {
                    let g = d.to_digit(10).unwrap_or(0) as usize;
                    if g >= 1 && g < groups.len() {
                        if let Some(s) = &groups[g] {
                            out.push_str(s);
                        }
                        i += 2;
                        continue;
                    }
                    // `$0` or out-of-range: emit literally.
                    out.push('$');
                    i += 1;
                    continue;
                }
                _ => {
                    out.push('$');
                    i += 1;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
}

// ─── Host KATs (cargo test -p ath_js) — FAIL-able by construction ────────────────

#[cfg(test)]
mod tests {
    use crate::interp::{ErrorKind, Interpreter, JsValue};
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    fn eval(src: &str) -> JsValue {
        let mut it = Interpreter::new();
        match it.eval_str(src) {
            Ok(v) => v,
            Err(e) => panic!("expected `{}` to eval, got error: {}", src, e),
        }
    }
    fn boolean(src: &str) -> bool {
        match eval(src) {
            JsValue::Bool(b) => b,
            other => panic!("expected bool from `{}`, got {:?}", src, other),
        }
    }
    fn string(src: &str) -> String {
        match eval(src) {
            JsValue::String(s) => s.to_string(),
            other => panic!("expected string from `{}`, got {:?}", src, other),
        }
    }
    fn num(src: &str) -> f64 {
        match eval(src) {
            JsValue::Number(n) => n,
            other => panic!("expected number from `{}`, got {:?}", src, other),
        }
    }
    fn err_kind(src: &str) -> ErrorKind {
        let mut it = Interpreter::new();
        let program = crate::parse(src).expect("parse");
        match it.eval_typed(&program) {
            Ok(v) => panic!("expected `{}` to throw, got {:?}", src, v),
            Err(e) => e.kind,
        }
    }

    // ── RegExp.prototype.test ────────────────────────────────────────────────
    #[test]
    fn test_method() {
        assert!(boolean(r"/\d+/.test('abc123')"));
        assert!(!boolean(r"/^\d+$/.test('abc')"));
        // FAIL-ability: if test() ignored the pattern, the negative case would flip.
        assert!(!boolean(r"/^\d+$/.test('12a')"));
        assert!(boolean(r"/^\d+$/.test('12345')"));
    }

    // ── String.prototype.match (single + capture) ────────────────────────────
    #[test]
    fn match_single_and_capture() {
        assert_eq!(string(r"'abc123def'.match(/\d+/)[0]"), "123");
        // Capture groups via exec: /(\d)(\d)/.exec("ab42") → ["42","4","2"].
        assert_eq!(string(r"/(\d)(\d)/.exec('ab42')[1]"), "4");
        assert_eq!(string(r"/(\d)(\d)/.exec('ab42')[2]"), "2");
        assert_eq!(string(r"/(\d)(\d)/.exec('ab42')[0]"), "42");
        // .index is the char offset of the match.
        assert_eq!(num(r"/(\d)(\d)/.exec('ab42').index"), 2.0);
    }

    // ── Global match → all matches ───────────────────────────────────────────
    #[test]
    fn match_global_all() {
        // "a1b2c3".match(/\d/g) → ["1","2","3"].
        assert_eq!(num(r"'a1b2c3'.match(/\d/g).length"), 3.0);
        assert_eq!(string(r"'a1b2c3'.match(/\d/g)[0]"), "1");
        assert_eq!(string(r"'a1b2c3'.match(/\d/g)[2]"), "3");
        // FAIL-ability: a non-global match returns one element, length 1.
        assert_eq!(num(r"'a1b2c3'.match(/\d/).length"), 1.0);
    }

    // ── replace (global) ─────────────────────────────────────────────────────
    #[test]
    fn replace_global() {
        assert_eq!(string(r"'Hello World'.replace(/o/g, '0')"), "Hell0 W0rld");
        // Non-global replace hits only the first.
        assert_eq!(string(r"'Hello World'.replace(/o/, '0')"), "Hell0 World");
        // $1 substitution + $& whole-match.
        assert_eq!(
            string(r"'2026-06'.replace(/(\d+)-(\d+)/, '$2/$1')"),
            "06/2026"
        );
        assert_eq!(string(r"'ab'.replace(/a/, '[$&]')"), "[a]b");
    }

    // ── replaceAll + function replacer ───────────────────────────────────────
    #[test]
    fn replace_all_and_function() {
        assert_eq!(string(r"'a.b.c'.replaceAll(/\./g, '-')"), "a-b-c");
        // Function replacer: uppercase each matched letter.
        assert_eq!(
            string(r"'a1b2'.replace(/[a-z]/g, function(m){return m.toUpperCase();})"),
            "A1B2"
        );
    }

    // ── split ────────────────────────────────────────────────────────────────
    #[test]
    fn split_regex_and_string() {
        // "a,b;c".split(/[,;]/) → ["a","b","c"].
        assert_eq!(num(r"'a,b;c'.split(/[,;]/).length"), 3.0);
        assert_eq!(string(r"'a,b;c'.split(/[,;]/)[1]"), "b");
        // JS treats a STRING split arg literally: "a.b.c".split(".") → 3 parts.
        assert_eq!(num(r"'a.b.c'.split('.').length"), 3.0);
        assert_eq!(string(r"'a.b.c'.split('.')[2]"), "c");
    }

    // ── search ─────────────────────────────────────────────────────────────────
    #[test]
    fn search_index() {
        assert_eq!(num(r"'hello'.search(/l/)"), 2.0);
        assert_eq!(num(r"'hello'.search(/z/)"), -1.0);
    }

    // ── new RegExp(...) + bad pattern → SyntaxError (load-bearing safety) ──────
    #[test]
    fn new_regexp_and_bad_pattern() {
        assert!(boolean(r#"new RegExp('\\d+').test('x9')"#));
        // A bad pattern must THROW (caught in try/catch — never a host panic) and the
        // thrown value identifies it as a SyntaxError. (Engine-thrown errors surface as a
        // "SyntaxError: …" string to `catch`, so we assert the throw + the SyntaxError tag
        // via String(e); the typed-kind assert below is the load-bearing kind check.)
        assert!(boolean(
            r#"
            let threw = false;
            try { new RegExp('('); } catch (e) { threw = String(e).indexOf('SyntaxError') === 0; }
            threw
        "#
        ));
        // The load-bearing safety assert: a bad pattern is a typed SyntaxError, not a panic.
        assert_eq!(err_kind(r#"new RegExp('(')"#), ErrorKind::SyntaxError);
        // Same for a bad regex LITERAL (`/(/` lexes as a complete regex body `(` that
        // fails to compile → SyntaxError at evaluation, not a panic).
        assert_eq!(err_kind(r"/(/.test('x')"), ErrorKind::SyntaxError);
    }

    // ── case-insensitive `i` flag (ASCII fold, documented) ────────────────────
    #[test]
    fn ignore_case_flag() {
        assert!(boolean(r"/abc/i.test('ABC')"));
        assert!(boolean(r"/Hello/i.test('hello world')"));
        // Inside a class too.
        assert!(boolean(r"/[a-z]+/i.test('ABC')"));
        // Without the flag, no match.
        assert!(!boolean(r"/abc/.test('ABC')"));
        // The accessor reports it.
        assert!(boolean(r"/x/i.ignoreCase"));
        assert!(!boolean(r"/x/.ignoreCase"));
    }

    // ── source/flags/global accessors + toString ──────────────────────────────
    #[test]
    fn accessors_and_to_string() {
        assert_eq!(string(r"/ab+c/gi.source"), "ab+c");
        assert_eq!(string(r"/ab+c/gi.flags"), "gi");
        assert!(boolean(r"/x/g.global"));
        assert!(!boolean(r"/x/.global"));
        assert_eq!(string(r"/ab+c/gi.toString()"), "/ab+c/gi");
    }

    // ── lastIndex advances on a global exec ────────────────────────────────────
    #[test]
    fn last_index_advances() {
        assert_eq!(num(r"let r=/\d/g; r.exec('a1b2'); r.lastIndex"), 2.0);
        // A second exec finds the next match.
        assert_eq!(
            string(r"let r=/\d/g; r.exec('a1b2'); r.exec('a1b2')[0]"),
            "2"
        );
    }

    // ── matchAll ────────────────────────────────────────────────────────────
    #[test]
    fn match_all_groups() {
        // Each element is an exec-style array; group 1 of "(\d)" over "a1b2".
        assert_eq!(num(r"[...'a1b2'.matchAll(/(\d)/g)].length"), 2.0);
        assert_eq!(string(r"[...'a1b2'.matchAll(/(\d)/g)][0][1]"), "1");
        assert_eq!(string(r"[...'a1b2'.matchAll(/(\d)/g)][1][1]"), "2");
    }

    // ── never-panic: regex method on undefined receiver → TypeError ───────────
    #[test]
    fn regex_on_bad_receiver_type_error() {
        // `.match` on undefined → TypeError (reading property of undefined).
        assert_eq!(err_kind("undefined.match(/x/)"), ErrorKind::TypeError);
        // RegExp.prototype.test called with a non-regex `this` → TypeError.
        assert_eq!(
            err_kind("let t = /x/.test; t.call({}, 'x')"),
            ErrorKind::TypeError
        );
    }

    // ── never-panic fuzz: regex-using snippets never panic the host ───────────
    #[test]
    fn fuzz_never_panics() {
        let snippets = [
            r"/a*/.test('')",
            r"/(a+)+$/.test('aaaaaaaaaaaaaaaaaaaab')",
            r"''.match(/x/g)",
            r"'...'.split(/\./)",
            r"'abc'.replace(/(?:)/g, '-')",
            r"'x'.replace(/x/, '$1$2$&$$')",
            r"new RegExp('[0-9]{2,4}').test('12345')",
            r"/\w+/i.exec('Hello_World')",
            r"'a'.matchAll(/a/g)",
            r"/z/.exec('')",
        ];
        // 4000-iteration loop: every snippet must terminate without a host panic.
        for _ in 0..400 {
            for s in &snippets {
                let mut it = Interpreter::new();
                // We do not assert the value — only that eval returns (Ok or Err), never
                // panics. A panic here fails the test (the never-panic guarantee).
                let _ = it.eval_str(s);
            }
        }
        // Touch the imports so an accidental unused-import never slips in.
        let _v: Vec<u8> = Vec::new();
    }
}
