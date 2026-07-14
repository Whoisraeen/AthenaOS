//! # RaeJs collection + temporal built-ins: `Map`, `Set`, `Date`, basic `Symbol`.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy (criterion #5 — "the web browser is the
//! universal app runtime; PWAs that feel native"): real-world page script reaches for
//! `Map`/`Set` for keyed/unique collections and `Date` for timestamps on essentially every
//! interactive page; the [interpreter](crate::interp) (which deferred them) cannot run such
//! a page without these globals. This module installs them as native built-ins registered
//! in the global scope — the same mechanism as [`Array`](crate::builtins)/`Object`.
//!
//! ## Implemented (the deliverable)
//! - **`Map`**: `new Map([iterable])`, `set`(chainable)/`get`/`has`/`delete`/`clear`,
//!   `size` (accessor), `keys`/`values`/`entries`/`forEach`, and `for-of` iteration in
//!   insertion order. Key equality is **SameValueZero** (`NaN` equals `NaN`; `±0` are the
//!   same; objects by reference identity).
//! - **`Set`**: `new Set([iterable])`, `add`(chainable)/`has`/`delete`/`clear`, `size`,
//!   `forEach`, `keys`/`values`/`entries`, and `for-of` iteration. SameValueZero
//!   membership (so duplicate `NaN`s collapse).
//! - **`Date`**: `new Date()` / `new Date(ms)` / `new Date(y, m, d, …)`, `Date.now()`, and
//!   the UTC accessors `getTime`/`getFullYear`/`getMonth`/`getDate`/`getDay`/`getHours`/
//!   `getMinutes`/`getSeconds`/`getMilliseconds`, plus `toISOString`/`valueOf`/`toString`.
//!   Civil-date math (`days <-> y/m/d`) is implemented internally — no `libm`, no
//!   `ath_pim` dependency.
//! - **`Symbol`**: `Symbol(desc)` returns a unique, frozen, tagged object (a primitive-ish
//!   distinct value); `Symbol.iterator` is exposed as a well-known string key so for-of
//!   recognition stays uniform. (Minimal by design — see "Deferred".)
//!
//! ## Deferred (documented, honest scope)
//! - **`Date` is UTC only** (local-timezone accessors / `getTimezoneOffset` deferred) and
//!   **"now" is deterministic**: `no_std` has no wall clock, so `Date.now()` and
//!   `new Date()` return a *fixed* build-time epoch ([`DETERMINISTIC_NOW_MS`]) — documented
//!   so tests are reproducible. `Date` string PARSING (`new Date("2020-01-01")`) is
//!   deferred.
//! - **`Symbol`** is minimal: no global symbol registry (`Symbol.for`/`keyFor`), no
//!   description accessor beyond `toString`, and well-known symbols other than
//!   `Symbol.iterator` are not wired.
//! - **`WeakMap`/`WeakSet`** are not provided (no weak references in this value model).
//!
//! ## Never-panic / bounded (load-bearing)
//! `#![forbid(unsafe_code)]` (workspace). Map/Set growth is capped at [`MAX_ENTRIES`];
//! exceeding it throws `RangeError` (never OOM/hang). Key lookup is a **bounded linear
//! scan** over the insertion-ordered store — correctness (SameValueZero, including object
//! identity) over speed for now; the cap keeps the worst case `O(MAX_ENTRIES)`. Calling a
//! method on the wrong receiver → `TypeError`; `get` on a missing key → `undefined`. Run
//! the FAIL-able KATs with `cargo test -p ath_js`.

use crate::interp::{ErrorKind, Interpreter, JsValue, RuntimeError};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

type R = Result<JsValue, RuntimeError>;

/// Maximum number of entries a single `Map`/`Set` may hold. Past this → `RangeError`,
/// bounding both memory and the linear-scan lookup cost.
pub const MAX_ENTRIES: usize = 1_000_000;

/// The fixed "now" returned by `Date.now()` / `new Date()` in this `no_std` build. There is
/// no wall clock, so the value is deterministic for reproducible tests. Chosen as
/// 2020-01-01T00:00:00.000Z (1_577_836_800_000 ms since the Unix epoch).
pub const DETERMINISTIC_NOW_MS: f64 = 1_577_836_800_000.0;

// ─── Internal slot ─────────────────────────────────────────────────────────────

/// The exotic internal state attached to a Map/Set/Date instance object. Shared via `Rc`
/// so the slot is cheap to clone out of the borrow.
#[derive(Clone)]
pub enum Internal {
    Map(Rc<RefCell<MapData>>),
    Set(Rc<RefCell<SetData>>),
    Date(Rc<RefCell<DateData>>),
    /// A `Symbol` — a unique value identified by its `Rc` address; `desc` is the optional
    /// description string for `toString`.
    Symbol(Rc<SymbolData>),
    /// A `RegExp` — a compiled [`ath_regex::Regex`] plus its source/flags. See
    /// [`crate::builtins_regexp`]. Kept in the internal slot so the compiled program and
    /// the parsed flags never leak into `props` (so `for-in` / `JSON.stringify` stay clean,
    /// matching how the spec hides RegExp internal slots).
    RegExp(Rc<crate::builtins_regexp::RegExpData>),
    /// A `Promise` — its settle state + pending reactions. See [`crate::builtins_async`].
    /// Kept in the internal slot so the state never leaks into `props` (matching the spec's
    /// hidden `[[PromiseState]]`/`[[PromiseResult]]` internal slots).
    Promise(Rc<RefCell<crate::builtins_async::PromiseData>>),
    /// A **host object** — an embedder-supplied native object whose property reads/writes
    /// and method calls dispatch into Rust through the [`crate::host::HostObject`] trait.
    /// This is the seam the browser uses to expose a live `document`/element DOM binding to
    /// page script (see [`crate::host`]). Kept in the internal slot for the same reason
    /// Map/Set/RegExp are: the native backing must NOT leak into `props` (so `for-in` /
    /// `JSON.stringify` / `Object.keys` see only the JS-visible surface the host advertises).
    Host(Rc<dyn crate::host::HostObject>),
}

/// An insertion-ordered key/value store with SameValueZero key semantics.
pub struct MapData {
    /// Insertion-ordered `(key, value)` pairs. Lookup is a bounded linear scan (documented
    /// in the module header) so object-identity + NaN keys are correct.
    pub entries: Vec<(JsValue, JsValue)>,
}

/// An insertion-ordered unique-value store with SameValueZero membership.
pub struct SetData {
    pub values: Vec<JsValue>,
}

/// A timestamp as milliseconds since the Unix epoch (UTC). `NaN` is an Invalid Date.
pub struct DateData {
    pub ms: f64,
}

/// A unique symbol's description (for `toString` / debugging).
pub struct SymbolData {
    pub desc: Option<String>,
}

// ─── Cross-module hooks (called from interp.rs) ─────────────────────────────────

/// The `size` of a Map/Set internal (`None` for Date/Symbol). Used by `get_property` to
/// resolve the `size` accessor without invoking a getter.
pub(crate) fn internal_size(internal: &Internal) -> Option<usize> {
    match internal {
        Internal::Map(m) => Some(m.borrow().entries.len()),
        Internal::Set(s) => Some(s.borrow().values.len()),
        _ => None,
    }
}

/// The for-of element sequence of a Map/Set internal: Map → `[k, v]` pairs, Set → values,
/// both in insertion order. `None` for Date/Symbol (not iterable here).
pub(crate) fn iterate_internal(it: &Interpreter, internal: &Internal) -> Option<Vec<JsValue>> {
    match internal {
        Internal::Map(m) => {
            let entries = m.borrow().entries.clone();
            Some(
                entries
                    .into_iter()
                    .map(|(k, v)| it.new_array(vec![k, v]))
                    .collect(),
            )
        }
        Internal::Set(s) => Some(s.borrow().values.clone()),
        _ => None,
    }
}

// ─── installer ───────────────────────────────────────────────────────────────

/// Install `Map`, `Set`, `Date`, and `Symbol` into the interpreter's global scope. Must run
/// after the core builtins (it links instance prototypes to `Object.prototype`).
pub(crate) fn install(it: &mut Interpreter) {
    install_map(it);
    install_set(it);
    install_date(it);
    install_symbol(it);
}

fn type_err(msg: &str) -> RuntimeError {
    RuntimeError::new_pub(ErrorKind::TypeError, msg)
}

fn range_err(msg: &str) -> RuntimeError {
    RuntimeError::new_pub(ErrorKind::RangeError, msg)
}

/// Build a prototype object (linked to `Object.prototype`) carrying the given native
/// methods, returning the prototype value.
fn make_proto(it: &Interpreter, methods: &[(&str, crate::interp::NativeFn)]) -> JsValue {
    let proto = it.new_object_with_proto(it.object_proto_value());
    for (name, f) in methods {
        let _ = it.set_property_raw(&proto, name, it.native(name, *f));
    }
    proto
}

// ═══════════════════════════════════════════════════════════════════════════
//  Map
// ═══════════════════════════════════════════════════════════════════════════

fn install_map(it: &mut Interpreter) {
    let proto = make_proto(
        it,
        &[
            ("set", map_set),
            ("get", map_get),
            ("has", map_has),
            ("delete", map_delete),
            ("clear", map_clear),
            ("forEach", map_for_each),
            ("keys", map_keys),
            ("values", map_values),
            ("entries", map_entries),
        ],
    );
    it.map_proto = proto.clone();
    let ctor = it.native("Map", map_ctor);
    if let JsValue::Function(f) = &ctor {
        *f.prototype.borrow_mut() = Some(proto);
    }
    it.define_global("Map", ctor);
}

/// Resolve `this` to its `MapData`, or a `TypeError` if the receiver is not a Map.
fn this_map(it: &Interpreter, this: &JsValue) -> Result<Rc<RefCell<MapData>>, RuntimeError> {
    match it.get_internal(this) {
        Some(Internal::Map(m)) => Ok(m),
        _ => Err(type_err(
            "Method Map.prototype.<m> called on incompatible receiver",
        )),
    }
}

fn map_ctor(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    // `new Map()` passes the fresh instance as `this`; a bare `Map()` call is a TypeError
    // in real JS — we tolerate it by allocating a fresh instance.
    let instance = match this {
        JsValue::Object(_) => this.clone(),
        _ => it.new_object_with_proto(it.map_proto.clone()),
    };
    let data = Rc::new(RefCell::new(MapData {
        entries: Vec::new(),
    }));
    it.set_internal(&instance, Internal::Map(data.clone()));
    // Seed from an iterable of [k, v] pairs.
    if let Some(arg) = a.first() {
        if !matches!(arg, JsValue::Undefined | JsValue::Null) {
            for pair in it.iterate(arg)? {
                let k = it.get_property(&pair, "0")?;
                let v = it.get_property(&pair, "1")?;
                map_insert(&data, k, v)?;
            }
        }
    }
    Ok(instance)
}

/// SameValueZero insert-or-update into a MapData (bounded).
fn map_insert(
    data: &Rc<RefCell<MapData>>,
    key: JsValue,
    value: JsValue,
) -> Result<(), RuntimeError> {
    let mut d = data.borrow_mut();
    for (k, v) in d.entries.iter_mut() {
        if svz(k, &key) {
            *v = value;
            return Ok(());
        }
    }
    if d.entries.len() >= MAX_ENTRIES {
        return Err(range_err("Map entry budget exceeded"));
    }
    d.entries.push((key, value));
    Ok(())
}

fn map_set(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    let key = a.first().cloned().unwrap_or(JsValue::Undefined);
    let value = a.get(1).cloned().unwrap_or(JsValue::Undefined);
    map_insert(&data, key, value)?;
    Ok(this.clone()) // chainable
}

fn map_get(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    let key = a.first().cloned().unwrap_or(JsValue::Undefined);
    let d = data.borrow();
    for (k, v) in d.entries.iter() {
        if svz(k, &key) {
            return Ok(v.clone());
        }
    }
    Ok(JsValue::Undefined)
}

fn map_has(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    let key = a.first().cloned().unwrap_or(JsValue::Undefined);
    let found = data.borrow().entries.iter().any(|(k, _)| svz(k, &key));
    Ok(JsValue::Bool(found))
}

fn map_delete(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    let key = a.first().cloned().unwrap_or(JsValue::Undefined);
    let mut d = data.borrow_mut();
    if let Some(idx) = d.entries.iter().position(|(k, _)| svz(k, &key)) {
        d.entries.remove(idx);
        Ok(JsValue::Bool(true))
    } else {
        Ok(JsValue::Bool(false))
    }
}

fn map_clear(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    data.borrow_mut().entries.clear();
    Ok(JsValue::Undefined)
}

fn map_for_each(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    let cb = a.first().cloned().unwrap_or(JsValue::Undefined);
    if !matches!(cb, JsValue::Function(_)) {
        return Err(type_err("Map.prototype.forEach callback is not a function"));
    }
    let entries = data.borrow().entries.clone();
    for (k, v) in entries {
        // forEach calls back (value, key, map) — note value before key, per spec.
        it.call_function(&cb, &JsValue::Undefined, &[v, k, this.clone()])?;
    }
    Ok(JsValue::Undefined)
}

fn map_keys(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    let keys: Vec<JsValue> = data
        .borrow()
        .entries
        .iter()
        .map(|(k, _)| k.clone())
        .collect();
    Ok(it.new_array(keys))
}

fn map_values(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    let vals: Vec<JsValue> = data
        .borrow()
        .entries
        .iter()
        .map(|(_, v)| v.clone())
        .collect();
    Ok(it.new_array(vals))
}

fn map_entries(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let data = this_map(it, this)?;
    let entries = data.borrow().entries.clone();
    let pairs: Vec<JsValue> = entries
        .into_iter()
        .map(|(k, v)| it.new_array(vec![k, v]))
        .collect();
    Ok(it.new_array(pairs))
}

// ═══════════════════════════════════════════════════════════════════════════
//  Set
// ═══════════════════════════════════════════════════════════════════════════

fn install_set(it: &mut Interpreter) {
    let proto = make_proto(
        it,
        &[
            ("add", set_add),
            ("has", set_has),
            ("delete", set_delete),
            ("clear", set_clear),
            ("forEach", set_for_each),
            ("keys", set_values),
            ("values", set_values),
            ("entries", set_entries),
        ],
    );
    it.set_proto = proto.clone();
    let ctor = it.native("Set", set_ctor);
    if let JsValue::Function(f) = &ctor {
        *f.prototype.borrow_mut() = Some(proto);
    }
    it.define_global("Set", ctor);
}

fn this_set(it: &Interpreter, this: &JsValue) -> Result<Rc<RefCell<SetData>>, RuntimeError> {
    match it.get_internal(this) {
        Some(Internal::Set(s)) => Ok(s),
        _ => Err(type_err(
            "Method Set.prototype.<m> called on incompatible receiver",
        )),
    }
}

fn set_ctor(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let instance = match this {
        JsValue::Object(_) => this.clone(),
        _ => it.new_object_with_proto(it.set_proto.clone()),
    };
    let data = Rc::new(RefCell::new(SetData { values: Vec::new() }));
    it.set_internal(&instance, Internal::Set(data.clone()));
    if let Some(arg) = a.first() {
        if !matches!(arg, JsValue::Undefined | JsValue::Null) {
            for v in it.iterate(arg)? {
                set_insert(&data, v)?;
            }
        }
    }
    Ok(instance)
}

fn set_insert(data: &Rc<RefCell<SetData>>, value: JsValue) -> Result<(), RuntimeError> {
    let mut d = data.borrow_mut();
    if d.values.iter().any(|v| svz(v, &value)) {
        return Ok(());
    }
    if d.values.len() >= MAX_ENTRIES {
        return Err(range_err("Set entry budget exceeded"));
    }
    d.values.push(value);
    Ok(())
}

fn set_add(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_set(it, this)?;
    let value = a.first().cloned().unwrap_or(JsValue::Undefined);
    set_insert(&data, value)?;
    Ok(this.clone()) // chainable
}

fn set_has(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_set(it, this)?;
    let value = a.first().cloned().unwrap_or(JsValue::Undefined);
    let found = data.borrow().values.iter().any(|v| svz(v, &value));
    Ok(JsValue::Bool(found))
}

fn set_delete(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_set(it, this)?;
    let value = a.first().cloned().unwrap_or(JsValue::Undefined);
    let mut d = data.borrow_mut();
    if let Some(idx) = d.values.iter().position(|v| svz(v, &value)) {
        d.values.remove(idx);
        Ok(JsValue::Bool(true))
    } else {
        Ok(JsValue::Bool(false))
    }
}

fn set_clear(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let data = this_set(it, this)?;
    data.borrow_mut().values.clear();
    Ok(JsValue::Undefined)
}

fn set_for_each(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let data = this_set(it, this)?;
    let cb = a.first().cloned().unwrap_or(JsValue::Undefined);
    if !matches!(cb, JsValue::Function(_)) {
        return Err(type_err("Set.prototype.forEach callback is not a function"));
    }
    let values = data.borrow().values.clone();
    for v in values {
        // Set forEach: (value, value, set) — the "key" is the value itself, per spec.
        it.call_function(&cb, &JsValue::Undefined, &[v.clone(), v, this.clone()])?;
    }
    Ok(JsValue::Undefined)
}

fn set_values(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let data = this_set(it, this)?;
    let vals = data.borrow().values.clone();
    Ok(it.new_array(vals))
}

fn set_entries(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let data = this_set(it, this)?;
    let values = data.borrow().values.clone();
    let pairs: Vec<JsValue> = values
        .into_iter()
        .map(|v| it.new_array(vec![v.clone(), v]))
        .collect();
    Ok(it.new_array(pairs))
}

// ═══════════════════════════════════════════════════════════════════════════
//  SameValueZero
// ═══════════════════════════════════════════════════════════════════════════

/// SameValueZero key/membership equality for Map/Set. Delegated to the interpreter's
/// reference-identity-aware comparison; objects/arrays/functions compare by `Rc` identity,
/// `NaN` equals `NaN`, `±0` are the same.
fn svz(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Number(x), JsValue::Number(y)) => (x.is_nan() && y.is_nan()) || x == y,
        (JsValue::Undefined, JsValue::Undefined) => true,
        (JsValue::Null, JsValue::Null) => true,
        (JsValue::Bool(x), JsValue::Bool(y)) => x == y,
        (JsValue::String(x), JsValue::String(y)) => x == y,
        (JsValue::Object(x), JsValue::Object(y)) => Rc::ptr_eq(x, y),
        (JsValue::Array(x), JsValue::Array(y)) => Rc::ptr_eq(x, y),
        (JsValue::Function(x), JsValue::Function(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Date  (UTC-only, deterministic "now")
// ═══════════════════════════════════════════════════════════════════════════

fn install_date(it: &mut Interpreter) {
    let proto = make_proto(
        it,
        &[
            ("getTime", date_get_time),
            ("valueOf", date_get_time),
            ("getFullYear", date_get_full_year),
            ("getUTCFullYear", date_get_full_year),
            ("getMonth", date_get_month),
            ("getUTCMonth", date_get_month),
            ("getDate", date_get_date),
            ("getUTCDate", date_get_date),
            ("getDay", date_get_day),
            ("getUTCDay", date_get_day),
            ("getHours", date_get_hours),
            ("getUTCHours", date_get_hours),
            ("getMinutes", date_get_minutes),
            ("getUTCMinutes", date_get_minutes),
            ("getSeconds", date_get_seconds),
            ("getUTCSeconds", date_get_seconds),
            ("getMilliseconds", date_get_ms),
            ("getUTCMilliseconds", date_get_ms),
            ("toISOString", date_to_iso),
            ("toJSON", date_to_iso),
            ("toString", date_to_string),
        ],
    );
    it.date_proto = proto.clone();
    let ctor = it.native("Date", date_ctor);
    if let JsValue::Function(f) = &ctor {
        *f.prototype.borrow_mut() = Some(proto);
    }
    it.set_func_static(&ctor, "now", it.native("now", date_now));
    it.set_func_static(&ctor, "UTC", it.native("UTC", date_utc));
    it.define_global("Date", ctor);
}

fn date_now(_it: &mut Interpreter, _this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::Number(DETERMINISTIC_NOW_MS))
}

fn date_utc(it: &mut Interpreter, _this: &JsValue, a: &[JsValue]) -> R {
    let ms = ms_from_components(it, a, 1970.0)?;
    Ok(JsValue::Number(ms))
}

fn date_ctor(it: &mut Interpreter, this: &JsValue, a: &[JsValue]) -> R {
    let instance = match this {
        JsValue::Object(_) => this.clone(),
        _ => it.new_object_with_proto(it.date_proto.clone()),
    };
    let ms = match a.len() {
        0 => DETERMINISTIC_NOW_MS,
        1 => {
            // new Date(ms) — numeric only; string parsing is deferred → NaN (Invalid Date).
            match &a[0] {
                JsValue::Number(n) => *n,
                JsValue::String(_) => f64::NAN, // Date string parsing deferred
                other => it.to_number(other)?,
            }
        }
        _ => ms_from_components(it, a, 1900.0)?,
    };
    it.set_internal(
        &instance,
        Internal::Date(Rc::new(RefCell::new(DateData { ms }))),
    );
    Ok(instance)
}

/// Build epoch-ms from `(year, month, day, hours, minutes, seconds, ms)` components.
/// `year_base` is added to a year < 100 only for the multi-arg constructor (1900) and is
/// 1970 for `Date.UTC` semantics where no rebasing applies — callers pass the base they
/// want; we only rebase two-digit years for the 1900 case.
fn ms_from_components(
    it: &mut Interpreter,
    a: &[JsValue],
    year_base: f64,
) -> Result<f64, RuntimeError> {
    let mut year = it.to_number(a.first().unwrap_or(&JsValue::Number(f64::NAN)))?;
    let month = match a.get(1) {
        Some(v) => it.to_number(v)?,
        None => 0.0,
    };
    let day = match a.get(2) {
        Some(v) => it.to_number(v)?,
        None => 1.0,
    };
    let hours = match a.get(3) {
        Some(v) => it.to_number(v)?,
        None => 0.0,
    };
    let minutes = match a.get(4) {
        Some(v) => it.to_number(v)?,
        None => 0.0,
    };
    let seconds = match a.get(5) {
        Some(v) => it.to_number(v)?,
        None => 0.0,
    };
    let millis = match a.get(6) {
        Some(v) => it.to_number(v)?,
        None => 0.0,
    };
    if [year, month, day, hours, minutes, seconds, millis]
        .iter()
        .any(|n| !n.is_finite())
    {
        return Ok(f64::NAN);
    }
    // Per spec: a two-digit year (0..=99) in the `new Date(y, m, …)` form maps to 1900+y.
    if (year_base - 1900.0).abs() < 0.5 && (0.0..=99.0).contains(&year) {
        year += 1900.0;
    }
    let y = year as i64;
    let mo = month as i64;
    let d = day as i64;
    let days = days_from_civil(y, mo, d);
    let ms = (days as f64) * 86_400_000.0
        + hours * 3_600_000.0
        + minutes * 60_000.0
        + seconds * 1000.0
        + millis;
    Ok(ms)
}

fn this_date(it: &Interpreter, this: &JsValue) -> Result<Rc<RefCell<DateData>>, RuntimeError> {
    match it.get_internal(this) {
        Some(Internal::Date(d)) => Ok(d),
        _ => Err(type_err("this is not a Date object")),
    }
}

fn date_ms(it: &Interpreter, this: &JsValue) -> Result<f64, RuntimeError> {
    Ok(this_date(it, this)?.borrow().ms)
}

fn date_get_time(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    Ok(JsValue::Number(date_ms(it, this)?))
}

fn date_get_full_year(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    Ok(field_or_nan(ms, |c| c.year as f64))
}
fn date_get_month(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    Ok(field_or_nan(ms, |c| (c.month - 1) as f64)) // JS months are 0-based
}
fn date_get_date(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    Ok(field_or_nan(ms, |c| c.day as f64))
}
fn date_get_day(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    Ok(field_or_nan(ms, |c| c.weekday as f64))
}
fn date_get_hours(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    Ok(field_or_nan(ms, |c| c.hours as f64))
}
fn date_get_minutes(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    Ok(field_or_nan(ms, |c| c.minutes as f64))
}
fn date_get_seconds(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    Ok(field_or_nan(ms, |c| c.seconds as f64))
}
fn date_get_ms(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    Ok(field_or_nan(ms, |c| c.millis as f64))
}

fn field_or_nan(ms: f64, f: impl Fn(&Civil) -> f64) -> JsValue {
    if !ms.is_finite() {
        return JsValue::Number(f64::NAN);
    }
    JsValue::Number(f(&civil_from_ms(ms)))
}

fn date_to_iso(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    if !ms.is_finite() {
        return Err(range_err("Invalid time value"));
    }
    let c = civil_from_ms(ms);
    Ok(JsValue::str(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        c.year, c.month, c.day, c.hours, c.minutes, c.seconds, c.millis
    )))
}

fn date_to_string(it: &mut Interpreter, this: &JsValue, _a: &[JsValue]) -> R {
    let ms = date_ms(it, this)?;
    if !ms.is_finite() {
        return Ok(JsValue::str("Invalid Date"));
    }
    // UTC-only build: render the ISO-ish UTC string (local-tz rendering is deferred).
    let c = civil_from_ms(ms);
    Ok(JsValue::str(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        c.year, c.month, c.day, c.hours, c.minutes, c.seconds
    )))
}

// ─── civil-date math (no_std, no libm, no ath_pim) ─────────────────────────────

/// A decomposed UTC calendar instant. `month`/`day` are 1-based; `weekday` 0=Sunday.
struct Civil {
    year: i64,
    month: i64,
    day: i64,
    weekday: i64,
    hours: i64,
    minutes: i64,
    seconds: i64,
    millis: i64,
}

/// Days from the Unix epoch (1970-01-01) for a proleptic-Gregorian `y-m-d` (1-based month/
/// day). Howard Hinnant's `days_from_civil` algorithm — integer-only, no overflow for the
/// realistic year range a script uses.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // Normalize the month into 1..=12 carrying into the year (so `new Date(2020, 13, …)`
    // rolls over like JS).
    let mut year = y;
    let mut month = m;
    // Carry months (handles negatives too) into the year.
    year += month.div_euclid(12);
    month = month.rem_euclid(12); // 0..=11
                                  // Hinnant expects month 1..=12; shift our 0..=11 back to 1..=12.
    let month = month + 1;
    let yy = if month <= 2 { year - 1 } else { year };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = (yy - era * 400) as i64; // [0, 399]
    let mp = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`]: civil `y-m-d` from days since the Unix epoch.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Decompose epoch-ms (UTC) into civil fields, with floor semantics so negative timestamps
/// (pre-1970) still map to correct dates/times.
fn civil_from_ms(ms: f64) -> Civil {
    // floor(ms) to whole milliseconds.
    let total_ms = floor_f64(ms) as i64;
    let day_ms = 86_400_000i64;
    let days = total_ms.div_euclid(day_ms);
    let rem = total_ms.rem_euclid(day_ms); // [0, day_ms)
    let (year, month, day) = civil_from_days(days);
    let hours = rem / 3_600_000;
    let minutes = (rem / 60_000) % 60;
    let seconds = (rem / 1000) % 60;
    let millis = rem % 1000;
    // Weekday: 1970-01-01 was a Thursday (=4). Use Euclidean mod for negative days.
    let weekday = (days + 4).rem_euclid(7);
    Civil {
        year,
        month,
        day,
        weekday,
        hours,
        minutes,
        seconds,
        millis,
    }
}

/// `floor` for f64 without the `std`-only method (mirrors the interpreter's math helpers).
fn floor_f64(x: f64) -> f64 {
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

// ═══════════════════════════════════════════════════════════════════════════
//  Symbol (minimal)
// ═══════════════════════════════════════════════════════════════════════════

fn install_symbol(it: &mut Interpreter) {
    let ctor = it.native("Symbol", symbol_ctor);
    // Symbol.iterator as a well-known string key, so iteration recognition is uniform.
    it.set_func_static(&ctor, "iterator", JsValue::str("Symbol(Symbol.iterator)"));
    it.set_func_static(
        &ctor,
        "asyncIterator",
        JsValue::str("Symbol(Symbol.asyncIterator)"),
    );
    it.define_global("Symbol", ctor);
}

fn symbol_ctor(it: &mut Interpreter, _this: &JsValue, a: &[JsValue]) -> R {
    // `new Symbol()` is a TypeError in real JS; calling `Symbol(desc)` returns a unique
    // value. We model it as a frozen, tagged object whose identity is its `Rc`.
    let desc = match a.first() {
        Some(JsValue::Undefined) | None => None,
        Some(v) => Some(it.to_string(v)?),
    };
    let sym = it.new_object_with_proto(it.object_proto_value());
    it.set_internal(
        &sym,
        Internal::Symbol(Rc::new(SymbolData { desc: desc.clone() })),
    );
    let label = match &desc {
        Some(d) => format!("Symbol({})", d),
        None => "Symbol()".to_string(),
    };
    let _ = it.set_property(
        &sym,
        "description",
        match desc {
            Some(d) => JsValue::str(d),
            None => JsValue::Undefined,
        },
    );
    // A toString so `String(sym)` / template interpolation render sensibly.
    let _ = it.set_property(&sym, "__symbol_label__", JsValue::str(label));
    if let JsValue::Object(o) = &sym {
        o.borrow_mut().frozen = true;
    }
    Ok(sym)
}
