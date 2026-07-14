//! Browser JS↔DOM bridge — wiring `rae_js`'s host-object API to `raeweb`'s live DOM.
//!
//! RaeenOS_Concept.md §"3. Web apps via PWA support that actually feels native (renders
//! through RaeUI)": a page is only interactive if its script can read and mutate the
//! document. The two engines now each ship half of that — `rae_js` exposes a host-object
//! embedder API ([`rae_js::HostObject`]) and `raeweb` exposes a mutable document
//! ([`raeweb::DomDocument`]). This module is the join that lives where both crates are in
//! scope (the browser app), so neither engine depends on the other.
//!
//! ## What it builds
//! A `document` host object whose `getElementById(id)` returns an **element handle** host
//! object (or `null` for a missing id). The element handle reflects `textContent`:
//!   - **read** `el.textContent` → the live element's current text,
//!   - **write** `el.textContent = 'new'` → mutates the `raeweb` DOM and marks it dirty,
//! and `setAttribute(name, value)` / `getAttribute(name)`. The browser runs the page's
//! inline scripts with this `document` installed, then — if the document went dirty —
//! re-lays-out the mutated DOM so the change shows. That is the engine gap closed:
//! `document.getElementById('out').textContent = 'new'` actually changes what renders.
//!
//! ## Shared state
//! One [`raeweb::DomDocument`] is shared (via `Rc<RefCell<…>>`) between the render loop and
//! every host object. Both the `document` host and each element handle hold a clone of that
//! `Rc`; an element handle additionally remembers its element `id` so a write knows which
//! node to mutate. (v1 keys elements by `id` — the common interactive case; node-handle
//! identity for id-less elements is a documented follow-up.)
//!
//! Never panics: a missing id yields JS `null`; a write to a vanished id is a no-op; a
//! script touching an unknown property reads `undefined` (the engine's existing contract).

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use rae_js::{native_function_value, HostObject, Interpreter, JsValue, RuntimeError};
use raeweb::DomDocument;

use crate::events::{ListenerRegistry, SharedRegistry};

/// The shared, mutable document every host object reflects into.
pub type SharedDoc = Rc<RefCell<DomDocument>>;

/// The `document` global. Holds the shared document + the listener registry so the element
/// handles it hands out can register `addEventListener` callbacks against the shared registry.
pub struct DocumentHost {
    doc: SharedDoc,
    registry: SharedRegistry,
}

impl DocumentHost {
    pub fn new(doc: SharedDoc, registry: SharedRegistry) -> Self {
        DocumentHost { doc, registry }
    }
}

impl HostObject for DocumentHost {
    fn host_get(&self, key: &str) -> Option<JsValue> {
        match key {
            // `document.getElementById(id)` — a native method that recovers `this`
            // (the document host) and returns an element handle or null.
            "getElementById" => Some(native_function_value("getElementById", get_element_by_id)),
            _ => None,
        }
    }

    fn host_keys(&self) -> Vec<String> {
        vec!["getElementById".to_string()]
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// Native body of `document.getElementById(id)`. `this` is the `document` host object; we
/// recover its concrete [`DocumentHost`] to reach the shared DOM, then return an
/// [`ElementHost`] handle (as a JS object) for the matching element, or `null`.
fn get_element_by_id(
    it: &mut Interpreter,
    this: &JsValue,
    args: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    let id = match args.first() {
        Some(JsValue::String(s)) => s.as_str().to_string(),
        Some(other) => it.to_string(other)?,
        None => return Ok(JsValue::Null),
    };
    let doc = match it.host_object_of(this) {
        Some(h) => match h.as_any().downcast_ref::<DocumentHost>() {
            Some(d) => d.doc.clone(),
            None => return Ok(JsValue::Null),
        },
        None => return Ok(JsValue::Null),
    };
    let registry = match it.host_object_of(this) {
        Some(h) => match h.as_any().downcast_ref::<DocumentHost>() {
            Some(d) => d.registry.clone(),
            None => return Ok(JsValue::Null),
        },
        None => return Ok(JsValue::Null),
    };
    // null for a missing id (matches the DOM: getElementById returns null, not undefined).
    if !doc.borrow().has_element(&id) {
        return Ok(JsValue::Null);
    }
    let el = Rc::new(ElementHost { doc, id, registry });
    Ok(it.new_host_object(el))
}

/// An element handle bound to one element (by id) of the shared document.
pub struct ElementHost {
    doc: SharedDoc,
    id: String,
    registry: SharedRegistry,
}

impl HostObject for ElementHost {
    fn host_get(&self, key: &str) -> Option<JsValue> {
        match key {
            // textContent / innerText read the live element text.
            "textContent" | "innerText" => {
                let text = self.doc.borrow().get_element_text(&self.id);
                Some(JsValue::str(text.unwrap_or_default()))
            }
            "id" => Some(JsValue::str(self.id.clone())),
            "getAttribute" => Some(native_function_value("getAttribute", get_attribute)),
            "setAttribute" => Some(native_function_value("setAttribute", set_attribute)),
            "addEventListener" => Some(native_function_value(
                "addEventListener",
                add_event_listener,
            )),
            "removeEventListener" => Some(native_function_value(
                "removeEventListener",
                remove_event_listener,
            )),
            _ => None,
        }
    }

    fn host_set(&self, key: &str, value: &JsValue) -> bool {
        match key {
            "textContent" | "innerText" => {
                let text = js_to_plain_string(value);
                self.doc.borrow_mut().set_text_content(&self.id, &text);
                true
            }
            _ => false,
        }
    }

    fn host_keys(&self) -> Vec<String> {
        vec![
            "textContent".to_string(),
            "innerText".to_string(),
            "id".to_string(),
            "addEventListener".to_string(),
            "removeEventListener".to_string(),
        ]
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// `el.getAttribute(name)` → string or null.
fn get_attribute(
    it: &mut Interpreter,
    this: &JsValue,
    args: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    let name = match args.first() {
        Some(JsValue::String(s)) => s.as_str().to_string(),
        Some(other) => it.to_string(other)?,
        None => return Ok(JsValue::Null),
    };
    if let Some(h) = it.host_object_of(this) {
        if let Some(el) = h.as_any().downcast_ref::<ElementHost>() {
            return Ok(match el.doc.borrow().get_attribute(&el.id, &name) {
                Some(v) => JsValue::str(v),
                None => JsValue::Null,
            });
        }
    }
    Ok(JsValue::Null)
}

/// `el.setAttribute(name, value)` → undefined (mutates + dirties the doc).
fn set_attribute(
    it: &mut Interpreter,
    this: &JsValue,
    args: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    let name = match args.first() {
        Some(JsValue::String(s)) => s.as_str().to_string(),
        Some(other) => it.to_string(other)?,
        None => return Ok(JsValue::Undefined),
    };
    let value = match args.get(1) {
        Some(v) => js_to_plain_string_via(it, v)?,
        None => String::new(),
    };
    if let Some(h) = it.host_object_of(this) {
        if let Some(el) = h.as_any().downcast_ref::<ElementHost>() {
            el.doc.borrow_mut().set_attribute(&el.id, &name, &value);
        }
    }
    Ok(JsValue::Undefined)
}

/// `el.addEventListener(type, listener)` — register `listener` (must be callable) against the
/// element's id + event type in the shared registry, and tag the raeweb `EventListener` seam
/// with the assigned callback id. Returns `undefined`. A non-function listener or a missing
/// arg is ignored (matches the DOM's lenient `addEventListener`, which throws only on the
/// modern overloads — v1 degrades to a no-op rather than risk a crash on hostile input).
fn add_event_listener(
    it: &mut Interpreter,
    this: &JsValue,
    args: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    let event_type = match args.first() {
        Some(JsValue::String(s)) => s.as_str().to_string(),
        Some(other) => it.to_string(other)?,
        None => return Ok(JsValue::Undefined),
    };
    let listener = match args.get(1) {
        Some(v @ JsValue::Function(_)) => v.clone(),
        // Non-callable listener: no-op (don't store a value we can't invoke).
        _ => return Ok(JsValue::Undefined),
    };
    if let Some(h) = it.host_object_of(this) {
        if let Some(el) = h.as_any().downcast_ref::<ElementHost>() {
            let cid = el.registry.borrow_mut().add(&el.id, &event_type, listener);
            // Reflect the listener on the engine's own DOM node (informational seam).
            el.doc
                .borrow_mut()
                .register_event_listener(&el.id, &event_type, cid);
        }
    }
    Ok(JsValue::Undefined)
}

/// `el.removeEventListener(type, listener)` — remove a previously registered identical
/// callable for `type`. v1 matches by function identity (`Rc` pointer equality via the
/// engine's `===`). Returns `undefined`; a no-match is a silent no-op (DOM semantics).
fn remove_event_listener(
    it: &mut Interpreter,
    this: &JsValue,
    args: &[JsValue],
) -> Result<JsValue, RuntimeError> {
    let event_type = match args.first() {
        Some(JsValue::String(s)) => s.as_str().to_string(),
        Some(other) => it.to_string(other)?,
        None => return Ok(JsValue::Undefined),
    };
    let listener = match args.get(1) {
        Some(v @ JsValue::Function(_)) => v.clone(),
        _ => return Ok(JsValue::Undefined),
    };
    if let Some(h) = it.host_object_of(this) {
        if let Some(el) = h.as_any().downcast_ref::<ElementHost>() {
            el.registry
                .borrow_mut()
                .remove(&el.id, &event_type, &listener);
        }
    }
    Ok(JsValue::Undefined)
}

/// Coerce a JS value to a plain string for a DOM write WITHOUT needing the interpreter
/// (used in `host_set`, which has no interpreter). Strings pass through; primitives render
/// simply; objects become the empty string (a real `ToString` on an object can call user
/// code, which the no-interpreter setter path cannot do — documented limitation).
fn js_to_plain_string(v: &JsValue) -> String {
    match v {
        JsValue::String(s) => s.as_str().to_string(),
        JsValue::Number(n) => fmt_number(*n),
        JsValue::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        _ => String::new(),
    }
}

/// Like [`js_to_plain_string`] but uses the interpreter's full `ToString` (so objects with a
/// `toString` work) — for call sites that hold an interpreter.
fn js_to_plain_string_via(it: &mut Interpreter, v: &JsValue) -> Result<String, RuntimeError> {
    it.to_string(v)
}

/// Minimal f64 → string (integer-fast-path) without `std::fmt` float machinery.
fn fmt_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    // Integer-valued + within i64 range, using only core-safe ops (no std
    // f64::trunc/abs — those are libm intrinsics absent on the soft-float
    // bare target). The magnitude guard makes the i64 cast lossless.
    if n > -1.0e15 && n < 1.0e15 && n == (n as i64 as f64) {
        // Integer-valued: format as an integer.
        let mut i = n as i64;
        if i == 0 {
            return "0".to_string();
        }
        let neg = i < 0;
        if neg {
            i = -i;
        }
        let mut buf: Vec<u8> = Vec::new();
        while i > 0 {
            buf.push(b'0' + (i % 10) as u8);
            i /= 10;
        }
        if neg {
            buf.push(b'-');
        }
        buf.reverse();
        return String::from_utf8(buf).unwrap_or_default();
    }
    // Fall back to a short fixed representation for non-integers (rare in DOM text).
    use core::fmt::Write as _;
    let mut s = String::new();
    let _ = write!(s, "{}", n);
    s
}

/// Build a shared document from `html`+`css` at the given viewport, install a `document`
/// host object bound to it into `interp`, and return the shared handle so the caller can
/// re-layout it after the scripts run. This is the one call the browser makes before
/// executing a page's inline scripts.
pub fn install_document(
    interp: &mut Interpreter,
    html: &str,
    css: &str,
    viewport_w: f32,
    viewport_h: f32,
) -> SharedDoc {
    let (doc, _registry) = install_document_interactive(interp, html, css, viewport_w, viewport_h);
    doc
}

/// Like [`install_document`] but ALSO returns the shared event-listener [`SharedRegistry`] so
/// the caller can dispatch clicks into the registered handlers later (see
/// [`crate::events::dispatch_click_by_id`]). The `addEventListener` calls a page's scripts make
/// land in this registry; keeping the returned interpreter + doc + registry alive is what makes
/// the page interactive past the initial render.
pub fn install_document_interactive(
    interp: &mut Interpreter,
    html: &str,
    css: &str,
    viewport_w: f32,
    viewport_h: f32,
) -> (SharedDoc, SharedRegistry) {
    let doc: SharedDoc = Rc::new(RefCell::new(DomDocument::parse(
        html, css, viewport_w, viewport_h,
    )));
    let registry = ListenerRegistry::new();
    let document_host = Rc::new(DocumentHost::new(doc.clone(), registry.clone()));
    let value = interp.new_host_object(document_host);
    interp.define_global("document", value);
    (doc, registry)
}
