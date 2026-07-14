//! # RaeJs tree-walking interpreter — JavaScript that actually EXECUTES.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy (criterion #5 — "the web browser is the
//! universal app runtime; PWAs that feel native"): the [parser](crate::parse) turns page
//! script into an AST but does not *run* it, so a page is still a static document. This
//! module walks the [`Program`] AST and evaluates it to real [`JsValue`]s with real side
//! effects (variable binding, closures, prototype chains, `console.log` output) — the
//! synchronous ECMAScript core that interactive pages depend on.
//!
//! ## What executes (this slice)
//! Expressions (all operators with JS coercion semantics — `+` string-concat-vs-add via
//! ToPrimitive, `==` vs `===`, ToBoolean truthiness, bitwise via ToInt32, `typeof`,
//! `instanceof`, `in`, `?.`, `??`, `&&`/`||` short-circuit, ternary, pre/post `++`/`--`,
//! compound assignment, comma); statements (var/let/const with array+object
//! destructuring, blocks with lexical scope, if/else, C-style/for-in/for-of loops,
//! while/do-while, switch with fallthrough+break, labeled break/continue, return, throw,
//! try/catch/finally, function & class declarations, arrows with lexical `this`);
//! function calls (params/defaults/rest, `this` binding, `new`, `arguments`, recursion
//! with a depth cap, closures); classes (constructor/methods/extends/super/static →
//! prototype objects); and a [core built-in library](crate::builtins) — `console`,
//! `Math`, `JSON`, `Object`, `Array.prototype`, `String.prototype`, `Number`, and the
//! global coercion/parse functions.
//!
//! ## Deferred (documented, not done)
//! `async`/`await`, generators (`function*`/`yield`), `Promise`, the microtask/event
//! loop, `Symbol`, `Proxy`/`Reflect`, getters/setters as live property accessors (parsed
//! and stored, but not invoked on read/write), tagged templates as a call, `BigInt`
//! arithmetic (a BigInt
//! literal evaluates to its `f64` approximation as a Number), labeled-loop `continue` to
//! an outer loop body. DOM bindings + the event loop are the NEXT slices.
//!
//! ## Never-panic / never-hang (load-bearing)
//! `#![forbid(unsafe_code)]` (workspace). Every script — valid or hostile — terminates
//! without panicking the host: call depth is capped ([`MAX_CALL_DEPTH`] → `RangeError`),
//! every loop and the total step count are budgeted ([`MAX_STEPS`] → `RangeError`, so
//! `while(true){}` throws instead of hanging), and object/array growth is bounded. A JS
//! `throw` becomes a [`JsError`]; a host cap breach becomes a `RangeError`. Run the
//! FAIL-able KATs with `cargo test -p ath_js`.

use crate::{
    ArrayElement, AssignOp, BinaryOp, Class, ClassMember, ClassMemberKind, Expr, ForInit, Function,
    JsError, LogicalOp, MemberProp, ObjectPatternProp, ObjectProp, Param, Pattern, Program,
    PropertyKey, Stmt, UnaryOp, UpdateOp, VarDeclarator,
};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

// ─── Budgets (never-hang / never-OOM) ────────────────────────────────────────

/// Maximum nested call frames before a `RangeError` is thrown. Guards the host stack
/// against unbounded recursion (`function f(){return f();}`). Kept conservative: each JS
/// call expands to ~8 native recursion frames (eval_call → call_function → call_user →
/// call_user_inner → exec_stmts → exec_stmt → eval_expr → …), so the effective native
/// depth is a multiple of this; 200 leaves headroom under a default ~2 MiB thread stack
/// while comfortably exceeding any realistic non-tail recursion (fib/factorial/JSON).
pub const MAX_CALL_DEPTH: usize = 200;

/// Maximum number of evaluation steps (statements + loop iterations) before a
/// `RangeError` is thrown. Guarantees `while(true){}` terminates instead of hanging.
pub const MAX_STEPS: u64 = 5_000_000;

/// Maximum length an array may reach via interpreter mutation. Past this → `RangeError`.
pub const MAX_ARRAY_LEN: usize = 16_000_000;

/// Maximum number of own properties an object may hold. Past this → `RangeError`.
pub const MAX_OBJECT_PROPS: usize = 4_000_000;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// The kind of a runtime exception, mirroring the JS error constructors a script can
/// observe via `e instanceof TypeError` (best-effort; we expose the name string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Error,
    TypeError,
    ReferenceError,
    RangeError,
    SyntaxError,
}

impl ErrorKind {
    pub fn name(self) -> &'static str {
        match self {
            ErrorKind::Error => "Error",
            ErrorKind::TypeError => "TypeError",
            ErrorKind::ReferenceError => "ReferenceError",
            ErrorKind::RangeError => "RangeError",
            ErrorKind::SyntaxError => "SyntaxError",
        }
    }
}

/// A runtime error surfaced to the host: a thrown JS value rendered to a string plus the
/// error kind. A host-level budget breach is a `RangeError`.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub kind: ErrorKind,
    pub message: String,
    /// The original thrown JS value (so a `catch (e)` binding sees the real object).
    pub value: JsValue,
}

impl RuntimeError {
    /// Public constructor for the builtins module.
    pub(crate) fn new_pub(kind: ErrorKind, message: impl Into<String>) -> Self {
        RuntimeError::new(kind, message)
    }

    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        let message = message.into();
        let value = JsValue::String(Rc::new(format!("{}: {}", kind.name(), message)));
        RuntimeError {
            kind,
            message,
            value,
        }
    }
}

impl core::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.kind.name(), self.message)
    }
}

/// Convert a runtime error into the crate-level [`JsError`] for the public API.
impl From<RuntimeError> for JsError {
    fn from(e: RuntimeError) -> JsError {
        JsError {
            message: e.to_string(),
            pos: 0,
            line: 0,
            col: 0,
        }
    }
}

// ─── Values ──────────────────────────────────────────────────────────────────

/// A JavaScript value. Objects/arrays/functions are reference types
/// (`Rc<RefCell<…>>`) so `let b = a; b.x = 1` is observable through `a`, matching JS.
#[derive(Clone)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(Rc<String>),
    Object(Rc<RefCell<JsObject>>),
    Array(Rc<RefCell<JsArray>>),
    Function(Rc<JsFunction>),
}

/// An ordered, string-keyed property map with a prototype link.
pub struct JsObject {
    /// Insertion-ordered own properties (JS preserves definition order for string keys).
    pub props: Vec<(String, JsValue)>,
    /// The `[[Prototype]]` link (`Object.getPrototypeOf`).
    pub proto: Option<JsValue>,
    /// Frozen objects (`Object.freeze`) reject writes silently.
    pub frozen: bool,
    /// A class-name tag used by error objects / `toString`.
    pub class_name: Option<String>,
    /// An internal slot for exotic built-ins (Map/Set/Date) whose state must NOT leak into
    /// `props` (so `for-in`/`JSON.stringify`/`Object.keys` stay clean). `None` for plain
    /// objects. See [`crate::builtins_collections`].
    pub internal: Option<crate::builtins_collections::Internal>,
    /// Accessor (getter/setter) properties, kept OUT of `props` so plain
    /// data-property lookups stay simple value reads. Each entry is
    /// `(key, getter, setter)`; either function may be absent (getter-only /
    /// setter-only). Installed by object literals (`{ get x(){…} }`) and classes
    /// (`get x(){…}` on the prototype), and invoked by `get_property` /
    /// `set_property` on read / write — the live-accessor behaviour the module
    /// docstring promised but previously dropped (a getter read its own function
    /// back instead of the computed value).
    pub accessors: Vec<(String, Option<JsValue>, Option<JsValue>)>,
}

impl JsObject {
    fn new() -> Self {
        JsObject {
            props: Vec::new(),
            proto: None,
            frozen: false,
            class_name: None,
            internal: None,
            accessors: Vec::new(),
        }
    }

    pub fn get_own(&self, key: &str) -> Option<&JsValue> {
        self.props.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The (getter, setter) pair for `key` if this object defines it as an
    /// accessor property, cloned so the caller can drop the borrow before
    /// invoking either function.
    fn get_accessor_cloned(&self, key: &str) -> Option<(Option<JsValue>, Option<JsValue>)> {
        self.accessors
            .iter()
            .find(|(k, _, _)| k == key)
            .map(|(_, g, s)| (g.clone(), s.clone()))
    }

    /// Define (or extend) an accessor. A second `get`/`set` for the same key
    /// merges into the existing slot (so `{ get x(){…}, set x(v){…} }` yields one
    /// accessor with both halves). Defining an accessor removes any shadowing
    /// data property of the same name (an object property is data OR accessor).
    pub fn define_accessor(&mut self, key: &str, getter: Option<JsValue>, setter: Option<JsValue>) {
        if self.frozen {
            return;
        }
        self.props.retain(|(k, _)| k != key);
        if let Some(slot) = self.accessors.iter_mut().find(|(k, _, _)| k == key) {
            if getter.is_some() {
                slot.1 = getter;
            }
            if setter.is_some() {
                slot.2 = setter;
            }
        } else {
            self.accessors.push((key.to_string(), getter, setter));
        }
    }

    fn set_own(&mut self, key: &str, val: JsValue) -> Result<(), RuntimeError> {
        if self.frozen {
            return Ok(());
        }
        if let Some(slot) = self.props.iter_mut().find(|(k, _)| k == key) {
            slot.1 = val;
            return Ok(());
        }
        if self.props.len() >= MAX_OBJECT_PROPS {
            return Err(RuntimeError::new(
                ErrorKind::RangeError,
                "object property budget exceeded",
            ));
        }
        self.props.push((key.to_string(), val));
        Ok(())
    }

    fn delete(&mut self, key: &str) -> bool {
        if let Some(idx) = self.props.iter().position(|(k, _)| k == key) {
            self.props.remove(idx);
            true
        } else {
            false
        }
    }

    fn has_own(&self, key: &str) -> bool {
        self.props.iter().any(|(k, _)| k == key)
    }
}

/// An array: dense indexed storage plus the JS `length` invariant (sparse holes become
/// `Undefined`).
pub struct JsArray {
    pub items: Vec<JsValue>,
    /// Extra named properties an array can carry (rare, but `arr.foo = 1` is legal JS).
    pub props: Vec<(String, JsValue)>,
}

impl JsArray {
    fn new(items: Vec<JsValue>) -> Self {
        JsArray {
            items,
            props: Vec::new(),
        }
    }
}

/// A callable value: either a user-defined closure (carrying its defining environment) or
/// a native built-in.
pub struct JsFunction {
    pub kind: FunctionKind,
    /// The `prototype` property used by `new` (lazily an object); shared via the cell.
    pub prototype: RefCell<Option<JsValue>>,
    /// Static/own properties on the function object itself (e.g. `Math.PI` lives on an
    /// object, but `Array.isArray` lives here).
    pub props: RefCell<Vec<(String, JsValue)>>,
    pub name: String,
}

/// Either an interpreted closure or a native Rust function.
pub enum FunctionKind {
    User(UserFunction),
    Native(NativeFn),
    /// A class constructor: like a user function but carries the resolved superclass and
    /// the methods to install on instances.
    Class(Rc<ClassInfo>),
    /// A `Function.prototype.bind` result: a target plus a fixed `this` and prepended
    /// arguments.
    Bound(BoundFunction),
}

/// The captured state of a `.bind()` call.
pub struct BoundFunction {
    pub target: JsValue,
    pub bound_this: JsValue,
    pub pre_args: Vec<JsValue>,
}

/// A user-defined function closure.
pub struct UserFunction {
    pub def: Rc<Function>,
    pub env: Env,
    /// The lexically-captured `this` for an arrow function (`Some` only for arrows).
    pub bound_this: Option<JsValue>,
    pub is_arrow: bool,
}

/// A resolved class: its instance methods + the parent constructor, ready to instantiate.
pub struct ClassInfo {
    pub def: Rc<Class>,
    pub env: Env,
    /// Resolved superclass constructor value (`Some` if `extends`).
    pub super_ctor: Option<JsValue>,
    /// The shared `prototype` object holding instance methods.
    pub proto: JsValue,
}

/// The signature of a native built-in. Receives the interpreter (for allocation / nested
/// calls), the `this` value, and the argument list.
pub type NativeFn = fn(&mut Interpreter, &JsValue, &[JsValue]) -> Result<JsValue, RuntimeError>;

/// Build a native-function [`JsValue`] without needing an [`Interpreter`] handle.
///
/// Constructing a callable does not require any interpreter state, so this is exposed as a
/// free function for the one place that lacks an interpreter: a [`crate::HostObject::host_get`]
/// that wants to return a **method** value (a host object advertises a method by handing back
/// one of these from a property read). The browser's `document.getElementById` is built this
/// way. For code that *does* hold an interpreter, prefer [`Interpreter::host_function`].
pub fn native_function_value(name: &str, f: crate::host::HostFn) -> JsValue {
    JsValue::Function(Rc::new(JsFunction {
        kind: FunctionKind::Native(f),
        prototype: RefCell::new(None),
        props: RefCell::new(Vec::new()),
        name: name.to_string(),
    }))
}

impl JsValue {
    pub fn str(s: impl Into<String>) -> JsValue {
        JsValue::String(Rc::new(s.into()))
    }

    /// `typeof` result.
    pub fn type_of(&self) -> &'static str {
        match self {
            JsValue::Undefined => "undefined",
            JsValue::Null => "object", // the famous quirk
            JsValue::Bool(_) => "boolean",
            JsValue::Number(_) => "number",
            JsValue::String(_) => "string",
            JsValue::Function(_) => "function",
            // A Symbol is an Object carrying the Symbol internal slot, but
            // `typeof` must report "symbol" (ES6).
            JsValue::Object(o)
                if matches!(
                    o.borrow().internal,
                    Some(crate::builtins_collections::Internal::Symbol(_))
                ) =>
            {
                "symbol"
            }
            JsValue::Object(_) | JsValue::Array(_) => "object",
        }
    }

    fn is_callable(&self) -> bool {
        matches!(self, JsValue::Function(_))
    }
}

impl PartialEq for JsValue {
    /// Reference/value identity used by tests and `Object.is`-ish needs. NOT JS `==`;
    /// abstract/strict equality live in [`Interpreter::strict_eq`] / [`abstract_eq`].
    fn eq(&self, other: &JsValue) -> bool {
        match (self, other) {
            (JsValue::Undefined, JsValue::Undefined) => true,
            (JsValue::Null, JsValue::Null) => true,
            (JsValue::Bool(a), JsValue::Bool(b)) => a == b,
            (JsValue::Number(a), JsValue::Number(b)) => a == b,
            (JsValue::String(a), JsValue::String(b)) => a == b,
            (JsValue::Object(a), JsValue::Object(b)) => Rc::ptr_eq(a, b),
            (JsValue::Array(a), JsValue::Array(b)) => Rc::ptr_eq(a, b),
            (JsValue::Function(a), JsValue::Function(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl core::fmt::Debug for JsValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            JsValue::Undefined => write!(f, "undefined"),
            JsValue::Null => write!(f, "null"),
            JsValue::Bool(b) => write!(f, "{}", b),
            JsValue::Number(n) => write!(f, "{}", n),
            JsValue::String(s) => write!(f, "{:?}", s.as_str()),
            JsValue::Object(_) => write!(f, "[object Object]"),
            JsValue::Array(_) => write!(f, "[array]"),
            JsValue::Function(_) => write!(f, "[function]"),
        }
    }
}

// ─── Environments / scopes ─────────────────────────────────────────────────────

/// One lexical scope: a mutable map of bindings plus an optional parent (closure chain).
pub struct Scope {
    vars: BTreeMap<String, Binding>,
    parent: Option<Env>,
}

/// A reference-counted, interior-mutable scope so closures can share and outlive the
/// frame that created them.
pub type Env = Rc<RefCell<Scope>>;

/// A single binding: its value plus mutability (const) and TDZ state (let/const before
/// initialization).
#[derive(Clone)]
struct Binding {
    value: JsValue,
    mutable: bool,
    /// `true` if declared (let/const) but not yet initialized — reading is a TDZ
    /// `ReferenceError`.
    tdz: bool,
}

fn new_scope(parent: Option<Env>) -> Env {
    Rc::new(RefCell::new(Scope {
        vars: BTreeMap::new(),
        parent,
    }))
}

fn scope_declare(env: &Env, name: &str, value: JsValue, mutable: bool) {
    env.borrow_mut().vars.insert(
        name.to_string(),
        Binding {
            value,
            mutable,
            tdz: false,
        },
    );
}

fn scope_declare_tdz(env: &Env, name: &str, mutable: bool) {
    env.borrow_mut().vars.insert(
        name.to_string(),
        Binding {
            value: JsValue::Undefined,
            mutable,
            tdz: true,
        },
    );
}

/// Read a binding, walking the scope chain. TDZ → ReferenceError; missing → None.
fn scope_get(env: &Env, name: &str) -> Result<Option<JsValue>, RuntimeError> {
    let mut cur = Some(env.clone());
    while let Some(e) = cur {
        let borrow = e.borrow();
        if let Some(b) = borrow.vars.get(name) {
            if b.tdz {
                return Err(RuntimeError::new(
                    ErrorKind::ReferenceError,
                    format!("Cannot access '{}' before initialization", name),
                ));
            }
            return Ok(Some(b.value.clone()));
        }
        cur = borrow.parent.clone();
    }
    Ok(None)
}

/// Assign to an existing binding, walking the chain. Returns Ok(true) if found.
fn scope_set(env: &Env, name: &str, value: JsValue) -> Result<bool, RuntimeError> {
    let mut cur = Some(env.clone());
    while let Some(e) = cur {
        let mut borrow = e.borrow_mut();
        if let Some(b) = borrow.vars.get_mut(name) {
            if !b.mutable && !b.tdz {
                return Err(RuntimeError::new(
                    ErrorKind::TypeError,
                    format!("Assignment to constant variable '{}'", name),
                ));
            }
            b.value = value;
            b.tdz = false;
            return Ok(true);
        }
        cur = borrow.parent.clone();
    }
    Ok(false)
}

// ─── Control-flow signals ──────────────────────────────────────────────────────

/// The non-local result of executing a statement: normal completion, or an abrupt
/// completion that unwinds (return/break/continue).
enum Flow {
    Normal,
    Return(JsValue),
    Break(Option<String>),
    Continue(Option<String>),
}

// ─── The interpreter ───────────────────────────────────────────────────────────

/// A tree-walking ECMAScript interpreter over the [`Program`] AST.
///
/// One instance owns a global scope, the captured `console` output buffer, and the
/// budgets that make execution total. Re-use across [`Interpreter::eval`] calls preserves
/// global state (so a REPL-style sequence sees earlier definitions).
pub struct Interpreter {
    pub(crate) global: Env,
    pub(crate) global_this: JsValue,
    console: Vec<String>,
    steps: u64,
    depth: usize,
    /// Deterministic PRNG state for `Math.random` (seeded, reproducible for tests).
    rng: u64,
    /// Cached shared prototypes for primitive method dispatch.
    pub(crate) object_proto: JsValue,
    /// Shared prototype objects for the exotic built-ins (Map/Set/Date). Each carries the
    /// native methods; instances link to it as `[[Prototype]]`. Populated by the builtins
    /// installer; `Undefined` until then.
    pub(crate) map_proto: JsValue,
    pub(crate) set_proto: JsValue,
    pub(crate) date_proto: JsValue,
    /// Shared `RegExp.prototype` carrying `.test`/`.exec`/`.toString` + the `source`/`flags`/
    /// `global`/`ignoreCase` accessors. RegExp instances link to it as `[[Prototype]]`.
    /// `Undefined` until the builtins installer runs. See [`crate::builtins_regexp`].
    pub(crate) regexp_proto: JsValue,
    /// The completion value of the most recently executed expression statement, used as
    /// the program/eval result (REPL-style — like the spec's completion record).
    completion: JsValue,
    /// Shared `Promise.prototype` carrying `.then`/`.catch`/`.finally`. New promises link to
    /// it as `[[Prototype]]`. `Undefined` until the async builtins installer runs. See
    /// [`crate::builtins_async`].
    pub(crate) promise_proto: JsValue,
    /// The event-loop state: the microtask + macrotask queues, the virtual clock, the
    /// task budget, and the unhandled-rejection tracker. See [`crate::builtins_async`].
    pub(crate) loop_state: crate::builtins_async::EventLoop,
}

/// Flatten the whole value + promise graph before the recursive `Rc` destructors run, so a
/// hostile deep/cyclic graph (`for(...) a={next:a}`) can never overflow the host stack on
/// teardown. See [`Interpreter::teardown_graph`] and
/// [`crate::builtins_async::teardown_chains`].
impl Drop for Interpreter {
    fn drop(&mut self) {
        crate::builtins_async::teardown_chains(self);
        self.teardown_graph();
    }
}

impl Interpreter {
    /// Build a fresh interpreter with the global object and the core built-in library
    /// installed. `Math.random` is seeded deterministically.
    pub fn new() -> Self {
        let global = new_scope(None);
        let object_proto = JsValue::Object(Rc::new(RefCell::new(JsObject::new())));
        let global_this = JsValue::Object(Rc::new(RefCell::new(JsObject::new())));
        let mut interp = Interpreter {
            global,
            global_this,
            console: Vec::new(),
            steps: 0,
            depth: 0,
            rng: 0x2545F4914F6CDD1D,
            object_proto,
            map_proto: JsValue::Undefined,
            set_proto: JsValue::Undefined,
            date_proto: JsValue::Undefined,
            regexp_proto: JsValue::Undefined,
            completion: JsValue::Undefined,
            promise_proto: JsValue::Undefined,
            loop_state: crate::builtins_async::EventLoop::new(),
        };
        crate::builtins::install(&mut interp);
        interp
    }

    /// Drain and return everything written to `console.*` since the last call.
    pub fn take_console_output(&mut self) -> Vec<String> {
        core::mem::take(&mut self.console)
    }

    /// Iteratively dismantle the entire reachable value graph (objects, arrays, Map/Set
    /// internals, function closure environments, scopes) so the subsequent `Rc` drops are
    /// shallow and **never recurse**.
    ///
    /// ## Why this exists (never-host-overflow)
    /// A hostile script can build an arbitrarily deep reference chain in O(n) time:
    /// `let a={}; for(let i=0;i<300000;i++){a={next:a};}`. The chain is a 300k-deep
    /// `Rc<RefCell<JsObject>>` spine. When the `Interpreter` is dropped, the naive recursive
    /// `Rc` destructor walks that spine one native frame per link and **overflows the host
    /// stack** (confirmed `STATUS_STACK_OVERFLOW`) — turning "never-hang/never-panic" into a
    /// hard crash on untrusted input. [`crate::builtins_async::teardown_chains`] already
    /// solved this for promise reaction lists; this generalizes the same flat-sever pass to
    /// every reference type.
    ///
    /// ## How it stays bounded + cycle-safe
    /// A breadth-first worklist seeded from the interpreter roots. Each node is visited at
    /// most once (a `visited` set keyed by the `Rc` allocation address), so a cyclic graph
    /// (`let a={}; a.self=a;`) cannot re-enqueue forever. On visit, every child reference is
    /// *moved out* of the node into the worklist (the node's own collections are replaced
    /// with empty ones), so each link is severed before any refcount reaches the recursive
    /// drop path. The worklist holds the only remaining strong refs; draining it drops each
    /// node with already-empty children → every drop is depth-1.
    ///
    /// Run on [`Drop`] and exposed for between-eval cleanup; idempotent and side-effect-free
    /// w.r.t. observable behavior (it only runs once the interpreter/graph is being torn
    /// down or explicitly reset).
    pub(crate) fn teardown_graph(&mut self) {
        // Seed roots: everything the interpreter holds that can anchor a deep graph.
        // NOTE: `self.completion` (the last eval's result value) is deliberately NOT seeded —
        // an embedder/test holds that returned `JsValue` *after* the interpreter is dropped,
        // and severing it would gut the caller's value. Deep hostile graphs live in the
        // global scope (`let a = …`) and the proto/global-object roots, all of which ARE
        // seeded, so the overflow case is still flattened.
        let mut queue: Vec<JsValue> = alloc::vec![
            self.global_this.clone(),
            self.object_proto.clone(),
            self.map_proto.clone(),
            self.set_proto.clone(),
            self.date_proto.clone(),
            self.regexp_proto.clone(),
            self.promise_proto.clone(),
        ];

        // Scope graph (global + every reachable closure env) is torn down through a parallel
        // worklist of `Env`s, also visited-deduped.
        let mut scope_queue: Vec<Env> = Vec::new();
        scope_queue.push(self.global.clone());

        // Visited sets keyed by allocation address (cycle-safe, no infinite re-enqueue).
        let mut seen_obj: BTreeMap<usize, ()> = BTreeMap::new();
        let mut seen_arr: BTreeMap<usize, ()> = BTreeMap::new();
        let mut seen_fn: BTreeMap<usize, ()> = BTreeMap::new();
        let mut seen_scope: BTreeMap<usize, ()> = BTreeMap::new();

        // A hard cap on total nodes processed, so even a pathological graph can never spin
        // the teardown unbounded (defense in depth alongside the visited set).
        let mut budget: u64 = 0;
        const TEARDOWN_BUDGET: u64 = 200_000_000;

        loop {
            if budget >= TEARDOWN_BUDGET {
                break;
            }
            // Process scopes first so closure-captured envs get flattened too.
            if let Some(scope) = scope_queue.pop() {
                budget = budget.saturating_add(1);
                let key = Rc::as_ptr(&scope) as usize;
                if seen_scope.insert(key, ()).is_some() {
                    continue;
                }
                let (vars, parent) = {
                    let mut s = scope.borrow_mut();
                    let vars = core::mem::take(&mut s.vars);
                    let parent = s.parent.take();
                    (vars, parent)
                };
                for (_, binding) in vars {
                    queue.push(binding.value);
                }
                if let Some(p) = parent {
                    scope_queue.push(p);
                }
                continue;
            }

            let v = match queue.pop() {
                Some(v) => v,
                None => break,
            };
            budget = budget.saturating_add(1);
            match v {
                JsValue::Object(o) => {
                    let key = Rc::as_ptr(&o) as usize;
                    if seen_obj.insert(key, ()).is_some() {
                        continue;
                    }
                    let (props, proto, internal) = {
                        let mut b = o.borrow_mut();
                        let props = core::mem::take(&mut b.props);
                        let proto = b.proto.take();
                        let internal = b.internal.take();
                        (props, proto, internal)
                    };
                    for (_, val) in props {
                        queue.push(val);
                    }
                    if let Some(p) = proto {
                        queue.push(p);
                    }
                    if let Some(internal) = internal {
                        Self::drain_internal(internal, &mut queue);
                    }
                }
                JsValue::Array(a) => {
                    let key = Rc::as_ptr(&a) as usize;
                    if seen_arr.insert(key, ()).is_some() {
                        continue;
                    }
                    let (items, props) = {
                        let mut b = a.borrow_mut();
                        let items = core::mem::take(&mut b.items);
                        let props = core::mem::take(&mut b.props);
                        (items, props)
                    };
                    for val in items {
                        queue.push(val);
                    }
                    for (_, val) in props {
                        queue.push(val);
                    }
                }
                JsValue::Function(f) => {
                    let key = Rc::as_ptr(&f) as usize;
                    if seen_fn.insert(key, ()).is_some() {
                        continue;
                    }
                    // Function-object own props + prototype value.
                    let props = core::mem::take(&mut *f.props.borrow_mut());
                    for (_, val) in props {
                        queue.push(val);
                    }
                    if let Some(p) = f.prototype.borrow_mut().take() {
                        queue.push(p);
                    }
                    // A user closure / arrow captures its defining env — enqueue it for the
                    // scope worklist so a closure spine is flattened too.
                    match &f.kind {
                        FunctionKind::User(uf) => {
                            scope_queue.push(uf.env.clone());
                            if let Some(t) = &uf.bound_this {
                                queue.push(t.clone());
                            }
                        }
                        FunctionKind::Class(ci) => {
                            scope_queue.push(ci.env.clone());
                            queue.push(ci.proto.clone());
                            if let Some(sc) = &ci.super_ctor {
                                queue.push(sc.clone());
                            }
                        }
                        FunctionKind::Bound(b) => {
                            queue.push(b.target.clone());
                            queue.push(b.bound_this.clone());
                            for arg in &b.pre_args {
                                queue.push(arg.clone());
                            }
                        }
                        FunctionKind::Native(_) => {}
                    }
                }
                // Primitives anchor nothing.
                JsValue::Undefined
                | JsValue::Null
                | JsValue::Bool(_)
                | JsValue::Number(_)
                | JsValue::String(_) => {}
            }
        }
    }

    /// Move every reference-type value held inside a Map/Set internal slot into the teardown
    /// worklist (keys + values for Map, members for Set), clearing the backing store so the
    /// internal's own drop is shallow. Date/Symbol/RegExp/Promise hold no JS value graph
    /// (Promise reaction chains are dismantled separately by `teardown_chains`).
    fn drain_internal(internal: crate::builtins_collections::Internal, queue: &mut Vec<JsValue>) {
        use crate::builtins_collections::Internal;
        match internal {
            Internal::Map(m) => {
                let entries = core::mem::take(&mut m.borrow_mut().entries);
                for (k, val) in entries {
                    queue.push(k);
                    queue.push(val);
                }
            }
            Internal::Set(s) => {
                let values = core::mem::take(&mut s.borrow_mut().values);
                for val in values {
                    queue.push(val);
                }
            }
            Internal::Date(_)
            | Internal::Symbol(_)
            | Internal::RegExp(_)
            | Internal::Promise(_)
            // A host object bridges to native (embedder) state, not a JS value graph, so
            // there is nothing to sever here — the `Rc<dyn HostObject>` drop is shallow.
            | Internal::Host(_) => {}
        }
    }

    pub(crate) fn push_console(&mut self, line: String) {
        // Bound the buffer so a logging loop can't OOM the host (it still throws on the
        // step budget first; this is belt-and-suspenders).
        if self.console.len() < 1_000_000 {
            self.console.push(line);
        }
    }

    /// A reproducible xorshift step in `[0, 1)` for `Math.random`.
    pub(crate) fn next_random(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        // 53-bit mantissa → [0,1).
        ((x >> 11) as f64) / ((1u64 << 53) as f64)
    }

    fn tick(&mut self) -> Result<(), RuntimeError> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > MAX_STEPS {
            return Err(RuntimeError::new(
                ErrorKind::RangeError,
                "execution step budget exceeded (possible infinite loop)",
            ));
        }
        Ok(())
    }

    /// Charge one step against the global budget (the `[MAX_STEPS]` guard). Exposed so a
    /// native built-in with its own inner loop (e.g. `Array.prototype.sort`'s O(n²)
    /// comparisons, `flat`'s walk) bounds itself the same way the interpreter does —
    /// otherwise a hostile array length turns "never-hang" into a hang.
    pub(crate) fn charge_step(&mut self) -> Result<(), RuntimeError> {
        self.tick()
    }

    /// Allocate a plain object linked to `Object.prototype`.
    pub(crate) fn new_object(&self) -> JsValue {
        let mut o = JsObject::new();
        o.proto = Some(self.object_proto.clone());
        JsValue::Object(Rc::new(RefCell::new(o)))
    }

    pub(crate) fn new_array(&self, items: Vec<JsValue>) -> JsValue {
        JsValue::Array(Rc::new(RefCell::new(JsArray::new(items))))
    }

    /// Allocate a plain object whose `[[Prototype]]` is `proto` (used to make instances
    /// linked to a shared Map/Set/Date prototype that carries the native methods).
    pub(crate) fn new_object_with_proto(&self, proto: JsValue) -> JsValue {
        let mut o = JsObject::new();
        o.proto = Some(proto);
        JsValue::Object(Rc::new(RefCell::new(o)))
    }

    /// Stamp an internal slot (Map/Set/Date state) onto an existing object value. No-op for
    /// non-object values. Used by the Map/Set/Date constructors.
    pub(crate) fn set_internal(
        &self,
        obj: &JsValue,
        internal: crate::builtins_collections::Internal,
    ) {
        if let JsValue::Object(o) = obj {
            o.borrow_mut().internal = Some(internal);
        }
    }

    /// Read the internal slot of an object value (`None` for plain/non-object values).
    pub(crate) fn get_internal(
        &self,
        obj: &JsValue,
    ) -> Option<crate::builtins_collections::Internal> {
        match obj {
            JsValue::Object(o) => o.borrow().internal.clone(),
            _ => None,
        }
    }

    /// The shared `Object.prototype` value (for building exotic prototypes).
    pub(crate) fn object_proto_value(&self) -> JsValue {
        self.object_proto.clone()
    }

    /// The shared `RegExp.prototype` value (links new RegExp instances). Falls back to
    /// `Object.prototype` if the RegExp builtins have not been installed yet.
    pub(crate) fn regexp_proto_value(&self) -> JsValue {
        if matches!(self.regexp_proto, JsValue::Undefined) {
            self.object_proto.clone()
        } else {
            self.regexp_proto.clone()
        }
    }

    /// Record the shared `RegExp.prototype` (called once by the builtins installer).
    pub(crate) fn set_regexp_proto(&mut self, proto: JsValue) {
        self.regexp_proto = proto;
    }

    /// Build a native-function value.
    pub(crate) fn native(&self, name: &str, f: NativeFn) -> JsValue {
        native_function_value(name, f)
    }

    /// Define a global binding. Used by the builtins installer **and by an embedder** to
    /// install a host object (e.g. `document`/`window`) so page script can reach it as a bare
    /// global identifier. The binding is also mirrored onto `globalThis`. See [`crate::host`].
    ///
    /// ```
    /// # use ath_js::{Interpreter, JsValue};
    /// let mut it = Interpreter::new();
    /// it.define_global("APP_NAME", JsValue::str("RaeWeb"));
    /// assert_eq!(it.eval_str("APP_NAME").unwrap(), JsValue::str("RaeWeb"));
    /// ```
    pub fn define_global(&self, name: &str, value: JsValue) {
        scope_declare(&self.global, name, value.clone(), true);
        if let JsValue::Object(o) = &self.global_this {
            let _ = o.borrow_mut().set_own(name, value);
        }
    }

    /// Build a [`JsValue`] callable backed by an embedder-supplied [`crate::HostFn`]. This is
    /// the public form of the engine's internal `native` constructor: an embedder hands a
    /// host object's method out of [`crate::HostObject::host_get`] as one of these, so a JS
    /// call `obj.method(args)` runs the Rust body. See [`crate::host`].
    pub fn host_function(&self, name: &str, f: crate::host::HostFn) -> JsValue {
        // `HostFn` and the engine-internal `NativeFn` are the same `fn(...)` shape; reuse the
        // proven native dispatch path so host methods are bounded by the same call-depth /
        // step budgets as every builtin.
        self.native(name, f)
    }

    /// Wrap an embedder [`crate::HostObject`] into a [`JsValue`]. The result is a plain JS
    /// object (linked to `Object.prototype`, so `typeof` is `"object"` and inherited methods
    /// work) carrying the host backing in its exotic internal slot — so
    /// [`get_property`](Self::get_property)/[`set_property`](Self::set_property) consult the
    /// host first. Install it as a global with [`define_global`](Self::define_global). See
    /// [`crate::host`].
    pub fn new_host_object(&self, host: alloc::rc::Rc<dyn crate::host::HostObject>) -> JsValue {
        let obj = self.new_object();
        if let JsValue::Object(o) = &obj {
            o.borrow_mut().internal = Some(crate::builtins_collections::Internal::Host(host));
        }
        obj
    }

    /// Recover the [`crate::HostObject`] backing a value, or `None` if it is not a host
    /// object. A host **method** uses this on its `this` receiver to reach the concrete host
    /// state (then `host.as_any().downcast_ref::<MyType>()`). See [`crate::host`].
    pub fn host_object_of(
        &self,
        v: &JsValue,
    ) -> Option<alloc::rc::Rc<dyn crate::host::HostObject>> {
        match v {
            JsValue::Object(o) => match &o.borrow().internal {
                Some(crate::builtins_collections::Internal::Host(h)) => Some(h.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Set a static/own property on a function value (used by the builtins installer).
    pub(crate) fn set_func_static(&self, func: &JsValue, key: &str, value: JsValue) {
        if let JsValue::Function(f) = func {
            let mut props = f.props.borrow_mut();
            if let Some(slot) = props.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                props.push((key.to_string(), value));
            }
        }
    }

    /// The global object value (for `globalThis`).
    pub(crate) fn global_this_value(&self) -> JsValue {
        self.global_this.clone()
    }

    /// Build a `bind`-result function value.
    pub(crate) fn make_bound_function(
        &self,
        target: JsValue,
        bound_this: JsValue,
        pre_args: Vec<JsValue>,
    ) -> JsValue {
        let name = match &target {
            JsValue::Function(f) => format!("bound {}", f.name),
            _ => "bound".to_string(),
        };
        JsValue::Function(Rc::new(JsFunction {
            kind: FunctionKind::Bound(BoundFunction {
                target,
                bound_this,
                pre_args,
            }),
            prototype: RefCell::new(None),
            props: RefCell::new(Vec::new()),
            name,
        }))
    }

    /// Build a native error constructor (callable with or without `new`); it sets
    /// `name`/`message`/`stack` on the receiver/instance and tags `class_name`.
    pub(crate) fn native_error_ctor(&self, kind: ErrorKind) -> JsValue {
        let ctor = match kind {
            ErrorKind::Error => self.native("Error", error_ctor_error),
            ErrorKind::TypeError => self.native("TypeError", error_ctor_type),
            ErrorKind::RangeError => self.native("RangeError", error_ctor_range),
            ErrorKind::ReferenceError => self.native("ReferenceError", error_ctor_reference),
            ErrorKind::SyntaxError => self.native("SyntaxError", error_ctor_syntax),
        };
        // Give the constructor a prototype whose `name` matches, so `instanceof` works
        // against the chain we set on instances.
        let proto = self.new_object();
        let _ = self.set_property_raw(&proto, "name", JsValue::str(kind.name()));
        if let JsValue::Function(f) = &ctor {
            *f.prototype.borrow_mut() = Some(proto);
        }
        ctor
    }

    /// Build a runtime error object that a `catch` binding can inspect (`.name`,
    /// `.message`).
    pub(crate) fn make_error(&self, kind: ErrorKind, message: &str) -> JsValue {
        let mut o = JsObject::new();
        o.proto = Some(self.object_proto.clone());
        o.class_name = Some(kind.name().to_string());
        let _ = o.set_own("name", JsValue::str(kind.name()));
        let _ = o.set_own("message", JsValue::str(message));
        let _ = o.set_own(
            "stack",
            JsValue::str(format!("{}: {}", kind.name(), message)),
        );
        JsValue::Object(Rc::new(RefCell::new(o)))
    }

    // ── Public eval entry points ──────────────────────────────────────────────

    /// Evaluate a parsed [`Program`], returning the completion value (the value of the
    /// last expression statement, else `undefined`).
    pub fn eval(&mut self, program: &Program) -> Result<JsValue, JsError> {
        self.eval_internal(program).map_err(JsError::from)
    }

    /// Like [`eval`](Self::eval) but surfaces the typed [`RuntimeError`] (kind + thrown
    /// value) instead of the flattened [`JsError`]. Useful for tests asserting the error
    /// class, and for an embedder that wants to inspect the thrown value.
    pub fn eval_typed(&mut self, program: &Program) -> Result<JsValue, RuntimeError> {
        self.eval_internal(program)
    }

    fn eval_internal(&mut self, program: &Program) -> Result<JsValue, RuntimeError> {
        let env = self.global.clone();
        self.completion = JsValue::Undefined;
        self.hoist(&program.body, &env, true)?;
        for stmt in &program.body {
            match self.exec_stmt(stmt, &env)? {
                Flow::Normal => {}
                Flow::Return(v) => return Ok(v),
                Flow::Break(_) | Flow::Continue(_) => {
                    return Err(RuntimeError::new(
                        ErrorKind::SyntaxError,
                        "Illegal break/continue at top level",
                    ));
                }
            }
        }
        Ok(self.completion.clone())
    }

    /// Parse + evaluate source text in one call (convenience for tests/embedders).
    ///
    /// After the top-level script runs, this **auto-drains the event loop**
    /// ([`run_event_loop`](Self::run_event_loop)): all microtasks (promise reactions,
    /// `queueMicrotask`) and macrotasks (`setTimeout`/`setInterval`, in virtual-time order)
    /// run to quiescence before this returns, so a test can assert the full async execution
    /// order from a single call. The returned value is the script's completion value (the
    /// loop's side effects are observed via `console`). The loop is bounded by a task budget
    /// so a self-rescheduling task terminates instead of hanging.
    pub fn eval_str(&mut self, src: &str) -> Result<JsValue, JsError> {
        let program = crate::parse(src)?;
        let result = self.eval(&program)?;
        self.run_event_loop().map_err(JsError::from)?;
        Ok(result)
    }

    /// Drain the event loop to quiescence (or until the task budget is hit): run every
    /// microtask, then the earliest-due macrotask (advancing the virtual clock), then drain
    /// microtasks again, repeating until both queues are empty. Returns `Ok(())` normally;
    /// a runtime error escaping a *bare* callback is swallowed into the relevant promise's
    /// rejection (the loop never aborts on a JS throw), but a host-budget breach surfaces.
    /// See [`crate::builtins_async::run_event_loop`].
    pub fn run_event_loop(&mut self) -> Result<(), RuntimeError> {
        crate::builtins_async::run_event_loop(self)
    }

    // ── Hoisting ──────────────────────────────────────────────────────────────

    /// Pre-pass over a statement list: hoist `function` declarations (callable before
    /// their textual position) and `var` names (function-scoped, initialized to
    /// `undefined`). `let`/`const` get a TDZ binding. `func_scope` marks the function/
    /// global body where `var` lands.
    fn hoist(&mut self, body: &[Stmt], env: &Env, func_scope: bool) -> Result<(), RuntimeError> {
        // First: var names + function declarations at this level.
        for stmt in body {
            match stmt {
                Stmt::FunctionDecl(f) => {
                    if let Some(name) = &f.name {
                        let func = self.make_function(Rc::new(f.clone()), env, None);
                        scope_declare(env, name, func, true);
                    }
                }
                Stmt::VarDecl {
                    kind: crate::VarKind::Var,
                    declarations,
                } if func_scope => {
                    for d in declarations {
                        self.hoist_var_names(&d.target, env);
                    }
                }
                Stmt::VarDecl {
                    kind: kind @ (crate::VarKind::Let | crate::VarKind::Const),
                    declarations,
                } => {
                    // let/const are hoisted to the top of their block but uninitialized
                    // (Temporal Dead Zone): reading before the declaration line throws.
                    let mutable = *kind != crate::VarKind::Const;
                    for d in declarations {
                        self.hoist_tdz_names(&d.target, env, mutable);
                    }
                }
                Stmt::ClassDecl(_) => { /* class decls are not hoisted (TDZ) */ }
                _ => {}
            }
        }
        Ok(())
    }

    fn hoist_tdz_names(&self, pat: &Pattern, env: &Env, mutable: bool) {
        match pat {
            Pattern::Ident(name) => scope_declare_tdz(env, name, mutable),
            Pattern::Array { elements, rest } => {
                for el in elements.iter().flatten() {
                    self.hoist_tdz_names(el, env, mutable);
                }
                if let Some(r) = rest {
                    self.hoist_tdz_names(r, env, mutable);
                }
            }
            Pattern::Object { properties, rest } => {
                for p in properties {
                    self.hoist_tdz_names(&p.value, env, mutable);
                }
                if let Some(r) = rest {
                    self.hoist_tdz_names(r, env, mutable);
                }
            }
            Pattern::Default { target, .. } => self.hoist_tdz_names(target, env, mutable),
            Pattern::Member(_) => {}
        }
    }

    fn hoist_var_names(&self, pat: &Pattern, env: &Env) {
        match pat {
            Pattern::Ident(name) => {
                if scope_get(env, name).ok().flatten().is_none() {
                    // Only set undefined if not already a hoisted function.
                    if !env.borrow().vars.contains_key(name) {
                        scope_declare(env, name, JsValue::Undefined, true);
                    }
                }
            }
            Pattern::Array { elements, rest } => {
                for el in elements.iter().flatten() {
                    self.hoist_var_names(el, env);
                }
                if let Some(r) = rest {
                    self.hoist_var_names(r, env);
                }
            }
            Pattern::Object { properties, rest } => {
                for p in properties {
                    self.hoist_var_names(&p.value, env);
                }
                if let Some(r) = rest {
                    self.hoist_var_names(r, env);
                }
            }
            Pattern::Default { target, .. } => self.hoist_var_names(target, env),
            Pattern::Member(_) => {}
        }
    }

    // ── Statement execution ───────────────────────────────────────────────────

    fn exec_block(&mut self, body: &[Stmt], parent: &Env) -> Result<Flow, RuntimeError> {
        let env = new_scope(Some(parent.clone()));
        self.hoist(body, &env, false)?;
        self.exec_stmts(body, &env)
    }

    fn exec_stmts(&mut self, body: &[Stmt], env: &Env) -> Result<Flow, RuntimeError> {
        for stmt in body {
            match self.exec_stmt(stmt, env)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_stmt(&mut self, stmt: &Stmt, env: &Env) -> Result<Flow, RuntimeError> {
        self.tick()?;
        match stmt {
            Stmt::Empty => Ok(Flow::Normal),
            Stmt::Expr(e) => {
                let v = self.eval_expr(e, env)?;
                self.completion = v;
                Ok(Flow::Normal)
            }
            Stmt::VarDecl { kind, declarations } => {
                self.exec_var_decl(*kind, declarations, env)?;
                Ok(Flow::Normal)
            }
            Stmt::Block(body) => self.exec_block(body, env),
            Stmt::If {
                test,
                consequent,
                alternate,
            } => {
                let t = self.eval_expr(test, env)?;
                if to_boolean(&t) {
                    self.exec_stmt(consequent, env)
                } else if let Some(alt) = alternate {
                    self.exec_stmt(alt, env)
                } else {
                    Ok(Flow::Normal)
                }
            }
            Stmt::While { test, body } => self.exec_while(test, body, env, None),
            Stmt::DoWhile { body, test } => self.exec_do_while(body, test, env, None),
            Stmt::For {
                init,
                test,
                update,
                body,
            } => self.exec_for(init, test, update, body, env, None),
            Stmt::ForOf { left, right, body } => self.exec_for_of(left, right, body, env, None),
            Stmt::ForIn { left, right, body } => self.exec_for_in(left, right, body, env, None),
            Stmt::Switch {
                discriminant,
                cases,
            } => self.exec_switch(discriminant, cases, env),
            Stmt::Break(label) => Ok(Flow::Break(label.clone())),
            Stmt::Continue(label) => Ok(Flow::Continue(label.clone())),
            Stmt::Return(arg) => {
                let v = match arg {
                    Some(e) => self.eval_expr(e, env)?,
                    None => JsValue::Undefined,
                };
                Ok(Flow::Return(v))
            }
            Stmt::Throw(e) => {
                let v = self.eval_expr(e, env)?;
                Err(self.throw_value(v))
            }
            Stmt::Try {
                block,
                handler,
                finalizer,
            } => self.exec_try(block, handler, finalizer, env),
            Stmt::FunctionDecl(_) => Ok(Flow::Normal), // already hoisted
            Stmt::ClassDecl(c) => {
                let val = self.eval_class(c, env)?;
                if let Some(name) = &c.name {
                    scope_declare(env, name, val, false);
                }
                Ok(Flow::Normal)
            }
            Stmt::Labeled { label, body } => self.exec_labeled(label, body, env),
        }
    }

    fn exec_labeled(&mut self, label: &str, body: &Stmt, env: &Env) -> Result<Flow, RuntimeError> {
        // For loops, push the label so a `break label` / `continue label` targets them.
        let flow = match body {
            Stmt::While { test, body } => self.exec_while(test, body, env, Some(label))?,
            Stmt::DoWhile { body, test } => self.exec_do_while(body, test, env, Some(label))?,
            Stmt::For {
                init,
                test,
                update,
                body,
            } => self.exec_for(init, test, update, body, env, Some(label))?,
            Stmt::ForOf { left, right, body } => {
                self.exec_for_of(left, right, body, env, Some(label))?
            }
            Stmt::ForIn { left, right, body } => {
                self.exec_for_in(left, right, body, env, Some(label))?
            }
            other => self.exec_stmt(other, env)?,
        };
        // A `break label;` that escaped a non-loop labeled block stops here.
        match flow {
            Flow::Break(Some(l)) if l == label => Ok(Flow::Normal),
            other => Ok(other),
        }
    }

    fn exec_var_decl(
        &mut self,
        kind: crate::VarKind,
        declarations: &[VarDeclarator],
        env: &Env,
    ) -> Result<(), RuntimeError> {
        let mutable = kind != crate::VarKind::Const;
        for d in declarations {
            let value = match &d.init {
                Some(e) => self.eval_expr(e, env)?,
                None => JsValue::Undefined,
            };
            self.bind_pattern(&d.target, value, env, Some((kind, mutable)))?;
        }
        Ok(())
    }

    fn exec_while(
        &mut self,
        test: &Expr,
        body: &Stmt,
        env: &Env,
        label: Option<&str>,
    ) -> Result<Flow, RuntimeError> {
        loop {
            self.tick()?;
            let t = self.eval_expr(test, env)?;
            if !to_boolean(&t) {
                break;
            }
            match self.exec_stmt(body, env)? {
                Flow::Normal | Flow::Continue(None) => {}
                Flow::Continue(Some(l)) if Some(l.as_str()) == label => {}
                Flow::Continue(other) => return Ok(Flow::Continue(other)),
                Flow::Break(None) => break,
                Flow::Break(Some(l)) if Some(l.as_str()) == label => break,
                Flow::Break(other) => return Ok(Flow::Break(other)),
                Flow::Return(v) => return Ok(Flow::Return(v)),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_do_while(
        &mut self,
        body: &Stmt,
        test: &Expr,
        env: &Env,
        label: Option<&str>,
    ) -> Result<Flow, RuntimeError> {
        loop {
            self.tick()?;
            match self.exec_stmt(body, env)? {
                Flow::Normal | Flow::Continue(None) => {}
                Flow::Continue(Some(l)) if Some(l.as_str()) == label => {}
                Flow::Continue(other) => return Ok(Flow::Continue(other)),
                Flow::Break(None) => break,
                Flow::Break(Some(l)) if Some(l.as_str()) == label => break,
                Flow::Break(other) => return Ok(Flow::Break(other)),
                Flow::Return(v) => return Ok(Flow::Return(v)),
            }
            let t = self.eval_expr(test, env)?;
            if !to_boolean(&t) {
                break;
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_for(
        &mut self,
        init: &Option<Box<ForInit>>,
        test: &Option<Expr>,
        update: &Option<Expr>,
        body: &Stmt,
        parent: &Env,
        label: Option<&str>,
    ) -> Result<Flow, RuntimeError> {
        // The for-head gets its own scope so `for (let i…)` binds per loop.
        let env = new_scope(Some(parent.clone()));
        if let Some(init) = init {
            match &**init {
                ForInit::VarDecl { kind, declarations } => {
                    self.exec_var_decl(*kind, declarations, &env)?
                }
                ForInit::Expr(e) => {
                    self.eval_expr(e, &env)?;
                }
                ForInit::Pattern(_) => {}
            }
        }
        loop {
            self.tick()?;
            if let Some(t) = test {
                let tv = self.eval_expr(t, &env)?;
                if !to_boolean(&tv) {
                    break;
                }
            }
            match self.exec_stmt(body, &env)? {
                Flow::Normal | Flow::Continue(None) => {}
                Flow::Continue(Some(l)) if Some(l.as_str()) == label => {}
                Flow::Continue(other) => return Ok(Flow::Continue(other)),
                Flow::Break(None) => break,
                Flow::Break(Some(l)) if Some(l.as_str()) == label => break,
                Flow::Break(other) => return Ok(Flow::Break(other)),
                Flow::Return(v) => return Ok(Flow::Return(v)),
            }
            if let Some(u) = update {
                self.eval_expr(u, &env)?;
            }
        }
        Ok(Flow::Normal)
    }

    fn for_each_iter(
        &mut self,
        left: &ForInit,
        body: &Stmt,
        parent: &Env,
        label: Option<&str>,
        items: Vec<JsValue>,
    ) -> Result<Flow, RuntimeError> {
        for item in items {
            self.tick()?;
            let env = new_scope(Some(parent.clone()));
            self.bind_for_target(left, item, &env)?;
            match self.exec_stmt(body, &env)? {
                Flow::Normal | Flow::Continue(None) => {}
                Flow::Continue(Some(l)) if Some(l.as_str()) == label => {}
                Flow::Continue(other) => return Ok(Flow::Continue(other)),
                Flow::Break(None) => return Ok(Flow::Normal),
                Flow::Break(Some(l)) if Some(l.as_str()) == label => return Ok(Flow::Normal),
                Flow::Break(other) => return Ok(Flow::Break(other)),
                Flow::Return(v) => return Ok(Flow::Return(v)),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_for_of(
        &mut self,
        left: &ForInit,
        right: &Expr,
        body: &Stmt,
        parent: &Env,
        label: Option<&str>,
    ) -> Result<Flow, RuntimeError> {
        let iterable = self.eval_expr(right, parent)?;
        let items = self.iterate(&iterable)?;
        self.for_each_iter(left, body, parent, label, items)
    }

    fn exec_for_in(
        &mut self,
        left: &ForInit,
        right: &Expr,
        body: &Stmt,
        parent: &Env,
        label: Option<&str>,
    ) -> Result<Flow, RuntimeError> {
        let obj = self.eval_expr(right, parent)?;
        let keys: Vec<JsValue> = self
            .enumerate_keys(&obj)
            .into_iter()
            .map(JsValue::str)
            .collect();
        self.for_each_iter(left, body, parent, label, keys)
    }

    fn bind_for_target(
        &mut self,
        left: &ForInit,
        value: JsValue,
        env: &Env,
    ) -> Result<(), RuntimeError> {
        match left {
            ForInit::VarDecl { kind, declarations } => {
                let mutable = *kind != crate::VarKind::Const;
                if let Some(d) = declarations.first() {
                    self.bind_pattern(&d.target, value, env, Some((*kind, mutable)))?;
                }
                Ok(())
            }
            ForInit::Pattern(p) => self.bind_pattern(p, value, env, None),
            ForInit::Expr(e) => {
                // `for (x of …)` where x is an existing lvalue.
                self.assign_to_target(e, value, env)?;
                Ok(())
            }
        }
    }

    fn exec_switch(
        &mut self,
        discriminant: &Expr,
        cases: &[crate::SwitchCase],
        parent: &Env,
    ) -> Result<Flow, RuntimeError> {
        let disc = self.eval_expr(discriminant, parent)?;
        let env = new_scope(Some(parent.clone()));
        // Hoist lexical decls across all case bodies (one block scope).
        for c in cases {
            self.hoist(&c.body, &env, false)?;
        }
        // Find the first matching case (strict equality), else default.
        let mut matched: Option<usize> = None;
        for (i, c) in cases.iter().enumerate() {
            if let Some(test) = &c.test {
                let tv = self.eval_expr(test, &env)?;
                if self.strict_eq(&disc, &tv) {
                    matched = Some(i);
                    break;
                }
            }
        }
        let start = match matched {
            Some(i) => i,
            None => match cases.iter().position(|c| c.test.is_none()) {
                Some(i) => i,
                None => return Ok(Flow::Normal),
            },
        };
        // Execute from the matched case, falling through until break.
        for c in &cases[start..] {
            match self.exec_stmts(&c.body, &env)? {
                Flow::Normal => {}
                Flow::Break(None) => return Ok(Flow::Normal),
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_try(
        &mut self,
        block: &[Stmt],
        handler: &Option<crate::CatchClause>,
        finalizer: &Option<Vec<Stmt>>,
        env: &Env,
    ) -> Result<Flow, RuntimeError> {
        let try_result = self.exec_block(block, env);
        let after_catch = match try_result {
            Err(err) => {
                if let Some(h) = handler {
                    let catch_env = new_scope(Some(env.clone()));
                    if let Some(param) = &h.param {
                        self.bind_pattern(param, err.value.clone(), &catch_env, None)?;
                    }
                    self.hoist(&h.body, &catch_env, false)?;
                    self.exec_stmts(&h.body, &catch_env)
                } else {
                    Err(err)
                }
            }
            ok => ok,
        };
        // The finalizer always runs; its abrupt completion overrides.
        if let Some(fin) = finalizer {
            match self.exec_block(fin, env)? {
                Flow::Normal => after_catch,
                fin_flow => Ok(fin_flow),
            }
        } else {
            after_catch
        }
    }

    // ── Pattern binding (declarations + destructuring) ────────────────────────

    /// Bind a value to a pattern. `decl` = Some when this is a declaration (creates a
    /// binding); None when assigning to existing lvalues (`for (x of …)`).
    fn bind_pattern(
        &mut self,
        pat: &Pattern,
        value: JsValue,
        env: &Env,
        decl: Option<(crate::VarKind, bool)>,
    ) -> Result<(), RuntimeError> {
        match pat {
            Pattern::Ident(name) => {
                match decl {
                    Some((_, mutable)) => scope_declare(env, name, value, mutable),
                    None => {
                        if !scope_set(env, name, value.clone())? {
                            // Implicit global (sloppy mode).
                            scope_declare(&self.global, name, value, true);
                        }
                    }
                }
                Ok(())
            }
            Pattern::Default { target, default } => {
                let v = if matches!(value, JsValue::Undefined) {
                    self.eval_expr(default, env)?
                } else {
                    value
                };
                self.bind_pattern(target, v, env, decl)
            }
            Pattern::Array { elements, rest } => {
                let items = self.iterate(&value)?;
                let mut idx = 0;
                for slot in elements {
                    let item = items.get(idx).cloned().unwrap_or(JsValue::Undefined);
                    if let Some(p) = slot {
                        self.bind_pattern(p, item, env, decl)?;
                    }
                    idx += 1;
                }
                if let Some(r) = rest {
                    let rest_items: Vec<JsValue> = items.into_iter().skip(idx).collect();
                    let arr = self.new_array(rest_items);
                    self.bind_pattern(r, arr, env, decl)?;
                }
                Ok(())
            }
            Pattern::Object { properties, rest } => {
                let mut used: Vec<String> = Vec::new();
                for ObjectPatternProp { key, value: vp, .. } in properties {
                    let key_str = self.property_key_string(key, env)?;
                    let v = self.get_property(&value, &key_str)?;
                    used.push(key_str);
                    self.bind_pattern(vp, v, env, decl)?;
                }
                if let Some(r) = rest {
                    // Collect remaining own enumerable keys into a fresh object.
                    let obj = self.new_object();
                    for k in self.enumerate_keys(&value) {
                        if !used.contains(&k) {
                            let v = self.get_property(&value, &k)?;
                            self.set_property(&obj, &k, v)?;
                        }
                    }
                    self.bind_pattern(r, obj, env, decl)?;
                }
                Ok(())
            }
            Pattern::Member(e) => {
                self.assign_to_target(e, value, env)?;
                Ok(())
            }
        }
    }

    fn property_key_string(
        &mut self,
        key: &PropertyKey,
        env: &Env,
    ) -> Result<String, RuntimeError> {
        Ok(match key {
            PropertyKey::Ident(s) | PropertyKey::String(s) => s.clone(),
            PropertyKey::Number(n) => number_to_string(*n),
            PropertyKey::Computed(e) => {
                let v = self.eval_expr(e, env)?;
                self.to_string(&v)?
            }
        })
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Interpreter::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Expression evaluation
// ═══════════════════════════════════════════════════════════════════════════

impl Interpreter {
    fn eval_expr(&mut self, expr: &Expr, env: &Env) -> Result<JsValue, RuntimeError> {
        self.tick()?;
        match expr {
            Expr::Number(n) => Ok(JsValue::Number(*n)),
            Expr::BigInt(s) => Ok(JsValue::Number(string_to_number(s))),
            Expr::String(s) => Ok(JsValue::str(s.clone())),
            Expr::Bool(b) => Ok(JsValue::Bool(*b)),
            Expr::Null => Ok(JsValue::Null),
            Expr::Undefined => Ok(JsValue::Undefined),
            Expr::This => Ok(self.lookup_this(env)),
            Expr::Super => Ok(JsValue::Undefined), // resolved specially in call/member
            Expr::Ident(name) => self.eval_ident(name, env),
            Expr::Regex { pattern, flags } => self.make_regex(pattern, flags),
            Expr::Template {
                quasis,
                expressions,
            } => self.eval_template(quasis, expressions, env),
            Expr::TaggedTemplate {
                tag,
                quasis,
                expressions,
            } => {
                // `tag`a${x}b`` calls tag(strings, ...values): `strings` is the
                // array of cooked literal chunks (with a `.raw` array), followed
                // by each interpolated value. Used by styled-components, graphql,
                // String.raw, etc. — previously dropped to a plain cooked string.
                let tag_fn = self.eval_expr(tag, env)?;
                let chunks: Vec<JsValue> = quasis.iter().map(|s| JsValue::str(s.clone())).collect();
                let strings_arr = self.new_array(chunks.clone());
                let raw_arr = self.new_array(chunks);
                self.set_property(&strings_arr, "raw", raw_arr)?;
                let mut args = alloc::vec![strings_arr];
                for e in expressions {
                    args.push(self.eval_expr(e, env)?);
                }
                self.call_function(&tag_fn, &JsValue::Undefined, &args)
            }
            Expr::Array(elems) => self.eval_array_literal(elems, env),
            Expr::Object(props) => self.eval_object_literal(props, env),
            Expr::Function(f) => Ok(self.make_function(Rc::new(f.clone()), env, None)),
            Expr::Arrow(f) => {
                let this = self.lookup_this(env);
                Ok(self.make_function(Rc::new(f.clone()), env, Some(this)))
            }
            Expr::Class(c) => self.eval_class(c, env),
            Expr::Unary { op, operand } => self.eval_unary(*op, operand, env),
            Expr::Await { operand } => self.eval_await(operand, env),
            Expr::Update {
                op,
                prefix,
                operand,
            } => self.eval_update(*op, *prefix, operand, env),
            Expr::Binary { op, left, right } => {
                let l = self.eval_expr(left, env)?;
                let r = self.eval_expr(right, env)?;
                self.eval_binary(*op, l, r)
            }
            Expr::Logical { op, left, right } => self.eval_logical(*op, left, right, env),
            Expr::Assign { op, target, value } => self.eval_assign(*op, target, value, env),
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                let t = self.eval_expr(test, env)?;
                if to_boolean(&t) {
                    self.eval_expr(consequent, env)
                } else {
                    self.eval_expr(alternate, env)
                }
            }
            Expr::Member {
                object,
                property,
                optional,
            } => {
                let (val, _this) = self.eval_member(object, property, *optional, env)?;
                Ok(val)
            }
            Expr::Call {
                callee,
                args,
                optional,
            } => self.eval_call(callee, args, *optional, env),
            Expr::New { callee, args } => self.eval_new(callee, args, env),
            Expr::Sequence(items) => {
                let mut last = JsValue::Undefined;
                for e in items {
                    last = self.eval_expr(e, env)?;
                }
                Ok(last)
            }
            Expr::Spread(_) => Err(RuntimeError::new(
                ErrorKind::SyntaxError,
                "Unexpected spread in expression position",
            )),
        }
    }

    fn eval_ident(&mut self, name: &str, env: &Env) -> Result<JsValue, RuntimeError> {
        if name == "undefined" {
            return Ok(JsValue::Undefined);
        }
        if name == "NaN" {
            return Ok(JsValue::Number(f64::NAN));
        }
        if name == "Infinity" {
            return Ok(JsValue::Number(f64::INFINITY));
        }
        if name == "globalThis" {
            return Ok(self.global_this.clone());
        }
        match scope_get(env, name)? {
            Some(v) => Ok(v),
            None => Err(RuntimeError::new(
                ErrorKind::ReferenceError,
                format!("{} is not defined", name),
            )),
        }
    }

    fn lookup_this(&self, env: &Env) -> JsValue {
        scope_get(env, "this")
            .ok()
            .flatten()
            .unwrap_or(JsValue::Undefined)
    }

    fn eval_template(
        &mut self,
        quasis: &[String],
        expressions: &[Expr],
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        let mut out = String::new();
        for (i, q) in quasis.iter().enumerate() {
            out.push_str(q);
            if let Some(e) = expressions.get(i) {
                let v = self.eval_expr(e, env)?;
                let s = self.to_string(&v)?;
                out.push_str(&s);
            }
        }
        Ok(JsValue::str(out))
    }

    fn eval_array_literal(
        &mut self,
        elems: &[Option<ArrayElement>],
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        let mut items = Vec::new();
        for slot in elems {
            match slot {
                None => items.push(JsValue::Undefined),
                Some(ArrayElement::Expr(e)) => items.push(self.eval_expr(e, env)?),
                Some(ArrayElement::Spread(e)) => {
                    let v = self.eval_expr(e, env)?;
                    for it in self.iterate(&v)? {
                        if items.len() >= MAX_ARRAY_LEN {
                            return Err(RuntimeError::new(
                                ErrorKind::RangeError,
                                "array length budget exceeded",
                            ));
                        }
                        items.push(it);
                    }
                }
            }
        }
        Ok(self.new_array(items))
    }

    fn eval_object_literal(
        &mut self,
        props: &[ObjectProp],
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        let obj = self.new_object();
        for p in props {
            match p {
                ObjectProp::KeyValue { key, value } => {
                    let k = self.property_key_string(key, env)?;
                    let v = self.eval_expr(value, env)?;
                    self.set_property(&obj, &k, v)?;
                }
                ObjectProp::Shorthand(name) => {
                    let v = self.eval_ident(name, env)?;
                    self.set_property(&obj, name, v)?;
                }
                ObjectProp::Method { key, kind, value } => {
                    let k = self.property_key_string(key, env)?;
                    let f = self.make_function(Rc::new(value.clone()), env, None);
                    match kind {
                        ClassMemberKind::Getter => {
                            if let JsValue::Object(o) = &obj {
                                o.borrow_mut().define_accessor(&k, Some(f), None);
                            }
                        }
                        ClassMemberKind::Setter => {
                            if let JsValue::Object(o) = &obj {
                                o.borrow_mut().define_accessor(&k, None, Some(f));
                            }
                        }
                        _ => self.set_property(&obj, &k, f)?,
                    }
                }
                ObjectProp::Spread(e) => {
                    let v = self.eval_expr(e, env)?;
                    for k in self.enumerate_keys(&v) {
                        let pv = self.get_property(&v, &k)?;
                        self.set_property(&obj, &k, pv)?;
                    }
                }
            }
        }
        Ok(obj)
    }

    fn make_regex(&self, pattern: &str, flags: &str) -> Result<JsValue, RuntimeError> {
        // A regex literal `/abc/gi` constructs a real RegExp wrapping a compiled
        // `ath_regex::Regex`. A bad pattern throws SyntaxError (never panics).
        crate::builtins_regexp::construct_regexp(self, pattern, flags)
    }

    fn eval_unary(
        &mut self,
        op: UnaryOp,
        operand: &Expr,
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        // `typeof <undeclared ident>` must NOT throw — special-case it.
        if op == UnaryOp::Typeof {
            if let Expr::Ident(name) = operand {
                let v = match scope_get(env, name)? {
                    Some(v) => v,
                    None => return Ok(JsValue::str("undefined")),
                };
                return Ok(JsValue::str(v.type_of()));
            }
        }
        if op == UnaryOp::Delete {
            if let Expr::Member {
                object, property, ..
            } = operand
            {
                let obj = self.eval_expr(object, env)?;
                let key = self.member_key(property, env)?;
                return Ok(JsValue::Bool(self.delete_property(&obj, &key)));
            }
            return Ok(JsValue::Bool(true));
        }
        let v = self.eval_expr(operand, env)?;
        Ok(match op {
            UnaryOp::Not => JsValue::Bool(!to_boolean(&v)),
            UnaryOp::Neg => JsValue::Number(-self.to_number(&v)?),
            UnaryOp::Pos => JsValue::Number(self.to_number(&v)?),
            UnaryOp::BitNot => JsValue::Number(!(to_int32(self.to_number(&v)?)) as f64),
            UnaryOp::Typeof => JsValue::str(v.type_of()),
            UnaryOp::Void => JsValue::Undefined,
            UnaryOp::Delete => JsValue::Bool(true),
        })
    }

    /// Evaluate `await operand`.
    ///
    /// Without full async-function suspension (deferred), `await` here is a **value
    /// pass-through** with honest, spec-correct behavior for the settled/synchronous case:
    /// - `await v` where `v` is not a promise → `v` itself (so `await 42 === 42`).
    /// - `await p` where `p` is an already-fulfilled promise → its resolved value.
    /// - `await p` where `p` is rejected → its reason is re-thrown as the awaited error.
    /// - `await p` where `p` is still pending → drain the microtask/event loop once (bounded
    ///   by the loop's task budget) to let synchronously-resolvable promises settle, then
    ///   read the slot. If it is *still* pending after the drain (it depends on real
    ///   external async that we cannot resume without suspension), the awaited value is
    ///   `undefined` — documented, not silent: the value path is honest for everything that
    ///   can settle synchronously, which is the realistic web-script case here.
    ///
    /// This never returns `undefined` for a non-promise or a settled promise — that was the
    /// fake-green bug (await parsed as `void`). Never panics / never hangs (the drain is
    /// budget-bounded).
    fn eval_await(&mut self, operand: &Expr, env: &Env) -> Result<JsValue, RuntimeError> {
        self.tick()?;
        let v = self.eval_expr(operand, env)?;
        let data = match crate::builtins_async::as_promise(self, &v) {
            Some(d) => d,
            // Not a thenable → await is the identity on the value.
            None => return Ok(v),
        };
        // Already settled? Read the internal result slot directly.
        if let Some(result) = self.read_settled_promise(&data)? {
            return Ok(result);
        }
        // Pending: try to settle it synchronously by draining the loop (bounded).
        self.run_event_loop()?;
        if let Some(result) = self.read_settled_promise(&data)? {
            return Ok(result);
        }
        // Genuinely still pending (needs real suspension we do not yet model). Honest
        // fallback: undefined — but only after the value path proved unsettleable.
        Ok(JsValue::Undefined)
    }

    /// If the promise is settled, return `Some(value)` for a fulfillment or re-throw the
    /// rejection reason; return `Ok(None)` while still pending. Marks the promise handled so
    /// a consumed rejection is not double-reported as unhandled.
    fn read_settled_promise(
        &mut self,
        data: &Rc<RefCell<crate::builtins_async::PromiseData>>,
    ) -> Result<Option<JsValue>, RuntimeError> {
        use crate::builtins_async::PromiseState;
        let (state, value) = {
            let mut d = data.borrow_mut();
            d.handled = true;
            (d.state, d.value.clone())
        };
        match state {
            PromiseState::Fulfilled => Ok(Some(value)),
            PromiseState::Rejected => Err(self.throw_value(value)),
            PromiseState::Pending => Ok(None),
        }
    }

    fn eval_update(
        &mut self,
        op: UpdateOp,
        prefix: bool,
        operand: &Expr,
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        let old = self.eval_expr(operand, env)?;
        let old_num = self.to_number(&old)?;
        let new_num = match op {
            UpdateOp::Inc => old_num + 1.0,
            UpdateOp::Dec => old_num - 1.0,
        };
        self.assign_to_target(operand, JsValue::Number(new_num), env)?;
        Ok(JsValue::Number(if prefix { new_num } else { old_num }))
    }

    fn eval_logical(
        &mut self,
        op: LogicalOp,
        left: &Expr,
        right: &Expr,
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        let l = self.eval_expr(left, env)?;
        match op {
            LogicalOp::And => {
                if to_boolean(&l) {
                    self.eval_expr(right, env)
                } else {
                    Ok(l)
                }
            }
            LogicalOp::Or => {
                if to_boolean(&l) {
                    Ok(l)
                } else {
                    self.eval_expr(right, env)
                }
            }
            LogicalOp::Nullish => {
                if matches!(l, JsValue::Undefined | JsValue::Null) {
                    self.eval_expr(right, env)
                } else {
                    Ok(l)
                }
            }
        }
    }

    fn eval_binary(
        &mut self,
        op: BinaryOp,
        l: JsValue,
        r: JsValue,
    ) -> Result<JsValue, RuntimeError> {
        use BinaryOp::*;
        Ok(match op {
            Add => self.js_add(l, r)?,
            Sub => JsValue::Number(self.to_number(&l)? - self.to_number(&r)?),
            Mul => JsValue::Number(self.to_number(&l)? * self.to_number(&r)?),
            Div => JsValue::Number(self.to_number(&l)? / self.to_number(&r)?),
            Mod => JsValue::Number(js_mod(self.to_number(&l)?, self.to_number(&r)?)),
            Exp => JsValue::Number(crate::builtins::mathfn::powf(
                self.to_number(&l)?,
                self.to_number(&r)?,
            )),
            EqEq => JsValue::Bool(self.abstract_eq(&l, &r)?),
            NotEq => JsValue::Bool(!self.abstract_eq(&l, &r)?),
            EqEqEq => JsValue::Bool(self.strict_eq(&l, &r)),
            NotEqEq => JsValue::Bool(!self.strict_eq(&l, &r)),
            Lt => self.compare(&l, &r, |o| o == core::cmp::Ordering::Less)?,
            Gt => self.compare(&l, &r, |o| o == core::cmp::Ordering::Greater)?,
            LtEq => self.compare(&l, &r, |o| o != core::cmp::Ordering::Greater)?,
            GtEq => self.compare(&l, &r, |o| o != core::cmp::Ordering::Less)?,
            Shl => JsValue::Number(
                (to_int32(self.to_number(&l)?).wrapping_shl(to_uint32(self.to_number(&r)?) & 31))
                    as f64,
            ),
            Shr => JsValue::Number(
                (to_int32(self.to_number(&l)?).wrapping_shr(to_uint32(self.to_number(&r)?) & 31))
                    as f64,
            ),
            UShr => JsValue::Number(
                (to_uint32(self.to_number(&l)?) >> (to_uint32(self.to_number(&r)?) & 31)) as f64,
            ),
            BitAnd => JsValue::Number(
                (to_int32(self.to_number(&l)?) & to_int32(self.to_number(&r)?)) as f64,
            ),
            BitOr => JsValue::Number(
                (to_int32(self.to_number(&l)?) | to_int32(self.to_number(&r)?)) as f64,
            ),
            BitXor => JsValue::Number(
                (to_int32(self.to_number(&l)?) ^ to_int32(self.to_number(&r)?)) as f64,
            ),
            Instanceof => JsValue::Bool(self.instance_of(&l, &r)?),
            In => {
                let key = self.to_string(&l)?;
                JsValue::Bool(self.has_property(&r, &key))
            }
        })
    }

    /// The `+` operator: string concat if either operand is a string after ToPrimitive,
    /// else numeric add.
    fn js_add(&mut self, l: JsValue, r: JsValue) -> Result<JsValue, RuntimeError> {
        let lp = self.to_primitive(&l)?;
        let rp = self.to_primitive(&r)?;
        if matches!(lp, JsValue::String(_)) || matches!(rp, JsValue::String(_)) {
            let ls = self.to_string(&lp)?;
            let rs = self.to_string(&rp)?;
            Ok(JsValue::str(format!("{}{}", ls, rs)))
        } else {
            Ok(JsValue::Number(
                primitive_to_number(&lp) + primitive_to_number(&rp),
            ))
        }
    }

    fn compare(
        &mut self,
        l: &JsValue,
        r: &JsValue,
        pred: impl Fn(core::cmp::Ordering) -> bool,
    ) -> Result<JsValue, RuntimeError> {
        let lp = self.to_primitive(l)?;
        let rp = self.to_primitive(r)?;
        // String < String is lexicographic; otherwise numeric.
        if let (JsValue::String(a), JsValue::String(b)) = (&lp, &rp) {
            return Ok(JsValue::Bool(pred(a.as_str().cmp(b.as_str()))));
        }
        let ln = primitive_to_number(&lp);
        let rn = primitive_to_number(&rp);
        if ln.is_nan() || rn.is_nan() {
            return Ok(JsValue::Bool(false)); // any comparison with NaN is false
        }
        let ord = if ln < rn {
            core::cmp::Ordering::Less
        } else if ln > rn {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Equal
        };
        Ok(JsValue::Bool(pred(ord)))
    }

    /// `SameValueZero` (the equality Map/Set keys + `Array.prototype.includes` use): like
    /// strict equality but `NaN` equals `NaN`, and `+0`/`-0` are the same. Objects compare
    /// by reference identity.
    pub fn same_value_zero(&self, a: &JsValue, b: &JsValue) -> bool {
        match (a, b) {
            (JsValue::Number(x), JsValue::Number(y)) => {
                if x.is_nan() && y.is_nan() {
                    return true;
                }
                // `==` on f64 already treats +0.0 == -0.0 as true, which is what
                // SameValueZero wants (unlike SameValue).
                x == y
            }
            _ => self.strict_eq(a, b),
        }
    }

    /// Strict equality (`===`): no coercion.
    pub fn strict_eq(&self, a: &JsValue, b: &JsValue) -> bool {
        match (a, b) {
            (JsValue::Undefined, JsValue::Undefined) => true,
            (JsValue::Null, JsValue::Null) => true,
            (JsValue::Bool(x), JsValue::Bool(y)) => x == y,
            (JsValue::Number(x), JsValue::Number(y)) => x == y, // NaN !== NaN handled by f64
            (JsValue::String(x), JsValue::String(y)) => x == y,
            (JsValue::Object(x), JsValue::Object(y)) => Rc::ptr_eq(x, y),
            (JsValue::Array(x), JsValue::Array(y)) => Rc::ptr_eq(x, y),
            (JsValue::Function(x), JsValue::Function(y)) => Rc::ptr_eq(x, y),
            _ => false,
        }
    }

    /// Abstract equality (`==`): the coercion table.
    fn abstract_eq(&mut self, a: &JsValue, b: &JsValue) -> Result<bool, RuntimeError> {
        use JsValue::*;
        Ok(match (a, b) {
            (Undefined, Undefined) | (Null, Null) | (Undefined, Null) | (Null, Undefined) => true,
            (Number(_), Number(_)) | (String(_), String(_)) | (Bool(_), Bool(_)) => {
                self.strict_eq(a, b)
            }
            (Object(_), Object(_)) | (Array(_), Array(_)) | (Function(_), Function(_)) => {
                self.strict_eq(a, b)
            }
            // Number == String → compare as numbers.
            (Number(n), String(s)) | (String(s), Number(n)) => *n == string_to_number(s),
            // Bool == anything → ToNumber(bool) then retry.
            (Bool(x), _) => {
                let nx = Number(if *x { 1.0 } else { 0.0 });
                self.abstract_eq(&nx, b)?
            }
            (_, Bool(y)) => {
                let ny = Number(if *y { 1.0 } else { 0.0 });
                self.abstract_eq(a, &ny)?
            }
            // (Number|String) == Object → ToPrimitive(object) then retry.
            (Number(_) | String(_), Object(_) | Array(_) | Function(_)) => {
                let bp = self.to_primitive(b)?;
                self.abstract_eq(a, &bp)?
            }
            (Object(_) | Array(_) | Function(_), Number(_) | String(_)) => {
                let ap = self.to_primitive(a)?;
                self.abstract_eq(&ap, b)?
            }
            // null/undefined == object → false (already handled the null==undefined case).
            _ => false,
        })
    }

    fn instance_of(&mut self, obj: &JsValue, ctor: &JsValue) -> Result<bool, RuntimeError> {
        let ctor_fn = match ctor {
            JsValue::Function(f) => f.clone(),
            _ => {
                return Err(RuntimeError::new(
                    ErrorKind::TypeError,
                    "Right-hand side of 'instanceof' is not callable",
                ))
            }
        };
        // Exotic native values (arrays, functions) and plain objects aren't
        // linked to the global Array/Function/Object prototypes, so the chain
        // walk below misses `[] instanceof Array`, `f instanceof Function`, and
        // `{} instanceof Object`. Special-case the built-in constructors by name;
        // class instances still resolve through the prototype-chain walk.
        let ctor_name = ctor_fn.name.as_str();
        match obj {
            JsValue::Array(_) if ctor_name == "Array" || ctor_name == "Object" => return Ok(true),
            JsValue::Function(_) if ctor_name == "Function" || ctor_name == "Object" => {
                return Ok(true)
            }
            JsValue::Object(_) if ctor_name == "Object" => return Ok(true),
            _ => {}
        }

        let proto = ctor_fn.prototype.borrow().clone();
        let target = match proto {
            Some(p) => p,
            None => return Ok(false),
        };
        // Walk obj's prototype chain looking for `target`.
        let mut cur = self.get_prototype(obj);
        let mut guard = 0;
        while let Some(p) = cur {
            guard += 1;
            if guard > 10_000 {
                break;
            }
            if self.strict_eq(&p, &target) {
                return Ok(true);
            }
            cur = self.get_prototype(&p);
        }
        Ok(false)
    }

    // ── Assignment ────────────────────────────────────────────────────────────

    fn eval_assign(
        &mut self,
        op: AssignOp,
        target: &Expr,
        value: &Expr,
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        if op == AssignOp::Assign {
            // Plain assignment may be a destructuring pattern on the LHS.
            if let Some(pat) = expr_to_pattern(target) {
                let v = self.eval_expr(value, env)?;
                self.bind_pattern(&pat, v.clone(), env, None)?;
                return Ok(v);
            }
            let v = self.eval_expr(value, env)?;
            self.assign_to_target(target, v.clone(), env)?;
            return Ok(v);
        }
        // Logical compound assignment short-circuits.
        if matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish) {
            let cur = self.eval_expr(target, env)?;
            let should = match op {
                AssignOp::And => to_boolean(&cur),
                AssignOp::Or => !to_boolean(&cur),
                AssignOp::Nullish => matches!(cur, JsValue::Undefined | JsValue::Null),
                _ => unreachable!(),
            };
            if !should {
                return Ok(cur);
            }
            let v = self.eval_expr(value, env)?;
            self.assign_to_target(target, v.clone(), env)?;
            return Ok(v);
        }
        // Arithmetic/bitwise compound: read, combine, write.
        let cur = self.eval_expr(target, env)?;
        let rhs = self.eval_expr(value, env)?;
        let bin_op = match op {
            AssignOp::Add => BinaryOp::Add,
            AssignOp::Sub => BinaryOp::Sub,
            AssignOp::Mul => BinaryOp::Mul,
            AssignOp::Div => BinaryOp::Div,
            AssignOp::Mod => BinaryOp::Mod,
            AssignOp::Exp => BinaryOp::Exp,
            AssignOp::Shl => BinaryOp::Shl,
            AssignOp::Shr => BinaryOp::Shr,
            AssignOp::UShr => BinaryOp::UShr,
            AssignOp::BitAnd => BinaryOp::BitAnd,
            AssignOp::BitOr => BinaryOp::BitOr,
            AssignOp::BitXor => BinaryOp::BitXor,
            _ => unreachable!(),
        };
        let combined = self.eval_binary(bin_op, cur, rhs)?;
        self.assign_to_target(target, combined.clone(), env)?;
        Ok(combined)
    }

    /// Write a value to an lvalue expression (identifier or member access).
    fn assign_to_target(
        &mut self,
        target: &Expr,
        value: JsValue,
        env: &Env,
    ) -> Result<(), RuntimeError> {
        match target {
            Expr::Ident(name) => {
                if !scope_set(env, name, value.clone())? {
                    scope_declare(&self.global, name, value, true);
                }
                Ok(())
            }
            Expr::Member {
                object, property, ..
            } => {
                let obj = self.eval_expr(object, env)?;
                let key = self.member_key(property, env)?;
                self.set_property(&obj, &key, value)
            }
            _ => Err(RuntimeError::new(
                ErrorKind::SyntaxError,
                "Invalid assignment target",
            )),
        }
    }

    // ── Member access / calls / new ───────────────────────────────────────────

    fn member_key(&mut self, prop: &MemberProp, env: &Env) -> Result<String, RuntimeError> {
        Ok(match prop {
            MemberProp::Ident(name) => name.clone(),
            MemberProp::Computed(e) => {
                let v = self.eval_expr(e, env)?;
                self.to_string(&v)?
            }
        })
    }

    /// Evaluate a member access, returning (value, receiver) so a following call binds
    /// `this` to the receiver.
    fn eval_member(
        &mut self,
        object: &Expr,
        property: &MemberProp,
        optional: bool,
        env: &Env,
    ) -> Result<(JsValue, JsValue), RuntimeError> {
        // `super.method` — resolve against the home object's prototype's prototype.
        if matches!(object, Expr::Super) {
            let this = self.lookup_this(env);
            let home_proto = scope_get(env, "__superproto__").ok().flatten();
            let key = self.member_key(property, env)?;
            if let Some(sp) = home_proto {
                let v = self.get_property(&sp, &key)?;
                return Ok((v, this));
            }
            return Ok((JsValue::Undefined, this));
        }
        let obj = self.eval_expr(object, env)?;
        if optional && matches!(obj, JsValue::Undefined | JsValue::Null) {
            return Ok((JsValue::Undefined, JsValue::Undefined));
        }
        let key = self.member_key(property, env)?;
        let v = self.get_property(&obj, &key)?;
        Ok((v, obj))
    }

    fn eval_args(
        &mut self,
        args: &[ArrayElement],
        env: &Env,
    ) -> Result<Vec<JsValue>, RuntimeError> {
        let mut out = Vec::new();
        for a in args {
            match a {
                ArrayElement::Expr(e) => out.push(self.eval_expr(e, env)?),
                ArrayElement::Spread(e) => {
                    let v = self.eval_expr(e, env)?;
                    for it in self.iterate(&v)? {
                        out.push(it);
                    }
                }
            }
        }
        Ok(out)
    }

    fn eval_call(
        &mut self,
        callee: &Expr,
        args: &[ArrayElement],
        optional: bool,
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        // `super(...)` — call the parent constructor with the current `this`.
        if matches!(callee, Expr::Super) {
            let super_ctor = scope_get(env, "__superctor__").ok().flatten();
            let this = self.lookup_this(env);
            let argv = self.eval_args(args, env)?;
            if let Some(sc) = super_ctor {
                self.invoke_constructor_on(&sc, &this, &argv)?;
            }
            return Ok(JsValue::Undefined);
        }
        // Method call: the receiver becomes `this`.
        let (func, this) = match callee {
            Expr::Member {
                object,
                property,
                optional: mopt,
            } => self.eval_member(object, property, *mopt, env)?,
            _ => (self.eval_expr(callee, env)?, JsValue::Undefined),
        };
        if optional && matches!(func, JsValue::Undefined | JsValue::Null) {
            return Ok(JsValue::Undefined);
        }
        let argv = self.eval_args(args, env)?;
        self.call_function(&func, &this, &argv)
    }

    fn eval_new(
        &mut self,
        callee: &Expr,
        args: &[ArrayElement],
        env: &Env,
    ) -> Result<JsValue, RuntimeError> {
        let ctor = self.eval_expr(callee, env)?;
        let argv = self.eval_args(args, env)?;
        self.construct(&ctor, &argv)
    }

    /// Call any callable value. Dispatches native vs user vs class-constructor (as a
    /// plain call, which JS forbids for classes but we tolerate → TypeError on class).
    pub fn call_function(
        &mut self,
        func: &JsValue,
        this: &JsValue,
        args: &[JsValue],
    ) -> Result<JsValue, RuntimeError> {
        let f = match func {
            JsValue::Function(f) => f.clone(),
            _ => {
                return Err(RuntimeError::new(
                    ErrorKind::TypeError,
                    format!("{} is not a function", func.type_of()),
                ))
            }
        };
        match &f.kind {
            FunctionKind::Native(nf) => {
                self.enter()?;
                let r = nf(self, this, args);
                self.leave();
                r
            }
            FunctionKind::User(uf) => self.call_user(uf, this, args, &f),
            FunctionKind::Bound(b) => {
                let mut all = b.pre_args.clone();
                all.extend_from_slice(args);
                let target = b.target.clone();
                let bthis = b.bound_this.clone();
                self.call_function(&target, &bthis, &all)
            }
            FunctionKind::Class(_) => Err(RuntimeError::new(
                ErrorKind::TypeError,
                format!(
                    "Class constructor {} cannot be invoked without 'new'",
                    f.name
                ),
            )),
        }
    }

    fn enter(&mut self) -> Result<(), RuntimeError> {
        self.depth += 1;
        if self.depth > MAX_CALL_DEPTH {
            self.depth -= 1;
            return Err(RuntimeError::new(
                ErrorKind::RangeError,
                "Maximum call stack size exceeded",
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn call_user(
        &mut self,
        uf: &UserFunction,
        this: &JsValue,
        args: &[JsValue],
        func: &Rc<JsFunction>,
    ) -> Result<JsValue, RuntimeError> {
        self.enter()?;
        let result = self.call_user_inner(uf, this, args, func);
        self.leave();
        result
    }

    fn call_user_inner(
        &mut self,
        uf: &UserFunction,
        this: &JsValue,
        args: &[JsValue],
        _func: &Rc<JsFunction>,
    ) -> Result<JsValue, RuntimeError> {
        let scope = new_scope(Some(uf.env.clone()));
        // `this`: arrows use the captured value; others use the call receiver.
        let this_val = if uf.is_arrow {
            uf.bound_this.clone().unwrap_or(JsValue::Undefined)
        } else {
            this.clone()
        };
        if !uf.is_arrow {
            scope_declare(&scope, "this", this_val, false);
            // `arguments` (best-effort, array-like as a real array).
            let args_arr = self.new_array(args.to_vec());
            scope_declare(&scope, "arguments", args_arr, true);
        }
        self.bind_params(&uf.def.params, args, &scope)?;
        // Concise arrow body is a single expression.
        if uf.def.is_arrow {
            if let Some(expr) = &uf.def.arrow_expr {
                return self.eval_expr(expr, &scope);
            }
        }
        self.hoist(&uf.def.body, &scope, true)?;
        match self.exec_stmts(&uf.def.body, &scope)? {
            Flow::Return(v) => Ok(v),
            _ => Ok(JsValue::Undefined),
        }
    }

    fn bind_params(
        &mut self,
        params: &[Param],
        args: &[JsValue],
        scope: &Env,
    ) -> Result<(), RuntimeError> {
        let mut i = 0;
        for p in params {
            if p.rest {
                let rest: Vec<JsValue> = args.iter().skip(i).cloned().collect();
                let arr = self.new_array(rest);
                self.bind_pattern(&p.pattern, arr, scope, Some((crate::VarKind::Let, true)))?;
                break;
            }
            let v = args.get(i).cloned().unwrap_or(JsValue::Undefined);
            self.bind_pattern(&p.pattern, v, scope, Some((crate::VarKind::Let, true)))?;
            i += 1;
        }
        Ok(())
    }

    /// `new ctor(args)` — allocate an instance, link its prototype, run the constructor.
    pub fn construct(&mut self, ctor: &JsValue, args: &[JsValue]) -> Result<JsValue, RuntimeError> {
        let f = match ctor {
            JsValue::Function(f) => f.clone(),
            _ => return Err(RuntimeError::new(ErrorKind::TypeError, "not a constructor")),
        };
        // Build the instance with the constructor's `prototype` as [[Prototype]].
        let proto = f.prototype.borrow().clone();
        let mut inst_obj = JsObject::new();
        inst_obj.proto = Some(proto.unwrap_or_else(|| self.object_proto.clone()));
        let instance = JsValue::Object(Rc::new(RefCell::new(inst_obj)));
        let ret = self.invoke_constructor_on(ctor, &instance, args)?;
        // If the constructor returned an object, use it; else the instance.
        match ret {
            JsValue::Object(_) | JsValue::Array(_) | JsValue::Function(_) => Ok(ret),
            _ => Ok(instance),
        }
    }

    /// Run a constructor body with `this` bound to an already-allocated instance.
    fn invoke_constructor_on(
        &mut self,
        ctor: &JsValue,
        instance: &JsValue,
        args: &[JsValue],
    ) -> Result<JsValue, RuntimeError> {
        let f = match ctor {
            JsValue::Function(f) => f.clone(),
            _ => return Err(RuntimeError::new(ErrorKind::TypeError, "not a constructor")),
        };
        match &f.kind {
            FunctionKind::Native(nf) => {
                self.enter()?;
                let r = nf(self, instance, args);
                self.leave();
                r
            }
            FunctionKind::User(uf) => {
                self.enter()?;
                let r = self.call_user_inner(uf, instance, args, &f);
                self.leave();
                r
            }
            FunctionKind::Bound(b) => {
                let mut all = b.pre_args.clone();
                all.extend_from_slice(args);
                let target = b.target.clone();
                self.invoke_constructor_on(&target, instance, &all)
            }
            FunctionKind::Class(ci) => self.construct_class(ci, instance, args),
        }
    }

    // ── Functions / closures ──────────────────────────────────────────────────

    /// Build a function value capturing `env`. `arrow_this` is Some for arrow functions.
    pub(crate) fn make_function(
        &self,
        def: Rc<Function>,
        env: &Env,
        arrow_this: Option<JsValue>,
    ) -> JsValue {
        let is_arrow = def.is_arrow;
        let name = def.name.clone().unwrap_or_default();
        let func = JsFunction {
            kind: FunctionKind::User(UserFunction {
                def,
                env: env.clone(),
                bound_this: arrow_this,
                is_arrow,
            }),
            prototype: RefCell::new(None),
            props: RefCell::new(Vec::new()),
            name,
        };
        // Non-arrow functions get a fresh `.prototype` object (for `new`).
        if !is_arrow {
            let proto = self.new_object();
            *func.prototype.borrow_mut() = Some(proto);
        }
        JsValue::Function(Rc::new(func))
    }

    // ── Classes ───────────────────────────────────────────────────────────────

    fn eval_class(&mut self, class: &Class, env: &Env) -> Result<JsValue, RuntimeError> {
        // Resolve superclass (if any).
        let super_ctor = match &class.super_class {
            Some(e) => Some(self.eval_expr(e, env)?),
            None => None,
        };
        // The prototype object holds instance methods; link to super's prototype.
        let proto = self.new_object();
        if let Some(JsValue::Function(sf)) = &super_ctor {
            if let Some(sp) = sf.prototype.borrow().clone() {
                if let JsValue::Object(o) = &proto {
                    o.borrow_mut().proto = Some(sp);
                }
            }
        }
        // Install instance methods + static members.
        let class_rc = Rc::new(class.clone());
        let info = Rc::new(ClassInfo {
            def: class_rc.clone(),
            env: env.clone(),
            super_ctor: super_ctor.clone(),
            proto: proto.clone(),
        });
        let ctor_val = JsValue::Function(Rc::new(JsFunction {
            kind: FunctionKind::Class(info.clone()),
            prototype: RefCell::new(Some(proto.clone())),
            props: RefCell::new(Vec::new()),
            name: class.name.clone().unwrap_or_default(),
        }));
        // proto.constructor = ctor
        self.set_property(&proto, "constructor", ctor_val.clone())?;
        for member in &class.members {
            self.install_class_member(&ctor_val, &proto, member, env)?;
        }
        Ok(ctor_val)
    }

    fn install_class_member(
        &mut self,
        ctor: &JsValue,
        proto: &JsValue,
        member: &ClassMember,
        env: &Env,
    ) -> Result<(), RuntimeError> {
        match member.kind {
            ClassMemberKind::Constructor => Ok(()), // handled at construct time
            ClassMemberKind::Method | ClassMemberKind::Getter | ClassMemberKind::Setter => {
                let key = self.property_key_string(&member.key, env)?;
                if let Some(func) = &member.value {
                    let f = self.make_function(Rc::new(func.clone()), env, None);
                    // Getters/setters become accessor properties on the prototype
                    // so `instance.x` invokes the getter; plain methods stay data
                    // properties. Static accessors are rare — keep them as data
                    // props on the ctor (prior behaviour).
                    match member.kind {
                        ClassMemberKind::Getter if !member.is_static => {
                            if let JsValue::Object(o) = proto {
                                o.borrow_mut().define_accessor(&key, Some(f), None);
                            }
                        }
                        ClassMemberKind::Setter if !member.is_static => {
                            if let JsValue::Object(o) = proto {
                                o.borrow_mut().define_accessor(&key, None, Some(f));
                            }
                        }
                        _ if member.is_static => self.set_function_prop(ctor, &key, f),
                        _ => self.set_property(proto, &key, f)?,
                    }
                }
                Ok(())
            }
            ClassMemberKind::Field => {
                if member.is_static {
                    let key = self.property_key_string(&member.key, env)?;
                    let v = match &member.field_init {
                        Some(e) => self.eval_expr(e, env)?,
                        None => JsValue::Undefined,
                    };
                    self.set_function_prop(ctor, &key, v);
                }
                // Instance fields are applied at construction (handled in construct_class).
                Ok(())
            }
        }
    }

    fn construct_class(
        &mut self,
        info: &Rc<ClassInfo>,
        instance: &JsValue,
        args: &[JsValue],
    ) -> Result<JsValue, RuntimeError> {
        self.enter()?;
        let r = self.construct_class_inner(info, instance, args);
        self.leave();
        r
    }

    fn construct_class_inner(
        &mut self,
        info: &Rc<ClassInfo>,
        instance: &JsValue,
        args: &[JsValue],
    ) -> Result<JsValue, RuntimeError> {
        // Apply instance field initializers first (before constructor body, after super in
        // real JS; best-effort ordering here: fields then constructor).
        // Find the constructor member.
        let ctor_member = info
            .def
            .members
            .iter()
            .find(|m| m.kind == ClassMemberKind::Constructor);

        // Build the scope where the constructor/methods run: super links available.
        let scope = new_scope(Some(info.env.clone()));
        scope_declare(&scope, "this", instance.clone(), false);
        if let Some(sc) = &info.super_ctor {
            scope_declare(&scope, "__superctor__", sc.clone(), false);
            if let JsValue::Function(sf) = sc {
                if let Some(sp) = sf.prototype.borrow().clone() {
                    scope_declare(&scope, "__superproto__", sp, false);
                }
            }
        }

        // Instance field initializers.
        for m in &info.def.members {
            if m.kind == ClassMemberKind::Field && !m.is_static {
                let key = self.property_key_string(&m.key, &scope)?;
                let v = match &m.field_init {
                    Some(e) => self.eval_expr(e, &scope)?,
                    None => JsValue::Undefined,
                };
                self.set_property(instance, &key, v)?;
            }
        }

        match ctor_member {
            Some(m) => {
                if let Some(func) = &m.value {
                    let args_arr = self.new_array(args.to_vec());
                    scope_declare(&scope, "arguments", args_arr, true);
                    self.bind_params(&func.params, args, &scope)?;
                    self.hoist(&func.body, &scope, true)?;
                    match self.exec_stmts(&func.body, &scope)? {
                        Flow::Return(v @ JsValue::Object(_)) => return Ok(v),
                        _ => {}
                    }
                }
            }
            None => {
                // Default constructor: implicitly call super(...args) if extends.
                if info.super_ctor.is_some() {
                    let sc = info.super_ctor.clone().unwrap();
                    self.invoke_constructor_on(&sc, instance, args)?;
                }
            }
        }
        Ok(instance.clone())
    }

    fn throw_value(&self, v: JsValue) -> RuntimeError {
        // Render a thrown value for the host; if it's an error-like object, use its name.
        let (kind, message) = match &v {
            JsValue::Object(o) => {
                let b = o.borrow();
                let name = b
                    .get_own("name")
                    .and_then(|n| match n {
                        JsValue::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .or_else(|| b.class_name.clone());
                let msg = b
                    .get_own("message")
                    .and_then(|m| match m {
                        JsValue::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let kind = match name.as_deref() {
                    Some("TypeError") => ErrorKind::TypeError,
                    Some("RangeError") => ErrorKind::RangeError,
                    Some("ReferenceError") => ErrorKind::ReferenceError,
                    Some("SyntaxError") => ErrorKind::SyntaxError,
                    _ => ErrorKind::Error,
                };
                (kind, msg)
            }
            JsValue::String(s) => (ErrorKind::Error, s.to_string()),
            other => (ErrorKind::Error, format!("{:?}", other)),
        };
        RuntimeError {
            kind,
            message,
            value: v,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Property access + coercions + iteration
// ═══════════════════════════════════════════════════════════════════════════

impl Interpreter {
    /// `obj.key` read with prototype-chain lookup. Built-in methods on primitives
    /// (string/number) and arrays are resolved here too.
    pub fn get_property(&mut self, obj: &JsValue, key: &str) -> Result<JsValue, RuntimeError> {
        match obj {
            JsValue::Undefined | JsValue::Null => Err(RuntimeError::new(
                ErrorKind::TypeError,
                format!(
                    "Cannot read properties of {} (reading '{}')",
                    obj.type_of(),
                    key
                ),
            )),
            JsValue::String(s) => Ok(self.string_property(s, key)),
            JsValue::Number(_) => Ok(crate::builtins::number_property(self, key)),
            JsValue::Bool(_) => Ok(JsValue::Undefined),
            JsValue::Array(a) => {
                // length / index / array methods / extra props.
                if key == "length" {
                    return Ok(JsValue::Number(a.borrow().items.len() as f64));
                }
                if let Ok(idx) = key.parse::<usize>() {
                    return Ok(a
                        .borrow()
                        .items
                        .get(idx)
                        .cloned()
                        .unwrap_or(JsValue::Undefined));
                }
                if let Some(v) = a
                    .borrow()
                    .props
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                {
                    return Ok(v);
                }
                Ok(crate::builtins::array_property(self, key))
            }
            JsValue::Object(o) => {
                // Host objects (the embedder's DOM binding) dispatch reads into native code
                // first — `document.getElementById`, `el.textContent`, etc. A `None` from the
                // host falls through to ordinary own-prop / prototype lookup below, so a
                // script can still stash plain fields on the object.
                if let Some(crate::builtins_collections::Internal::Host(h)) =
                    o.borrow().internal.clone()
                {
                    if let Some(v) = h.host_get(key) {
                        return Ok(v);
                    }
                }
                // Map/Set expose `size` as an accessor; the interpreter does not invoke
                // getters on read, so resolve it from the internal slot here.
                if key == "size" {
                    if let Some(internal) = o.borrow().internal.clone() {
                        if let Some(n) = crate::builtins_collections::internal_size(&internal) {
                            return Ok(JsValue::Number(n as f64));
                        }
                    }
                }
                // RegExp exposes `source`/`flags`/`global`/`ignoreCase`/… as accessors
                // backed by the compiled internal slot (not own data properties), so
                // resolve them here the same way `size` is resolved above.
                if let Some(crate::builtins_collections::Internal::RegExp(rd)) =
                    o.borrow().internal.clone()
                {
                    if let Some(v) = crate::builtins_regexp::regexp_accessor(&rd, key) {
                        return Ok(v);
                    }
                }
                // Walk own props then the prototype chain. At each level an
                // accessor for `key` (own or inherited from the prototype, as
                // class getters are installed on the prototype) shadows nothing
                // below it: if found, invoke the getter with `this` = the
                // ORIGINAL receiver (`o`), not the level the accessor lives on.
                let mut cur = Some(JsValue::Object(o.clone()));
                let mut guard = 0;
                while let Some(JsValue::Object(co)) = cur {
                    guard += 1;
                    if guard > 10_000 {
                        break;
                    }
                    enum Level {
                        Getter(Option<JsValue>),
                        Next(Option<JsValue>),
                    }
                    let level = {
                        let b = co.borrow();
                        if let Some((g, _s)) = b.get_accessor_cloned(key) {
                            Level::Getter(g)
                        } else if let Some(v) = b.get_own(key) {
                            return Ok(v.clone());
                        } else {
                            Level::Next(b.proto.clone())
                        }
                    };
                    match level {
                        // Getter-only-defined accessor read with no getter → undefined.
                        Level::Getter(None) => return Ok(JsValue::Undefined),
                        Level::Getter(Some(getter)) => {
                            return self.call_function(&getter, &JsValue::Object(o.clone()), &[]);
                        }
                        Level::Next(next) => cur = next,
                    }
                }
                // Object.prototype method fallback (hasOwnProperty) — see builtins.
                Ok(crate::builtins::object_property(self, key))
            }
            JsValue::Function(f) => {
                if key == "name" {
                    return Ok(JsValue::str(f.name.clone()));
                }
                if key == "prototype" {
                    return Ok(f.prototype.borrow().clone().unwrap_or(JsValue::Undefined));
                }
                if key == "length" {
                    let n = match &f.kind {
                        FunctionKind::User(uf) => {
                            uf.def.params.iter().take_while(|p| !p.rest).count()
                        }
                        _ => 0,
                    };
                    return Ok(JsValue::Number(n as f64));
                }
                if let Some(v) = f
                    .props
                    .borrow()
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                {
                    return Ok(v);
                }
                // call/apply/bind are handled via builtins.
                Ok(crate::builtins::function_property(self, key))
            }
        }
    }

    /// `obj.key = value` write. `&mut self` because a write may land on an
    /// accessor property whose setter is a user function that must run on the
    /// interpreter. The plain data-store path is `set_property_raw`.
    pub fn set_property(
        &mut self,
        obj: &JsValue,
        key: &str,
        value: JsValue,
    ) -> Result<(), RuntimeError> {
        // Accessor property (own or inherited)? Invoke its setter with `this` =
        // the receiver. A getter-only accessor swallows the write (sloppy-mode
        // semantics) rather than shadowing it with a data property.
        if let JsValue::Object(o) = obj {
            let mut cur = Some(JsValue::Object(o.clone()));
            let mut guard = 0;
            while let Some(JsValue::Object(co)) = cur {
                guard += 1;
                if guard > 10_000 {
                    break;
                }
                let found = co.borrow().get_accessor_cloned(key);
                match found {
                    Some((_g, Some(setter))) => {
                        self.call_function(&setter, obj, &[value])?;
                        return Ok(());
                    }
                    Some((_g, None)) => return Ok(()), // getter-only: no-op
                    None => {
                        let next = co.borrow().proto.clone();
                        cur = next;
                    }
                }
            }
        }
        self.set_property_raw(obj, key, value)
    }

    /// Plain data-property write that never consults accessors — for native code
    /// building a fresh object (prototypes, instances, match arrays) that
    /// provably has none. `&self` so the many `&Interpreter` builder helpers need
    /// no signature change.
    pub fn set_property_raw(
        &self,
        obj: &JsValue,
        key: &str,
        value: JsValue,
    ) -> Result<(), RuntimeError> {
        match obj {
            JsValue::Object(o) => {
                // Host objects observe writes live (the DOM binding reflects
                // `el.textContent = 'new'` into the real tree + marks it dirty). If the host
                // declines the key, fall through to a plain own-property store so script can
                // attach arbitrary fields.
                let host = match &o.borrow().internal {
                    Some(crate::builtins_collections::Internal::Host(h)) => Some(h.clone()),
                    _ => None,
                };
                if let Some(h) = host {
                    if h.host_set(key, &value) {
                        return Ok(());
                    }
                }
                o.borrow_mut().set_own(key, value)
            }
            JsValue::Array(a) => {
                if key == "length" {
                    let n = primitive_to_number(&value);
                    if n.is_finite() && n >= 0.0 {
                        let new_len = n as usize;
                        if new_len > MAX_ARRAY_LEN {
                            return Err(RuntimeError::new(
                                ErrorKind::RangeError,
                                "Invalid array length",
                            ));
                        }
                        a.borrow_mut().items.resize(new_len, JsValue::Undefined);
                    }
                    return Ok(());
                }
                if let Ok(idx) = key.parse::<usize>() {
                    if idx >= MAX_ARRAY_LEN {
                        return Err(RuntimeError::new(
                            ErrorKind::RangeError,
                            "array index budget exceeded",
                        ));
                    }
                    let mut b = a.borrow_mut();
                    if idx >= b.items.len() {
                        b.items.resize(idx + 1, JsValue::Undefined);
                    }
                    b.items[idx] = value;
                    return Ok(());
                }
                let mut b = a.borrow_mut();
                if let Some(slot) = b.props.iter_mut().find(|(k, _)| k == key) {
                    slot.1 = value;
                } else {
                    b.props.push((key.to_string(), value));
                }
                Ok(())
            }
            JsValue::Function(f) => {
                let mut props = f.props.borrow_mut();
                if let Some(slot) = props.iter_mut().find(|(k, _)| k == key) {
                    slot.1 = value;
                } else {
                    props.push((key.to_string(), value));
                }
                Ok(())
            }
            // Writes to primitives are silently ignored (sloppy mode).
            _ => Ok(()),
        }
    }

    fn set_function_prop(&self, func: &JsValue, key: &str, value: JsValue) {
        if let JsValue::Function(f) = func {
            let mut props = f.props.borrow_mut();
            if let Some(slot) = props.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                props.push((key.to_string(), value));
            }
        }
    }

    fn delete_property(&self, obj: &JsValue, key: &str) -> bool {
        match obj {
            JsValue::Object(o) => o.borrow_mut().delete(key),
            JsValue::Array(a) => {
                if let Ok(idx) = key.parse::<usize>() {
                    let mut b = a.borrow_mut();
                    if idx < b.items.len() {
                        b.items[idx] = JsValue::Undefined;
                    }
                    return true;
                }
                let mut b = a.borrow_mut();
                if let Some(p) = b.props.iter().position(|(k, _)| k == key) {
                    b.props.remove(p);
                }
                true
            }
            _ => true,
        }
    }

    /// `key in obj` / property existence (own + prototype chain).
    pub fn has_property(&self, obj: &JsValue, key: &str) -> bool {
        match obj {
            JsValue::Object(o) => {
                let mut cur = Some(JsValue::Object(o.clone()));
                let mut guard = 0;
                while let Some(JsValue::Object(co)) = cur {
                    guard += 1;
                    if guard > 10_000 {
                        break;
                    }
                    let next = {
                        let b = co.borrow();
                        if b.has_own(key) {
                            return true;
                        }
                        b.proto.clone()
                    };
                    cur = next;
                }
                false
            }
            JsValue::Array(a) => {
                if key == "length" {
                    return true;
                }
                if let Ok(idx) = key.parse::<usize>() {
                    return idx < a.borrow().items.len();
                }
                a.borrow().props.iter().any(|(k, _)| k == key)
            }
            _ => false,
        }
    }

    pub(crate) fn get_prototype(&self, obj: &JsValue) -> Option<JsValue> {
        match obj {
            JsValue::Object(o) => o.borrow().proto.clone(),
            _ => None,
        }
    }

    fn string_property(&mut self, s: &Rc<String>, key: &str) -> JsValue {
        if key == "length" {
            return JsValue::Number(s.chars().count() as f64);
        }
        if let Ok(idx) = key.parse::<usize>() {
            return match s.chars().nth(idx) {
                Some(c) => JsValue::str(c.to_string()),
                None => JsValue::Undefined,
            };
        }
        crate::builtins::string_property(self, key)
    }

    /// Own enumerable string keys, in order (for `for-in`, spread, `Object.keys`).
    pub fn enumerate_keys(&self, obj: &JsValue) -> Vec<String> {
        match obj {
            JsValue::Object(o) => {
                let b = o.borrow();
                // Host objects advertise their enumerable keys; precede the object's own
                // plain props (which a script may have stashed) so `for-in` sees both.
                let mut keys: Vec<String> = match &b.internal {
                    Some(crate::builtins_collections::Internal::Host(h)) => h.host_keys(),
                    _ => Vec::new(),
                };
                for (k, _) in b.props.iter() {
                    if !keys.iter().any(|existing| existing == k) {
                        keys.push(k.clone());
                    }
                }
                // Accessor properties are enumerable too (object-literal/class
                // getters appear in `Object.keys`/`for-in`, and `JSON.stringify`
                // then invokes the getter for the value).
                for (k, _, _) in b.accessors.iter() {
                    if !keys.iter().any(|existing| existing == k) {
                        keys.push(k.clone());
                    }
                }
                keys
            }
            JsValue::Array(a) => {
                let b = a.borrow();
                let mut keys: Vec<String> = (0..b.items.len()).map(|i| i.to_string()).collect();
                keys.extend(b.props.iter().map(|(k, _)| k.clone()));
                keys
            }
            _ => Vec::new(),
        }
    }

    /// Produce the element sequence of an iterable (array, string, or array-like object).
    pub fn iterate(&mut self, v: &JsValue) -> Result<Vec<JsValue>, RuntimeError> {
        match v {
            JsValue::Array(a) => Ok(a.borrow().items.clone()),
            JsValue::String(s) => Ok(s.chars().map(|c| JsValue::str(c.to_string())).collect()),
            JsValue::Object(o) => {
                // Map/Set carry their iteration order in an internal slot; yield it
                // directly (Map → [k,v] pairs, Set → values) so for-of / spread work.
                if let Some(internal) = o.borrow().internal.clone() {
                    if let Some(seq) =
                        crate::builtins_collections::iterate_internal(self, &internal)
                    {
                        return Ok(seq);
                    }
                }
                // Array-like (has numeric length) fallback.
                let len_v = self.get_property(v, "length")?;
                if let JsValue::Number(n) = len_v {
                    if n.is_finite() && n >= 0.0 {
                        let len = n as usize;
                        let mut out = Vec::new();
                        for i in 0..len.min(MAX_ARRAY_LEN) {
                            out.push(self.get_property(v, &i.to_string())?);
                        }
                        return Ok(out);
                    }
                }
                Err(RuntimeError::new(
                    ErrorKind::TypeError,
                    "value is not iterable",
                ))
            }
            JsValue::Undefined | JsValue::Null => Err(RuntimeError::new(
                ErrorKind::TypeError,
                format!("{} is not iterable", v.type_of()),
            )),
            _ => Err(RuntimeError::new(
                ErrorKind::TypeError,
                "value is not iterable",
            )),
        }
    }

    // ── Coercions that may call user code (ToPrimitive / ToString / ToNumber) ──

    /// `ToPrimitive` (number hint default): for objects, try `valueOf` then `toString`;
    /// arrays stringify to a comma-joined list; primitives pass through.
    pub fn to_primitive(&mut self, v: &JsValue) -> Result<JsValue, RuntimeError> {
        match v {
            JsValue::Object(_) => {
                // valueOf
                let vo = self.get_property(v, "valueOf")?;
                if vo.is_callable() {
                    let r = self.call_function(&vo, v, &[])?;
                    if !matches!(
                        r,
                        JsValue::Object(_) | JsValue::Array(_) | JsValue::Function(_)
                    ) {
                        return Ok(r);
                    }
                }
                let ts = self.get_property(v, "toString")?;
                if ts.is_callable() {
                    let r = self.call_function(&ts, v, &[])?;
                    if !matches!(
                        r,
                        JsValue::Object(_) | JsValue::Array(_) | JsValue::Function(_)
                    ) {
                        return Ok(r);
                    }
                }
                Ok(JsValue::str("[object Object]"))
            }
            JsValue::Array(a) => {
                // Array ToPrimitive → join with commas.
                let items = a.borrow().items.clone();
                let mut parts = Vec::with_capacity(items.len());
                for it in &items {
                    parts.push(match it {
                        JsValue::Undefined | JsValue::Null => String::new(),
                        other => self.to_string(other)?,
                    });
                }
                Ok(JsValue::str(parts.join(",")))
            }
            JsValue::Function(_) => Ok(JsValue::str("function")),
            other => Ok(other.clone()),
        }
    }

    /// `ToNumber` (object → ToPrimitive first).
    pub fn to_number(&mut self, v: &JsValue) -> Result<f64, RuntimeError> {
        match v {
            JsValue::Object(_) | JsValue::Array(_) | JsValue::Function(_) => {
                let p = self.to_primitive(v)?;
                Ok(primitive_to_number(&p))
            }
            other => Ok(primitive_to_number(other)),
        }
    }

    /// `ToString` (object → ToPrimitive with string preference, simplified).
    pub fn to_string(&mut self, v: &JsValue) -> Result<String, RuntimeError> {
        Ok(match v {
            JsValue::Undefined => "undefined".to_string(),
            JsValue::Null => "null".to_string(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Number(n) => number_to_string(*n),
            JsValue::String(s) => s.to_string(),
            JsValue::Object(o) => {
                // Prefer a user toString.
                let ts = self.get_property(v, "toString")?;
                if ts.is_callable() {
                    let r = self.call_function(&ts, v, &[])?;
                    if let JsValue::String(s) = &r {
                        return Ok(s.to_string());
                    }
                    if !matches!(r, JsValue::Object(_) | JsValue::Array(_)) {
                        return self.to_string(&r);
                    }
                }
                let _ = o;
                "[object Object]".to_string()
            }
            JsValue::Array(_) => {
                let p = self.to_primitive(v)?;
                self.to_string(&p)?
            }
            JsValue::Function(f) => format!("function {}() {{ [native code] }}", f.name),
        })
    }
}

// ─── Error constructors (native) ───────────────────────────────────────────────

fn error_ctor_impl(
    it: &mut Interpreter,
    this: &JsValue,
    args: &[JsValue],
    kind: ErrorKind,
) -> Result<JsValue, RuntimeError> {
    let message = match args.first() {
        Some(JsValue::Undefined) | None => String::new(),
        Some(v) => it.to_string(v)?,
    };
    // If called as `new Error(...)`, `this` is the fresh instance; populate it.
    match this {
        JsValue::Object(o) => {
            {
                let mut b = o.borrow_mut();
                b.class_name = Some(kind.name().to_string());
            }
            it.set_property(this, "name", JsValue::str(kind.name()))?;
            it.set_property(this, "message", JsValue::str(message.clone()))?;
            it.set_property(
                this,
                "stack",
                JsValue::str(format!("{}: {}", kind.name(), message)),
            )?;
            Ok(this.clone())
        }
        // Called without `new`: return a fresh error object.
        _ => Ok(it.make_error(kind, &message)),
    }
}

fn error_ctor_error(
    it: &mut Interpreter,
    this: &JsValue,
    a: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    error_ctor_impl(it, this, a, ErrorKind::Error)
}
fn error_ctor_type(
    it: &mut Interpreter,
    this: &JsValue,
    a: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    error_ctor_impl(it, this, a, ErrorKind::TypeError)
}
fn error_ctor_range(
    it: &mut Interpreter,
    this: &JsValue,
    a: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    error_ctor_impl(it, this, a, ErrorKind::RangeError)
}
fn error_ctor_reference(
    it: &mut Interpreter,
    this: &JsValue,
    a: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    error_ctor_impl(it, this, a, ErrorKind::ReferenceError)
}
fn error_ctor_syntax(
    it: &mut Interpreter,
    this: &JsValue,
    a: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    error_ctor_impl(it, this, a, ErrorKind::SyntaxError)
}

// Pure-f64 helpers (no_std core has no f64::trunc/fract/abs). Mirror the builtins set.
#[inline]
fn f_abs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}
#[inline]
fn f_trunc(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    x as i64 as f64
}
#[inline]
fn is_integer_valued(x: f64) -> bool {
    x.is_finite() && f_trunc(x) == x
}

/// Numeric `%` with JS semantics (truncated remainder, NaN on 0 divisor / non-finite).
fn js_mod(a: f64, b: f64) -> f64 {
    if b == 0.0 || a.is_infinite() || b.is_nan() || a.is_nan() {
        return f64::NAN;
    }
    if b.is_infinite() {
        return a;
    }
    a - b * f_trunc(a / b)
}

/// Best-effort conversion of an assignment-target expression into a destructuring
/// [`Pattern`] (so `[a, b] = arr` and `({x} = o)` work). Returns None if not a pattern.
fn expr_to_pattern(expr: &Expr) -> Option<Pattern> {
    match expr {
        Expr::Array(elems) => {
            let mut elements = Vec::new();
            let mut rest = None;
            for slot in elems {
                match slot {
                    None => elements.push(None),
                    Some(ArrayElement::Expr(e)) => elements.push(Some(expr_to_pattern(e)?)),
                    Some(ArrayElement::Spread(e)) => {
                        rest = Some(Box::new(expr_to_pattern(e)?));
                    }
                }
            }
            Some(Pattern::Array { elements, rest })
        }
        Expr::Object(props) => {
            let mut properties = Vec::new();
            let mut rest = None;
            for p in props {
                match p {
                    ObjectProp::Shorthand(name) => properties.push(ObjectPatternProp {
                        key: PropertyKey::Ident(name.clone()),
                        value: Pattern::Ident(name.clone()),
                        shorthand: true,
                    }),
                    ObjectProp::KeyValue { key, value } => properties.push(ObjectPatternProp {
                        key: key.clone(),
                        value: expr_to_pattern(value)?,
                        shorthand: false,
                    }),
                    ObjectProp::Spread(e) => rest = Some(Box::new(expr_to_pattern(e)?)),
                    ObjectProp::Method { .. } => return None,
                }
            }
            Some(Pattern::Object { properties, rest })
        }
        Expr::Ident(name) => Some(Pattern::Ident(name.clone())),
        Expr::Member { .. } => Some(Pattern::Member(Box::new(expr.clone()))),
        Expr::Assign {
            op: AssignOp::Assign,
            target,
            value,
        } => Some(Pattern::Default {
            target: Box::new(expr_to_pattern(target)?),
            default: value.clone(),
        }),
        _ => None,
    }
}

// ─── Coercions (ToBoolean / ToNumber / ToString / ToPrimitive / ToInt32) ──────

/// ECMAScript `ToBoolean`.
pub fn to_boolean(v: &JsValue) -> bool {
    match v {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(b) => *b,
        JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
        JsValue::String(s) => !s.is_empty(),
        JsValue::Object(_) | JsValue::Array(_) | JsValue::Function(_) => true,
    }
}

/// Render an `f64` the way JS `String(n)` does for the common cases (integers without a
/// trailing `.0`, NaN/Infinity words). Not bit-exact to V8's shortest-round-trip for all
/// fractions, but correct for integers and typical decimals — documented limitation.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if n == 0.0 {
        return "0".to_string();
    }
    if is_integer_valued(n) && f_abs(n) < 1e21 {
        // Integer-valued: print without a decimal point.
        return format!("{}", n as i64);
    }
    // Fallback: Rust's default float formatting (shortest round-trip for f64).
    let s = format!("{}", n);
    s
}

/// ECMAScript `ToNumber` for primitives (object→primitive handled by the caller).
fn primitive_to_number(v: &JsValue) -> f64 {
    match v {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(true) => 1.0,
        JsValue::Bool(false) => 0.0,
        JsValue::Number(n) => *n,
        JsValue::String(s) => string_to_number(s),
        // Objects/arrays/functions reach here only after ToPrimitive in the caller.
        _ => f64::NAN,
    }
}

/// ECMAScript `StringToNumber`: trims whitespace, supports decimal/hex/octal/binary,
/// `Infinity`, empty→0.
fn string_to_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    if t == "Infinity" || t == "+Infinity" {
        return f64::INFINITY;
    }
    if t == "-Infinity" {
        return f64::NEG_INFINITY;
    }
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(oct) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        return i64::from_str_radix(oct, 8)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if let Some(bin) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        return i64::from_str_radix(bin, 2)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

/// ECMAScript `ToInt32` (used by the bitwise operators).
pub fn to_int32(n: f64) -> i32 {
    to_uint32(n) as i32
}

/// ECMAScript `ToUint32`. Takes `ToNumber` mod 2^32 via i64 wrapping (no `rem_euclid`,
/// which is a `std` float method unavailable in `no_std` core).
pub fn to_uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    let t = f_trunc(n);
    // Values beyond i64 range can't be represented; clamp to 0 (matches the spec result
    // for the magnitudes a script realistically bit-twiddles).
    if f_abs(t) >= 9.223_372_036_854_776e18 {
        return 0;
    }
    (t as i64 as u64 & 0xFFFF_FFFF) as u32
}

#[cfg(test)]
#[path = "interp_tests.rs"]
mod interp_tests;
