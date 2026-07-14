//! # RaeJs — a never-panic, `no_std` ECMAScript LEXER + PARSER → AST.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy (criterion #5 — "the web browser is the
//! universal app runtime; PWAs that feel native"): [`athweb`](../athweb) already parses
//! and lays out HTML/CSS and paints through `athgfx`, but it has **no JavaScript**, so a
//! page is a static document — no buttons that do anything, no PWA. JavaScript is "the
//! biggest missing app" between RaeWeb and a real browser. This crate is the **language
//! front-end** that gap starts with.
//!
//! ## What this slice IS (honestly scoped)
//! A correct, host-KAT-proven **lexer + parser** that turns ECMAScript source text into
//! an Abstract Syntax Tree ([`Program`]). It covers ES5 core plus the common ES2015+
//! additions a real page uses:
//!   - **Lexer**: identifiers, all keywords, the full punctuator/operator set
//!     (incl. `=== !== == != <= >= && || ?? ?. << >> >>> & | ^ ~`, the compound
//!     assignments `+= … **= &&= ||= ??=`, `++ -- => ...`), numeric literals
//!     (decimal / `0x` / `0o` / `0b` / floats / exponents / `_` separators / `BigInt`
//!     `n` suffix), string literals with escapes (`\n \t \\ \" \uXXXX \u{…} \xHH`),
//!     **template literals** (`` `…${expr}…` `` with nesting), **regex literals**
//!     (`/…/flags`, disambiguated from division by the previous-token heuristic),
//!     `//` and `/* */` comments, and the newlines that drive **ASI**.
//!   - **Parser** (recursive descent + Pratt expression parsing with the correct
//!     ECMAScript precedence/associativity): every expression form (literals, arrays,
//!     objects with shorthand/computed/spread, function & arrow expressions, member /
//!     optional-chaining / call / `new`, all unary/binary/logical/nullish/conditional/
//!     assignment/sequence, template literals) and every statement form (`var`/`let`/
//!     `const` with array & object **destructuring**, `if`/`else`, `for` C-style /
//!     `for-in` / `for-of`, `while`, `do-while`, `switch`, `break`/`continue` with
//!     labels, `return`, `throw`, `try`/`catch`/`finally`, function & class
//!     declarations, labeled & empty statements, blocks).
//!
//! ## What this slice is NOT (deferred to later slices — stated up front)
//! There is **no interpreter, no runtime, no values, no DOM, no event loop** here. This
//! crate does not *run* JavaScript; it only proves the text is well-formed and exposes
//! its structure. The next slice is a **tree-walking interpreter** over this AST; DOM
//! bindings (wiring [`athweb`]'s DOM to the interpreter) and the event loop come after
//! that. Parser coverage is the common language, not every ES2022 corner: getters/
//! setters in object literals, generators/`async`/`await` bodies, and `with` are
//! recognized only as far as their statement/expression *shape* (see the per-feature
//! notes on [`Stmt`]/[`Expr`]); private `#fields` and decorators are out of scope.
//!
//! ## Safety property (never-panic / never-hang — load-bearing)
//! Every byte is treated as hostile. `#![forbid(unsafe_code)]`; no `unwrap`/`expect`/
//! raw-index panic is reachable from [`parse`]. Source longer than [`MAX_SOURCE_LEN`],
//! more than [`MAX_TOKENS`] tokens, or nesting deeper than [`MAX_DEPTH`] is rejected with
//! a positioned [`JsError`] rather than overflowing the stack or looping. A seeded fuzz
//! over arbitrary source strings (see the tests) must never panic. Run the FAIL-able KATs
//! with `cargo test -p ath_js` — no QEMU, no image build.
//!
//! ```
//! let program = ath_js::parse("var x = 1 + 2 * 3;").expect("valid");
//! assert_eq!(program.body.len(), 1);
//! ```

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

pub mod builtins;
pub mod builtins_async;
pub mod builtins_collections;
pub mod builtins_regexp;
pub mod host;
pub mod interp;
pub mod lexer;
pub mod parser;

pub use host::{HostFn, HostObject};
pub use interp::{native_function_value, ErrorKind, Interpreter, JsValue, RuntimeError};
pub use lexer::{Token, TokenKind};

// ─── Bounds (never-hang / never-OOM) ─────────────────────────────────────────

/// Longest source text the lexer will accept. Past this → [`JsError`] (not parsed).
pub const MAX_SOURCE_LEN: usize = 4 * 1024 * 1024; // 4 MiB

/// Largest number of tokens a source may produce. Past this → [`JsError`].
pub const MAX_TOKENS: usize = 2_000_000;

/// Deepest expression/statement nesting the parser will descend. Checked on every
/// recursive entry, so a crafted `((((…))))` cannot overflow the stack → [`JsError`].
/// Kept conservative: a single source nesting level expands to several physical parser
/// frames (assign → conditional → binary → unary → lhs → primary), so the effective
/// stack depth is a multiple of this; 160 leaves generous headroom under a default
/// 1–2 MiB thread stack while comfortably exceeding any hand-written nesting.
pub const MAX_DEPTH: usize = 160;

/// Largest number of AST nodes a parse may allocate. Past this → [`JsError`].
pub const MAX_NODES: usize = 4_000_000;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// A parse/lex failure carrying a human-readable message and a source position.
///
/// `pos` is a byte offset into the original source (`line`/`col` are 1-based). Every
/// failure path in this crate returns one of these — never a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsError {
    pub message: String,
    pub pos: usize,
    pub line: usize,
    pub col: usize,
}

impl JsError {
    pub(crate) fn new(message: impl Into<String>, pos: usize, line: usize, col: usize) -> Self {
        JsError {
            message: message.into(),
            pos,
            line,
            col,
        }
    }
}

impl core::fmt::Display for JsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} (line {}, col {})", self.message, self.line, self.col)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  AST  (ESTree-flavored, but our own owned types)
// ═══════════════════════════════════════════════════════════════════════════

/// The root of a parsed source: an ordered list of top-level statements.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub body: Vec<Stmt>,
}

/// A binding/assignment **target** or destructuring **pattern**.
///
/// A plain [`Pattern::Ident`] is the common `var x`; the array/object forms model the
/// ES2015 destructuring patterns (`const [a, b] = …`, `const {a, b} = …`), including a
/// rest element and defaults.
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `x`
    Ident(String),
    /// `[a, b, ...rest]` — `None` slots are elisions (`[, x]`).
    Array {
        elements: Vec<Option<Pattern>>,
        rest: Option<Box<Pattern>>,
    },
    /// `{a, b: c, ...rest}`
    Object {
        properties: Vec<ObjectPatternProp>,
        rest: Option<Box<Pattern>>,
    },
    /// `a = default` (a pattern with a default initializer).
    Default {
        target: Box<Pattern>,
        default: Box<Expr>,
    },
    /// A member expression used as an assignment target (`obj.x = …`, `a[i] = …`).
    /// Only valid in assignment positions, not in declarations.
    Member(Box<Expr>),
}

/// One property of an object destructuring pattern: `key: value`, where `value` is the
/// nested pattern (shorthand `{a}` has `key == value == Ident("a")`).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPatternProp {
    pub key: PropertyKey,
    pub value: Pattern,
    /// `{a}` shorthand vs `{a: b}`.
    pub shorthand: bool,
}

/// A statement node.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `var`/`let`/`const` declaration with one or more declarators.
    VarDecl {
        kind: VarKind,
        declarations: Vec<VarDeclarator>,
    },
    /// A bare expression used as a statement.
    Expr(Expr),
    /// `{ … }`
    Block(Vec<Stmt>),
    /// `;`
    Empty,
    /// `if (test) consequent else alternate`
    If {
        test: Expr,
        consequent: Box<Stmt>,
        alternate: Option<Box<Stmt>>,
    },
    /// C-style `for (init; test; update) body`.
    For {
        init: Option<Box<ForInit>>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    /// `for (left in right) body`
    ForIn {
        left: Box<ForInit>,
        right: Expr,
        body: Box<Stmt>,
    },
    /// `for (left of right) body`
    ForOf {
        left: Box<ForInit>,
        right: Expr,
        body: Box<Stmt>,
    },
    /// `while (test) body`
    While { test: Expr, body: Box<Stmt> },
    /// `do body while (test)`
    DoWhile { body: Box<Stmt>, test: Expr },
    /// `switch (disc) { case … default … }`
    Switch {
        discriminant: Expr,
        cases: Vec<SwitchCase>,
    },
    /// `break;` / `break label;`
    Break(Option<String>),
    /// `continue;` / `continue label;`
    Continue(Option<String>),
    /// `return;` / `return expr;`
    Return(Option<Expr>),
    /// `throw expr;`
    Throw(Expr),
    /// `try { … } catch (e) { … } finally { … }`
    Try {
        block: Vec<Stmt>,
        handler: Option<CatchClause>,
        finalizer: Option<Vec<Stmt>>,
    },
    /// `function name(params) { body }`
    FunctionDecl(Function),
    /// `class Name extends Super { … }`
    ClassDecl(Class),
    /// `label: stmt`
    Labeled { label: String, body: Box<Stmt> },
}

/// The `init` clause of a `for` loop: either a declaration or an expression.
#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    VarDecl {
        kind: VarKind,
        declarations: Vec<VarDeclarator>,
    },
    Expr(Expr),
    /// A bare pattern (`for (x of …)` where `x` is already declared).
    Pattern(Pattern),
}

/// `var` / `let` / `const`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

/// One `target = init` inside a declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct VarDeclarator {
    pub target: Pattern,
    pub init: Option<Expr>,
}

/// One `case x:` / `default:` arm of a `switch`.
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    /// `None` for the `default:` arm.
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

/// The `catch (param) { … }` clause; `param` is optional (ES2019 optional binding).
#[derive(Debug, Clone, PartialEq)]
pub struct CatchClause {
    pub param: Option<Pattern>,
    pub body: Vec<Stmt>,
}

/// A function definition (declaration, expression, or arrow body share this shape).
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// `None` for anonymous function expressions / arrows.
    pub name: Option<String>,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    pub is_arrow: bool,
    /// `function*` generator (body still parses; semantics are a later slice).
    pub is_generator: bool,
    /// `async function` (recognized; semantics are a later slice).
    pub is_async: bool,
    /// For a concise arrow body (`x => x + 1`) this holds the expression; `body` is empty.
    pub arrow_expr: Option<Box<Expr>>,
}

/// One function parameter: a pattern, optionally a rest param (`...args`).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub pattern: Pattern,
    pub rest: bool,
}

/// A class definition (declaration or expression).
#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    pub name: Option<String>,
    pub super_class: Option<Box<Expr>>,
    pub members: Vec<ClassMember>,
}

/// One member of a class body: a method (incl. constructor / getter / setter) or a
/// field. Method/field *shape* is captured; full semantics are a later slice.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassMember {
    pub key: PropertyKey,
    pub kind: ClassMemberKind,
    pub is_static: bool,
    /// For a method: the function. For a field: `None`.
    pub value: Option<Function>,
    /// For a field with an initializer.
    pub field_init: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassMemberKind {
    Method,
    Constructor,
    Getter,
    Setter,
    Field,
}

/// A property key: an identifier/string/number name, or a `[computed]` expression.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyKey {
    Ident(String),
    String(String),
    Number(f64),
    Computed(Box<Expr>),
}

/// An expression node.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A numeric literal (BigInt `n`-suffixed literals carry their `f64` approximation
    /// plus the original text in [`Expr::BigInt`]).
    Number(f64),
    /// A `…n` BigInt literal — the digit text without the `n` (no bignum in this slice).
    BigInt(String),
    /// A string literal with escapes already decoded.
    String(String),
    /// `true` / `false`
    Bool(bool),
    /// `null`
    Null,
    /// `undefined` (lexed as an identifier; surfaced distinctly for convenience).
    Undefined,
    /// An identifier reference.
    Ident(String),
    /// `this`
    This,
    /// `super`
    Super,
    /// A regex literal `/pattern/flags`.
    Regex { pattern: String, flags: String },
    /// A template literal: `quasis` are the cooked string chunks, `expressions` the
    /// `${…}` parts. `quasis.len() == expressions.len() + 1`.
    Template {
        quasis: Vec<String>,
        expressions: Vec<Expr>,
    },
    /// A tagged template: `tag`\`…\`.
    TaggedTemplate {
        tag: Box<Expr>,
        quasis: Vec<String>,
        expressions: Vec<Expr>,
    },
    /// `[a, b, ...c]` — `None` slots are elisions.
    Array(Vec<Option<ArrayElement>>),
    /// `{a, b: c, [d]: e, ...f}`
    Object(Vec<ObjectProp>),
    /// A function expression.
    Function(Function),
    /// An arrow function (its [`Function::is_arrow`] is true).
    Arrow(Function),
    /// A class expression.
    Class(Class),
    /// `op operand` (prefix unary: `! - + ~ typeof void delete`).
    Unary { op: UnaryOp, operand: Box<Expr> },
    /// `await operand`. Modeled as its own node (NOT `void`, which discards the value):
    /// `await v` evaluates to `v` itself when `v` is not a thenable, and to the resolved
    /// value of an already-settled (or synchronously-settleable) promise. Full
    /// async-function suspension is deferred; see [`crate::interp`].
    Await { operand: Box<Expr> },
    /// `++x` / `--x` (prefix) and `x++` / `x--` (postfix).
    Update {
        op: UpdateOp,
        prefix: bool,
        operand: Box<Expr>,
    },
    /// A binary expression (arithmetic / comparison / bitwise / `instanceof` / `in`).
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// `a && b`, `a || b`, `a ?? b` (short-circuiting — kept distinct from [`Expr::Binary`]).
    Logical {
        op: LogicalOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// `target op= value` (all compound assignments, incl. `&&= ||= ??=`).
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `test ? consequent : alternate`
    Conditional {
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    /// `object.property` / `object[property]`, optionally an optional-chain link (`?.`).
    Member {
        object: Box<Expr>,
        property: Box<MemberProp>,
        optional: bool,
    },
    /// `callee(args)`, optionally an optional call (`fn?.()`).
    Call {
        callee: Box<Expr>,
        args: Vec<ArrayElement>,
        optional: bool,
    },
    /// `new callee(args)`
    New {
        callee: Box<Expr>,
        args: Vec<ArrayElement>,
    },
    /// `a, b, c`
    Sequence(Vec<Expr>),
    /// `...expr` (spread; valid only as an array element / call arg / object property).
    Spread(Box<Expr>),
}

/// An array element / call argument: either a plain expression or a `...spread`.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayElement {
    Expr(Expr),
    Spread(Expr),
}

/// The property side of a member access.
#[derive(Debug, Clone, PartialEq)]
pub enum MemberProp {
    /// `.name` (a static identifier).
    Ident(String),
    /// `[expr]` (a computed key).
    Computed(Expr),
}

/// One property of an object literal.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectProp {
    /// `key: value`
    KeyValue { key: PropertyKey, value: Expr },
    /// `{a}` shorthand → an identifier name reused as both key and value.
    Shorthand(String),
    /// A method / getter / setter (`{ m() {}, get x() {}, set x(v) {} }`).
    Method {
        key: PropertyKey,
        kind: ClassMemberKind,
        value: Function,
    },
    /// `...expr`
    Spread(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `!`
    Not,
    /// `-`
    Neg,
    /// `+`
    Pos,
    /// `~`
    BitNot,
    /// `typeof`
    Typeof,
    /// `void`
    Void,
    /// `delete`
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOp {
    /// `++`
    Inc,
    /// `--`
    Dec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    /// `**`
    Exp,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `===`
    EqEqEq,
    /// `!==`
    NotEqEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `>>>`
    UShr,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    Instanceof,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    /// `&&`
    And,
    /// `||`
    Or,
    /// `??`
    Nullish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    /// `=`
    Assign,
    /// `+=`
    Add,
    /// `-=`
    Sub,
    /// `*=`
    Mul,
    /// `/=`
    Div,
    /// `%=`
    Mod,
    /// `**=`
    Exp,
    /// `<<=`
    Shl,
    /// `>>=`
    Shr,
    /// `>>>=`
    UShr,
    /// `&=`
    BitAnd,
    /// `|=`
    BitOr,
    /// `^=`
    BitXor,
    /// `&&=`
    And,
    /// `||=`
    Or,
    /// `??=`
    Nullish,
}

// ═══════════════════════════════════════════════════════════════════════════
//  PUBLIC API
// ═══════════════════════════════════════════════════════════════════════════

/// Parse ECMAScript source into a [`Program`] AST.
///
/// Returns `Err(JsError)` with a positioned message on any malformed, truncated,
/// over-nested, or oversized input. **Never panics and never hangs** on any input,
/// valid or hostile (the bounds in [`MAX_SOURCE_LEN`]/[`MAX_TOKENS`]/[`MAX_DEPTH`]/
/// [`MAX_NODES`] guarantee termination).
pub fn parse(src: &str) -> Result<Program, JsError> {
    let tokens = lexer::lex(src)?;
    parser::Parser::new(&tokens).parse_program()
}

/// Tokenize ECMAScript source into the raw token stream (without parsing). Useful for
/// tooling and for testing the lexer in isolation. Same never-panic guarantee.
pub fn tokenize(src: &str) -> Result<Vec<Token>, JsError> {
    lexer::lex(src)
}
