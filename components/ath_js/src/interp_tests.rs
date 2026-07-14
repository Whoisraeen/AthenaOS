//! Host KATs for the ath_js tree-walking interpreter. FAIL-able by construction: each
//! asserts a concrete [`JsValue`] / console output, so a semantics regression turns a
//! green run red. Run with `cargo test -p ath_js`. (`#![cfg_attr(not(test), no_std)]`
//! gives these tests std; we still avoid `use std::` so the R7 gate stays clean.)

use crate::interp::{ErrorKind, Interpreter, JsValue};

/// Eval source and require a successful completion value.
fn eval(src: &str) -> JsValue {
    let mut it = Interpreter::new();
    match it.eval_str(src) {
        Ok(v) => v,
        Err(e) => panic!("expected `{}` to eval, got error: {}", src, e),
    }
}

fn num(src: &str) -> f64 {
    match eval(src) {
        JsValue::Number(n) => n,
        other => panic!("expected number from `{}`, got {:?}", src, other),
    }
}

fn boolean(src: &str) -> bool {
    match eval(src) {
        JsValue::Bool(b) => b,
        other => panic!("expected bool from `{}`, got {:?}", src, other),
    }
}

fn string(src: &str) -> alloc::string::String {
    use alloc::string::ToString;
    match eval(src) {
        JsValue::String(s) => s.to_string(),
        other => panic!("expected string from `{}`, got {:?}", src, other),
    }
}

/// Eval and return the error kind (must FAIL to eval).
fn err_kind(src: &str) -> ErrorKind {
    let mut it = Interpreter::new();
    match it.eval_str(src) {
        Ok(v) => panic!("expected `{}` to throw, but got {:?}", src, v),
        Err(_) => {}
    }
    // Re-run capturing the typed error.
    let mut it2 = Interpreter::new();
    let program = crate::parse(src).expect("parse");
    match crate::interp::Interpreter::eval_typed(&mut it2, &program) {
        Ok(_) => panic!("expected throw"),
        Err(e) => e.kind,
    }
}

// ─── arithmetic + precedence ────────────────────────────────────────────────

#[test]
fn arithmetic_precedence() {
    assert_eq!(num("1 + 2 * 3"), 7.0);
    assert_eq!(num("(1 + 2) * 3"), 9.0);
    assert_eq!(num("2 ** 10"), 1024.0);
    assert_eq!(num("7 % 3"), 1.0);
    assert_eq!(num("10 / 4"), 2.5);
    assert_eq!(num("1 + 2 + 3 + 4"), 10.0);
}

#[test]
fn division_semantics() {
    // 1/0 → Infinity, 0/0 → NaN (NOT an error).
    assert!(num("1 / 0").is_infinite());
    assert!(num("0 / 0").is_nan());
    assert!(num("-1 / 0").is_infinite() && num("-1 / 0") < 0.0);
}

// ─── coercion quirks (the load-bearing semantics proofs) ─────────────────────

#[test]
fn coercion_plus_string_concat() {
    assert_eq!(string("1 + '2'"), "12");
    assert_eq!(string("'' + 1 + 2"), "12");
    assert_eq!(num("1 + 2 + 3"), 6.0);
}

#[test]
fn coercion_string_minus_is_numeric() {
    assert_eq!(num("'5' - 1"), 4.0);
    assert_eq!(num("'10' * '2'"), 20.0);
}

#[test]
fn abstract_vs_strict_equality() {
    assert!(boolean("1 == '1'"), "1 == '1' is true (coercion)");
    assert!(!boolean("1 === '1'"), "1 === '1' is false (no coercion)");
    assert!(boolean("null == undefined"));
    assert!(!boolean("null === undefined"));
    assert!(boolean("0 == false"));
    assert!(!boolean("0 === false"));
}

#[test]
fn truthiness() {
    assert!(!boolean("!!''"), "empty string is falsy");
    assert!(boolean("!!'x'"));
    assert!(!boolean("!!0"));
    assert!(boolean("!!1"));
    assert!(!boolean("!!null"));
    assert!(!boolean("!!undefined"));
    assert!(boolean("!![]"), "empty array is truthy");
}

#[test]
fn nullish_and_logical() {
    assert_eq!(num("null ?? 5"), 5.0);
    assert_eq!(num("0 ?? 5"), 0.0, "?? only triggers on null/undefined");
    assert_eq!(string("0 || 'x'"), "x");
    assert_eq!(num("3 && 4"), 4.0);
    assert_eq!(num("0 && 4"), 0.0);
}

#[test]
fn array_plus_array_quirk() {
    // [1,2]+[3] → "1,2" + "3" → "1,23".
    assert_eq!(string("[1,2]+[3]"), "1,23");
}

#[test]
fn typeof_quirks() {
    assert_eq!(string("typeof undefined"), "undefined");
    assert_eq!(string("typeof 1"), "number");
    assert_eq!(string("typeof 'x'"), "string");
    assert_eq!(string("typeof true"), "boolean");
    assert_eq!(string("typeof null"), "object", "the famous null quirk");
    assert_eq!(string("typeof []"), "object");
    assert_eq!(string("typeof {}"), "object");
    assert_eq!(string("typeof function(){}"), "function");
    assert_eq!(
        string("typeof undeclaredVar"),
        "undefined",
        "typeof undeclared is safe"
    );
}

#[test]
fn comparison_string_and_numeric() {
    assert!(boolean("2 < 10"));
    assert!(boolean("'a' < 'b'"), "lexicographic string compare");
    assert!(!boolean("'b' < 'a'"));
    assert!(boolean("'10' < '9'"), "string compare: '1' < '9'");
    assert!(boolean("10 > 9"), "numeric compare when not both strings");
    assert!(
        !boolean("NaN < 1") && !boolean("NaN > 1"),
        "NaN comparisons are false"
    );
}

#[test]
fn bitwise_to_int32() {
    assert_eq!(num("5 & 3"), 1.0);
    assert_eq!(num("5 | 2"), 7.0);
    assert_eq!(num("5 ^ 1"), 4.0);
    assert_eq!(num("~5"), -6.0);
    assert_eq!(num("1 << 4"), 16.0);
    assert_eq!(num("256 >> 2"), 64.0);
    assert_eq!(num("-1 >>> 28"), 15.0, "unsigned shift");
}

// ─── update / compound assignment ────────────────────────────────────────────

#[test]
fn update_and_compound() {
    assert_eq!(num("let x = 5; x++; x"), 6.0);
    assert_eq!(
        num("let x = 5; let y = x++; y"),
        5.0,
        "postfix returns old value"
    );
    assert_eq!(
        num("let x = 5; let y = ++x; y"),
        6.0,
        "prefix returns new value"
    );
    assert_eq!(num("let x = 10; x += 5; x"), 15.0);
    assert_eq!(num("let x = 10; x *= 3; x"), 30.0);
    assert_eq!(num("let a = null; a ??= 7; a"), 7.0);
    assert_eq!(num("let a = 1; a ||= 7; a"), 1.0);
}

// ─── closures ─────────────────────────────────────────────────────────────────

#[test]
fn closure_counter_factory() {
    let v = eval(
        "let make = function(){ let c = 0; return function(){ return ++c; }; };\
         let f = make();\
         let a = f();\
         let b = f();\
         [a, b]",
    );
    match v {
        JsValue::Array(arr) => {
            let items = arr.borrow().items.clone();
            assert_eq!(items.len(), 2);
            assert!(matches!(items[0], JsValue::Number(n) if n == 1.0));
            assert!(matches!(items[1], JsValue::Number(n) if n == 2.0));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn closures_are_independent() {
    let v = num("function counter(){ let n = 0; return () => ++n; }\
         let a = counter(); let b = counter();\
         a(); a(); b();");
    assert_eq!(v, 1.0, "b's closure is independent of a's");
}

// ─── recursion ───────────────────────────────────────────────────────────────

#[test]
fn recursion_factorial_and_fib() {
    assert_eq!(
        num("function f(n){ return n <= 1 ? 1 : n * f(n-1); } f(5)"),
        120.0
    );
    assert_eq!(
        num("function fib(n){ return n < 2 ? n : fib(n-1)+fib(n-2); } fib(10)"),
        55.0
    );
}

// ─── control flow ────────────────────────────────────────────────────────────

#[test]
fn for_loop_sum() {
    assert_eq!(
        num("let s = 0; for (let i = 1; i <= 100; i++) s += i; s"),
        5050.0
    );
}

#[test]
fn for_of_and_for_in() {
    assert_eq!(num("let s = 0; for (const x of [1,2,3,4]) s += x; s"), 10.0);
    assert_eq!(
        num("let s = 0; for (const c of 'abc') s++; s"),
        3.0,
        "for-of over string chars"
    );
    let keys = string("let o = {a:1, b:2}; let r = ''; for (const k in o) r += k; r");
    assert_eq!(keys, "ab");
}

#[test]
fn while_break_continue() {
    assert_eq!(
        num("let i = 0, s = 0; while (true) { i++; if (i > 5) break; if (i % 2 == 0) continue; s += i; } s"),
        9.0,
        "1 + 3 + 5"
    );
}

#[test]
fn do_while_runs_once() {
    assert_eq!(num("let n = 0; do { n++; } while (false); n"), 1.0);
}

#[test]
fn switch_fallthrough() {
    assert_eq!(
        num("let r = 0; switch (2) { case 1: r += 1; case 2: r += 2; case 3: r += 3; break; case 4: r += 4; } r"),
        5.0,
        "case 2 falls through to 3, breaks before 4"
    );
    assert_eq!(
        num("let r = 0; switch (99) { case 1: r = 1; break; default: r = -1; } r"),
        -1.0
    );
}

#[test]
fn labeled_break() {
    assert_eq!(
        num("let count = 0; outer: for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { if (j == 1) continue outer; count++; } } count"),
        3.0,
        "continue outer skips inner remainder each iteration"
    );
}

// ─── try / catch / finally ──────────────────────────────────────────────────

#[test]
fn try_catch_message() {
    assert_eq!(
        string("try { throw new Error('boom'); } catch (e) { e.message; }"),
        "boom"
    );
}

#[test]
fn try_finally_always_runs() {
    assert_eq!(
        num("let x = 0; try { x = 1; throw 'e'; } catch (e) { x = 2; } finally { x = 3; } x"),
        3.0
    );
}

#[test]
fn throw_string_is_caught() {
    assert_eq!(string("try { throw 'plain'; } catch (e) { e; }"), "plain");
}

// ─── objects / prototype / class ─────────────────────────────────────────────

#[test]
fn object_literal_and_access() {
    assert_eq!(num("let o = {a: 1, b: 2}; o.a + o.b"), 3.0);
    assert_eq!(num("let o = {}; o.x = 42; o.x"), 42.0);
    assert_eq!(string("let k = 'dyn'; let o = {[k]: 'v'}; o.dyn"), "v");
}

#[test]
fn this_binding_in_method() {
    assert_eq!(
        num("let o = { n: 10, get() { return this.n; } }; o.get()"),
        10.0
    );
}

#[test]
fn class_method_and_inheritance() {
    let src = "class Animal { constructor(name) { this.name = name; } speak() { return this.name + ' makes a sound'; } }\
               class Dog extends Animal { speak() { return this.name + ' barks'; } }\
               let d = new Dog('Rex'); d.speak();";
    assert_eq!(string(src), "Rex barks");
}

#[test]
fn class_super_call() {
    let src = "class A { constructor(x) { this.x = x; } }\
               class B extends A { constructor(x) { super(x); this.y = x * 2; } }\
               let b = new B(5); b.x + b.y;";
    assert_eq!(num(src), 15.0);
}

#[test]
fn class_static_method() {
    assert_eq!(
        num("class M { static double(x) { return x * 2; } } M.double(21)"),
        42.0
    );
}

#[test]
fn object_keys_values() {
    assert_eq!(string("Object.keys({a:1, b:2, c:3}).join(',')"), "a,b,c");
    assert_eq!(num("Object.values({a:1, b:2}).reduce((x,y)=>x+y, 0)"), 3.0);
}

#[test]
fn instanceof_works() {
    assert!(boolean("class C {} let c = new C(); c instanceof C"));
    assert!(!boolean("class C {} class D {} (new C()) instanceof D"));
}

// ─── arrays ───────────────────────────────────────────────────────────────────

#[test]
fn array_method_chain() {
    assert_eq!(
        num("[1,2,3].map(x=>x*2).filter(x=>x>2).reduce((a,b)=>a+b,0)"),
        10.0,
        "[2,4,6] -> [4,6] -> 10"
    );
}

#[test]
fn array_push_pop_join() {
    assert_eq!(num("let a = [1,2]; a.push(3); a.length"), 3.0);
    assert_eq!(num("let a = [1,2,3]; a.pop()"), 3.0);
    assert_eq!(string("['a','b','c'].join('-')"), "a-b-c");
    assert_eq!(string("[3,1,2].sort().join(',')"), "1,2,3");
    assert_eq!(string("[1,2,3].reverse().join(',')"), "3,2,1");
}

#[test]
fn array_sort_numeric_comparator() {
    assert_eq!(
        string("[10,2,33,4].sort((a,b)=>a-b).join(',')"),
        "2,4,10,33"
    );
}

#[test]
fn array_find_includes_indexof() {
    assert_eq!(num("[5,6,7].find(x=>x>5)"), 6.0);
    assert!(boolean("[1,2,3].includes(2)"));
    assert_eq!(num("[1,2,3].indexOf(3)"), 2.0);
    assert!(boolean("[1,2,3].some(x=>x>2)"));
    assert!(boolean("[2,4,6].every(x=>x%2==0)"));
}

#[test]
fn array_spread_and_destructure() {
    assert_eq!(
        string("let a=[1,2]; let b=[...a,3,4]; b.join(',')"),
        "1,2,3,4"
    );
    assert_eq!(num("let [x, y] = [10, 20]; x + y"), 30.0);
    assert_eq!(num("let [first, ...rest] = [1,2,3,4]; rest.length"), 3.0);
}

#[test]
fn object_destructuring_exec() {
    assert_eq!(num("let {a, b} = {a: 7, b: 8}; a + b"), 15.0);
    assert_eq!(num("let {x = 5} = {}; x"), 5.0, "destructure default");
}

// ─── strings ──────────────────────────────────────────────────────────────────

#[test]
fn string_methods() {
    assert_eq!(string("'hello'.toUpperCase().slice(0,3)"), "HEL");
    assert_eq!(string("'a,b,c'.split(',').join('|')"), "a|b|c");
    assert_eq!(num("'hello'.length"), 5.0);
    assert_eq!(string("'  hi  '.trim()"), "hi");
    assert_eq!(string("'ab'.repeat(3)"), "ababab");
    assert!(boolean("'hello world'.includes('world')"));
    assert!(boolean("'filename.js'.endsWith('.js')"));
    assert_eq!(string("'5'.padStart(3, '0')"), "005");
    assert_eq!(string("'cat'.replace('c', 'b')"), "bat");
}

#[test]
fn template_literal_exec() {
    assert_eq!(
        string("let name='world'; `hello, ${name}!`"),
        "hello, world!"
    );
    assert_eq!(string("let a=2, b=3; `${a}+${b}=${a+b}`"), "2+3=5");
}

// ─── Math / Number ──────────────────────────────────────────────────────────

#[test]
fn math_functions() {
    assert_eq!(num("Math.abs(-5)"), 5.0);
    assert_eq!(num("Math.floor(3.7)"), 3.0);
    assert_eq!(num("Math.ceil(3.2)"), 4.0);
    assert_eq!(num("Math.round(2.5)"), 3.0);
    assert_eq!(num("Math.max(1, 9, 3)"), 9.0);
    assert_eq!(num("Math.min(4, 2, 8)"), 2.0);
    assert_eq!(num("Math.pow(2, 8)"), 256.0);
    assert_eq!(num("Math.sqrt(144)"), 12.0);
}

#[test]
fn math_clz32_counts_leading_zeros() {
    assert_eq!(num("Math.clz32(1)"), 31.0);
    assert_eq!(num("Math.clz32(0)"), 32.0);
    assert_eq!(num("Math.clz32(1000)"), 22.0);
    assert_eq!(num("Math.clz32(0xFFFFFFFF)"), 0.0);
}

#[test]
fn string_locale_compare_orders() {
    assert_eq!(num("'a'.localeCompare('b')"), -1.0);
    assert_eq!(num("'b'.localeCompare('a')"), 1.0);
    assert_eq!(num("'a'.localeCompare('a')"), 0.0);
    // Usable as a sort comparator.
    assert_eq!(
        string("['c','a','b'].sort((x,y)=>x.localeCompare(y)).join('')"),
        "abc"
    );
}

#[test]
fn string_from_code_point_builds_strings() {
    assert_eq!(string("String.fromCodePoint(72, 105)"), "Hi");
    // Astral plane (emoji) — beyond fromCharCode's 16-bit code units.
    assert_eq!(string("String.fromCodePoint(0x1F600)"), "\u{1F600}");
    // Invalid code point (> 0x10FFFF) throws RangeError.
    assert_eq!(
        err_kind("String.fromCodePoint(0x110000)"),
        ErrorKind::RangeError
    );
}

#[test]
fn math_random_is_bounded_and_deterministic() {
    // Seeded → reproducible; always in [0, 1).
    let mut it = Interpreter::new();
    let r1 = match it.eval_str("Math.random()").unwrap() {
        JsValue::Number(n) => n,
        _ => panic!("not a number"),
    };
    assert!((0.0..1.0).contains(&r1), "random in [0,1)");
    let mut it2 = Interpreter::new();
    let r2 = match it2.eval_str("Math.random()").unwrap() {
        JsValue::Number(n) => n,
        _ => panic!("not a number"),
    };
    assert_eq!(r1, r2, "seeded random is deterministic across instances");
}

#[test]
fn number_parsing_and_format() {
    assert_eq!(num("parseInt('42')"), 42.0);
    assert_eq!(num("parseInt('0xFF', 16)"), 255.0);
    assert_eq!(num("parseInt('101', 2)"), 5.0);
    assert_eq!(num("parseFloat('3.14abc')"), 3.14);
    assert!(boolean("isNaN(NaN)"));
    assert!(!boolean("isFinite(Infinity)"));
    assert_eq!(string("(255).toString(16)"), "ff");
    assert_eq!(string("(3.14159).toFixed(2)"), "3.14");
}

// ─── JSON ─────────────────────────────────────────────────────────────────────

#[test]
fn json_parse_roundtrip() {
    assert_eq!(num("JSON.parse('{\"a\":[1,2]}').a[1]"), 2.0);
    assert_eq!(string("JSON.stringify({a:1})"), "{\"a\":1}");
    assert_eq!(
        string("JSON.stringify([1,'two',true,null])"),
        "[1,\"two\",true,null]"
    );
    assert_eq!(
        string("JSON.stringify(JSON.parse('{\"x\":{\"y\":5}}'))"),
        "{\"x\":{\"y\":5}}"
    );
}

// ─── console capture ────────────────────────────────────────────────────────

#[test]
fn console_log_capture() {
    let mut it = Interpreter::new();
    it.eval_str("console.log('hello'); console.log(1 + 2); console.log('a', 'b');")
        .unwrap();
    let out = it.take_console_output();
    assert_eq!(out.len(), 3);
    assert_eq!(out[0], "hello");
    assert_eq!(out[1], "3");
    assert_eq!(out[2], "a b");
    // The buffer drains.
    assert!(it.take_console_output().is_empty());
}

#[test]
fn console_log_object_and_array() {
    let mut it = Interpreter::new();
    it.eval_str("console.log([1,2,3]); console.log({a:1});")
        .unwrap();
    let out = it.take_console_output();
    assert_eq!(out[0], "[ 1, 2, 3 ]");
    assert_eq!(out[1], "{ a: 1 }");
}

// ─── never-hang / never-panic (the load-bearing safety asserts) ──────────────

#[test]
fn infinite_loop_terminates_with_range_error() {
    // THE load-bearing assert: an infinite loop must throw (step budget), not hang.
    assert_eq!(err_kind("while (true) {}"), ErrorKind::RangeError);
    assert_eq!(err_kind("for (;;) {}"), ErrorKind::RangeError);
}

#[test]
fn deep_recursion_throws_not_overflow() {
    // Unbounded recursion → RangeError (call-depth cap), never a host stack overflow.
    assert_eq!(
        err_kind("function f(){ return f(); } f()"),
        ErrorKind::RangeError
    );
}

#[test]
fn calling_non_function_is_type_error() {
    assert_eq!(err_kind("let x = 5; x();"), ErrorKind::TypeError);
    assert_eq!(err_kind("undefined();"), ErrorKind::TypeError);
}

#[test]
fn reading_prop_of_undefined_is_type_error() {
    assert_eq!(err_kind("let x; x.y;"), ErrorKind::TypeError);
    assert_eq!(err_kind("null.foo;"), ErrorKind::TypeError);
}

#[test]
fn reference_error_on_undeclared() {
    assert_eq!(
        err_kind("nonexistentVariable + 1"),
        ErrorKind::ReferenceError
    );
}

#[test]
fn const_reassignment_is_type_error() {
    assert_eq!(err_kind("const x = 1; x = 2;"), ErrorKind::TypeError);
}

#[test]
fn accessing_undefined_property_yields_undefined() {
    // Reading a missing property of an OBJECT is undefined, not an error.
    assert!(matches!(eval("let o = {}; o.missing"), JsValue::Undefined));
    assert!(matches!(eval("({}).a"), JsValue::Undefined));
}

#[test]
fn optional_chaining_short_circuits() {
    assert!(matches!(eval("let o = null; o?.a?.b"), JsValue::Undefined));
    assert_eq!(num("let o = {a:{b:5}}; o?.a?.b"), 5.0);
}

#[test]
fn seeded_fuzz_never_panics() {
    // A deterministic LCG composes random snippets from JS fragments; the property under
    // test is total: eval() must terminate without panicking (Ok or Err, never a host
    // panic / stack overflow / hang). The step + depth budgets guarantee termination.
    use alloc::string::String;
    use alloc::vec::Vec;
    let fragments: &[&str] = &[
        "1+2",
        "x",
        "x=1",
        "f()",
        "[1,2,3]",
        "{a:1}",
        "while(x){x--}",
        "if(a){b}else{c}",
        "function f(){return f()}",
        "for(let i=0;i<9;i++)s+=i",
        "x.y.z",
        "a?.b",
        "'s'.repeat(3)",
        "Math.sqrt(2)",
        "JSON.parse('[1]')",
        "a==b",
        "a===b",
        "!x",
        "x++",
        "x?y:z",
        "[...a]",
        "({...o})",
        "x=>x",
        "class C{}",
        "new C()",
        "try{throw 1}catch(e){e}",
        "switch(x){case 1:break}",
        "a&&b||c",
        "typeof x",
    ];
    let mut state: u64 = 0xDEADBEEF_CAFEBABE;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    for _ in 0..4000 {
        let n = 1 + (next() % 4) as usize;
        let mut parts: Vec<&str> = Vec::new();
        for _ in 0..n {
            parts.push(fragments[(next() as usize) % fragments.len()]);
        }
        let src = parts.join(";");
        let _ = src;
        let mut joined = String::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                joined.push(';');
            }
            joined.push_str(p);
        }
        let mut it = Interpreter::new();
        let _ = it.eval_str(&joined); // must not panic / hang
    }
}

// ─── realistic program ──────────────────────────────────────────────────────

#[test]
fn realistic_program_runs() {
    let mut it = Interpreter::new();
    let src = r#"
        class Stack {
            constructor() { this.items = []; }
            push(x) { this.items.push(x); return this; }
            pop() { return this.items.pop(); }
            get size() { return this.items.length; }
        }
        function sumDigits(n) {
            let s = String(n).split('').map(c => parseInt(c, 10));
            return s.reduce((a, b) => a + b, 0);
        }
        let st = new Stack();
        st.push(1).push(2).push(3);
        console.log('size', st.items.length);
        console.log('top', st.pop());
        console.log('digitsum', sumDigits(12345));
    "#;
    it.eval_str(src).expect("program runs");
    let out = it.take_console_output();
    assert_eq!(out[0], "size 3");
    assert_eq!(out[1], "top 3");
    assert_eq!(out[2], "digitsum 15");
}

// ═══════════════════════════════════════════════════════════════════════════
//  Map / Set / Date / Symbol — the modern-built-ins slice
// ═══════════════════════════════════════════════════════════════════════════

// ─── Map ──────────────────────────────────────────────────────────────────

#[test]
fn map_basic_set_get_chainable() {
    // set is chainable; get returns the value; size + has + delete behave.
    assert_eq!(
        num("const m=new Map(); m.set('a',1).set('b',2); m.get('a')"),
        1.0
    );
    assert_eq!(
        num("const m=new Map(); m.set('a',1).set('b',2); m.size"),
        2.0
    );
    assert!(boolean(
        "const m=new Map(); m.set('a',1).set('b',2); m.has('b')"
    ));
    assert_eq!(
        num("const m=new Map(); m.set('a',1).set('b',2); m.delete('a'); m.size"),
        1.0
    );
    // FAIL-able: get of a missing key is undefined, not an error.
    assert!(matches!(eval("new Map().get('nope')"), JsValue::Undefined));
}

#[test]
fn map_constructor_from_iterable() {
    assert_eq!(num("new Map([['x',9]]).get('x')"), 9.0);
    assert_eq!(num("new Map([['a',1],['b',2],['c',3]]).size"), 3.0);
}

#[test]
fn map_samevaluezero_nan_key() {
    // SameValueZero: NaN keys are equal, so set(NaN) then get(NaN) round-trips.
    assert_eq!(num("const m=new Map(); m.set(NaN,1); m.get(NaN)"), 1.0);
    // +0 and -0 are the same key.
    assert_eq!(num("const m=new Map(); m.set(-0,7); m.get(0)"), 7.0);
    // Object keys are by reference identity (two distinct {} are different keys).
    assert_eq!(
        num("const k={}; const m=new Map(); m.set(k,5); m.get(k)"),
        5.0
    );
    assert!(matches!(
        eval("const m=new Map(); m.set({},5); m.get({})"),
        JsValue::Undefined
    ));
}

#[test]
fn map_set_overwrites_not_duplicates() {
    assert_eq!(
        num("const m=new Map(); m.set('a',1); m.set('a',2); m.size"),
        1.0
    );
    assert_eq!(
        num("const m=new Map(); m.set('a',1); m.set('a',2); m.get('a')"),
        2.0
    );
}

#[test]
fn map_for_of_insertion_order() {
    let mut it = Interpreter::new();
    it.eval_str(
        r#"
        const m = new Map();
        m.set('z', 26).set('a', 1).set('m', 13);
        for (const [k, v] of m) { console.log(k + '=' + v); }
    "#,
    )
    .expect("runs");
    let out = it.take_console_output();
    assert_eq!(out, alloc::vec!["z=26", "a=1", "m=13"]);
}

#[test]
fn map_keys_values_entries_foreach() {
    assert_eq!(
        string("[...new Map([['a',1],['b',2]]).keys()].join(',')"),
        "a,b"
    );
    assert_eq!(
        string("[...new Map([['a',1],['b',2]]).values()].join(',')"),
        "1,2"
    );
    // forEach signature is (value, key, map).
    let mut it = Interpreter::new();
    it.eval_str(
        r#"
        const m = new Map([['a',1],['b',2]]);
        m.forEach((v, k) => console.log(k, v));
    "#,
    )
    .expect("runs");
    assert_eq!(it.take_console_output(), alloc::vec!["a 1", "b 2"]);
}

#[test]
fn map_clear_and_wrong_receiver() {
    assert_eq!(num("const m=new Map([['a',1]]); m.clear(); m.size"), 0.0);
    // Method on the wrong receiver → TypeError.
    assert_eq!(
        err_kind("const f=new Map().set; f.call({}, 'a', 1)"),
        ErrorKind::TypeError
    );
}

// ─── Set ──────────────────────────────────────────────────────────────────

#[test]
fn set_dedup_and_membership() {
    assert_eq!(num("new Set([1,2,2,3]).size"), 3.0);
    assert!(boolean("new Set([1,2,3]).has(2)"));
    assert_eq!(num("const s=new Set([1,2,3]); s.add(2); s.size"), 3.0);
    assert_eq!(
        num("const s=new Set(); s.add(1).add(2).add(2); s.size"),
        2.0
    );
}

#[test]
fn set_nan_dedup_samevaluezero() {
    assert_eq!(num("new Set([NaN, NaN]).size"), 1.0);
    assert!(boolean("new Set([NaN]).has(NaN)"));
    // +0 / -0 collapse.
    assert_eq!(num("new Set([0, -0]).size"), 1.0);
}

#[test]
fn set_for_of_order_and_delete() {
    let mut it = Interpreter::new();
    it.eval_str(
        r#"
        const s = new Set();
        s.add('c').add('a').add('b');
        for (const v of s) { console.log(v); }
    "#,
    )
    .expect("runs");
    assert_eq!(it.take_console_output(), alloc::vec!["c", "a", "b"]);
    assert!(boolean("const s=new Set([1,2]); s.delete(1)"));
    assert_eq!(num("const s=new Set([1,2]); s.delete(1); s.size"), 1.0);
}

#[test]
fn set_values_spread() {
    assert_eq!(string("[...new Set([3,1,2,1])].join(',')"), "3,1,2");
}

// ─── Date ─────────────────────────────────────────────────────────────────

#[test]
fn date_epoch_and_accessors() {
    assert_eq!(num("new Date(0).getTime()"), 0.0);
    assert_eq!(num("new Date(0).getFullYear()"), 1970.0);
    assert_eq!(num("new Date(0).getMonth()"), 0.0); // January = 0
    assert_eq!(num("new Date(0).getDate()"), 1.0);
    assert_eq!(num("new Date(0).getDay()"), 4.0); // 1970-01-01 was a Thursday
    assert_eq!(num("new Date(0).getHours()"), 0.0);
    assert_eq!(num("new Date(0).getMinutes()"), 0.0);
    assert_eq!(num("new Date(0).getSeconds()"), 0.0);
    assert_eq!(num("new Date(0).getMilliseconds()"), 0.0);
}

#[test]
fn date_ms_fields() {
    // 1000 ms = 1 second past the epoch.
    assert_eq!(num("new Date(1000).getSeconds()"), 1.0);
    assert_eq!(num("new Date(1500).getMilliseconds()"), 500.0);
    // valueOf == getTime.
    assert_eq!(num("new Date(12345).valueOf()"), 12345.0);
    assert_eq!(num("+new Date(777)"), 777.0); // unary + coerces via valueOf
}

#[test]
fn date_to_iso_civil_conversion() {
    assert_eq!(
        string("new Date(0).toISOString()"),
        "1970-01-01T00:00:00.000Z"
    );
    // A known modern instant: 2020-01-01T00:00:00.000Z.
    assert_eq!(
        string("new Date(1577836800000).toISOString()"),
        "2020-01-01T00:00:00.000Z"
    );
    // A mid-day, mid-month instant proves the full civil math.
    // 2021-06-15T13:45:30.123Z = 1623764730123 ms.
    assert_eq!(
        string("new Date(1623764730123).toISOString()"),
        "2021-06-15T13:45:30.123Z"
    );
    assert_eq!(num("new Date(1623764730123).getFullYear()"), 2021.0);
    assert_eq!(num("new Date(1623764730123).getMonth()"), 5.0); // June
    assert_eq!(num("new Date(1623764730123).getDate()"), 15.0);
}

#[test]
fn date_components_constructor() {
    // new Date(2021, 5, 15) → June 15, 2021 (month is 0-based).
    assert_eq!(num("new Date(2021, 5, 15).getFullYear()"), 2021.0);
    assert_eq!(num("new Date(2021, 5, 15).getMonth()"), 5.0);
    assert_eq!(num("new Date(2021, 5, 15).getDate()"), 15.0);
    assert_eq!(
        string("new Date(2021, 5, 15).toISOString()"),
        "2021-06-15T00:00:00.000Z"
    );
    // Month rollover: month 12 → next January.
    assert_eq!(num("new Date(2020, 12, 1).getFullYear()"), 2021.0);
    assert_eq!(num("new Date(2020, 12, 1).getMonth()"), 0.0);
}

#[test]
fn date_now_is_deterministic_number() {
    // no_std build: Date.now() is a fixed, reproducible Number (2020-01-01Z).
    assert_eq!(num("Date.now()"), 1577836800000.0);
    assert_eq!(num("new Date().getTime()"), 1577836800000.0);
    assert_eq!(num("new Date().getFullYear()"), 2020.0);
}

#[test]
fn date_utc_static_and_invalid() {
    assert_eq!(num("Date.UTC(1970, 0, 1)"), 0.0);
    // Invalid Date: a NaN ms → NaN accessors, toISOString throws RangeError.
    assert!(num("new Date(NaN).getTime()").is_nan());
    assert_eq!(
        err_kind("new Date(NaN).toISOString()"),
        ErrorKind::RangeError
    );
}

// ─── Symbol (basic) ─────────────────────────────────────────────────────────

#[test]
fn symbol_unique_and_iterator_key() {
    // Two symbols with the same description are distinct values.
    assert!(!boolean("Symbol('x') === Symbol('x')"));
    // Same binding is itself.
    assert!(boolean("const s = Symbol('x'); s === s"));
    // Symbol.iterator is exposed as a well-known key (string in this minimal model).
    assert!(boolean("typeof Symbol.iterator !== 'undefined'"));
    assert_eq!(string("Symbol('hi').description"), "hi");
}

// ─── for-of regression: arrays + strings still iterate ──────────────────────

#[test]
fn for_of_still_works_over_arrays_and_strings() {
    let mut it = Interpreter::new();
    it.eval_str(
        r#"
        let acc = '';
        for (const x of [10, 20, 30]) acc += x + ',';
        for (const c of 'ab') acc += c;
        console.log(acc);
    "#,
    )
    .expect("runs");
    assert_eq!(it.take_console_output(), alloc::vec!["10,20,30,ab"]);
}

// ─── safety: budgets + never-panic ──────────────────────────────────────────

#[test]
fn map_over_budget_throws_range_error_not_oom() {
    // An unbounded Map-building loop must terminate with a RangeError (the step budget),
    // never hang / OOM. The body reuses a single key so each `set` is an O(1) update — this
    // proves the *termination* property quickly. (The distinct-key path is additionally
    // bounded by the MAX_ENTRIES cap, which throws RangeError before OOM.)
    assert_eq!(
        err_kind("const m=new Map(); let i=0; while(true){ m.set('k', i); i++; }"),
        ErrorKind::RangeError
    );
    // A growing Set likewise terminates rather than hanging (step budget bites).
    assert_eq!(
        err_kind("const s=new Set(); while(true){ s.add(0); }"),
        ErrorKind::RangeError
    );
}

#[test]
fn collections_seeded_fuzz_never_panics() {
    use alloc::string::String;
    use alloc::vec::Vec;
    let fragments: &[&str] = &[
        "new Map()",
        "new Set()",
        "new Date(0)",
        "Symbol('x')",
        "m.set(1,2)",
        "m.get(1)",
        "m.has(NaN)",
        "m.delete('a')",
        "m.size",
        "s.add(1)",
        "s.has(2)",
        "s.delete(3)",
        "s.size",
        "s.clear()",
        "new Map([['a',1]])",
        "new Set([1,2,2])",
        "Date.now()",
        "new Date(1000).getSeconds()",
        "new Date(NaN).getTime()",
        "for(const x of new Set([1,2,3]))y+=x",
        "[...new Map([['a',1]])]",
        "new Date(2020,5,15).toISOString()",
        "m.forEach(v=>v)",
    ];
    let mut state: u64 = 0x12345678_9ABCDEF0;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    for _ in 0..4000 {
        let n = 1 + (next() % 4) as usize;
        let mut parts: Vec<&str> = Vec::new();
        for _ in 0..n {
            parts.push(fragments[(next() as usize) % fragments.len()]);
        }
        let mut joined = String::from("let m=new Map();let s=new Set();let y=0;");
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                joined.push(';');
            }
            joined.push_str(p);
        }
        let mut it = Interpreter::new();
        let _ = it.eval_str(&joined); // must not panic / hang
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Promise + event loop (criterion #5 — real web JS is async)
// ═══════════════════════════════════════════════════════════════════════════

use alloc::string::String as Str;
use alloc::vec::Vec as Vector;

/// Eval a script and return the console output AFTER the event loop has fully drained.
/// `eval_str` auto-drains, so this captures sync logs, then microtasks, then macrotasks in
/// execution order — the load-bearing async-ordering observation.
fn console(src: &str) -> Vector<Str> {
    let mut it = Interpreter::new();
    it.eval_str(src)
        .unwrap_or_else(|e| panic!("eval `{}`: {}", src, e));
    it.take_console_output()
}

#[test]
fn microtask_runs_after_sync_code() {
    // The canonical async-ordering proof: the microtask logs LAST even though it is scheduled
    // before the final sync log. FAIL-able: expecting ["1","3","2"] would fail.
    let out = console(
        "console.log('1'); Promise.resolve().then(()=>console.log('3')); console.log('2');",
    );
    assert_eq!(out, ["1", "2", "3"], "sync 1,2 then microtask 3");
}

#[test]
fn promise_chaining_propagates_values() {
    let out = console("Promise.resolve(42).then(v => v+1).then(v => console.log(v))");
    assert_eq!(out, ["43"], "value flows through the .then chain");
}

#[test]
fn promise_then_returning_promise_is_adopted() {
    let out = console(
        "Promise.resolve(1)\
         .then(v => Promise.resolve(v + 10))\
         .then(v => console.log(v))",
    );
    assert_eq!(out, ["11"], "a returned promise is adopted, not wrapped");
}

#[test]
fn catch_handles_rejection_reason() {
    let out = console("Promise.reject('boom').catch(e => console.log('caught ' + e))");
    assert_eq!(out, ["caught boom"]);
}

#[test]
fn throw_in_then_is_caught_downstream() {
    let out = console(
        "Promise.resolve(1)\
         .then(() => { throw 'kaboom'; })\
         .catch(e => console.log('got ' + e))",
    );
    assert_eq!(
        out,
        ["got kaboom"],
        "a throw in a handler rejects the chain"
    );
}

#[test]
fn finally_runs_regardless_and_propagates() {
    let fulfilled = console(
        "Promise.resolve('v').finally(() => console.log('fin')).then(v => console.log('then ' + v))",
    );
    assert_eq!(
        fulfilled,
        ["fin", "then v"],
        "finally runs, value passes through"
    );

    let rejected = console(
        "Promise.reject('r').finally(() => console.log('fin2')).catch(e => console.log('catch ' + e))",
    );
    assert_eq!(
        rejected,
        ["fin2", "catch r"],
        "finally runs, rejection passes through"
    );
}

#[test]
fn promise_all_collects_in_order() {
    let out = console(
        "Promise.all([Promise.resolve(1), Promise.resolve(2)]).then(a => console.log(a.join(',')))",
    );
    assert_eq!(out, ["1,2"], "all fulfills with the ordered results array");
}

#[test]
fn promise_all_rejects_on_first_rejection() {
    let out = console(
        "Promise.all([Promise.resolve(1), Promise.reject('x'), Promise.resolve(3)])\
         .then(() => console.log('NO')).catch(e => console.log('rej ' + e))",
    );
    assert_eq!(out, ["rej x"], "all rejects on the first rejection");
}

#[test]
fn promise_all_settled_reports_both_outcomes() {
    let out = console(
        "Promise.allSettled([Promise.resolve(1), Promise.reject('e')]).then(rs => {\
            console.log(rs[0].status + ':' + rs[0].value);\
            console.log(rs[1].status + ':' + rs[1].reason);\
         })",
    );
    assert_eq!(out, ["fulfilled:1", "rejected:e"]);
}

#[test]
fn promise_race_takes_first_settled() {
    let out = console(
        "Promise.race([Promise.resolve('a'), Promise.reject('b')]).then(v => console.log('won ' + v))",
    );
    assert_eq!(
        out,
        ["won a"],
        "race resolves with the first settled (a fulfills first here)"
    );
}

#[test]
fn promise_any_takes_first_fulfilled() {
    let out = console(
        "Promise.any([Promise.reject('e1'), Promise.resolve('ok')]).then(v => console.log(v))",
    );
    assert_eq!(
        out,
        ["ok"],
        "any skips rejections and resolves with first fulfillment"
    );
}

#[test]
fn settimeout_runs_after_microtasks() {
    // Phase proof: sync, then ALL microtasks, then the macrotask. FAIL-able: ["sync","late","micro"].
    let out = console(
        "setTimeout(()=>console.log('late'),10); \
         Promise.resolve().then(()=>console.log('micro')); \
         console.log('sync');",
    );
    assert_eq!(
        out,
        ["sync", "micro", "late"],
        "sync -> microtask -> macrotask"
    );
}

#[test]
fn settimeout_virtual_time_orders_by_delay() {
    // Queued reversed (20 before 10) but the 10ms timer must fire first (virtual-time order).
    let out =
        console("setTimeout(()=>console.log('b20'),20); setTimeout(()=>console.log('a10'),10);");
    assert_eq!(
        out,
        ["a10", "b20"],
        "earlier due-time runs first regardless of queue order"
    );
}

#[test]
fn queue_microtask_runs_before_timeout() {
    let out = console(
        "setTimeout(()=>console.log('macro'),0); queueMicrotask(()=>console.log('qm')); console.log('s');",
    );
    assert_eq!(out, ["s", "qm", "macro"]);
}

#[test]
fn clear_timeout_cancels() {
    let out = console("let id = setTimeout(()=>console.log('never'),5); clearTimeout(id);");
    assert!(
        out.is_empty(),
        "a cleared timeout never fires, got {:?}",
        out
    );
}

#[test]
fn settimeout_extra_args_forwarded() {
    let out = console("setTimeout((a,b)=>console.log(a+b), 0, 3, 4);");
    assert_eq!(out, ["7"]);
}

// ─── safety: the loop must TERMINATE, never hang the host (load-bearing) ─────────

#[test]
fn never_cleared_interval_terminates() {
    // A setInterval that is never cleared would run forever in a real browser; here the task
    // budget terminates the drain. The test PASSES by returning at all (no hang).
    let mut it = Interpreter::new();
    let r = it.eval_str("let n=0; setInterval(()=>{n++;}, 1);");
    assert!(r.is_ok(), "interval drain must terminate cleanly");
}

#[test]
fn self_rescheduling_then_terminates() {
    // A promise chain that re-schedules itself in .then would spin forever; the microtask
    // budget bounds it. Must return (no hang).
    let mut it = Interpreter::new();
    let r = it.eval_str("function loop(){ return Promise.resolve().then(loop); } loop();");
    assert!(r.is_ok(), "self-rescheduling microtask must terminate");
}

#[test]
fn self_rescheduling_timeout_terminates() {
    // setTimeout(function f(){setTimeout(f,0)},0) — the classic infinite timer. Budget bounds it.
    let mut it = Interpreter::new();
    let r = it.eval_str("setTimeout(function f(){ setTimeout(f, 0); }, 0);");
    assert!(r.is_ok(), "self-rescheduling timeout must terminate");
}

#[test]
fn throw_in_timeout_does_not_abort_loop() {
    // A throw in one timer callback must not crash the loop; later timers still run.
    let out = console(
        "setTimeout(()=>{ throw new Error('x'); }, 0); setTimeout(()=>console.log('after'), 1);",
    );
    assert_eq!(
        out,
        ["after"],
        "other tasks still run after a callback throws"
    );
}

#[test]
fn unhandled_rejection_is_tracked_not_panicked() {
    let mut it = Interpreter::new();
    it.eval_str("Promise.reject('lonely');")
        .expect("no host error");
    assert_eq!(
        it.loop_state.unhandled_rejections.len(),
        1,
        "an unhandled rejection is recorded, not a panic"
    );
}

#[test]
fn handled_rejection_is_not_tracked() {
    let mut it = Interpreter::new();
    it.eval_str("Promise.reject('seen').catch(()=>{});")
        .expect("no host error");
    assert!(
        it.loop_state.unhandled_rejections.is_empty(),
        "a caught rejection is not reported unhandled, got {:?}",
        it.loop_state.unhandled_rejections
    );
}

#[test]
fn new_promise_executor_resolves() {
    let out = console("new Promise((resolve)=>{ resolve(99); }).then(v => console.log(v));");
    assert_eq!(out, ["99"]);
}

#[test]
fn new_promise_executor_throw_rejects() {
    let out = console("new Promise(()=>{ throw 'oops'; }).catch(e => console.log('e:' + e));");
    assert_eq!(
        out,
        ["e:oops"],
        "a throw in the executor rejects the promise"
    );
}

#[test]
fn nested_microtask_ordering() {
    // Two independent chains interleave by microtask turn (breadth order), not depth.
    let out = console(
        "Promise.resolve().then(()=>console.log('a1')).then(()=>console.log('a2'));\
         Promise.resolve().then(()=>console.log('b1')).then(()=>console.log('b2'));",
    );
    assert_eq!(
        out,
        ["a1", "b1", "a2", "b2"],
        "microtasks interleave by turn"
    );
}

#[test]
fn async_promise_seeded_fuzz_never_panics() {
    // 4000 random async snippets must never panic or hang the host (the drain is bounded).
    let fragments = [
        "Promise.resolve(1).then(x=>x)",
        "Promise.reject('e').catch(x=>x)",
        "setTimeout(()=>{}, 0)",
        "setInterval(()=>{}, 1)",
        "queueMicrotask(()=>{})",
        "Promise.all([Promise.resolve(1)])",
        "Promise.race([Promise.resolve(2)])",
        "Promise.any([Promise.reject('z')])",
        "new Promise((r)=>r(1)).then(v=>v)",
        "new Promise((_,j)=>j('x')).catch(e=>e)",
        "Promise.allSettled([Promise.reject('q')])",
        "Promise.resolve().then(()=>Promise.resolve())",
        "let id=setTimeout(()=>{},0);clearTimeout(id)",
        "Promise.resolve(1).finally(()=>{})",
    ];
    let mut state: u64 = 0x00C0FFEE_12345678;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    for _ in 0..4000 {
        let n = 1 + (next() % 4) as usize;
        let mut joined = Str::new();
        for i in 0..n {
            if i > 0 {
                joined.push(';');
            }
            joined.push_str(fragments[(next() as usize) % fragments.len()]);
        }
        let mut it = Interpreter::new();
        let _ = it.eval_str(&joined); // must not panic / hang
    }
}

// ─── REGRESSION KATs (Concept criterion #6: a hostile script never crashes/hangs the host)
//
// Each of these four reproduces a reviewer-confirmed defect and is FAIL-able by construction:
// on the pre-fix code it crashes the test process (host stack overflow), hangs past the test
// timeout, or asserts the wrong value. The fix makes each terminate correctly.

/// FIX 1 — deep-graph `Rc` drop must NOT recurse into a host-stack overflow.
///
/// `for(...) a={next:a}` builds a 300k-deep `Rc<RefCell<JsObject>>` spine; the naive
/// recursive `Rc` destructor on `Interpreter` drop overflowed the host stack
/// (STATUS_STACK_OVERFLOW). The iterative `teardown_graph` (run on `Drop`) flattens it first.
/// PROOF OF FAIL-ABILITY: on the old code, the `drop(it)` below aborts the test process with
/// a stack overflow; the test reaching its final line is the proof the fix works.
#[test]
fn regression_deep_object_graph_drops_without_stack_overflow() {
    // Each chain gets its own interpreter so they don't share the one step budget, and so the
    // load-bearing event — the recursive `Rc` destructor on `Interpreter` DROP — is exercised
    // once per shape. A chain ~50k+ deep reliably overflows the old naive recursive drop on a
    // typical host stack; 300k is the reviewer's canonical object example. Reaching the end of
    // this test (no STATUS_STACK_OVERFLOW abort) IS the proof the iterative teardown works.

    // Deep object chain (the reviewer's exact repro).
    {
        let mut it = Interpreter::new();
        it.eval_str("let a={}; for(let i=0;i<300000;i++){a={next:a};} a!==null")
            .expect("deep object chain should eval without hanging");
        drop(it);
    }
    // Deep array chain (each array nests the previous one).
    {
        let mut it = Interpreter::new();
        it.eval_str("let b=[]; for(let i=0;i<100000;i++){b=[b];} b.length")
            .expect("deep array chain should eval");
        drop(it);
    }
    // Deep Map value chain (Map value -> object holding the previous Map, etc.).
    {
        let mut it = Interpreter::new();
        it.eval_str(
            "let m=new Map(); let prev={}; for(let i=0;i<80000;i++){let n=new Map(); \
             n.set('p', prev); prev={mapref:n};} m.set('root', prev); m.size",
        )
        .expect("deep map chain should eval");
        drop(it);
    }
    // Deep Set member chain.
    {
        let mut it = Interpreter::new();
        it.eval_str(
            "let s=new Set(); let p2={}; for(let i=0;i<100000;i++){p2={prev:p2};} s.add(p2); s.size",
        )
        .expect("deep set chain should eval");
        drop(it);
    }
    // Cyclic graph must also tear down safely (visited-set guard, no infinite re-enqueue).
    {
        let mut it = Interpreter::new();
        it.eval_str("let c={}; c.self=c; c.arr=[c]; true")
            .expect("cyclic graph should eval");
        drop(it);
    }
}

/// FIX 2 — default `Array.prototype.sort()` (no comparator) must be step-budget-bounded.
///
/// The no-comparator branch is O(n²) and its `to_string` comparisons did NOT call `tick()`,
/// so MAX_STEPS never fired → `new Array(big).sort()` hung (343s for 60k; forever for 16M).
/// Now each comparison charges a step, so a large array hits the budget → RangeError instead
/// of hanging. PROOF OF FAIL-ABILITY: on the old code this test hangs past the harness
/// timeout; the fix makes it return a RangeError promptly.
#[test]
fn regression_default_sort_is_bounded_not_hang() {
    // 8000 elements → ~8000² / 2 ≈ 32M comparisons >> MAX_STEPS (5M) → RangeError fast.
    assert_eq!(
        err_kind("let a = new Array(8000).fill(0).map((_,i)=>8000-i); a.sort();"),
        ErrorKind::RangeError,
        "default sort of a large array must hit the step budget, not hang"
    );
    // A SMALL default sort still works correctly (the fix doesn't break normal sorts).
    assert_eq!(string("[3,1,2,10].sort().join(',')"), "1,10,2,3"); // string compare
}

/// FIX 3 — `Array.prototype.flat` must terminate on a cyclic / huge depth, never overflow.
///
/// `flatten` recursed to a user depth (`flat(Infinity)` → i64::MAX) with no cycle guard and
/// no native cap; `let a=[]; a.push(a); a.flat(Infinity)` overflowed the host stack. The fix
/// clamps native recursion to 512 and charges a step per element. PROOF OF FAIL-ABILITY: on
/// the old code the cyclic case overflows the test process; here it returns gracefully.
#[test]
fn regression_flat_cyclic_terminates_and_normal_still_works() {
    // Cyclic array .flat(Infinity): must NOT overflow — bounded array or RangeError, no crash.
    let mut it = Interpreter::new();
    let r = it.eval_str("let a=[]; a.push(a); let f=a.flat(Infinity); Array.isArray(f)");
    match r {
        Ok(JsValue::Bool(true)) => {}
        Ok(other) => panic!("expected a bounded array result, got {:?}", other),
        Err(e) => assert!(
            alloc::format!("{}", e).contains("RangeError"),
            "cyclic flat must be bounded or RangeError, got: {}",
            e
        ),
    }
    // Normal nested flatten still produces the right answer.
    assert_eq!(string("[1,[2,[3]]].flat(Infinity).join(',')"), "1,2,3");
    assert_eq!(string("[1,[2,[3,[4]]]].flat(1).join(',')"), "1,2,3,4");
}

/// FIX 4 — `await` must pass its value through, NOT discard it (the old `void` bug).
///
/// `await x` was parsed as `UnaryOp::Void`, so `await 42` → undefined and
/// `await Promise.resolve(7)` → undefined (both WRONG). The fix gives `await` its own eval
/// path: a non-promise awaits to itself; a settled promise awaits to its resolved value.
/// PROOF OF FAIL-ABILITY: on the old code every assert below saw `undefined`.
#[test]
fn regression_await_passes_value_through() {
    // await of a non-promise = the value itself.
    assert_eq!(num("await 42"), 42.0);
    assert_eq!(string("await 'hi'"), "hi");
    // await of an already-settled promise = its resolved value (NOT undefined).
    assert_eq!(num("await Promise.resolve(7)"), 7.0);
    // await of a synchronously-resolvable promise drains the loop then reads the value.
    assert_eq!(num("await new Promise((res)=>res(99))"), 99.0);
    // await of a rejected promise re-throws the reason.
    assert_eq!(
        err_kind("await Promise.reject(new TypeError('nope'))"),
        ErrorKind::TypeError
    );
}

// ─── Array.prototype: flatMap / findLast / findLastIndex ─────────────────────

#[test]
fn array_flat_map() {
    // flatMap = map then flatten exactly one level.
    assert_eq!(
        string("JSON.stringify([1,2,3].flatMap(function(x){return [x, x * 2];}))"),
        "[1,2,2,4,3,6]"
    );
    // Non-array results are appended, not flattened.
    assert_eq!(
        string("JSON.stringify([1,2].flatMap(function(x){return x + 1;}))"),
        "[2,3]"
    );
    // Only ONE level is flattened.
    assert_eq!(
        string("JSON.stringify([1].flatMap(function(x){return [[x]];}))"),
        "[[1]]"
    );
}

#[test]
fn array_find_last() {
    assert_eq!(
        num("[1,2,3,4].findLast(function(x){return x % 2 === 0;})"),
        4.0
    );
    assert_eq!(
        num("[1,2,3,4].findLastIndex(function(x){return x % 2 === 0;})"),
        3.0
    );
    assert_eq!(
        num("[1,3,5].findLastIndex(function(x){return x % 2 === 0;})"),
        -1.0
    );
    assert!(matches!(
        eval("[1,3].findLast(function(x){return x > 10;})"),
        JsValue::Undefined
    ));
}

#[test]
fn uri_encoding() {
    // encodeURIComponent percent-encodes the reserved set (space, &, =).
    assert_eq!(string("encodeURIComponent('a b&c=d')"), "a%20b%26c%3Dd");
    assert_eq!(string("decodeURIComponent('a%20b%26c%3Dd')"), "a b&c=d");
    // encodeURI KEEPS reserved chars (?, =, &) but still encodes the space.
    assert_eq!(string("encodeURI('a b?q=1&r=2')"), "a%20b?q=1&r=2");
    // round-trip through the component codec (slash is encoded by component).
    assert_eq!(
        string("decodeURIComponent(encodeURIComponent('x/y z?w'))"),
        "x/y z?w"
    );
    // decodeURI leaves a reserved-char %XX (%2F = '/') encoded; component decodes it.
    assert_eq!(string("decodeURI('a%2Fb%20c')"), "a%2Fb c");
    assert_eq!(string("decodeURIComponent('a%2Fb%20c')"), "a/b c");
    // a malformed % sequence throws (JS would raise URIError; ath_js uses Error).
    assert_eq!(err_kind("decodeURIComponent('%')"), ErrorKind::Error);
    assert_eq!(err_kind("decodeURIComponent('%zz')"), ErrorKind::Error);
}

#[test]
fn object_has_own_property() {
    // own data prop vs an absent one vs an inherited-style name.
    assert_eq!(boolean("var o = {a: 1, b: 2}; o.hasOwnProperty('a')"), true);
    assert_eq!(boolean("var o = {a: 1}; o.hasOwnProperty('z')"), false);
    assert_eq!(
        boolean("var o = {a: 1}; o.hasOwnProperty('hasOwnProperty')"),
        false
    );
    // a property explicitly set to undefined is still OWN.
    assert_eq!(
        boolean("var o = {a: undefined}; o.hasOwnProperty('a')"),
        true
    );
    // arrays: index in range, length, but not an out-of-range index.
    assert_eq!(boolean("[10,20,30].hasOwnProperty('1')"), true);
    assert_eq!(boolean("[10,20,30].hasOwnProperty('5')"), false);
    assert_eq!(boolean("[10,20,30].hasOwnProperty('length')"), true);
    // strings: index in range + length.
    assert_eq!(boolean("'abc'.hasOwnProperty('0')"), true);
    assert_eq!(boolean("'abc'.hasOwnProperty('9')"), false);
}

#[test]
fn array_copy_within() {
    // copyWithin(target, start): copy items[3..5]=[4,5] to index 0 -> [4,5,3,4,5].
    assert_eq!(string("[1,2,3,4,5].copyWithin(0,3).join(',')"), "4,5,3,4,5");
    // negative target indexes from the end: copyWithin(-2) copies items[0..] into len-2.
    assert_eq!(string("[1,2,3,4,5].copyWithin(-2).join(',')"), "1,2,3,1,2");
    // start/end window: copyWithin(0,1,3) copies items[1..3]=[2,3] to 0 -> [2,3,3,4,5].
    assert_eq!(
        string("[1,2,3,4,5].copyWithin(0,1,3).join(',')"),
        "2,3,3,4,5"
    );
    // mutates in place and returns the same array.
    assert_eq!(
        string("var a=[1,2,3]; a.copyWithin(0,1); a.join(',')"),
        "2,3,3"
    );
}

#[test]
fn number_is_safe_integer() {
    assert_eq!(boolean("Number.isSafeInteger(42)"), true);
    assert_eq!(boolean("Number.isSafeInteger(0)"), true);
    // 2^53-1 (MAX_SAFE_INTEGER) is safe; 2^53 is NOT.
    assert_eq!(boolean("Number.isSafeInteger(9007199254740991)"), true);
    assert_eq!(boolean("Number.isSafeInteger(9007199254740992)"), false);
    // non-integers / NaN / Infinity / non-numbers are all false.
    assert_eq!(boolean("Number.isSafeInteger(1.5)"), false);
    assert_eq!(boolean("Number.isSafeInteger(NaN)"), false);
    assert_eq!(boolean("Number.isSafeInteger(Infinity)"), false);
    assert_eq!(boolean("Number.isSafeInteger('5')"), false);
}

// ─── ES2022 relative indexing + live accessor properties ─────────────────────

#[test]
fn array_at_relative_indexing() {
    // Array.prototype.at: positive, negative (from end), and out-of-range.
    assert_eq!(num("[10,20,30].at(0)"), 10.0);
    assert_eq!(num("[10,20,30].at(2)"), 30.0);
    assert_eq!(num("[10,20,30].at(-1)"), 30.0);
    assert_eq!(num("[10,20,30].at(-3)"), 10.0);
    assert!(matches!(eval("[10,20,30].at(5)"), JsValue::Undefined));
    assert!(matches!(eval("[10,20,30].at(-9)"), JsValue::Undefined));
    assert_eq!(num("[1,2,3].at()"), 1.0); // no arg → index 0
}

#[test]
fn object_literal_getter_is_invoked() {
    // A `get` accessor returns its COMPUTED value, not the function itself
    // (the bug this fixes returned `[function]`).
    assert_eq!(num("(()=>{let o={get x(){return 42}};return o.x})()"), 42.0);
    // Getter sees sibling state via `this`.
    assert_eq!(
        num("(()=>{let o={v:7,get dbl(){return this.v*2}};return o.dbl})()"),
        14.0
    );
    // Getter key enumerates and JSON.stringify invokes it.
    assert_eq!(string("JSON.stringify({get a(){return 5}})"), "{\"a\":5}");
}

#[test]
fn object_literal_setter_is_invoked() {
    // A `set` accessor runs the setter body; a paired get/set round-trips.
    assert_eq!(
        num("(()=>{let store=0;let o={set x(v){store=v*10},get x(){return store}};o.x=5;return o.x})()"),
        50.0
    );
    // Getter-only property swallows writes (sloppy mode) — no data-prop shadow.
    assert_eq!(
        num("(()=>{let o={get x(){return 1}};o.x=99;return o.x})()"),
        1.0
    );
}

#[test]
fn class_accessors_on_prototype() {
    // Class getters/setters live on the prototype and are inherited by instances.
    let src = "(()=>{\
        class C{constructor(){this._n=3} get n(){return this._n} set n(v){this._n=v+1}}\
        let c=new C(); let a=c.n; c.n=10; return a*100+c.n})()";
    assert_eq!(num(src), 311.0); // a=3 → 300, then setter stores 10+1=11
}

// ─── builtin completeness: Symbol typeof, Object reflection, instanceof, tags ─

#[test]
fn typeof_symbol_is_symbol() {
    assert_eq!(string("typeof Symbol()"), "symbol");
    assert_eq!(string("typeof Symbol('x')"), "symbol");
    assert_eq!(string("typeof {}"), "object");
}

#[test]
fn object_reflection_names_and_define() {
    assert_eq!(num("Object.getOwnPropertyNames({a:1,b:2,c:3}).length"), 3.0);
    // defineProperty data descriptor (the transpiled-module __esModule pattern).
    assert_eq!(
        num("(()=>{let o={}; Object.defineProperty(o,'x',{value:42}); return o.x})()"),
        42.0
    );
    // defineProperty accessor descriptor invokes the getter on read.
    assert_eq!(
        num("(()=>{let o={}; Object.defineProperty(o,'g',{get(){return 7}}); return o.g})()"),
        7.0
    );
}

#[test]
fn instanceof_builtin_constructors() {
    assert!(boolean("[] instanceof Array"));
    assert!(boolean("[] instanceof Object"));
    assert!(boolean("({}) instanceof Object"));
    // class instances still resolve through the prototype chain.
    assert!(boolean(
        "(()=>{class A{} class B extends A{} let b=new B; return b instanceof A})()"
    ));
}

#[test]
fn tagged_template_calls_the_tag() {
    // tag(strings, ...values): strings = cooked chunks, then each interpolation.
    assert_eq!(
        string("(()=>{function t(s,...v){return s.join('|')+'#'+v.join(',')} return t`a${1}b${2}c`})()"),
        "a|b|c#1,2"
    );
    // String.raw-style: the strings array carries `.raw`.
    assert_eq!(
        string("(()=>{function t(s){return s.raw.join('')} return t`x${1}y`})()"),
        "xy"
    );
}

#[test]
fn array_entries_iterable() {
    // entries() is spread/for-of iterable as [index, value] pairs.
    assert_eq!(
        string("[...['a','b'].entries()].map(p=>p.join(':')).join(',')"),
        "0:a,1:b"
    );
    assert_eq!(
        num("(()=>{let s=0; for(const [i,v] of [10,20,30].entries()) s+=i*v; return s})()"),
        80.0 // 0*10 + 1*20 + 2*30
    );
}

#[test]
fn zzz_idiom_probe3() {
    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    let cases: &[(&str, &str)] = &[
        // error handling
        ("(()=>{try{null.x}catch(e){return e instanceof TypeError}})()", "true"),
        ("(()=>{try{throw new Error('msg')}catch(e){return e.message}})()", "msg"),
        ("(()=>{try{throw new TypeError('bad')}catch(e){return e.name}})()", "TypeError"),
        ("(()=>{let e=new Error('x'); return e.toString()})()", "Error: x"),
        ("(()=>{try{throw new RangeError('r')}catch(e){return e instanceof Error}})()", "true"),
        // conversions
        ("String(null)", "null"),
        ("String(undefined)", "undefined"),
        ("String([1,2,3])", "1,2,3"),
        ("String({})", "[object Object]"),
        ("Number('  42  ')", "42"),
        ("Number('')", "0"),
        ("Number('abc')", "NaN"),
        ("(+'3.14')", "3.14"),
        ("(1e3).toString()", "1000"),
        ("isNaN(NaN)", "true"),
        ("isFinite(42)", "true"),
        ("(123.456).toPrecision(4)", "123.5"),
        ("(255).toString(16)", "ff"),
        ("parseInt('ff',16)", "255"),
        // array/string advanced
        ("[1,2,3,4,5].filter(x=>x%2).reduce((a,b)=>a+b)", "9"),
        ("[[1,2],[3,4]].map(([a,b])=>a+b).join(',')", "3,7"),
        ("[1,2,3].flatMap(x=>[x,x*10]).join(',')", "1,10,2,20,3,30"),
        ("Array(3).fill().map((_, i)=>i*i).join(',')", "0,1,4"),
        ("'a'.charCodeAt(0)", "97"),
        ("String.fromCharCode(72,105)", "Hi"),
        ("'Hello'.split('').reverse().join('')", "olleH"),
        ("'abcdef'.substring(2,4)", "cd"),
        ("'abcdef'.substr(2,2)", "cd"),
        ("'5'.padStart(3,'0')", "005"),
        ("'x'.codePointAt(0)", "120"),
        ("'a,b,c'.split(',').reverse().join('-')", "c-b-a"),
        ("'ABCDEF'.toLowerCase().toUpperCase()", "ABCDEF"),
        // closures/scope
        ("(()=>{let fns=[]; for(let i=0;i<3;i++)fns.push(()=>i); return fns.map(f=>f()).join('')})()", "012"),
        ("(()=>{let x=1; {let x=2;} return x})()", "1"),
        ("((x)=>(y)=>(z)=>x+y+z)(1)(2)(3)", "6"),
        ("(()=>{let c=0; let inc=()=>c++; inc(); inc(); return c})()", "2"),
        // misc
        ("(()=>{let o={a:1,b:2}; return {...o, c:3}})().c", "3"),
        ("(()=>{const sym=Symbol('id'); let o={[sym]:1}; return o[sym]})()", "1"),
        ("[NaN].includes(NaN)", "true"),
        ("Object.is(NaN, NaN)", "true"),
        ("Object.is(-0, 0)", "false"),
        ("[1,2,3].fill(0,1).join('')", "100"),
        ("Math.round(Math.PI*100)/100", "3.14"),
        ("[1,2,3,4].findLast(x=>x%2===0)", "4"),
        ("[3,1,2].toSorted((a,b)=>a-b).join('')", "123"),
        ("['a','b'].map((v,i)=>i+v).join(',')", "0a,1b"),
        ("(()=>{let [a=1,b=2,c=3]=[10,undefined]; return a+b+c})()", "15"),
        ("structuredClone({a:1,b:[2,3]}).b[1]", "3"),
    ];
    let mut fails: Vec<String> = Vec::new();
    for (src, expect) in cases {
        let mut it = Interpreter::new();
        match it.eval_str(src) {
            Ok(v) => {
                let got = match &v {
                    JsValue::Number(n) => format!("{}", n),
                    JsValue::String(s) => s.to_string(),
                    JsValue::Bool(b) => b.to_string(),
                    JsValue::Undefined => "undefined".to_string(),
                    JsValue::Null => "null".to_string(),
                    other => format!("{:?}", other),
                };
                if &got != expect {
                    fails.push(format!(
                        "MISMATCH `{}` => got `{}` want `{}`",
                        src, got, expect
                    ));
                }
            }
            Err(e) => fails.push(format!("ERROR    `{}` => {}", src, e)),
        }
    }
    if !fails.is_empty() {
        panic!(
            "\n{}/{} failures:\n{}\n",
            fails.len(),
            cases.len(),
            fails.join("\n")
        );
    }
}
