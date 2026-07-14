//! Browser event registry + click dispatch — turning a rendered page interactive.
//!
//! LEGACY_GAMING_CONCEPT.md §"3. Web apps via PWA support that actually feels native (renders
//! through AthUI)": a static render is not an *app*. A real page responds to clicks. The
//! engines already ship the parts — `ath_js` runs script + exposes a host-object API,
//! `athweb` lays out the DOM and hit-tests a point to a node id, and [`crate::dom_js`] binds
//! `document.getElementById(id).textContent = …` to the live tree. This module is the join
//! that makes a click *do* something:
//!
//!   1. **registration** — `el.addEventListener('click', fn)` stores the JS callable in a
//!      registry keyed by `(element id, event type)` (and tags the athweb `EventListener`
//!      seam with a callback id so the engine's own node reflects the listener).
//!   2. **dispatch** — a click at a node (by id, or by `(x,y)` via athweb hit-testing) invokes
//!      the registered callbacks in the live `ath_js` interpreter, bubbling up id-bearing
//!      ancestors. Any DOM mutation a handler makes marks the tree dirty → the caller
//!      re-lays-out → the change shows.
//!   3. **safety** — the interpreter's call-depth + step budgets bound every handler, so a
//!      handler that loops or throws cannot hang or crash the browser: it surfaces an error
//!      and the page stays alive. After dispatch the microtask/timer loop is drained, so a
//!      handler that schedules a `Promise`/`setTimeout` mutation also lands.
//!
//! The registry lives at the app layer (not on the `DomNode`) because a re-layout serializes
//! and reparses the DOM, which would not preserve node-attached callbacks; keying by element
//! id is durable across re-layout and is exactly what hit-testing yields.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use ath_js::{Interpreter, JsValue};

/// One registered listener: the JS callable plus the id it was tagged with on the athweb seam.
#[derive(Clone)]
struct Listener {
    callback: JsValue,
    #[allow(dead_code)]
    callback_id: u64,
}

/// The set of event listeners for a page, keyed by `(element id, event type)`. Shared (via
/// `Rc<RefCell<…>>`) between the `addEventListener` native function (which writes it during
/// script execution) and the dispatch entry points (which read it on a click).
#[derive(Default)]
pub struct ListenerRegistry {
    map: BTreeMap<(String, String), Vec<Listener>>,
    next_callback_id: u64,
}

/// Shared handle to the registry.
pub type SharedRegistry = Rc<RefCell<ListenerRegistry>>;

impl ListenerRegistry {
    /// A fresh, empty registry.
    pub fn new() -> SharedRegistry {
        Rc::new(RefCell::new(ListenerRegistry::default()))
    }

    /// Register `callback` for `event_type` on element `id`. Returns the callback id assigned
    /// (also used to tag the athweb `EventListener` seam). De-dups exact-same-callable repeats
    /// the way the DOM does (adding the identical function for the same type is a no-op there);
    /// here we keep it simple and append — repeated distinct closures all fire, which matches
    /// the common `addEventListener('click', () => …)` usage.
    pub fn add(&mut self, id: &str, event_type: &str, callback: JsValue) -> u64 {
        let cid = self.next_callback_id;
        self.next_callback_id = self.next_callback_id.wrapping_add(1);
        self.map
            .entry((id.to_string(), event_type.to_string()))
            .or_default()
            .push(Listener {
                callback,
                callback_id: cid,
            });
        cid
    }

    /// Remove the listener(s) for `(id, event_type)` whose callable is the SAME function value
    /// as `callback` (identity, like the DOM). A no-match is a no-op. Returns how many were
    /// removed.
    pub fn remove(&mut self, id: &str, event_type: &str, callback: &JsValue) -> usize {
        if let Some(v) = self.map.get_mut(&(id.to_string(), event_type.to_string())) {
            let before = v.len();
            v.retain(|l| !same_callable(&l.callback, callback));
            before - v.len()
        } else {
            0
        }
    }

    /// The callables registered for `(id, event_type)`, in registration order. Cloned out so
    /// the borrow on the registry is released before we call into the interpreter (a handler
    /// may itself register more listeners — re-entrancy must not alias the borrow).
    fn callbacks_for(&self, id: &str, event_type: &str) -> Vec<JsValue> {
        self.map
            .get(&(id.to_string(), event_type.to_string()))
            .map(|v| v.iter().map(|l| l.callback.clone()).collect())
            .unwrap_or_default()
    }

    /// Whether any listener is registered for `(id, event_type)` (anywhere — cheap probe).
    pub fn has(&self, id: &str, event_type: &str) -> bool {
        self.map
            .get(&(id.to_string(), event_type.to_string()))
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Total number of registered listeners (across all keys) — for tests/diagnostics.
    pub fn len(&self) -> usize {
        self.map.values().map(|v| v.len()).sum()
    }

    /// Whether the registry holds no listeners.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Function-identity equality (the DOM removes a listener by identity, not structural eq).
/// Two `JsValue::Function`s are the same callable iff they share the same `Rc` allocation.
fn same_callable(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Function(x), JsValue::Function(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

/// The outcome of dispatching one event: how many handlers fired and the first error (if a
/// handler threw). Never an `Err` at the Rust level — a thrown JS exception is reported here,
/// the page stays alive.
#[derive(Debug, Default, Clone)]
pub struct DispatchResult {
    /// Number of JS handlers actually invoked (across the target + bubbled ancestors).
    pub handlers_fired: usize,
    /// The message of the first handler that threw, if any (the rest still ran).
    pub error: Option<String>,
}

/// Invoke every `click` listener registered on `path` (innermost element id first, then
/// id-bearing ancestors — DOM bubbling), in the live interpreter. Each handler is called with
/// `this`/argument `undefined` (a richer `Event` object is a follow-up). A handler that throws
/// is caught: its error is recorded, remaining handlers still run, the browser never panics.
///
/// `path` is innermost-first; within one element, listeners fire in registration order. After
/// all handlers run, the caller drains the event loop (see [`dispatch_click_by_id`]).
fn fire_along_path(
    interp: &mut Interpreter,
    registry: &SharedRegistry,
    event_type: &str,
    path: &[String],
) -> DispatchResult {
    let mut result = DispatchResult::default();
    for id in path {
        // Snapshot the callables for this node BEFORE calling any (re-entrancy safe).
        let callbacks = registry.borrow().callbacks_for(id, event_type);
        for cb in callbacks {
            result.handlers_fired += 1;
            // `this` and args are `undefined` in v1 — enough for `() => mutate()` handlers.
            match interp.call_function(&cb, &JsValue::Undefined, &[]) {
                Ok(_) => {}
                Err(e) => {
                    if result.error.is_none() {
                        result.error = Some(e.message.clone());
                    }
                }
            }
        }
    }
    result
}

/// Dispatch a `click` to the element with `id`, bubbling up its id-bearing ancestors, then
/// drain the microtask/timer loop (so a handler scheduling a `Promise`/`setTimeout` mutation
/// also lands). This is the synthetic, by-id dispatch entry point — usable without pixel
/// coordinates, and the one the host KAT drives.
///
/// `ancestors` is the innermost-first id path (from [`athweb::DomDocument::ancestor_id_path`]).
/// Pass `&[id]` for no-bubble dispatch. Never panics: a thrown handler / a runaway handler is
/// bounded by the interpreter budget and reported in [`DispatchResult::error`].
pub fn dispatch_click_by_id(
    interp: &mut Interpreter,
    registry: &SharedRegistry,
    ancestors: &[String],
) -> DispatchResult {
    let mut result = fire_along_path(interp, registry, "click", ancestors);
    // A handler may have scheduled async work (setTimeout/Promise.then); drain it so the
    // mutation it performs is applied before the caller re-renders. The loop is itself
    // budget-bounded — a runaway timer chain cannot spin forever.
    if let Err(e) = interp.run_event_loop() {
        if result.error.is_none() {
            result.error = Some(e.message.clone());
        }
    }
    result
}
