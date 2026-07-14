//! RaeWeb live-DOM handle — the mutable document a script can reflect into.
//!
//! > "Web apps via PWA support that actually feels native (renders through AthUI)."
//! > — AthenaOS Concept §3
//!
//! The render pipeline ([`crate::RenderPipeline`]) parses HTML into an OWNED [`crate::DomNode`]
//! tree and lays it out. That is enough for a *static* page, but an interactive page mutates
//! its own document — `document.getElementById('out').textContent = 'new'` — and expects the
//! change to show. This module is the missing mutable handle:
//!
//!   1. **look up a node by id** ([`DomDocument::get_element_by_id`] / `..._text`),
//!   2. **get/set its text content** ([`DomDocument::set_text_content`]),
//!   3. **mark the tree dirty** so the embedder knows to re-lay-out/re-paint
//!      ([`DomDocument::is_dirty`] / [`DomDocument::take_dirty`]), and
//!   4. **re-render** the current (possibly mutated) DOM ([`DomDocument::render_to_layout`] /
//!      [`DomDocument::render`]).
//!
//! It is deliberately engine-only: it knows nothing about JavaScript. The browser app owns
//! the JS↔DOM bridge (it depends on both `athweb` and `ath_js`), wrapping a `DomDocument` in
//! a `ath_js` host object whose `getElementById`/`textContent` calls land here. Keeping the
//! mutable primitive here (and the binding in the app) preserves the layering: `athweb` never
//! depends on `ath_js`.
//!
//! `no_std + alloc`, never panics: a missing id / bad index degrades to `None`, not a crash.

use crate::{DomNode, LayoutBox, RenderPipeline, Viewport};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A live, mutable document: the parsed DOM plus the viewport it lays out at and a dirty flag
/// the embedder polls to decide when to recompute layout.
///
/// The DOM is owned here. The browser shares ONE `DomDocument` between its render loop and the
/// JS host object by holding it behind `Rc<RefCell<DomDocument>>` (interior mutability lives
/// at the app layer, where the JS binding needs it) — this type stays a plain owned struct so
/// the engine's own (non-JS) callers pay nothing.
pub struct DomDocument {
    dom: DomNode,
    css: String,
    viewport: Viewport,
    dirty: bool,
}

impl DomDocument {
    /// Parse `html` into a live document laid out at `width`×`height`, with `css` as the
    /// author/UA stylesheet. The document starts clean (a fresh parse needs no re-layout
    /// beyond the embedder's first render).
    pub fn parse(html: &str, css: &str, width: f32, height: f32) -> Self {
        DomDocument {
            dom: crate::parse_html(html),
            css: css.to_string(),
            viewport: Viewport::new(width, height),
            dirty: false,
        }
    }

    /// Borrow the underlying DOM tree (read-only) — for the embedder's own traversals
    /// (anchor hit-testing, title extraction, …).
    pub fn dom(&self) -> &DomNode {
        &self.dom
    }

    /// The `textContent` of the first element with `id`, or `None` if no such element. This
    /// is the JS `document.getElementById(id).textContent` *read* path.
    pub fn get_element_text(&self, id: &str) -> Option<String> {
        self.dom.get_element_by_id(id).map(|n| n.text_content())
    }

    /// Whether an element with `id` exists (so the binding can return a handle vs `null`).
    pub fn has_element(&self, id: &str) -> bool {
        self.dom.get_element_by_id(id).is_some()
    }

    /// Set the `textContent` of the first element with `id` and mark the document dirty.
    /// Returns `true` if the element existed (the write landed), `false` otherwise — so a
    /// script touching a missing id degrades cleanly. This is the JS
    /// `document.getElementById(id).textContent = value` *write* path.
    pub fn set_text_content(&mut self, id: &str, value: &str) -> bool {
        if let Some(node) = self.dom.get_element_by_id_mut(id) {
            node.set_text_content(value);
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Read an element's attribute (`getAttribute`), or `None`.
    pub fn get_attribute(&self, id: &str, name: &str) -> Option<String> {
        self.dom
            .get_element_by_id(id)
            .and_then(|n| n.get_attribute(name).map(|s| s.to_string()))
    }

    /// Set an element's attribute (`setAttribute`) and mark dirty. `true` if the element
    /// existed. Setting `id`/`class` updates the parsed element state (see
    /// [`DomNode::set_attribute`]).
    pub fn set_attribute(&mut self, id: &str, name: &str, value: &str) -> bool {
        if let Some(node) = self.dom.get_element_by_id_mut(id) {
            node.set_attribute(name, value);
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Has the DOM been mutated since the last [`take_dirty`](Self::take_dirty)?
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Read-and-clear the dirty flag: returns whether a re-layout is needed, then resets it.
    /// The embedder calls this once per frame/turn — if `true`, it re-renders.
    pub fn take_dirty(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }

    /// Lay out the CURRENT (possibly mutated) DOM and return the box tree. Reflects every
    /// `set_text_content`/`set_attribute` applied so far — this is how a JS mutation becomes
    /// visible: the embedder re-renders after a dirty turn.
    pub fn render_to_layout(&self) -> LayoutBox {
        let pipeline = RenderPipeline {
            viewport: self.viewport,
        };
        // Re-serialize the live DOM to HTML and run the public pipeline over it, so the
        // cascade + layout see the mutated tree. (The DOM round-trips through the engine's own
        // serializer, which already escapes text correctly.)
        let html = self.serialize();
        pipeline.render_to_layout(&html, &self.css)
    }

    /// Like [`render_to_layout`](Self::render_to_layout) but produces the [`crate::DisplayList`]
    /// the paint bridge consumes.
    pub fn render(&self) -> crate::DisplayList {
        let pipeline = RenderPipeline {
            viewport: self.viewport,
        };
        let html = self.serialize();
        pipeline.render(&html, &self.css)
    }

    /// The current document serialized back to HTML (the live tree, post-mutation).
    pub fn serialize(&self) -> String {
        self.dom.inner_html()
    }

    /// Record on the live DOM that element `id` has a listener for `event_type`, tagged with
    /// `callback_id`. This populates athweb's [`crate::EventListener`] seam so the engine's own
    /// node carries the fact a listener exists; the actual JS callable is held by the embedder
    /// (the browser keys it by the same `(id, event_type, callback_id)`). Returns `true` if the
    /// element existed. Does NOT mark the document dirty — adding a listener changes no layout.
    ///
    /// (The seam is informational/inspectable: a re-layout serializes+reparses the DOM and so
    /// does not preserve these entries — the embedder's registry, keyed by element id, is the
    /// durable source of truth for dispatch. Keeping the seam populated mirrors the real DOM and
    /// lets a future engine-internal dispatcher find listeners without the embedder.)
    pub fn register_event_listener(
        &mut self,
        id: &str,
        event_type: &str,
        callback_id: u64,
    ) -> bool {
        if let Some(node) = self.dom.get_element_by_id_mut(id) {
            node.add_event_listener(event_type, callback_id);
            true
        } else {
            false
        }
    }

    /// The chain of ancestor element ids for the element with `id`, **innermost first**
    /// (i.e. `[id, parent_id, …, root_id]`), skipping ancestors that have no id. Used by the
    /// embedder to bubble an event from the hit node up through its id-bearing ancestors. Empty
    /// if `id` is not found. Never panics.
    pub fn ancestor_id_path(&self, id: &str) -> Vec<String> {
        let mut path: Vec<String> = Vec::new();
        // Depth-first search tracking the id-bearing ancestor stack; when we reach the target,
        // the stack (reversed) is the innermost-first path.
        fn walk(
            node: &DomNode,
            target: &str,
            stack: &mut Vec<String>,
            out: &mut Vec<String>,
        ) -> bool {
            let pushed = match node.element_id() {
                Some(eid) => {
                    stack.push(eid.to_string());
                    true
                }
                None => false,
            };
            let found = if node.element_id() == Some(target) {
                // Emit the stack innermost-first.
                for s in stack.iter().rev() {
                    out.push(s.clone());
                }
                true
            } else {
                let mut f = false;
                for child in &node.children {
                    if walk(child, target, stack, out) {
                        f = true;
                        break;
                    }
                }
                f
            };
            if pushed {
                stack.pop();
            }
            found
        }
        let mut stack: Vec<String> = Vec::new();
        walk(&self.dom, id, &mut stack, &mut path);
        path
    }
}
