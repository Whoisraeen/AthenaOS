//! # RaeJs embedder host-object API — exposing native objects to page script.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy (criterion #5 — "the web browser is the
//! universal app runtime; PWAs that feel native"): the [interpreter](crate::interp) runs
//! ECMAScript and the [builtins](crate::builtins) give it `console`/`Math`/`JSON`, but a
//! *page* needs to read and mutate its own document — `document.getElementById('x')
//! .textContent = 'hi'`. Until now there was no way for an embedder (the browser app) to
//! install a `document`/`window` global whose property access calls back into Rust, so the
//! engine could only *evaluate* script for console side effects. This module is the bridge:
//! a small, public embedder API that lets the browser expose native-backed objects to the
//! interpreter.
//!
//! ## The shape (what an embedder implements)
//! A native object is a value implementing [`HostObject`]. The interpreter routes
//! JS-visible operations on a host object into the trait:
//!   - **property read** (`obj.key` / `obj[key]`) → [`HostObject::host_get`]
//!   - **property write** (`obj.key = v`) → [`HostObject::host_set`]
//!   - **method call** (`obj.m(args)`) is JS sugar for *read the method then call it*, so a
//!     host advertises a method by returning a native function value from `host_get` (build
//!     one with [`crate::Interpreter::host_function`]). The method closure captures whatever
//!     native handle it needs.
//!
//! The embedder wraps a `HostObject` into a [`crate::JsValue`] with
//! [`crate::Interpreter::new_host_object`] and installs it as a global with the now-public
//! [`crate::Interpreter::define_global`]. That is the entire surface — three calls.
//!
//! ## Why a trait + internal slot (not plain props)
//! A plain `JsObject` stores property values in a vec; a write just overwrites the slot, so
//! there is no way for the *engine* to learn that `el.textContent = 'new'` happened and push
//! it back to the real DOM. A host object carries an `Rc<dyn HostObject>` in the object's
//! exotic internal slot (the same mechanism Map/Set/RegExp use to hide native state), and
//! [`crate::Interpreter::get_property`]/[`set_property`](crate::Interpreter::set_property)
//! consult it *first* — so a write is observed live by native code. Reflecting through the
//! internal slot (rather than a parallel object type) keeps `typeof document === 'object'`,
//! `for-in`, and `JSON.stringify` behaving like the existing exotic objects.
//!
//! ## Safety property (unchanged — never-panic / bounded)
//! `#![forbid(unsafe_code)]` (workspace). A host callback returns `Result`/`Option`, never
//! panics by contract — an embedder error becomes a JS exception or `undefined`, exactly
//! like a missing property. The interpreter's call-depth and step budgets still bound any
//! host method the same way they bound a native builtin. Run the FAIL-able KATs with
//! `cargo test -p ath_js`.

use crate::interp::{JsValue, RuntimeError};
use alloc::string::String;
use alloc::vec::Vec;

/// A native object exposed to page script by an embedder (e.g. the browser's `document`).
///
/// The interpreter dispatches JS-visible property access and (indirectly, via returned
/// native functions) method calls into these hooks. All methods take `&self`: a host object
/// that needs to mutate native state holds it behind interior mutability (`RefCell`/`Cell`),
/// which is exactly how the DOM binding shares the live tree.
///
/// ## Contract
/// - Implementations **must not panic** (treat every key/argument as hostile input). Signal
///   absence/failure with `None`/`false` or a returned [`RuntimeError`], never a panic.
/// - `host_get` returning `None` means "this object has no such property" — the interpreter
///   then falls back to the object's own JS props / prototype chain, finally `undefined`.
/// - `host_set` returning `false` means "not a host-managed property" — the interpreter then
///   stores the value as an ordinary own property (so a script can stash arbitrary fields on
///   the object without the host having to model them).
pub trait HostObject {
    /// Read property `key`. `Some(value)` if the host manages this key (it may itself be a
    /// native function — see the module docs on methods); `None` to defer to ordinary
    /// property lookup.
    fn host_get(&self, key: &str) -> Option<JsValue>;

    /// Write `value` to property `key`. Return `true` if the host consumed the write
    /// (reflected it into native state); `false` to let the engine store it as a plain own
    /// property. Default: consumes nothing (read-only host object).
    fn host_set(&self, _key: &str, _value: &JsValue) -> bool {
        false
    }

    /// Enumerable own keys this host advertises (for `for-in` / `Object.keys`). Default:
    /// none — most host objects are accessed by known names, not enumerated.
    fn host_keys(&self) -> Vec<String> {
        Vec::new()
    }

    /// Upcast to [`core::any::Any`] so a host **method** (a native function returned from
    /// `host_get`) can recover its concrete host object from the call `this` receiver via
    /// [`crate::Interpreter::host_object_of`] and downcast it. This is how
    /// `document.getElementById(id)` reaches the real document handle it was read from:
    /// `this` is the `document` host value, and the method downcasts it back to the concrete
    /// document type. A host that has no methods can leave the default (which downcasts to
    /// nothing, so such methods simply find no receiver).
    fn as_any(&self) -> &dyn core::any::Any {
        // Default: an opaque unit, so the downcast in `host_object_of` returns `None` for
        // hosts that did not opt in. Implementors with methods override this with `self`.
        &()
    }
}

/// The signature of an embedder-supplied native function body. Mirrors the engine-internal
/// `NativeFn` but is the *public* form an embedder builds with
/// [`crate::Interpreter::host_function`]: it receives the interpreter (for allocation /
/// nested calls), the `this` receiver, and the JS arguments, and returns a value or throws.
pub type HostFn =
    fn(&mut crate::Interpreter, &JsValue, &[JsValue]) -> Result<JsValue, RuntimeError>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Interpreter, JsValue};
    use alloc::rc::Rc;
    use alloc::string::ToString;
    use alloc::vec;
    use core::cell::RefCell;

    /// A minimal host object modelling a single mutable "element" with a `textContent`
    /// property and a `setText(s)` method — the same shape the browser's DOM binding uses,
    /// reduced to the engine-level proof.
    struct Element {
        text: RefCell<alloc::string::String>,
    }

    impl HostObject for Element {
        fn host_get(&self, key: &str) -> Option<JsValue> {
            match key {
                "textContent" => Some(JsValue::str(self.text.borrow().clone())),
                _ => None,
            }
        }
        fn host_set(&self, key: &str, value: &JsValue) -> bool {
            if key == "textContent" {
                if let JsValue::String(s) = value {
                    *self.text.borrow_mut() = s.as_str().to_string();
                }
                return true;
            }
            false
        }
        fn host_keys(&self) -> alloc::vec::Vec<alloc::string::String> {
            vec!["textContent".to_string()]
        }
    }

    #[test]
    fn host_get_reads_native_state() {
        let mut it = Interpreter::new();
        let el = Rc::new(Element {
            text: RefCell::new("hello".to_string()),
        });
        let v = it.new_host_object(el);
        it.define_global("el", v);
        let r = it.eval_str("el.textContent").unwrap();
        assert_eq!(r, JsValue::str("hello"));
    }

    #[test]
    fn host_set_writes_native_state() {
        let mut it = Interpreter::new();
        let el = Rc::new(Element {
            text: RefCell::new("old".to_string()),
        });
        let backing = el.clone();
        let v = it.new_host_object(el);
        it.define_global("el", v);
        it.eval_str("el.textContent = 'new'").unwrap();
        // The write reflected into the native Rust object, not just a JS prop.
        assert_eq!(backing.text.borrow().as_str(), "new");
        // And a subsequent read sees it.
        assert_eq!(it.eval_str("el.textContent").unwrap(), JsValue::str("new"));
    }

    #[test]
    fn host_declined_key_falls_back_to_plain_prop() {
        let mut it = Interpreter::new();
        let el = Rc::new(Element {
            text: RefCell::new("x".to_string()),
        });
        let v = it.new_host_object(el);
        it.define_global("el", v);
        // `id` is not host-managed → stored as an ordinary own property + read back.
        let r = it.eval_str("el.id = 'banner'; el.id").unwrap();
        assert_eq!(r, JsValue::str("banner"));
    }

    /// A host with a stateful method, proving the full `obj.method(args)` path AND that a
    /// method recovers its concrete host state from `this` (the pattern the DOM binding's
    /// `document.getElementById` uses — `this` is the document, downcast back to it).
    struct Counter {
        n: RefCell<f64>,
    }
    impl HostObject for Counter {
        fn host_get(&self, key: &str) -> Option<JsValue> {
            match key {
                "value" => Some(JsValue::Number(*self.n.borrow())),
                // Advertise `add` as a native function; it reaches `self` via `this`.
                "add" => {
                    fn add(
                        it: &mut Interpreter,
                        this: &JsValue,
                        args: &[JsValue],
                    ) -> Result<JsValue, RuntimeError> {
                        let host = it.host_object_of(this);
                        if let Some(h) = host {
                            if let Some(c) = h.as_any().downcast_ref::<Counter>() {
                                let delta = match args.first() {
                                    Some(JsValue::Number(d)) => *d,
                                    _ => 0.0,
                                };
                                *c.n.borrow_mut() += delta;
                                return Ok(JsValue::Number(*c.n.borrow()));
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    Some(it_host_function(add))
                }
                _ => None,
            }
        }
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
    }

    // Tiny helper so the test can build a native fn value without an interpreter handle in
    // `host_get` (the real browser builds it via `Interpreter::host_function`, but inside a
    // `host_get(&self,...)` there is no interpreter; a fn pointer needs no interpreter to
    // construct the JsValue, so we expose the same constructor shape here).
    fn it_host_function(f: HostFn) -> JsValue {
        crate::interp::native_function_value("hostfn", f)
    }

    #[test]
    fn host_method_reaches_state_via_this() {
        let mut it = Interpreter::new();
        let c = Rc::new(Counter {
            n: RefCell::new(10.0),
        });
        let backing = c.clone();
        let v = it.new_host_object(c);
        it.define_global("c", v);
        let r = it.eval_str("c.add(5)").unwrap();
        assert_eq!(r, JsValue::Number(15.0));
        assert_eq!(*backing.n.borrow(), 15.0);
        assert_eq!(it.eval_str("c.value").unwrap(), JsValue::Number(15.0));
    }

    #[test]
    fn missing_host_property_is_undefined_not_panic() {
        let mut it = Interpreter::new();
        let el = Rc::new(Element {
            text: RefCell::new("x".to_string()),
        });
        let v = it.new_host_object(el);
        it.define_global("el", v);
        assert_eq!(it.eval_str("el.nope").unwrap(), JsValue::Undefined);
        // typeof a host object is "object" (it's a real JS object underneath).
        assert_eq!(it.eval_str("typeof el").unwrap(), JsValue::str("object"));
    }
}
