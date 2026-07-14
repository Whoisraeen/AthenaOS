//! Host KATs for the ath_js lexer + parser. FAIL-able by construction: each asserts a
//! concrete AST *shape*, so a precedence/associativity/ASI/regex regression turns a
//! green run red. Run with `cargo test -p ath_js`. (`#![cfg_attr(not(test), no_std)]`
//! gives these tests std; we still avoid `use std::` so the R7 gate stays clean.)

use crate::*;

fn prog(src: &str) -> Program {
    match parse(src) {
        Ok(p) => p,
        Err(e) => panic!("expected `{}` to parse, got error: {}", src, e),
    }
}

fn one_stmt(src: &str) -> Stmt {
    let p = prog(src);
    assert_eq!(
        p.body.len(),
        1,
        "expected exactly one statement in `{}`",
        src
    );
    p.body.into_iter().next().unwrap()
}

fn one_expr(src: &str) -> Expr {
    match one_stmt(src) {
        Stmt::Expr(e) => e,
        other => panic!("expected expression statement, got {:?}", other),
    }
}

// ─── precedence + associativity (the load-bearing correctness proofs) ─────────

#[test]
fn precedence_mul_binds_tighter_than_add() {
    // `var x = 1 + 2 * 3;` must parse as 1 + (2 * 3), NOT (1 + 2) * 3.
    let stmt = one_stmt("var x = 1 + 2 * 3;");
    let init = match stmt {
        Stmt::VarDecl { declarations, .. } => declarations[0].init.clone().unwrap(),
        _ => panic!("not a var decl"),
    };
    // Top must be Add, with right operand a Mul.
    match init {
        Expr::Binary {
            op: BinaryOp::Add,
            left,
            right,
        } => {
            assert_eq!(*left, Expr::Number(1.0));
            match *right {
                Expr::Binary {
                    op: BinaryOp::Mul,
                    left: ml,
                    right: mr,
                } => {
                    assert_eq!(*ml, Expr::Number(2.0));
                    assert_eq!(*mr, Expr::Number(3.0));
                }
                other => panic!("right of + should be *, got {:?}", other),
            }
        }
        // The FAIL-able alternative: a wrong parser yields top=Mul. Reject it loudly.
        Expr::Binary {
            op: BinaryOp::Mul, ..
        } => panic!("WRONG precedence: parsed as (1+2)*3"),
        other => panic!("unexpected top node {:?}", other),
    }
}

#[test]
fn left_assoc_addition() {
    // `1 + 2 + 3` => ((1 + 2) + 3): the top node's LEFT is itself an Add.
    match one_expr("1 + 2 + 3;") {
        Expr::Binary {
            op: BinaryOp::Add,
            left,
            right,
        } => {
            assert_eq!(*right, Expr::Number(3.0));
            assert!(
                matches!(
                    *left,
                    Expr::Binary {
                        op: BinaryOp::Add,
                        ..
                    }
                ),
                "addition must be left-associative"
            );
        }
        other => panic!("unexpected {:?}", other),
    }
}

#[test]
fn right_assoc_exponent() {
    // `2 ** 3 ** 2` => 2 ** (3 ** 2): the top node's RIGHT is itself an Exp.
    match one_expr("2 ** 3 ** 2;") {
        Expr::Binary {
            op: BinaryOp::Exp,
            left,
            right,
        } => {
            assert_eq!(*left, Expr::Number(2.0));
            assert!(
                matches!(
                    *right,
                    Expr::Binary {
                        op: BinaryOp::Exp,
                        ..
                    }
                ),
                "** must be right-associative"
            );
        }
        other => panic!("unexpected {:?}", other),
    }
}

#[test]
fn logical_and_binds_tighter_than_or() {
    // `a || b && c` => a || (b && c).
    match one_expr("a || b && c;") {
        Expr::Logical {
            op: LogicalOp::Or,
            left,
            right,
        } => {
            assert_eq!(*left, Expr::Ident("a".into()));
            assert!(
                matches!(
                    *right,
                    Expr::Logical {
                        op: LogicalOp::And,
                        ..
                    }
                ),
                "&& must bind tighter than ||"
            );
        }
        other => panic!("unexpected {:?}", other),
    }
}

#[test]
fn nullish_coalescing() {
    match one_expr("a ?? b;") {
        Expr::Logical {
            op: LogicalOp::Nullish,
            ..
        } => {}
        other => panic!("expected ?? logical, got {:?}", other),
    }
}

#[test]
fn optional_chaining_chain() {
    // a?.b?.c => Member(optional) over Member(optional) over a.
    match one_expr("a?.b?.c;") {
        Expr::Member {
            object,
            property,
            optional,
        } => {
            assert!(optional, "outer link is optional");
            assert!(matches!(*property, MemberProp::Ident(ref n) if n == "c"));
            match *object {
                Expr::Member {
                    optional: inner_opt,
                    property: inner_prop,
                    ..
                } => {
                    assert!(inner_opt);
                    assert!(matches!(*inner_prop, MemberProp::Ident(ref n) if n == "b"));
                }
                other => panic!("inner not a member: {:?}", other),
            }
        }
        other => panic!("unexpected {:?}", other),
    }
}

#[test]
fn comparison_below_arithmetic() {
    // `1 + 2 < 3 * 4` => (1+2) < (3*4).
    match one_expr("1 + 2 < 3 * 4;") {
        Expr::Binary {
            op: BinaryOp::Lt,
            left,
            right,
        } => {
            assert!(matches!(
                *left,
                Expr::Binary {
                    op: BinaryOp::Add,
                    ..
                }
            ));
            assert!(matches!(
                *right,
                Expr::Binary {
                    op: BinaryOp::Mul,
                    ..
                }
            ));
        }
        other => panic!("unexpected {:?}", other),
    }
}

#[test]
fn assignment_is_right_assoc() {
    // `a = b = c` => a = (b = c).
    match one_expr("a = b = c;") {
        Expr::Assign {
            op: AssignOp::Assign,
            value,
            ..
        } => {
            assert!(
                matches!(*value, Expr::Assign { .. }),
                "assignment right-assoc"
            );
        }
        other => panic!("unexpected {:?}", other),
    }
}

#[test]
fn ternary_conditional() {
    match one_expr("a ? b : c;") {
        Expr::Conditional { .. } => {}
        other => panic!("expected conditional, got {:?}", other),
    }
}

// ─── functions + arrows ───────────────────────────────────────────────────────

#[test]
fn function_declaration() {
    match one_stmt("function add(a, b) { return a + b; }") {
        Stmt::FunctionDecl(f) => {
            assert_eq!(f.name.as_deref(), Some("add"));
            assert_eq!(f.params.len(), 2);
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Stmt::Return(Some(Expr::Binary {
                    op: BinaryOp::Add, ..
                })) => {}
                other => panic!("body not `return a + b`, got {:?}", other),
            }
        }
        other => panic!("not a function decl: {:?}", other),
    }
}

#[test]
fn arrow_concise_body() {
    // const f = (x) => x * 2;
    let stmt = one_stmt("const f = (x) => x * 2;");
    let init = match stmt {
        Stmt::VarDecl { declarations, .. } => declarations[0].init.clone().unwrap(),
        _ => panic!("not var decl"),
    };
    match init {
        Expr::Arrow(f) => {
            assert!(f.is_arrow);
            assert_eq!(f.params.len(), 1);
            assert!(f.body.is_empty());
            match f.arrow_expr {
                Some(b) => assert!(matches!(
                    *b,
                    Expr::Binary {
                        op: BinaryOp::Mul,
                        ..
                    }
                )),
                None => panic!("arrow should have a concise expr body"),
            }
        }
        other => panic!("expected arrow, got {:?}", other),
    }
}

#[test]
fn arrow_object_return_paren() {
    // x => ({a: x}) — the paren makes the body an object literal, not a block.
    match one_expr("x => ({a: x});") {
        Expr::Arrow(f) => match f.arrow_expr {
            Some(b) => assert!(matches!(*b, Expr::Object(_))),
            None => panic!("expected concise object body"),
        },
        other => panic!("expected arrow, got {:?}", other),
    }
}

#[test]
fn arrow_vs_grouped_expr() {
    // `(a, b)` with no `=>` is a sequence expression, not an arrow.
    match one_expr("(a, b);") {
        Expr::Sequence(items) => assert_eq!(items.len(), 2),
        other => panic!("expected sequence, got {:?}", other),
    }
}

// ─── object / array literals + spread ─────────────────────────────────────────

#[test]
fn object_literal_shorthand_computed_spread() {
    match one_expr("({a, b: 1, [c]: 2, ...d});") {
        Expr::Object(props) => {
            assert_eq!(props.len(), 4);
            assert!(matches!(props[0], ObjectProp::Shorthand(ref n) if n == "a"));
            assert!(matches!(props[1], ObjectProp::KeyValue { .. }));
            assert!(matches!(
                props[2],
                ObjectProp::KeyValue {
                    key: PropertyKey::Computed(_),
                    ..
                }
            ));
            assert!(matches!(props[3], ObjectProp::Spread(_)));
        }
        other => panic!("expected object, got {:?}", other),
    }
}

#[test]
fn array_literal_with_spread_and_elision() {
    match one_expr("[1, , ...rest];") {
        Expr::Array(elems) => {
            assert_eq!(elems.len(), 3);
            assert!(matches!(elems[0], Some(ArrayElement::Expr(_))));
            assert!(elems[1].is_none(), "elision slot");
            assert!(matches!(elems[2], Some(ArrayElement::Spread(_))));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

// ─── destructuring ─────────────────────────────────────────────────────────────

#[test]
fn object_destructuring() {
    match one_stmt("const {a, b} = obj;") {
        Stmt::VarDecl { declarations, .. } => match &declarations[0].target {
            Pattern::Object { properties, .. } => {
                assert_eq!(properties.len(), 2);
                assert!(properties[0].shorthand);
            }
            other => panic!("expected object pattern, got {:?}", other),
        },
        other => panic!("not var decl: {:?}", other),
    }
}

#[test]
fn array_destructuring() {
    match one_stmt("const [x, y] = arr;") {
        Stmt::VarDecl { declarations, .. } => match &declarations[0].target {
            Pattern::Array { elements, .. } => assert_eq!(elements.len(), 2),
            other => panic!("expected array pattern, got {:?}", other),
        },
        other => panic!("not var decl: {:?}", other),
    }
}

#[test]
fn nested_destructuring_with_rest_and_default() {
    let stmt = one_stmt("const {a, b: [c, ...d] = []} = obj;");
    assert!(matches!(stmt, Stmt::VarDecl { .. }));
}

// ─── statements ────────────────────────────────────────────────────────────────

#[test]
fn if_else() {
    match one_stmt("if (a) b(); else c();") {
        Stmt::If {
            alternate: Some(_), ..
        } => {}
        other => panic!("expected if/else, got {:?}", other),
    }
}

#[test]
fn for_c_style() {
    match one_stmt("for (var i = 0; i < 10; i++) sum += i;") {
        Stmt::For {
            init: Some(_),
            test: Some(_),
            update: Some(_),
            ..
        } => {}
        other => panic!("expected C-style for, got {:?}", other),
    }
}

#[test]
fn for_of() {
    match one_stmt("for (const x of items) print(x);") {
        Stmt::ForOf { .. } => {}
        other => panic!("expected for-of, got {:?}", other),
    }
}

#[test]
fn for_in() {
    match one_stmt("for (var k in obj) use(k);") {
        Stmt::ForIn { .. } => {}
        other => panic!("expected for-in, got {:?}", other),
    }
}

#[test]
fn while_and_do_while() {
    assert!(matches!(one_stmt("while (x) y();"), Stmt::While { .. }));
    assert!(matches!(
        one_stmt("do y(); while (x);"),
        Stmt::DoWhile { .. }
    ));
}

#[test]
fn try_catch_finally() {
    match one_stmt("try { a(); } catch (e) { b(e); } finally { c(); }") {
        Stmt::Try {
            handler: Some(h),
            finalizer: Some(_),
            ..
        } => {
            assert!(h.param.is_some());
        }
        other => panic!("expected try/catch/finally, got {:?}", other),
    }
}

#[test]
fn switch_statement() {
    let src = "switch (x) { case 1: a(); break; default: b(); }";
    match one_stmt(src) {
        Stmt::Switch { cases, .. } => {
            assert_eq!(cases.len(), 2);
            assert!(cases[0].test.is_some());
            assert!(cases[1].test.is_none(), "default has no test");
        }
        other => panic!("expected switch, got {:?}", other),
    }
}

#[test]
fn class_with_method_and_super() {
    let src = "class Dog extends Animal { constructor(n) { super(n); } bark() { return 'woof'; } static make() {} }";
    match one_stmt(src) {
        Stmt::ClassDecl(c) => {
            assert_eq!(c.name.as_deref(), Some("Dog"));
            assert!(c.super_class.is_some());
            assert_eq!(c.members.len(), 3);
            assert_eq!(c.members[0].kind, ClassMemberKind::Constructor);
            assert_eq!(c.members[1].kind, ClassMemberKind::Method);
            assert!(c.members[2].is_static);
        }
        other => panic!("expected class, got {:?}", other),
    }
}

#[test]
fn labeled_statement() {
    match one_stmt("outer: for (;;) break outer;") {
        Stmt::Labeled { label, .. } => assert_eq!(label, "outer"),
        other => panic!("expected labeled stmt, got {:?}", other),
    }
}

#[test]
fn new_expression() {
    match one_expr("new Foo(1, 2);") {
        Expr::New { args, .. } => assert_eq!(args.len(), 2),
        other => panic!("expected new, got {:?}", other),
    }
}

#[test]
fn unary_operators() {
    assert!(matches!(
        one_expr("typeof x;"),
        Expr::Unary {
            op: UnaryOp::Typeof,
            ..
        }
    ));
    assert!(matches!(
        one_expr("!flag;"),
        Expr::Unary {
            op: UnaryOp::Not,
            ..
        }
    ));
    assert!(matches!(
        one_expr("-n;"),
        Expr::Unary {
            op: UnaryOp::Neg,
            ..
        }
    ));
}

#[test]
fn update_prefix_and_postfix() {
    match one_expr("++x;") {
        Expr::Update { prefix: true, .. } => {}
        other => panic!("expected prefix update, got {:?}", other),
    }
    match one_expr("x++;") {
        Expr::Update { prefix: false, .. } => {}
        other => panic!("expected postfix update, got {:?}", other),
    }
}

// ─── template literals ─────────────────────────────────────────────────────────

#[test]
fn template_literal_quasis_and_exprs() {
    match one_expr("`hi ${name}!`;") {
        Expr::Template {
            quasis,
            expressions,
        } => {
            assert_eq!(quasis.len(), 2, "quasis = expressions + 1");
            assert_eq!(quasis[0], "hi ");
            assert_eq!(quasis[1], "!");
            assert_eq!(expressions.len(), 1);
            assert_eq!(expressions[0], Expr::Ident("name".into()));
        }
        other => panic!("expected template, got {:?}", other),
    }
}

#[test]
fn template_with_nested_expression() {
    // The ${} hole contains an object/braces — must not close the template early.
    match one_expr("`v=${a + (b * 2)}`;") {
        Expr::Template { expressions, .. } => {
            assert!(matches!(
                expressions[0],
                Expr::Binary {
                    op: BinaryOp::Add,
                    ..
                }
            ));
        }
        other => panic!("expected template, got {:?}", other),
    }
}

// ─── regex vs division disambiguation (the classic) ───────────────────────────

#[test]
fn regex_literal_after_assignment() {
    // `a = /ab+c/g;` — after `=` an operand is expected → regex.
    match one_expr("a = /ab+c/g;") {
        Expr::Assign { value, .. } => match *value {
            Expr::Regex { pattern, flags } => {
                assert_eq!(pattern, "ab+c");
                assert_eq!(flags, "g");
            }
            other => panic!("expected regex value, got {:?}", other),
        },
        other => panic!("expected assignment, got {:?}", other),
    }
}

#[test]
fn division_not_regex() {
    // `a = b / c / d;` — after an identifier a value just ended → division.
    match one_expr("a = b / c / d;") {
        Expr::Assign { value, .. } => match *value {
            Expr::Binary {
                op: BinaryOp::Div,
                left,
                ..
            } => {
                // left-assoc division: ((b / c) / d) → left is itself a Div.
                assert!(matches!(
                    *left,
                    Expr::Binary {
                        op: BinaryOp::Div,
                        ..
                    }
                ));
            }
            other => panic!("expected division, got {:?}", other),
        },
        other => panic!("expected assignment, got {:?}", other),
    }
}

#[test]
fn regex_with_class_containing_slash() {
    // `/[/]/` — the `/` inside a char class must not terminate the regex.
    match one_expr("x = /[/]/;") {
        Expr::Assign { value, .. } => {
            assert!(matches!(*value, Expr::Regex { .. }));
        }
        other => panic!("unexpected {:?}", other),
    }
}

// ─── ASI (automatic semicolon insertion) ──────────────────────────────────────

#[test]
fn asi_return_newline_yields_undefined() {
    // `return\n5` — ASI inserts `;` after return; `5` is a separate statement.
    let src = "function f() {\n  return\n  5\n}";
    match one_stmt(src) {
        Stmt::FunctionDecl(f) => {
            assert_eq!(f.body.len(), 2, "return; and 5; are two statements");
            match &f.body[0] {
                Stmt::Return(None) => {}
                other => panic!("return should have no argument, got {:?}", other),
            }
            assert!(matches!(f.body[1], Stmt::Expr(Expr::Number(_))));
        }
        other => panic!("expected function, got {:?}", other),
    }
}

#[test]
fn return_same_line_has_argument() {
    let src = "function f() { return 5 }";
    match one_stmt(src) {
        Stmt::FunctionDecl(f) => match &f.body[0] {
            Stmt::Return(Some(Expr::Number(n))) => assert_eq!(*n, 5.0),
            other => panic!("expected `return 5`, got {:?}", other),
        },
        other => panic!("expected function, got {:?}", other),
    }
}

#[test]
fn asi_two_statements_across_newline() {
    // `a\nb` — two expression statements (ASI), not `a b`.
    let p = prog("a\nb");
    assert_eq!(p.body.len(), 2);
    assert!(matches!(p.body[0], Stmt::Expr(Expr::Ident(_))));
    assert!(matches!(p.body[1], Stmt::Expr(Expr::Ident(_))));
}

// ─── comments + literal decoding ──────────────────────────────────────────────

#[test]
fn comments_are_stripped() {
    let p = prog("// line\n/* block */ var x = 1; // trailing");
    assert_eq!(p.body.len(), 1);
    assert!(matches!(p.body[0], Stmt::VarDecl { .. }));
}

#[test]
fn string_escapes_decode() {
    match one_expr(r#""a\nb\tA\x42";"#) {
        Expr::String(s) => assert_eq!(s, "a\nb\tAB"),
        other => panic!("expected string, got {:?}", other),
    }
}

#[test]
fn numeric_literal_forms() {
    assert_eq!(num("0xFF;"), 255.0);
    assert_eq!(num("0o17;"), 15.0);
    assert_eq!(num("0b1010;"), 10.0);
    assert_eq!(num("1.5e3;"), 1500.0);
    assert_eq!(num("1_000;"), 1000.0);
    assert_eq!(num(".25;"), 0.25);
}

fn num(src: &str) -> f64 {
    match one_expr(src) {
        Expr::Number(n) => n,
        other => panic!("expected number for `{}`, got {:?}", src, other),
    }
}

#[test]
fn bigint_literal() {
    match one_expr("123n;") {
        Expr::BigInt(s) => assert_eq!(s, "123"),
        other => panic!("expected bigint, got {:?}", other),
    }
}

#[test]
fn null_true_false_undefined_this() {
    assert_eq!(one_expr("null;"), Expr::Null);
    assert_eq!(one_expr("true;"), Expr::Bool(true));
    assert_eq!(one_expr("false;"), Expr::Bool(false));
    assert_eq!(one_expr("undefined;"), Expr::Undefined);
    assert_eq!(one_expr("this;"), Expr::This);
}

#[test]
fn compound_assignments() {
    let cases = [
        ("a += 1;", AssignOp::Add),
        ("a -= 1;", AssignOp::Sub),
        ("a **= 2;", AssignOp::Exp),
        ("a >>>= 1;", AssignOp::UShr),
        ("a &&= b;", AssignOp::And),
        ("a ||= b;", AssignOp::Or),
        ("a ??= b;", AssignOp::Nullish),
    ];
    for (src, want) in cases {
        match one_expr(src) {
            Expr::Assign { op, .. } => assert_eq!(op, want, "for `{}`", src),
            other => panic!("expected assign for `{}`, got {:?}", src, other),
        }
    }
}

#[test]
fn member_and_call_chain() {
    // a.b.c(d)[e]
    match one_expr("a.b.c(d)[e];") {
        Expr::Member {
            property,
            optional: false,
            ..
        } => {
            assert!(matches!(*property, MemberProp::Computed(_)));
        }
        other => panic!("expected member access at top, got {:?}", other),
    }
}

// ─── hostile / never-panic / never-hang ────────────────────────────────────────

#[test]
fn deeply_nested_parens_bounded_not_overflow() {
    let mut s = String::new();
    for _ in 0..10_000 {
        s.push('(');
    }
    s.push('1');
    for _ in 0..10_000 {
        s.push(')');
    }
    // Must return an Err (depth bound) — NOT overflow the stack, NOT hang.
    assert!(parse(&s).is_err(), "deep nesting must be a bounded Err");
}

#[test]
fn unterminated_string_is_err() {
    assert!(parse("var x = \"oops").is_err());
}

#[test]
fn unterminated_block_comment_is_err() {
    assert!(parse("/* never ends").is_err());
}

#[test]
fn unterminated_regex_is_err() {
    assert!(parse("a = /unterminated").is_err());
}

#[test]
fn unterminated_template_is_err() {
    assert!(parse("`open ${a").is_err());
}

#[test]
fn truncated_inputs_never_panic() {
    let full = "function f(a, b) { return a.b.c(d)[e] + `t${x}`; }";
    for i in 0..=full.len() {
        if full.is_char_boundary(i) {
            // Any prefix either parses or errors — never panics.
            let _ = parse(&full[..i]);
        }
    }
}

#[test]
fn garbage_bytes_never_panic() {
    let inputs = [
        "@#$%^&",
        "\0\0\0",
        "}{)(][",
        "var var var",
        "1 2 3 4 5",
        "...",
        "=>=>=>",
        "????",
        "/*/*/*",
        "```",
        "\u{1F600}\u{1F4A9}", // emoji
        "function(",
        "class {",
        "for(;;",
    ];
    for s in inputs {
        let _ = parse(s); // must not panic, must return (Ok or Err)
    }
}

#[test]
fn seeded_fuzz_never_panics() {
    // A small deterministic LCG over an alphabet of JS-significant bytes. The property
    // under test is total: parse() must terminate without panicking for every input.
    let alphabet: &[u8] = b"abc123(){}[];,.+-*/%<>=!&|^~?:`\"'\\ \n\t_$";
    let mut state: u64 = 0x9E3779B97F4A7C15;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    for _ in 0..4000 {
        let len = (next() % 60) as usize;
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            let idx = (next() as usize) % alphabet.len();
            s.push(alphabet[idx] as char);
        }
        let _ = parse(&s);
    }
}

#[test]
fn empty_and_whitespace_only() {
    assert_eq!(prog("").body.len(), 0);
    assert_eq!(prog("   \n\t  ").body.len(), 0);
    assert_eq!(prog("// just a comment").body.len(), 0);
}

#[test]
fn error_carries_position() {
    // `var = 5` is invalid (missing binding name).
    let err = parse("\n\nvar = 5;").unwrap_err();
    assert!(err.line >= 1);
    assert!(!err.message.is_empty());
}

#[test]
fn realistic_snippet_parses() {
    let src = r#"
        const greet = (name = "world") => `hello, ${name}!`;
        class Counter {
            constructor() { this.n = 0; }
            inc() { this.n += 1; return this.n; }
            get value() { return this.n; }
        }
        function main() {
            const c = new Counter();
            for (let i = 0; i < 3; i++) {
                c.inc();
            }
            const {n} = c;
            return n > 0 ? greet(`#${n}`) : greet();
        }
    "#;
    let p = prog(src);
    assert_eq!(p.body.len(), 3, "three top-level declarations");
}
