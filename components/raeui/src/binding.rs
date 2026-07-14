//! RaeUI Reactive Data Binding
//!
//! Provides Observable values, computed/derived values, and a BindingContext
//! that wires observables to widget properties. Supports one-way and two-way
//! bindings with batch update coalescing.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

// ── Observable Value ────────────────────────────────────────────────────

static NEXT_OBSERVABLE_ID: AtomicU32 = AtomicU32::new(1);

fn alloc_observable_id() -> u32 {
    NEXT_OBSERVABLE_ID.fetch_add(1, Ordering::Relaxed)
}

/// A single bindable property. When the value changes, all bound widgets
/// are notified via the BindingContext.
#[derive(Clone, Debug)]
pub struct Observable {
    pub id: u32,
    value: ObservableValue,
    generation: u64,
}

#[derive(Clone, Debug)]
pub enum ObservableValue {
    Float(f32),
    Int(i64),
    Bool(bool),
    Text(alloc::string::String),
}

impl ObservableValue {
    pub fn as_f32(&self) -> f32 {
        match self {
            ObservableValue::Float(v) => *v,
            ObservableValue::Int(v) => *v as f32,
            ObservableValue::Bool(v) => {
                if *v {
                    1.0
                } else {
                    0.0
                }
            }
            ObservableValue::Text(_) => 0.0,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            ObservableValue::Bool(v) => *v,
            ObservableValue::Int(v) => *v != 0,
            ObservableValue::Float(v) => *v != 0.0,
            ObservableValue::Text(s) => !s.is_empty(),
        }
    }

    pub fn as_text(&self) -> &str {
        match self {
            ObservableValue::Text(s) => s.as_str(),
            _ => "",
        }
    }
}

impl Observable {
    pub fn new_float(value: f32) -> Self {
        Self {
            id: alloc_observable_id(),
            value: ObservableValue::Float(value),
            generation: 0,
        }
    }

    pub fn new_int(value: i64) -> Self {
        Self {
            id: alloc_observable_id(),
            value: ObservableValue::Int(value),
            generation: 0,
        }
    }

    pub fn new_bool(value: bool) -> Self {
        Self {
            id: alloc_observable_id(),
            value: ObservableValue::Bool(value),
            generation: 0,
        }
    }

    pub fn new_text(value: &str) -> Self {
        Self {
            id: alloc_observable_id(),
            value: ObservableValue::Text(alloc::string::String::from(value)),
            generation: 0,
        }
    }

    pub fn value(&self) -> &ObservableValue {
        &self.value
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn set_float(&mut self, v: f32) {
        self.value = ObservableValue::Float(v);
        self.generation += 1;
    }

    pub fn set_int(&mut self, v: i64) {
        self.value = ObservableValue::Int(v);
        self.generation += 1;
    }

    pub fn set_bool(&mut self, v: bool) {
        self.value = ObservableValue::Bool(v);
        self.generation += 1;
    }

    pub fn set_text(&mut self, v: &str) {
        self.value = ObservableValue::Text(alloc::string::String::from(v));
        self.generation += 1;
    }
}

// ── Bound Widget Property ───────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundProperty {
    X,
    Y,
    Width,
    Height,
    Opacity,
    Visible,
    Text,
    Value,
    Checked,
    Enabled,
    Progress,
}

// ── Binding Direction ───────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingDirection {
    OneWay,
    TwoWay,
}

// ── Binding Entry ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct BindingEntry {
    observable_id: u32,
    widget_id: u32,
    property: BoundProperty,
    direction: BindingDirection,
    last_generation: u64,
}

// ── Computed Value ──────────────────────────────────────────────────────

/// A value derived from one or more observables. Re-evaluated when any
/// dependency changes.
pub struct ComputedValue {
    pub id: u32,
    dependencies: Vec<u32>,
    compute_fn: fn(&[&ObservableValue]) -> ObservableValue,
    cached: Option<ObservableValue>,
    cached_generation: u64,
}

impl ComputedValue {
    pub fn new(
        dependencies: Vec<u32>,
        compute: fn(&[&ObservableValue]) -> ObservableValue,
    ) -> Self {
        Self {
            id: alloc_observable_id(),
            dependencies,
            compute_fn: compute,
            cached: None,
            cached_generation: 0,
        }
    }

    pub fn value(&self) -> Option<&ObservableValue> {
        self.cached.as_ref()
    }
}

// ── Property Update (output) ────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct BindingUpdate {
    pub widget_id: u32,
    pub property: BoundProperty,
    pub value: ObservableValue,
}

// ── Binding Context ─────────────────────────────────────────────────────

pub struct BindingContext {
    observables: BTreeMap<u32, Observable>,
    computed: Vec<ComputedValue>,
    bindings: Vec<BindingEntry>,
    dirty: Vec<u32>,
    batch_depth: u32,
}

impl BindingContext {
    pub fn new() -> Self {
        Self {
            observables: BTreeMap::new(),
            computed: Vec::new(),
            bindings: Vec::new(),
            dirty: Vec::new(),
            batch_depth: 0,
        }
    }

    /// Register an observable in the context.
    pub fn register(&mut self, observable: Observable) -> u32 {
        let id = observable.id;
        self.observables.insert(id, observable);
        id
    }

    /// Register a computed value.
    pub fn register_computed(&mut self, computed: ComputedValue) -> u32 {
        let id = computed.id;
        self.computed.push(computed);
        id
    }

    /// Get an observable by ID.
    pub fn get(&self, id: u32) -> Option<&Observable> {
        self.observables.get(&id)
    }

    /// Set a float value and mark dirty.
    pub fn set_float(&mut self, id: u32, value: f32) {
        if let Some(obs) = self.observables.get_mut(&id) {
            obs.set_float(value);
            self.mark_dirty(id);
        }
    }

    /// Set an int value and mark dirty.
    pub fn set_int(&mut self, id: u32, value: i64) {
        if let Some(obs) = self.observables.get_mut(&id) {
            obs.set_int(value);
            self.mark_dirty(id);
        }
    }

    /// Set a bool value and mark dirty.
    pub fn set_bool(&mut self, id: u32, value: bool) {
        if let Some(obs) = self.observables.get_mut(&id) {
            obs.set_bool(value);
            self.mark_dirty(id);
        }
    }

    /// Set a text value and mark dirty.
    pub fn set_text(&mut self, id: u32, value: &str) {
        if let Some(obs) = self.observables.get_mut(&id) {
            obs.set_text(value);
            self.mark_dirty(id);
        }
    }

    /// Bind an observable to a widget property (one-way: observable → widget).
    pub fn bind(&mut self, observable_id: u32, widget_id: u32, property: BoundProperty) {
        let gen = self
            .observables
            .get(&observable_id)
            .map(|o| o.generation)
            .unwrap_or(0);
        self.bindings.push(BindingEntry {
            observable_id,
            widget_id,
            property,
            direction: BindingDirection::OneWay,
            last_generation: gen,
        });
        self.mark_dirty(observable_id);
    }

    /// Two-way bind: observable ↔ widget. Widget changes propagate back.
    pub fn two_way_bind(&mut self, observable_id: u32, widget_id: u32, property: BoundProperty) {
        let gen = self
            .observables
            .get(&observable_id)
            .map(|o| o.generation)
            .unwrap_or(0);
        self.bindings.push(BindingEntry {
            observable_id,
            widget_id,
            property,
            direction: BindingDirection::TwoWay,
            last_generation: gen,
        });
        self.mark_dirty(observable_id);
    }

    /// Unbind all bindings for a widget.
    pub fn unbind_widget(&mut self, widget_id: u32) {
        self.bindings.retain(|b| b.widget_id != widget_id);
    }

    /// Unbind a specific observable from a widget.
    pub fn unbind(&mut self, observable_id: u32, widget_id: u32) {
        self.bindings
            .retain(|b| !(b.observable_id == observable_id && b.widget_id == widget_id));
    }

    /// Start a batch update — dirty notifications are coalesced until end_batch.
    pub fn begin_batch(&mut self) {
        self.batch_depth += 1;
    }

    /// End a batch update.
    pub fn end_batch(&mut self) {
        if self.batch_depth > 0 {
            self.batch_depth -= 1;
        }
    }

    /// Process all dirty observables and return the widget updates to apply.
    pub fn flush(&mut self) -> Vec<BindingUpdate> {
        if self.batch_depth > 0 {
            return Vec::new();
        }

        // Recompute dirty computed values
        self.recompute_derived();

        let mut updates = Vec::new();
        let dirty: Vec<u32> = self.dirty.drain(..).collect();

        for obs_id in &dirty {
            if let Some(obs) = self.observables.get(obs_id) {
                let value = obs.value.clone();
                let gen = obs.generation;

                for binding in &mut self.bindings {
                    if binding.observable_id == *obs_id && binding.last_generation != gen {
                        binding.last_generation = gen;
                        updates.push(BindingUpdate {
                            widget_id: binding.widget_id,
                            property: binding.property,
                            value: value.clone(),
                        });
                    }
                }
            }
        }

        updates
    }

    /// Handle a widget value change (for two-way bindings). Call this when
    /// a widget's value changes from user interaction.
    pub fn widget_changed(
        &mut self,
        widget_id: u32,
        property: BoundProperty,
        value: ObservableValue,
    ) {
        let two_way_obs: Vec<u32> = self
            .bindings
            .iter()
            .filter(|b| {
                b.widget_id == widget_id
                    && b.property == property
                    && b.direction == BindingDirection::TwoWay
            })
            .map(|b| b.observable_id)
            .collect();

        for obs_id in two_way_obs {
            if let Some(obs) = self.observables.get_mut(&obs_id) {
                obs.value = value.clone();
                obs.generation += 1;
                self.mark_dirty(obs_id);
            }
        }
    }

    fn mark_dirty(&mut self, id: u32) {
        if !self.dirty.contains(&id) {
            self.dirty.push(id);
        }
        // Also mark any computed values that depend on this observable
        let dependent_computed: Vec<u32> = self
            .computed
            .iter()
            .filter(|c| c.dependencies.contains(&id))
            .map(|c| c.id)
            .collect();
        for cid in dependent_computed {
            if !self.dirty.contains(&cid) {
                self.dirty.push(cid);
            }
        }
    }

    fn recompute_derived(&mut self) {
        for computed in &mut self.computed {
            let needs_update = computed
                .dependencies
                .iter()
                .any(|dep_id| self.dirty.contains(dep_id));
            if !needs_update {
                continue;
            }
            let dep_values: Vec<ObservableValue> = computed
                .dependencies
                .iter()
                .filter_map(|id| self.observables.get(id).map(|o| o.value.clone()))
                .collect();
            let refs: Vec<&ObservableValue> = dep_values.iter().collect();
            let result = (computed.compute_fn)(&refs);
            computed.cached = Some(result.clone());
            computed.cached_generation += 1;

            // Store the computed result as an observable too so bindings can reference it
            let obs = Observable {
                id: computed.id,
                value: result,
                generation: computed.cached_generation,
            };
            self.observables.insert(computed.id, obs);
        }
    }
}
