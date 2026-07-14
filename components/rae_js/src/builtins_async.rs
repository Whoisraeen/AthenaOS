//! # RaeJs async runtime: `Promise` + a microtask/macrotask EVENT LOOP.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy (criterion #5 — "the web browser is the
//! universal app runtime; PWAs that feel native"): real web JavaScript is **asynchronous** —
//! `fetch().then(...)`, `setTimeout`, event handlers, and `await` all rest on the
//! Promise/microtask machinery and the host event loop. The [interpreter](crate::interp)
//! (commits 4a992b0 / 15d25a7) ran only synchronous code and explicitly *deferred* async,
//! so a page that did anything time-based or promise-based could not run. This module adds
//! the runtime async substrate: `Promise` (states + chaining + the statics), a
//! `queueMicrotask` global, `setTimeout`/`setInterval`/`clearTimeout` over a deterministic
//! **virtual clock**, and the event loop that drains them in spec phase order (all
//! microtasks, then the earliest macrotask, repeat).
//!
//! ## What executes (this slice)
//! - **`Promise`**: `new Promise((resolve, reject) => {...})` (executor runs synchronously;
//!   `resolve`/`reject` are native callbacks that settle the promise and schedule its
//!   reactions as microtasks). `.then(onF, onR)` / `.catch(onR)` / `.finally(onFin)`
//!   returning a NEW chained promise with proper propagation: a handler that returns a value
//!   fulfills the chained promise; returning a promise adopts its eventual state; throwing
//!   rejects it. Statics: `Promise.resolve(v)` (passthrough if `v` is already a promise),
//!   `Promise.reject(r)`, `Promise.all`, `Promise.allSettled`, `Promise.race`,
//!   `Promise.any`.
//! - **Microtask queue**: promise reactions + `queueMicrotask(fn)`.
//! - **Macrotask queue + virtual time**: `setTimeout(fn, ms[, ...args])`,
//!   `setInterval(fn, ms)` (re-armed up to a bounded count), `clearTimeout`/`clearInterval`.
//!   There is **no real clock** in `no_std`; due-times are points on a monotonic
//!   [`EventLoop::virtual_now`] that only advances when the loop runs the earliest macrotask.
//! - **The event loop** ([`run_event_loop`]): drain ALL microtasks, then run the single
//!   earliest-due macrotask (advancing virtual time to its due-time), then drain microtasks
//!   again — repeat until both queues are empty OR the total task budget
//!   ([`EventLoop::MAX_TASKS`]) is hit. [`crate::Interpreter::eval_str`] auto-drains, so a
//!   test asserts the full async order from one call.
//!
//! ## Deferred (documented, honest scope)
//! **`async`/`await` SYNTAX** (function suspension) is **not** implemented. The
//! [parser](crate::parser) recognizes `async`/`await` as *shapes* ([`crate::Function::is_async`]
//! is set; `await x` parses as a unary-ish expression) but an `async function` here runs its
//! body **synchronously** and `await` is **not** a suspension point — so an `async`/`await`
//! program does not yet get true async semantics. Generators (`function*`/`yield`) are also
//! deferred. Use explicit `Promise`/`.then` for real async ordering until the suspension
//! slice lands. `Promise.prototype` is not exposed as a user-subclassable constructor target
//! beyond the standard methods.
//!
//! ## Never-panic / never-hang (load-bearing)
//! `#![forbid(unsafe_code)]` (workspace). The loop is bounded twice over: the existing
//! interpreter step budget bounds any single callback, and [`EventLoop::MAX_TASKS`] bounds
//! the *total* number of tasks the loop will run — so a `setInterval` that never clears, a
//! `.then` that reschedules itself, or `setTimeout(function f(){setTimeout(f,0)},0)` all hit
//! the budget and the loop **terminates** instead of hanging the host. A throw inside a
//! callback rejects the relevant promise (or, for a bare `setTimeout` callback, is recorded
//! and discarded) — it never panics the host and never aborts the loop. Unhandled rejections
//! are tracked in a list (a flag, not a panic). Run the FAIL-able KATs with
//! `cargo test -p rae_js`.

use crate::interp::{ErrorKind, Interpreter, JsValue, RuntimeError};
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

type R = Result<JsValue, RuntimeError>;

fn type_err(msg: &str) -> RuntimeError {
    RuntimeError::new_pub(ErrorKind::TypeError, msg)
}

// ═══════════════════════════════════════════════════════════════════════════
//  Promise state
// ═══════════════════════════════════════════════════════════════════════════

/// A promise's settle state, mirroring the spec's `[[PromiseState]]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

/// The hidden internal state of a `Promise` object (`[[PromiseState]]` / `[[PromiseResult]]`
/// / `[[PromiseFulfillReactions]]` etc.), held in the object's [`Internal`] slot so it never
/// leaks into enumerable props.
///
/// [`Internal`]: crate::builtins_collections::Internal
pub struct PromiseData {
    pub state: PromiseState,
    /// The fulfillment value or rejection reason once settled (`Undefined` while pending).
    pub value: JsValue,
    /// Reactions registered (via `.then` or an internal subscriber) while still pending;
    /// flushed to microtasks on settle. Each is a host callback invoked with the final state
    /// + value.
    pub reactions: Vec<Subscriber>,
    /// `true` once a handler has been attached (for unhandled-rejection tracking). A rejected
    /// promise with no handler at loop end is reported.
    pub handled: bool,
}

/// A host-side reaction to a promise settling: invoked (as a microtask) with the settled
/// state and value. This unifies JS `.then` handlers AND internal reactions (adoption,
/// `.finally`, the `Promise.all`/`race`/… combinators) under one closure type, so none of
/// them need a bare-`fn` native trampoline or a side registry — they just capture their
/// `Rc`s. Boxed; runs exactly once.
pub type Subscriber = Box<dyn FnOnce(&mut Interpreter, PromiseState, JsValue)>;

// ═══════════════════════════════════════════════════════════════════════════
//  The event loop
// ═══════════════════════════════════════════════════════════════════════════

/// A queued unit of work to run on the interpreter. Boxed so a reaction can capture the
/// promise/handler `Rc`s it needs. The closure handles its own JS errors internally (it
/// rejects the relevant promise rather than returning `Err`), so the loop never aborts on a
/// JS throw.
type Task = Box<dyn FnOnce(&mut Interpreter)>;

/// A scheduled macrotask (a `setTimeout`/`setInterval` callback) with its virtual due-time.
struct Macrotask {
    /// Virtual-clock time (ms) at which this is due.
    due: f64,
    /// Monotonic insertion sequence — ties at the same `due` run in scheduling order (FIFO),
    /// matching browser timer ordering for equal delays.
    seq: u64,
    /// The timer id (for `clearTimeout`/`clearInterval`); `0` is never a live id.
    id: u64,
    /// The callback value.
    callback: JsValue,
    /// Extra arguments forwarded to the callback (`setTimeout(fn, ms, a, b)`).
    args: Vec<JsValue>,
    /// `Some(period)` for `setInterval` — re-armed after firing; `None` for `setTimeout`.
    interval: Option<f64>,
}

/// The interpreter's event-loop state: the two queues, the virtual clock, the task budget,
/// and the unhandled-rejection tracker.
pub struct EventLoop {
    /// FIFO microtask queue (promise reactions + `queueMicrotask`).
    microtasks: Vec<Task>,
    /// The macrotask (timer) heap, kept as a Vec and scanned for the earliest due-time
    /// (bounded by [`MAX_TIMERS`], so the linear scan is cheap and never unbounded).
    macrotasks: Vec<Macrotask>,
    /// Monotonic virtual clock (ms). Advances only when the loop runs the earliest macrotask.
    virtual_now: f64,
    /// Next timer id to hand out (ids start at 1; `0` means "no timer").
    next_timer_id: u64,
    /// Monotonic scheduling counter for stable FIFO ordering at equal due-times.
    seq: u64,
    /// Total tasks (micro + macro) the loop has run this drain — bounded by [`MAX_TASKS`].
    tasks_run: u64,
    /// Number of times a `setInterval` callback has fired in aggregate — bounded so a
    /// never-cleared interval cannot run forever even below the task budget.
    interval_fires: u64,
    /// Rejection reasons that settled with no handler attached, observed at loop end. Tracked
    /// (not a panic) so an embedder can surface "Uncaught (in promise)".
    pub unhandled_rejections: Vec<JsValue>,
    /// Weak handles to every promise created. Used ONLY at loop end to iteratively break
    /// reaction chains (see [`teardown_chains`]): a long promise chain links each
    /// `PromiseData` to the next via a captured `Rc` inside a reaction closure, so a naive
    /// recursive `Drop` of the chain would overflow the host stack. Clearing reactions
    /// flatly first turns the drop into a shallow, non-recursive one. Weak so the registry
    /// never keeps a promise alive.
    all_promises: Vec<alloc::rc::Weak<RefCell<PromiseData>>>,
}

impl EventLoop {
    /// Maximum total tasks (microtasks + macrotasks) a single [`run_event_loop`] drain will
    /// run before terminating. Guarantees a self-rescheduling promise/timer cannot hang the
    /// host: a never-cleared `setInterval`, a `.then` that reschedules itself, or
    /// `setTimeout(function f(){setTimeout(f,0)},0)` all hit this and the loop returns. The
    /// front-popping microtask drain dismantles a long promise chain incrementally (one link
    /// freed per task), so reaching this bound never overflows the host stack on `Drop`.
    /// Generous enough for any realistic page-init burst (real pages schedule thousands, not
    /// tens of thousands, of tasks during load).
    pub const MAX_TASKS: u64 = 20_000;

    /// Maximum number of *live* scheduled timers at once. Past this, scheduling a new timer
    /// is dropped (returns id 0). Bounds the macrotask Vec / its scan.
    pub const MAX_TIMERS: usize = 100_000;

    /// Maximum aggregate `setInterval` firings per drain (a never-cleared interval is bounded
    /// independently of [`MAX_TASKS`] so it shares the budget fairly with other work).
    pub const MAX_INTERVAL_FIRES: u64 = 100_000;

    pub fn new() -> Self {
        EventLoop {
            microtasks: Vec::new(),
            macrotasks: Vec::new(),
            virtual_now: 0.0,
            next_timer_id: 1,
            seq: 0,
            tasks_run: 0,
            interval_fires: 0,
            unhandled_rejections: Vec::new(),
            all_promises: Vec::new(),
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.wrapping_add(1).max(1);
        id
    }
}

impl Default for EventLoop {
    fn default() -> Self {
        EventLoop::new()
    }
}

/// Enqueue a microtask (used by promise reactions and `queueMicrotask`).
pub(crate) fn enqueue_microtask(it: &mut Interpreter, task: Task) {
    // Bound the queue defensively; the task budget in the loop is the real guard.
    if it.loop_state.microtasks.len() < EventLoop::MAX_TASKS as usize {
        it.loop_state.microtasks.push(task);
    }
}

/// Drain the event loop. See the module header for the phase order + budget semantics.
pub fn run_event_loop(it: &mut Interpreter) -> Result<(), RuntimeError> {
    it.loop_state.tasks_run = 0;
    it.loop_state.interval_fires = 0;
    loop {
        // Phase 1: drain ALL microtasks (each may enqueue more — bounded by MAX_TASKS). We pop
        // from the FRONT and let each task drop immediately after running, so a long promise
        // chain is dismantled incrementally (one link freed per task) rather than accumulating
        // a deep `Rc` chain that would overflow the host stack on a single recursive `Drop`.
        loop {
            if it.loop_state.microtasks.is_empty() {
                break;
            }
            if it.loop_state.tasks_run >= EventLoop::MAX_TASKS {
                teardown_chains(it); // budget hit → terminate (never hang / never overflow drop)
                return Ok(());
            }
            let task = it.loop_state.microtasks.remove(0);
            it.loop_state.tasks_run += 1;
            task(it);
        }
        // Phase 2: run the single earliest-due macrotask (advancing virtual time).
        if it.loop_state.macrotasks.is_empty() {
            break;
        }
        if it.loop_state.tasks_run >= EventLoop::MAX_TASKS {
            teardown_chains(it);
            return Ok(());
        }
        // Find the earliest (due, seq).
        let mut best = 0usize;
        for i in 1..it.loop_state.macrotasks.len() {
            let a = &it.loop_state.macrotasks[i];
            let b = &it.loop_state.macrotasks[best];
            if a.due < b.due || (a.due == b.due && a.seq < b.seq) {
                best = i;
            }
        }
        let task = it.loop_state.macrotasks.remove(best);
        // Advance the virtual clock to the task's due-time (never backwards).
        if task.due > it.loop_state.virtual_now {
            it.loop_state.virtual_now = task.due;
        }
        it.loop_state.tasks_run += 1;
        // Re-arm an interval BEFORE firing (so a clear inside the callback wins), bounded.
        if let Some(period) = task.interval {
            if it.loop_state.interval_fires < EventLoop::MAX_INTERVAL_FIRES
                && it.loop_state.macrotasks.len() < EventLoop::MAX_TIMERS
            {
                it.loop_state.interval_fires += 1;
                let next_due = it.loop_state.virtual_now + period.max(0.0);
                let seq = it.loop_state.seq;
                it.loop_state.seq += 1;
                it.loop_state.macrotasks.push(Macrotask {
                    due: next_due,
                    seq,
                    id: task.id,
                    callback: task.callback.clone(),
                    args: task.args.clone(),
                    interval: Some(period),
                });
            }
        }
        // Fire the callback. A throw rejects nothing (a bare timer callback) — record &
        // continue so other tasks still run.
        let _ = it.call_function(&task.callback, &JsValue::Undefined, &task.args);
    }
    teardown_chains(it);
    Ok(())
}

/// Break every live promise's reaction list iteratively at loop end.
///
/// A long promise chain (`a.then(b).then(c)…`, or a self-rescheduling `.then`) links each
/// `PromiseData` to the next through an `Rc` captured inside a reaction closure stored in the
/// previous promise's `reactions`. If left intact, the eventual recursive `Drop` (when the
/// `Interpreter` or the root promise is dropped) recurses once per link and **overflows the
/// host stack** — turning "never-hang" into a crash. We sever every link in a flat pass first,
/// so the subsequent drop is shallow and non-recursive. Pending reactions at loop end never
/// run anyway (the loop is quiescent or budget-exhausted), so this changes no observable
/// behavior. Bounded: one pass over the registry; dead weak entries are skipped.
pub(crate) fn teardown_chains(it: &mut Interpreter) {
    let registry = core::mem::take(&mut it.loop_state.all_promises);
    // PASS 1: move every reactions Vec into one flat buffer WITHOUT dropping any closure yet.
    // Afterwards no `PromiseData.reactions` references another promise — the chain is flat.
    let mut harvested: Vec<Subscriber> = Vec::new();
    for weak in &registry {
        if let Some(data) = weak.upgrade() {
            let mut reactions = core::mem::take(&mut data.borrow_mut().reactions);
            harvested.append(&mut reactions);
        }
    }
    // PASS 2: drop the closures flatly. Each may hold a promise `Rc`, but those promises'
    // reactions are already empty, so dropping recurses at most one level — never the chain.
    drop(harvested);
    it.loop_state.microtasks.clear();
    it.loop_state.macrotasks.clear();
}

// ═══════════════════════════════════════════════════════════════════════════
//  Promise allocation / settle
// ═══════════════════════════════════════════════════════════════════════════

/// Allocate a fresh pending promise object linked to `Promise.prototype`, registering it for
/// end-of-loop chain teardown (so long chains never overflow the host stack on drop).
pub(crate) fn new_promise(it: &mut Interpreter) -> (JsValue, Rc<RefCell<PromiseData>>) {
    let obj = it.new_object_with_proto(it.promise_proto.clone());
    let data = Rc::new(RefCell::new(PromiseData {
        state: PromiseState::Pending,
        value: JsValue::Undefined,
        reactions: Vec::new(),
        handled: false,
    }));
    it.set_internal(
        &obj,
        crate::builtins_collections::Internal::Promise(data.clone()),
    );
    register_promise(it, &data);
    (obj, data)
}

/// Record a weak handle to a promise for [`teardown_chains`]. Bounded; compacts dead entries
/// before growing past the task budget.
fn register_promise(it: &mut Interpreter, data: &Rc<RefCell<PromiseData>>) {
    let reg = &mut it.loop_state.all_promises;
    if reg.len() >= EventLoop::MAX_TASKS as usize {
        reg.retain(|w| w.strong_count() > 0);
    }
    if reg.len() < EventLoop::MAX_TASKS as usize {
        reg.push(Rc::downgrade(data));
    }
}

/// Read the `PromiseData` of a value, if it is a promise.
pub(crate) fn as_promise(it: &Interpreter, v: &JsValue) -> Option<Rc<RefCell<PromiseData>>> {
    match it.get_internal(v) {
        Some(crate::builtins_collections::Internal::Promise(d)) => Some(d),
        _ => None,
    }
}

/// Resolve a promise with `value`. If `value` is itself a thenable/promise, the receiver
/// **adopts** its state (asynchronously). Settling an already-settled promise is a no-op.
pub(crate) fn resolve_promise(
    it: &mut Interpreter,
    data: &Rc<RefCell<PromiseData>>,
    value: JsValue,
) {
    if data.borrow().state != PromiseState::Pending {
        return;
    }
    // Adoption: if value is a promise, chain onto it instead of fulfilling with it.
    if let Some(inner) = as_promise(it, &value) {
        if Rc::ptr_eq(&inner, data) {
            // Resolving a promise with itself → reject with TypeError (spec).
            let reason = it.make_error(ErrorKind::TypeError, "Chaining cycle detected for promise");
            reject_promise(it, data, reason);
            return;
        }
        let dep = data.clone();
        // When `inner` settles, settle `dep` the same way (fulfill→resolve, reject→reject).
        subscribe(
            it,
            &inner,
            Box::new(move |it, state, v| match state {
                PromiseState::Fulfilled => resolve_promise(it, &dep, v),
                PromiseState::Rejected => reject_promise(it, &dep, v),
                PromiseState::Pending => {}
            }),
        );
        return;
    }
    settle(it, data, PromiseState::Fulfilled, value);
}

/// Reject a promise with `reason` and schedule its reactions.
pub(crate) fn reject_promise(
    it: &mut Interpreter,
    data: &Rc<RefCell<PromiseData>>,
    reason: JsValue,
) {
    let had_handler = data.borrow().handled;
    if !had_handler && data.borrow().state == PromiseState::Pending {
        // No handler attached yet → tentatively unhandled. A later `.then`/`.catch` removes
        // the record (see `perform_then`). The list is advisory, never a panic.
        it.loop_state.unhandled_rejections.push(reason.clone());
    }
    settle(it, data, PromiseState::Rejected, reason);
}

/// Settle a (still-pending) promise to a concrete state + value and schedule its reactions as
/// microtasks. A no-op if already settled.
fn settle(
    it: &mut Interpreter,
    data: &Rc<RefCell<PromiseData>>,
    state: PromiseState,
    value: JsValue,
) {
    let reactions = {
        let mut d = data.borrow_mut();
        if d.state != PromiseState::Pending {
            return;
        }
        d.state = state;
        d.value = value;
        core::mem::take(&mut d.reactions)
    };
    for r in reactions {
        schedule_subscriber(it, state, data.borrow().value.clone(), r);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Reaction scheduling (the core of .then chaining)
// ═══════════════════════════════════════════════════════════════════════════

/// Enqueue a settled subscriber as a microtask carrying the final state + value.
fn schedule_subscriber(it: &mut Interpreter, state: PromiseState, value: JsValue, sub: Subscriber) {
    enqueue_microtask(
        it,
        Box::new(move |it: &mut Interpreter| sub(it, state, value)),
    );
}

/// Attach a host subscriber to a promise: if pending, queue it; if already settled, schedule
/// it as a microtask immediately. Marks the promise handled (clears any tentative
/// unhandled-rejection record).
fn subscribe(it: &mut Interpreter, data: &Rc<RefCell<PromiseData>>, sub: Subscriber) {
    let state = data.borrow().state;
    // A subscriber counts as a handler → clear any tentative unhandled-rejection record.
    if state == PromiseState::Rejected || data.borrow().state == PromiseState::Pending {
        let reason = data.borrow().value.clone();
        data.borrow_mut().handled = true;
        if state == PromiseState::Rejected {
            if let Some(pos) = it
                .loop_state
                .unhandled_rejections
                .iter()
                .rposition(|v| it.strict_eq(v, &reason))
            {
                it.loop_state.unhandled_rejections.remove(pos);
            }
        }
    }
    match state {
        PromiseState::Pending => data.borrow_mut().reactions.push(sub),
        _ => {
            let value = data.borrow().value.clone();
            schedule_subscriber(it, state, value, sub);
        }
    }
}

/// The core `then`: register a subscriber that runs the JS handler for the matching branch
/// and settles the returned (chained) promise with the outcome — a value fulfills it, a
/// returned promise is adopted (via `resolve_promise`), a throw rejects it. A missing handler
/// passes the settlement through unchanged.
pub(crate) fn perform_then(
    it: &mut Interpreter,
    data: &Rc<RefCell<PromiseData>>,
    on_fulfilled: Option<JsValue>,
    on_rejected: Option<JsValue>,
) -> JsValue {
    let (dependent, dep_data) = new_promise(it);
    subscribe(
        it,
        data,
        Box::new(move |it, state, value| {
            let handler = match state {
                PromiseState::Fulfilled => on_fulfilled,
                PromiseState::Rejected => on_rejected,
                PromiseState::Pending => return,
            };
            match handler {
                Some(h) => match it.call_function(&h, &JsValue::Undefined, &[value]) {
                    Ok(out) => resolve_promise(it, &dep_data, out),
                    Err(e) => reject_promise(it, &dep_data, e.value),
                },
                None => match state {
                    PromiseState::Fulfilled => resolve_promise(it, &dep_data, value),
                    PromiseState::Rejected => reject_promise(it, &dep_data, value),
                    PromiseState::Pending => {}
                },
            }
        }),
    );
    dependent
}

// ═══════════════════════════════════════════════════════════════════════════
//  Installer
// ═══════════════════════════════════════════════════════════════════════════

/// Install `Promise`, `queueMicrotask`, `setTimeout`/`setInterval`/`clearTimeout`/
/// `clearInterval` into the global scope. Must run after the core builtins (links the promise
/// prototype to `Object.prototype`).
pub(crate) fn install(it: &mut Interpreter) {
    // Promise.prototype with then/catch/finally.
    let proto = it.new_object_with_proto(it.object_proto_value());
    let _ = it.set_property(&proto, "then", it.native("then", promise_then));
    let _ = it.set_property(&proto, "catch", it.native("catch", promise_catch));
    let _ = it.set_property(&proto, "finally", it.native("finally", promise_finally));
    it.promise_proto = proto.clone();

    let ctor = it.native("Promise", promise_ctor);
    if let JsValue::Function(f) = &ctor {
        *f.prototype.borrow_mut() = Some(proto);
    }
    it.set_func_static(
        &ctor,
        "resolve",
        it.native("resolve", promise_resolve_static),
    );
    it.set_func_static(&ctor, "reject", it.native("reject", promise_reject_static));
    it.set_func_static(&ctor, "all", it.native("all", promise_all));
    it.set_func_static(
        &ctor,
        "allSettled",
        it.native("allSettled", promise_all_settled),
    );
    it.set_func_static(&ctor, "race", it.native("race", promise_race));
    it.set_func_static(&ctor, "any", it.native("any", promise_any));
    it.define_global("Promise", ctor);

    it.define_global(
        "queueMicrotask",
        it.native("queueMicrotask", queue_microtask),
    );
    it.define_global("setTimeout", it.native("setTimeout", set_timeout));
    it.define_global("setInterval", it.native("setInterval", set_interval));
    it.define_global("clearTimeout", it.native("clearTimeout", clear_timer));
    it.define_global("clearInterval", it.native("clearInterval", clear_timer));
}

// ─── Promise constructor + methods ─────────────────────────────────────────────

fn promise_ctor(it: &mut Interpreter, this: &JsValue, args: &[JsValue]) -> R {
    let executor = args.first().cloned().unwrap_or(JsValue::Undefined);
    if !matches!(executor, JsValue::Function(_)) {
        return Err(type_err("Promise resolver is not a function"));
    }
    // `new Promise(...)` passes the fresh instance as `this`; tolerate a bare call.
    let (promise, data) = match this {
        JsValue::Object(o) if o.borrow().internal.is_none() => {
            // Stamp the freshly-`new`-ed instance as a promise.
            let data = Rc::new(RefCell::new(PromiseData {
                state: PromiseState::Pending,
                value: JsValue::Undefined,
                reactions: Vec::new(),
                handled: false,
            }));
            it.set_internal(
                this,
                crate::builtins_collections::Internal::Promise(data.clone()),
            );
            register_promise(it, &data);
            (this.clone(), data)
        }
        _ => new_promise(it),
    };
    // Build the JS-callable resolve/reject functions passed to the executor. These MUST be
    // real `JsValue::Function`s (user code calls them, typically as bare `resolve(v)` where
    // `this` is `undefined`), so we BIND `this` to the promise object itself; the native
    // trampoline then recovers the `PromiseData` from its `this`. Internal reactions instead
    // use host closures via `subscribe`.
    let resolve_native = it.native("resolve", resolver_trampoline);
    let resolve_fn = it.make_bound_function(resolve_native, promise.clone(), Vec::new());
    let reject_native = it.native("reject", rejecter_trampoline);
    let reject_fn = it.make_bound_function(reject_native, promise.clone(), Vec::new());
    // Run the executor synchronously; a throw rejects the promise.
    if let Err(e) = it.call_function(&executor, &JsValue::Undefined, &[resolve_fn, reject_fn]) {
        reject_promise(it, &data, e.value);
    }
    Ok(promise)
}

/// The native body of the `resolve` function handed to a Promise executor: `this` is bound to
/// the promise object (via `make_bound_function`), so recover its `PromiseData` and resolve it.
fn resolver_trampoline(it: &mut Interpreter, this: &JsValue, args: &[JsValue]) -> R {
    if let Some(d) = as_promise(it, this) {
        let v = args.first().cloned().unwrap_or(JsValue::Undefined);
        resolve_promise(it, &d, v);
    }
    Ok(JsValue::Undefined)
}

/// The native body of the `reject` function handed to a Promise executor (`this` is the bound
/// promise — see [`resolver_trampoline`]).
fn rejecter_trampoline(it: &mut Interpreter, this: &JsValue, args: &[JsValue]) -> R {
    if let Some(d) = as_promise(it, this) {
        let v = args.first().cloned().unwrap_or(JsValue::Undefined);
        reject_promise(it, &d, v);
    }
    Ok(JsValue::Undefined)
}

/// Validate a handler argument: callable → `Some`, anything else → `None` (spec: non-callable
/// handlers are ignored / pass through).
fn handler_arg(v: Option<&JsValue>) -> Option<JsValue> {
    match v {
        Some(f @ JsValue::Function(_)) => Some(f.clone()),
        _ => None,
    }
}

fn this_promise(
    it: &Interpreter,
    this: &JsValue,
) -> Result<Rc<RefCell<PromiseData>>, RuntimeError> {
    as_promise(it, this).ok_or_else(|| type_err("Promise.prototype method called on non-Promise"))
}

fn promise_then(it: &mut Interpreter, this: &JsValue, args: &[JsValue]) -> R {
    let data = this_promise(it, this)?;
    let on_f = handler_arg(args.first());
    let on_r = handler_arg(args.get(1));
    Ok(perform_then(it, &data, on_f, on_r))
}

fn promise_catch(it: &mut Interpreter, this: &JsValue, args: &[JsValue]) -> R {
    let data = this_promise(it, this)?;
    let on_r = handler_arg(args.first());
    Ok(perform_then(it, &data, None, on_r))
}

/// `.finally(fn)`: runs `fn` regardless of outcome, then propagates the original
/// settlement unchanged (a `finally` callback's return value is ignored; a throw in it
/// rejects the chain). Implemented with native pass-through wrappers around `fn`.
fn promise_finally(it: &mut Interpreter, this: &JsValue, args: &[JsValue]) -> R {
    let data = this_promise(it, this)?;
    let cb = handler_arg(args.first());
    let Some(cb) = cb else {
        // Non-callable: behaves like `.then()` with no handlers.
        return Ok(perform_then(it, &data, None, None));
    };
    // Build the chained promise; on each settlement run `cb()` (a throw in it rejects the
    // chain), then propagate the ORIGINAL settlement unchanged.
    let (dependent, dep_data) = new_promise(it);
    subscribe(
        it,
        &data,
        Box::new(
            move |it, state, value| match it.call_function(&cb, &JsValue::Undefined, &[]) {
                Err(e) => reject_promise(it, &dep_data, e.value),
                Ok(_) => match state {
                    PromiseState::Fulfilled => resolve_promise(it, &dep_data, value),
                    PromiseState::Rejected => reject_promise(it, &dep_data, value),
                    PromiseState::Pending => {}
                },
            },
        ),
    );
    Ok(dependent)
}

// ─── Promise statics ───────────────────────────────────────────────────────────

fn promise_resolve_static(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    let v = args.first().cloned().unwrap_or(JsValue::Undefined);
    // Passthrough if already a promise.
    if as_promise(it, &v).is_some() {
        return Ok(v);
    }
    let (p, data) = new_promise(it);
    resolve_promise(it, &data, v);
    Ok(p)
}

fn promise_reject_static(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    let r = args.first().cloned().unwrap_or(JsValue::Undefined);
    let (p, data) = new_promise(it);
    reject_promise(it, &data, r);
    Ok(p)
}

/// Shared aggregator state for `Promise.all`/`allSettled`/`race`/`any`.
struct Aggregate {
    /// Slots for results (all/allSettled), pre-sized to the input length.
    results: Vec<JsValue>,
    /// How many inputs are still pending.
    remaining: usize,
    /// The output promise to settle.
    output: Rc<RefCell<PromiseData>>,
    /// Errors collected for `Promise.any` (all rejected → AggregateError-ish).
    errors: Vec<JsValue>,
    /// Whether the output has already settled (so we ignore later resolutions).
    done: bool,
}

/// The kind of combinator, controlling how each input settlement updates the aggregate.
#[derive(Clone, Copy)]
enum Combinator {
    All,
    AllSettled,
    Race,
    Any,
}

fn promise_all(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    run_combinator(it, args, Combinator::All)
}
fn promise_all_settled(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    run_combinator(it, args, Combinator::AllSettled)
}
fn promise_race(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    run_combinator(it, args, Combinator::Race)
}
fn promise_any(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    run_combinator(it, args, Combinator::Any)
}

fn run_combinator(it: &mut Interpreter, args: &[JsValue], kind: Combinator) -> R {
    let iterable = args.first().cloned().unwrap_or(JsValue::Undefined);
    let inputs = it.iterate(&iterable)?;
    let (output_p, output_data) = new_promise(it);
    let n = inputs.len();

    // Empty-input fast paths (spec-defined).
    if n == 0 {
        match kind {
            Combinator::All | Combinator::AllSettled => {
                let empty = it.new_array(Vec::new());
                resolve_promise(it, &output_data, empty);
            }
            Combinator::Race => { /* forever pending */ }
            Combinator::Any => {
                let err = it.make_error(ErrorKind::Error, "All promises were rejected");
                reject_promise(it, &output_data, err);
            }
        }
        return Ok(output_p);
    }

    let agg = Rc::new(RefCell::new(Aggregate {
        results: vec![JsValue::Undefined; n],
        remaining: n,
        output: output_data,
        errors: vec![JsValue::Undefined; n],
        done: false,
    }));

    for (i, input) in inputs.into_iter().enumerate() {
        // Coerce each input to a promise (Promise.resolve semantics).
        let p_data = match as_promise(it, &input) {
            Some(d) => d,
            None => {
                let (_p, d) = new_promise(it);
                resolve_promise(it, &d, input);
                d
            }
        };
        let agg_f = agg.clone();
        subscribe(
            it,
            &p_data,
            Box::new(move |it, state, value| {
                combinator_step(it, &agg_f, i, kind, state, value);
            }),
        );
    }
    Ok(output_p)
}

/// Funnel one input's settlement into the aggregate, settling the output promise when the
/// combinator's completion condition is met. Pure host closure — no registry/trampoline.
fn combinator_step(
    it: &mut Interpreter,
    agg: &Rc<RefCell<Aggregate>>,
    index: usize,
    kind: Combinator,
    state: PromiseState,
    value: JsValue,
) {
    let fulfill = state == PromiseState::Fulfilled;

    // Decide the action while holding only short borrows; perform settle after dropping them.
    enum Action {
        None,
        Resolve(JsValue),
        Reject(JsValue),
    }
    let action = {
        let mut a = agg.borrow_mut();
        if a.done {
            Action::None
        } else {
            match kind {
                Combinator::All => {
                    if fulfill {
                        if index < a.results.len() {
                            a.results[index] = value;
                        }
                        a.remaining -= 1;
                        if a.remaining == 0 {
                            a.done = true;
                            Action::Resolve(JsValue::Undefined) // results-array built below
                        } else {
                            Action::None
                        }
                    } else {
                        a.done = true;
                        Action::Reject(value)
                    }
                }
                Combinator::AllSettled => {
                    // Build a {status, value|reason} object below; store a marker now.
                    if index < a.results.len() {
                        // Encode fulfill/reject + value in a tuple via results + errors.
                        a.results[index] = value.clone();
                        a.errors[index] = JsValue::Bool(fulfill);
                    }
                    a.remaining -= 1;
                    if a.remaining == 0 {
                        a.done = true;
                        Action::Resolve(JsValue::Undefined)
                    } else {
                        Action::None
                    }
                }
                Combinator::Race => {
                    a.done = true;
                    if fulfill {
                        Action::Resolve(value)
                    } else {
                        Action::Reject(value)
                    }
                }
                Combinator::Any => {
                    if fulfill {
                        a.done = true;
                        Action::Resolve(value)
                    } else {
                        if index < a.errors.len() {
                            a.errors[index] = value;
                        }
                        a.remaining -= 1;
                        if a.remaining == 0 {
                            a.done = true;
                            Action::Reject(JsValue::Undefined) // aggregate error built below
                        } else {
                            Action::None
                        }
                    }
                }
            }
        }
    };

    match action {
        Action::None => {}
        Action::Resolve(v) => {
            let output = agg.borrow().output.clone();
            match kind {
                Combinator::All => {
                    let results = agg.borrow().results.clone();
                    let arr = it.new_array(results);
                    resolve_promise(it, &output, arr);
                }
                Combinator::AllSettled => {
                    let (results, flags) = {
                        let a = agg.borrow();
                        (a.results.clone(), a.errors.clone())
                    };
                    let mut out = Vec::with_capacity(results.len());
                    for (val, flag) in results.into_iter().zip(flags.into_iter()) {
                        let obj = it.new_object();
                        let fulfilled = matches!(flag, JsValue::Bool(true));
                        if fulfilled {
                            let _ = it.set_property(&obj, "status", JsValue::str("fulfilled"));
                            let _ = it.set_property(&obj, "value", val);
                        } else {
                            let _ = it.set_property(&obj, "status", JsValue::str("rejected"));
                            let _ = it.set_property(&obj, "reason", val);
                        }
                        out.push(obj);
                    }
                    let arr = it.new_array(out);
                    resolve_promise(it, &output, arr);
                }
                Combinator::Race | Combinator::Any => {
                    resolve_promise(it, &output, v);
                }
            }
        }
        Action::Reject(v) => {
            let output = agg.borrow().output.clone();
            match kind {
                Combinator::Any => {
                    let errors = agg.borrow().errors.clone();
                    let err = it.make_error(ErrorKind::Error, "All promises were rejected");
                    let arr = it.new_array(errors);
                    let _ = it.set_property(&err, "errors", arr);
                    let _ = it.set_property(&err, "name", JsValue::str("AggregateError"));
                    reject_promise(it, &output, err);
                }
                _ => reject_promise(it, &output, v),
            }
        }
    }
}

// ─── queueMicrotask + timers ────────────────────────────────────────────────────

fn queue_microtask(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
    if !matches!(cb, JsValue::Function(_)) {
        return Err(type_err("queueMicrotask argument is not a function"));
    }
    enqueue_microtask(
        it,
        Box::new(move |it: &mut Interpreter| {
            let _ = it.call_function(&cb, &JsValue::Undefined, &[]);
        }),
    );
    Ok(JsValue::Undefined)
}

fn schedule_timer(it: &mut Interpreter, args: &[JsValue], interval: bool) -> R {
    let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
    if !matches!(cb, JsValue::Function(_)) {
        // Non-callable callback: a no-op timer (real browsers stringify+eval; we ignore).
        return Ok(JsValue::Number(0.0));
    }
    let delay = match args.get(1) {
        Some(JsValue::Number(n)) if n.is_finite() && *n >= 0.0 => *n,
        _ => 0.0,
    };
    let extra: Vec<JsValue> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        Vec::new()
    };
    if it.loop_state.macrotasks.len() >= EventLoop::MAX_TIMERS {
        return Ok(JsValue::Number(0.0)); // timer budget hit → drop (never unbounded)
    }
    let id = it.loop_state.next_id();
    let seq = it.loop_state.seq;
    it.loop_state.seq += 1;
    let due = it.loop_state.virtual_now + delay;
    it.loop_state.macrotasks.push(Macrotask {
        due,
        seq,
        id,
        callback: cb,
        args: extra,
        interval: if interval { Some(delay) } else { None },
    });
    Ok(JsValue::Number(id as f64))
}

fn set_timeout(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    schedule_timer(it, args, false)
}

fn set_interval(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    schedule_timer(it, args, true)
}

fn clear_timer(it: &mut Interpreter, _this: &JsValue, args: &[JsValue]) -> R {
    if let Some(JsValue::Number(n)) = args.first() {
        let id = *n as u64;
        if id != 0 {
            it.loop_state.macrotasks.retain(|m| m.id != id);
        }
    }
    Ok(JsValue::Undefined)
}
