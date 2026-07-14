//! Reactive state management for RaeKit.
//!
//! Provides `State<T>` for single-value reactive state, `Binding<T>` for
//! two-way data flow, `ObservableObject` for multi-field models, and
//! `StateValue` as the type-erased value type for dynamic state.

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── State<T> ─────────────────────────────────────────────────────────────

/// A reactive wrapper around a value. Tracks mutations via a generation
/// counter so the framework knows when to re-render.
pub struct State<T> {
    value: T,
    generation: u64,
}

impl<T> State<T> {
    pub fn new(initial: T) -> Self {
        Self {
            value: initial,
            generation: 0,
        }
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.generation += 1;
        &mut self.value
    }

    pub fn set(&mut self, new_value: T) {
        self.value = new_value;
        self.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn modify(&mut self, f: impl FnOnce(&mut T)) {
        f(&mut self.value);
        self.generation += 1;
    }

    pub fn is_dirty_since(&self, gen: u64) -> bool {
        self.generation > gen
    }
}

impl<T: Clone> State<T> {
    pub fn cloned(&self) -> T {
        self.value.clone()
    }

    pub fn binding(&mut self) -> Binding<T> {
        let ptr = &mut self.value as *mut T;
        let gen = &mut self.generation as *mut u64;
        Binding {
            value_ptr: ptr,
            generation_ptr: gen,
        }
    }
}

impl<T: Copy> State<T> {
    pub fn value(&self) -> T {
        self.value
    }
}

impl<T: PartialEq> State<T> {
    pub fn set_if_changed(&mut self, new_value: T) {
        if self.value != new_value {
            self.value = new_value;
            self.generation += 1;
        }
    }
}

// ── Binding<T> ───────────────────────────────────────────────────────────

/// Two-way data binding. Holds raw pointers into the parent `State<T>` so
/// that reads and writes flow through to the source of truth.
///
/// # Safety
/// The parent `State<T>` must outlive the `Binding`. Bindings are intended
/// to be short-lived — created at render time and consumed before the next
/// state mutation.
pub struct Binding<T> {
    value_ptr: *mut T,
    generation_ptr: *mut u64,
}

impl<T> Binding<T> {
    pub fn get(&self) -> &T {
        // SAFETY: caller guarantees the parent State is alive
        unsafe { &*self.value_ptr }
    }

    pub fn set(&self, new_value: T) {
        unsafe {
            *self.value_ptr = new_value;
            *self.generation_ptr += 1;
        }
    }
}

impl<T: Copy> Binding<T> {
    pub fn value(&self) -> T {
        unsafe { *self.value_ptr }
    }
}

impl<T: Clone> Binding<T> {
    pub fn cloned(&self) -> T {
        unsafe { (*self.value_ptr).clone() }
    }
}

// ── StateValue ───────────────────────────────────────────────────────────

/// Type-erased value for dynamic / heterogeneous state stores.
#[derive(Debug, Clone, PartialEq)]
pub enum StateValue {
    Bool(bool),
    Int(i64),
    Float(Float64),
    Text(String),
    List(Vec<StateValue>),
    Map(BTreeMap<String, StateValue>),
}

/// Wrapper for f64 that implements Eq and Ord via bit-pattern comparison.
/// NaN == NaN is true here — acceptable for UI state comparison.
#[derive(Debug, Clone, Copy)]
pub struct Float64(pub f64);

impl PartialEq for Float64 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Float64 {}

impl PartialOrd for Float64 {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Float64 {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.to_bits().cmp(&other.0.to_bits())
    }
}

impl StateValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            StateValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            StateValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            StateValue::Float(v) => Some(v.0),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            StateValue::Text(v) => Some(v.as_str()),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[StateValue]> {
        match self {
            StateValue::List(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, StateValue>> {
        match self {
            StateValue::Map(v) => Some(v),
            _ => None,
        }
    }
}

// ── ObservableObject ─────────────────────────────────────────────────────

/// A key-value store with field-level change tracking. Models complex
/// application state (e.g. a settings screen with many fields).
pub struct ObservableObject {
    fields: BTreeMap<String, StateValue>,
    generation: u64,
    field_generations: BTreeMap<String, u64>,
}

impl ObservableObject {
    pub fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
            generation: 0,
            field_generations: BTreeMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&StateValue> {
        self.fields.get(key)
    }

    pub fn set(&mut self, key: &str, value: StateValue) {
        self.generation += 1;
        self.field_generations
            .insert(String::from(key), self.generation);
        self.fields.insert(String::from(key), value);
    }

    pub fn remove(&mut self, key: &str) -> Option<StateValue> {
        self.generation += 1;
        self.field_generations.remove(key);
        self.fields.remove(key)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn field_generation(&self, key: &str) -> u64 {
        self.field_generations.get(key).copied().unwrap_or(0)
    }

    pub fn is_field_dirty_since(&self, key: &str, since: u64) -> bool {
        self.field_generation(key) > since
    }

    pub fn is_dirty_since(&self, since: u64) -> bool {
        self.generation > since
    }

    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.fields.keys()
    }

    // ── Typed convenience getters ────────────────────────────────────

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(StateValue::as_bool)
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(StateValue::as_int)
    }

    pub fn get_float(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(StateValue::as_float)
    }

    pub fn get_text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(StateValue::as_text)
    }

    // ── Typed convenience setters ────────────────────────────────────

    pub fn set_bool(&mut self, key: &str, v: bool) {
        self.set(key, StateValue::Bool(v));
    }

    pub fn set_int(&mut self, key: &str, v: i64) {
        self.set(key, StateValue::Int(v));
    }

    pub fn set_float(&mut self, key: &str, v: f64) {
        self.set(key, StateValue::Float(Float64(v)));
    }

    pub fn set_text(&mut self, key: &str, v: &str) {
        self.set(key, StateValue::Text(String::from(v)));
    }
}

// ── EnvironmentValue ─────────────────────────────────────────────────────

/// Read-only values injected by the framework into the view hierarchy.
/// Analogous to SwiftUI's `@Environment`.
pub struct Environment {
    values: BTreeMap<String, StateValue>,
}

impl Environment {
    pub fn new() -> Self {
        Self {
            values: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: &str, value: StateValue) {
        self.values.insert(String::from(key), value);
    }

    pub fn get(&self, key: &str) -> Option<&StateValue> {
        self.values.get(key)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(StateValue::as_bool)
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(StateValue::as_int)
    }

    pub fn get_text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(StateValue::as_text)
    }
}
