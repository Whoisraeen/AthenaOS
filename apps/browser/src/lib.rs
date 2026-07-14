//! AthenaOS Browser — *"the web is the universal app runtime; PWAs that feel
//! native"* (LEGACY_GAMING_CONCEPT.md §"3. Web apps via PWA support that actually feels
//! native (renders through AthUI)"; §Core Principles: "the web runs via the
//! browser/PWA path, never as an OS component").
//!
//! The biggest missing first-party app. It wires the two already-built, already-
//! host-KAT'd web engines into a clickable browser:
//!   * `raeweb` — HTML tokenizer → parser → DOM → CSS cascade → layout → paint.
//!     [`raeweb::RenderPipeline::render_to_layout`] gives the laid-out box tree;
//!     [`raeweb::backend::paint_displaylist_to_canvas`] blits the display list
//!     through the SAME crisp-AA `raegfx::Canvas` path every AthUI surface uses —
//!     the literal meaning of "renders through AthUI".
//!   * `rae_js` — a real tree-walking ECMAScript interpreter. An inline `<script>`
//!     is extracted from the page and executed via [`rae_js::Interpreter::eval_str`]
//!     for its evaluation + `console.*` side effects; the output is captured with
//!     [`rae_js::Interpreter::take_console_output`]. Budget-bounded: a hostile
//!     script returns a positioned error, it never hangs or panics the app.
//!
//! ## Honest scope (v1 — what is PROVEN vs DEFERRED)
//! * PROVEN: open a LOCAL `.html` resource, render it through the real layout
//!   engine to the app canvas (software raster, no GPU this session), extract +
//!   execute its inline `<script>`, and observe the script's result via captured
//!   console output. Real back/forward/reload/address-bar navigation over the
//!   local content set.
//! * PROVEN (v2 — the engine-gap close): **live JS → DOM mutation**. A `document`
//!   host object (`rae_js::HostObject` bound to a shared `raeweb::DomDocument`, see
//!   [`dom_js`]) is installed before the page's inline scripts run, so
//!   `document.getElementById('out').textContent = 'new'` mutates the real DOM and
//!   marks it dirty; `render_page` then re-lays-out the mutated tree so the change
//!   shows. Reads (`el.textContent`), `getAttribute`/`setAttribute`, and a missing
//!   id (→ JS `null`, degrades cleanly) are covered too. Element handles are keyed
//!   by `id` in v1.
//! * PROVEN (v4 — interactivity): `el.addEventListener('click', fn)` registers a JS
//!   callback against the element's id, and a click (synthetic by id via
//!   [`InteractivePage::click_node`], or by pixel coordinate via
//!   [`InteractivePage::click_at`] using `raeweb::hit_test`) invokes the handler in
//!   the live interpreter, bubbles to id-bearing ancestors, drains the microtask/
//!   timer loop, and re-lays-out so a handler's DOM mutation shows. A throwing or
//!   runaway handler is budget-bounded and surfaced as an error — the page never
//!   panics. `removeEventListener` (identity match) un-registers. See [`events`].
//! * DEFERRED: `createElement`/`appendChild` (structural mutation beyond text/attrs),
//!   id-less element handles (dispatch keys on `id`; hit-testing an id-less node finds
//!   no listener), a rich `Event` object argument (`event.target`/`preventDefault`),
//!   `onclick="…"` attribute handlers, and `innerHTML` writes (parsing a fragment back
//!   into the tree). Stated plainly, not faked.
//! * PROVEN (v3 — network fetch over a MOCK transport): a typed `http://` URL is
//!   fetched through `raeweb::loader::fetch_document` over an injectable
//!   [`raenet::http1::HttpTransport`], then run through the SAME render+execute path
//!   as a local page (parse → cascade → layout → paint, `document` host object,
//!   inline `<script>`s, re-layout on DOM mutation). The host KAT injects
//!   [`raenet::http1::MockTransport`] (a canned HTTP/1.1 response) and asserts the
//!   fetch happened (right host/path), the layout carries the page's text, and the
//!   script executed; failure modes (connection error / non-200 / malformed) render
//!   an HONEST error page, never a panic or a blank surface. Body size is bounded by
//!   the loader's `Limits` so a hostile response cannot OOM. See [`BrowserModel::fetch_and_render`].
//! * DEFERRED (iron-gated): LIVE connectivity. The live ELF wraps a real socket in
//!   `RaeSocketTransport` (DNS resolve + TCP connect + send/recv over the raekit
//!   socket syscalls), but actually reaching a server needs the kernel networking at
//!   DHCP-Bound (the RTL8125 RX/DHCP work on iron). Until that lands, a live fetch
//!   resolves cleanly to the error page. `https://` has a verified TLS 1.3 fetch
//!   path BUILT and host-proven in `raenet` (`HttpsClient::get_over` — validates the
//!   cert chain to a trusted root, binds the hostname to the cert SAN, and verifies
//!   CertificateVerify + the server Finished, fail-closed; see `cargo test -p raenet
//!   --features tls13`). It is not yet linked into this live ELF because the
//!   RustCrypto TLS stack (sha2/aes-gcm/polyval) does not codegen for the soft-float,
//!   no-SSE `x86_64-unknown-none` userspace target (CLAUDE.md pitfall #14). Until that
//!   is resolved, typing an `https://` URL shows an honest "not wired in yet" page —
//!   the browser NEVER falls back to an unverified/plaintext secure connection.
//!
//! The navigation + load/render/script DECISION logic lives in the syscall-free
//! [`BrowserModel`] so the host KAT (`cargo test -p browser --features host`) links
//! the LIVE `raeweb` + `rae_js` engines with no kernel: feed a known HTML string
//! (with CSS + an inline `<script>`), assert the rendered LAYOUT contains the
//! expected element box + heading text (real layout), AND assert the inline JS
//! EXECUTED (captured `console.log` output matches the computed value).

// no_std for the real userspace ELF; std under `cargo test` so the host KAT can
// link. The live ELF entry point lives in the thin `src/main.rs` bin, which calls
// `run()` below. (`run` uses `Canvas::new`, which is `unsafe`, so the LIBRARY
// cannot `#![forbid(unsafe_code)]` — the unsafe sites are the surface-buffer
// Canvas, documented.)
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod dom_js;
pub mod events;
pub mod tabs;

use alloc::string::String;
use alloc::vec::Vec;

use raeweb::{LayoutBox, RenderPipeline};

#[cfg(not(test))]
use raeweb::backend::paint_displaylist_to_canvas;

// The render/run path is live-ELF only; under `cargo test` only the BrowserModel
// (over raeweb + rae_js) is exercised, so the graphics/syscall imports are gated
// out to keep the host test warning-clean.
#[cfg(not(test))]
#[allow(unused_imports)]
use raekit;

#[cfg(not(test))]
use rae_tokens::DARK;
#[cfg(not(test))]
use raegfx::text::FontFamily;
#[cfg(not(test))]
use raegfx::Canvas;

// ── Window geometry (live ELF only) ──────────────────────────────────────

#[cfg(not(test))]
const WIN_W: usize = 760;
#[cfg(not(test))]
const WIN_H: usize = 620;
#[cfg(not(test))]
const SURFACE_VIRT: u64 = 0x0000_7C00_0000;

#[cfg(not(test))]
const TITLE_H: usize = 28;
/// Toolbar holding back/forward/reload + the address bar.
#[cfg(not(test))]
const TOOLBAR_H: usize = 40;
/// Status strip at the bottom (URL hover / script result).
#[cfg(not(test))]
const STATUS_H: usize = 24;

#[cfg(not(test))]
const PRESENT_X: i32 = 120;
#[cfg(not(test))]
const PRESENT_Y: i32 = 60;

/// The clickable toolbar control regions (surface-local px).
#[cfg(not(test))]
const BTN_W: usize = 32;

// ── Theme (live ELF only — the host KAT exercises only the BrowserModel) ──

#[cfg(not(test))]
const CHROME: u32 = DARK.bg_raised;
#[cfg(not(test))]
const VIEW_BG: u32 = 0xFF_FF_FF_FF; // pages assume a white canvas by default
#[cfg(not(test))]
const FIELD_BG: u32 = DARK.bg_base;
#[cfg(not(test))]
const STROKE: u32 = DARK.stroke_subtle;
#[cfg(not(test))]
const TEXT_PRIMARY: u32 = DARK.text_primary;
#[cfg(not(test))]
const TEXT_SECONDARY: u32 = DARK.text_secondary;
#[cfg(not(test))]
const TEXT_TERTIARY: u32 = DARK.text_tertiary;

#[cfg(not(test))]
fn accent() -> u32 {
    rae_tokens::derive_accent(raekit::sys::theme_accent(), &DARK).base
}

// ===========================================================================
// BrowserModel — the syscall-free heart (host-KAT'd against the live engines).
// ===========================================================================

/// The viewport width the model lays pages out at (matches the live web-view
/// content width). A fixed width keeps the host KAT's computed geometry
/// deterministic.
pub const VIEWPORT_W: f32 = 744.0;
/// The viewport height the model lays pages out at.
pub const VIEWPORT_H: f32 = 520.0;

/// The outcome of loading + rendering + scripting one document — everything the
/// chrome (and the host KAT) needs to observe, with NO syscalls involved.
pub struct LoadedPage {
    /// The address the page was loaded from (a local path or `about:` URL).
    pub url: String,
    /// The laid-out box tree from the LIVE `raeweb` layout engine. The chrome
    /// paints it; the KAT asserts element boxes + text against it.
    pub layout: LayoutBox,
    /// Every line the page's inline `<script>` wrote to `console.*` while it ran
    /// (empty if the page had no script, or the script wrote nothing).
    pub console: Vec<String>,
    /// `Some(msg)` if the inline script failed to parse/run (positioned error from
    /// `rae_js`); the page still renders — a broken script never blocks the DOM.
    pub script_error: Option<String>,
    /// The document's HTML, retained so the live paint path can re-derive the
    /// engine's [`raeweb::DisplayList`] each frame (the public paint bridge consumes
    /// a display list, not a layout tree).
    pub html: String,
    /// The page-level stylesheet, retained for the same reason.
    pub css: String,
}

impl LoadedPage {
    /// The visible text of the first element matching `tag` in the laid-out tree,
    /// or `None`. Used by the chrome's title heuristic and by the host KAT to prove
    /// the layout carries real, parsed text (not an empty box).
    pub fn first_text_of(&self, tag: &str) -> Option<String> {
        first_layout_text(&self.layout, tag)
    }

    /// The border-box `(x, y, w, h)` of the first element with `id`, walking the
    /// LIVE layout tree. Proves the cascade + layout produced a real computed box.
    pub fn box_of_id(&self, id: &str) -> Option<(f32, f32, f32, f32)> {
        first_box_of_id(&self.layout, id)
    }

    /// The total number of layout boxes (a coarse "did the tree actually build"
    /// signal).
    pub fn box_count(&self) -> usize {
        count_boxes(&self.layout)
    }
}

/// One entry in the navigation history (just the resource address; the content is
/// re-loaded on visit so back/forward always reflect the current local file).
#[derive(Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub url: String,
}

/// The browser's syscall-free core: a back/forward history stack and the
/// load→render→script pipeline over the LIVE engines. The live `App` owns one of
/// these and feeds it raw HTML/CSS it read from disk; the host KAT feeds it
/// in-memory strings.
pub struct BrowserModel {
    pipeline: RenderPipeline,
    /// The visited addresses, oldest → newest.
    history: Vec<HistoryEntry>,
    /// The index into `history` of the currently-shown page (`usize::MAX` = none).
    cursor: usize,
}

impl Default for BrowserModel {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserModel {
    /// A fresh model with an empty history, laying out at the content viewport.
    pub fn new() -> BrowserModel {
        BrowserModel {
            pipeline: RenderPipeline::new(VIEWPORT_W, VIEWPORT_H),
            history: Vec::new(),
            cursor: usize::MAX,
        }
    }

    /// Render `html` + `css` for the resource at `url`, run its inline `<script>`,
    /// and return the [`LoadedPage`]. Pure: it does NOT touch history (the caller
    /// records navigation). This is the single function the host KAT drives.
    ///
    /// The CSS fed here is the page-level stylesheet (the live app concatenates
    /// every `<style>` block it extracted from the document; the KAT passes one).
    pub fn render_page(&self, url: &str, html: &str, css: &str) -> LoadedPage {
        // Run the page's inline scripts against a LIVE document binding: a `document` host
        // object (bound to a shared raeweb DOM) is installed before execution, so
        // `document.getElementById('x').textContent = '…'` mutates the real tree. The
        // returned `mutated_html` is the document serialized AFTER the scripts ran — if a
        // script changed the DOM, it differs from the input and we lay THAT out (so the
        // change shows). If no script touched the DOM, it round-trips unchanged.
        let ScriptRun {
            console,
            script_error,
            mutated_html,
            dom_mutated,
        } = run_inline_scripts_with_dom(html, css);

        // Lay out the post-script DOM when it was mutated; else the original (identical
        // result, but avoids a needless re-serialize round-trip for the common static page).
        let (layout, render_html) = if dom_mutated {
            (
                self.pipeline.render_to_layout(&mutated_html, css),
                mutated_html,
            )
        } else {
            (
                self.pipeline.render_to_layout(html, css),
                String::from(html),
            )
        };

        LoadedPage {
            url: String::from(url),
            layout,
            console,
            script_error,
            html: render_html,
            css: String::from(css),
        }
    }

    /// The display list for `page`, for the live paint bridge. Re-renders through
    /// the engine's public pipeline (cheap for v1's small local docs).
    #[cfg(not(test))]
    pub fn display_list_for(&self, page: &LoadedPage) -> raeweb::DisplayList {
        self.pipeline.render(&page.html, &page.css)
    }

    /// Fetch `url` over the injected HTTP transport and render it through the SAME
    /// parse → cascade → layout → paint + inline-`<script>` execution path the local
    /// loader uses. This is the network front of the browser, host-testable because
    /// `transport` is any [`raenet::http1::HttpTransport`] — the host KAT passes a
    /// [`raenet::http1::MockTransport`] returning a canned HTTP/1.1 response; the live
    /// app passes a real socket transport.
    ///
    /// Always returns a `LoadedPage` (never an `Err`) so the chrome has something to
    /// paint no matter what: a fetch/transport failure, a non-2xx status, or a
    /// malformed body all resolve to an HONEST error page (a real HTML document run
    /// through the engine), never a panic or a blank surface. The response body size
    /// is bounded by the loader's [`raenet::http1::Limits`] (16 MiB default) so a
    /// hostile/huge response cannot OOM the app.
    ///
    /// `<style>` blocks in the fetched document are extracted and cascaded; any
    /// `<script>` runs against the live `document` binding (DOM mutation shows).
    pub fn fetch_and_render<T: raenet::http1::HttpTransport>(
        &self,
        url: &str,
        transport: &mut T,
    ) -> LoadedPage {
        match raeweb::loader::fetch_document(url, transport) {
            Ok(resource) => {
                // Honest server-error handling: a non-2xx is a real navigation result, not a
                // transport failure — render the server's own body if it sent one, else a
                // synthesized status page. Either way it goes through the live engine.
                if !(200..300).contains(&resource.status) {
                    let body = resource.as_text();
                    let doc = if body.trim().is_empty() {
                        http_status_error_doc(url, resource.status)
                    } else {
                        body
                    };
                    let css = extract_styles(&doc);
                    return self.render_page(url, &doc, &css);
                }
                let html = resource.as_text();
                // Cascade inline <style> PLUS any external <link rel=stylesheet>, fetched
                // over the SAME transport, resolved relative to the page URL. A linked
                // sheet that 404s / fails is skipped — the page still renders.
                let css = collect_page_css(url, &html, transport);
                self.render_page(url, &html, &css)
            }
            Err(e) => {
                let doc = load_error_doc(url, &e);
                let css = extract_styles(&doc);
                self.render_page(url, &doc, &css)
            }
        }
    }

    /// Record a navigation to `url`. Truncates any forward history (the standard
    /// browser model: visiting a new page from the middle of history discards the
    /// pages you had gone "back" past) and appends the new entry as current.
    pub fn navigate(&mut self, url: &str) {
        // Drop forward history.
        if self.cursor != usize::MAX && self.cursor + 1 < self.history.len() {
            self.history.truncate(self.cursor + 1);
        }
        self.history.push(HistoryEntry {
            url: String::from(url),
        });
        self.cursor = self.history.len() - 1;
    }

    /// Whether a [`back`](Self::back) is possible.
    pub fn can_go_back(&self) -> bool {
        self.cursor != usize::MAX && self.cursor > 0
    }

    /// Whether a [`forward`](Self::forward) is possible.
    pub fn can_go_forward(&self) -> bool {
        self.cursor != usize::MAX && self.cursor + 1 < self.history.len()
    }

    /// Move the history cursor back one entry and return its URL (the caller
    /// re-loads it). `None` if already at the start.
    pub fn back(&mut self) -> Option<&str> {
        if self.can_go_back() {
            self.cursor -= 1;
            Some(self.history[self.cursor].url.as_str())
        } else {
            None
        }
    }

    /// Move the history cursor forward one entry and return its URL. `None` if
    /// already at the newest entry.
    pub fn forward(&mut self) -> Option<&str> {
        if self.can_go_forward() {
            self.cursor += 1;
            Some(self.history[self.cursor].url.as_str())
        } else {
            None
        }
    }

    /// The URL of the currently-shown page (the reload target), or `None`.
    pub fn current_url(&self) -> Option<&str> {
        if self.cursor == usize::MAX {
            None
        } else {
            self.history.get(self.cursor).map(|e| e.url.as_str())
        }
    }

    /// Number of entries in the history (for tests / the chrome's hint).
    pub fn history_len(&self) -> usize {
        self.history.len()
    }
}

/// Extract the contents of every top-level `<style>` block in `html` and
/// concatenate them into one stylesheet string. A real document's CSS lives in
/// `<style>` (and `<link>`, fetched separately by [`collect_page_css`]); pulling the
/// `<style>` text out lets the cascade see author rules without the engine treating
/// them as text.
///
/// Never panics: an unterminated `<style>` simply takes the rest of the document.
pub fn extract_styles(html: &str) -> String {
    let mut css = String::new();
    let bytes = html.as_bytes();
    let lower = to_ascii_lower(html);
    let mut search_from = 0usize;
    while let Some(open) = find_from(&lower, "<style", search_from) {
        // Find the end of the opening tag (`>`).
        let after_open = match find_from(&lower, ">", open) {
            Some(gt) => gt + 1,
            None => break,
        };
        let close = find_from(&lower, "</style>", after_open).unwrap_or(bytes.len());
        if after_open <= close {
            css.push_str(&html[after_open..close]);
            css.push('\n');
        }
        search_from = if close < bytes.len() {
            close + 8
        } else {
            bytes.len()
        };
    }
    css
}

/// Collect the `href` of every `<link rel="stylesheet">` in `html`, in document
/// order. `rel` is matched token-wise (case-insensitive), so `rel="stylesheet"`,
/// `rel="alternate stylesheet"`, and `rel="STYLESHEET"` all qualify; a `<link>`
/// without an `href`, or one that is not a stylesheet (`rel="icon"` etc.), is
/// skipped. Uses raeweb's public DOM (`parse_html` → walk `tag_name`/`get_attribute`)
/// so it sees the real parsed tree, not a regex over text.
///
/// Never panics: a malformed document simply yields whatever links parsed.
pub fn link_stylesheet_hrefs(html: &str) -> Vec<String> {
    let dom = raeweb::parse_html(html);
    let mut out: Vec<String> = Vec::new();
    collect_link_hrefs(&dom, &mut out);
    out
}

/// Recursive DOM walk collecting stylesheet `<link>` hrefs in document order.
fn collect_link_hrefs(node: &raeweb::DomNode, out: &mut Vec<String>) {
    if node.tag_name() == Some("link") {
        let is_stylesheet = node
            .get_attribute("rel")
            .map(|rel| {
                rel.split_whitespace()
                    .any(|tok| tok.eq_ignore_ascii_case("stylesheet"))
            })
            .unwrap_or(false);
        if is_stylesheet {
            if let Some(href) = node.get_attribute("href") {
                let href = href.trim();
                if !href.is_empty() {
                    out.push(String::from(href));
                }
            }
        }
    }
    for child in &node.children {
        collect_link_hrefs(child, out);
    }
}

/// Build the page's full stylesheet for the cascade: the inline `<style>` blocks
/// (from [`extract_styles`]) followed by every external `<link rel="stylesheet">`
/// fetched over `transport` and resolved relative to `base_url`.
///
/// Document-order note: inline `<style>` text is concatenated first, then the linked
/// sheets in document order. The cascade is order-sensitive only for equal-specificity
/// rules; author `<style>` and `<link>` are both author-origin, so this preserves the
/// common case (a linked framework sheet overridden by a later inline `<style>` is the
/// rare case and is a known v1 simplification, stated plainly). Linked sheets that fail
/// to fetch are skipped — a broken stylesheet never fails the page.
fn collect_page_css<T: raenet::http1::HttpTransport>(
    base_url: &str,
    html: &str,
    transport: &mut T,
) -> String {
    let mut css = extract_styles(html);
    let hrefs = link_stylesheet_hrefs(html);
    if hrefs.is_empty() {
        return css;
    }
    // Resolve each href to an absolute http:// URL; drop the ones we can't/won't fetch.
    let mut urls: Vec<String> = Vec::new();
    for href in &hrefs {
        if let Some(abs) = raeweb::loader::resolve_stylesheet_url(base_url, href) {
            urls.push(abs);
        }
    }
    if !urls.is_empty() {
        let linked =
            raeweb::loader::fetch_stylesheets(&urls, transport, &raenet::http1::Limits::new());
        if !linked.is_empty() {
            css.push('\n');
            css.push_str(&linked);
        }
    }
    css
}

/// Run every top-level `<script>` block's contents through `rae_js`, in document
/// order, sharing ONE interpreter so later scripts see earlier globals (the real
/// browser semantics). Returns the concatenated console output and the FIRST
/// script error encountered (if any).
///
/// **No DOM binding** — this is the evaluation-only helper (kept for the
/// console-only call sites/tests). `<script src="...">` (external) is skipped (no
/// fetch). A script that touches `document` here gets a `ReferenceError`, surfaced
/// as the error, not a crash. For the live DOM-mutation path the browser actually
/// renders through, use [`run_inline_scripts_with_dom`].
pub fn run_inline_scripts(html: &str) -> (Vec<String>, Option<String>) {
    let scripts = extract_scripts(html);
    if scripts.is_empty() {
        return (Vec::new(), None);
    }
    let mut interp = rae_js::Interpreter::new();
    let mut out: Vec<String> = Vec::new();
    let mut first_err: Option<String> = None;
    for src in &scripts {
        match interp.eval_str(src) {
            Ok(_) => {}
            Err(e) => {
                if first_err.is_none() {
                    let mut msg = String::from("script error: ");
                    // JsError implements Display; build the string without std fmt
                    // machinery beyond what alloc provides.
                    msg.push_str(&e.message);
                    first_err = Some(msg);
                }
            }
        }
        // Drain whatever this script logged (shared interpreter, but we collect as
        // we go so output order is document order).
        for line in interp.take_console_output() {
            out.push(line);
        }
    }
    (out, first_err)
}

/// The result of running a page's inline scripts against a live DOM binding.
pub struct ScriptRun {
    /// Concatenated `console.*` output (document order).
    pub console: Vec<String>,
    /// First script error (parse/run), if any — the page still renders.
    pub script_error: Option<String>,
    /// The document serialized AFTER the scripts ran (reflects any DOM mutation a script
    /// performed via the `document` binding). Equal in content to the input when no script
    /// touched the DOM.
    pub mutated_html: String,
    /// Whether any script mutated the DOM (so the caller re-lays-out `mutated_html`).
    pub dom_mutated: bool,
}

/// Run every top-level inline `<script>` through `rae_js` with a LIVE `document` binding
/// installed — the engine-gap closer. A `document` host object backed by a shared
/// [`raeweb::DomDocument`] (parsed from `html`+`css`) is defined as a global before the
/// scripts run, so `document.getElementById('out').textContent = 'new'` mutates the real
/// DOM. After the scripts run, the (possibly mutated) document is serialized back to HTML so
/// the caller can re-lay-out it.
///
/// Scripts share ONE interpreter (later scripts see earlier globals) AND one document (later
/// scripts see earlier DOM mutations) — the real browser semantics. `<script src>` is still
/// skipped (no fetch in v1). Never panics: a script referencing a missing id reads `null`
/// and a `.textContent` on it throws a TypeError that is surfaced as `script_error`, not a
/// crash.
pub fn run_inline_scripts_with_dom(html: &str, css: &str) -> ScriptRun {
    let scripts = extract_scripts(html);
    let mut interp = rae_js::Interpreter::new();
    // Install the `document` binding bound to a shared DOM, even if there are no scripts
    // (cheap; keeps the path uniform). The viewport matches the model's layout viewport so
    // any geometry a future script reads is consistent.
    let doc = dom_js::install_document(&mut interp, html, css, VIEWPORT_W, VIEWPORT_H);

    let mut console: Vec<String> = Vec::new();
    let mut script_error: Option<String> = None;
    for src in &scripts {
        if let Err(e) = interp.eval_str(src) {
            if script_error.is_none() {
                let mut msg = String::from("script error: ");
                msg.push_str(&e.message);
                script_error = Some(msg);
            }
        }
        for line in interp.take_console_output() {
            console.push(line);
        }
    }

    // Read back the mutation state + serialized DOM before the interpreter (and its
    // `document` host, which holds an Rc to `doc`) drop.
    let dom_mutated = doc.borrow().is_dirty();
    let mutated_html = if dom_mutated {
        doc.borrow().serialize()
    } else {
        String::new()
    };
    ScriptRun {
        console,
        script_error,
        mutated_html,
        dom_mutated,
    }
}

/// A loaded page that stays INTERACTIVE: it keeps the live `rae_js` interpreter, the shared
/// `raeweb` document, and the event-listener registry alive after the initial render, so a
/// later click can invoke the handlers a page registered with `addEventListener` — and the
/// DOM mutations those handlers make become visible on re-layout.
///
/// This is the deferred interactivity layer: [`run_inline_scripts_with_dom`] renders + runs
/// load-time scripts then drops everything (fine for a static snapshot), whereas an
/// `InteractivePage` is what the live browser holds for the page it is currently showing.
///
/// Lifecycle:
///   1. [`InteractivePage::load`] — parse, install `document`, run inline `<script>`s (which
///      may call `addEventListener`), and lay out the post-script DOM.
///   2. [`InteractivePage::click_node`] / [`InteractivePage::click_at`] — dispatch a `click`,
///      run the handlers, drain the event loop, and (if the DOM changed) re-lay-out.
///   3. [`InteractivePage::layout`] — the current box tree (post any dispatch) to paint.
///
/// Never panics: a handler that throws or runs away is bounded by the interpreter budget and
/// surfaced as an error; the page stays alive and renderable.
pub struct InteractivePage {
    interp: rae_js::Interpreter,
    doc: dom_js::SharedDoc,
    registry: events::SharedRegistry,
    pipeline: RenderPipeline,
    layout: LayoutBox,
    css: String,
    /// First load-time script error (the page still rendered), if any.
    pub script_error: Option<String>,
    /// Load-time console output (document order).
    pub console: Vec<String>,
}

impl InteractivePage {
    /// Parse `html`+`css`, install the live `document` binding + listener registry, run the
    /// page's inline `<script>`s (registering any `addEventListener` callbacks), and lay out
    /// the resulting DOM. The interpreter/doc/registry are retained for later click dispatch.
    pub fn load(html: &str, css: &str) -> InteractivePage {
        let mut interp = rae_js::Interpreter::new();
        let (doc, registry) =
            dom_js::install_document_interactive(&mut interp, html, css, VIEWPORT_W, VIEWPORT_H);
        let scripts = extract_scripts(html);
        let mut console: Vec<String> = Vec::new();
        let mut script_error: Option<String> = None;
        for src in &scripts {
            if let Err(e) = interp.eval_str(src) {
                if script_error.is_none() {
                    let mut msg = String::from("script error: ");
                    msg.push_str(&e.message);
                    script_error = Some(msg);
                }
            }
            for line in interp.take_console_output() {
                console.push(line);
            }
        }
        // Drain any load-time async work (a top-level `setTimeout`/Promise), then clear dirty
        // (the first layout below reflects all of it).
        let _ = interp.run_event_loop();
        for line in interp.take_console_output() {
            console.push(line);
        }
        let pipeline = RenderPipeline::new(VIEWPORT_W, VIEWPORT_H);
        let layout = doc.borrow().render_to_layout();
        doc.borrow_mut().take_dirty();
        InteractivePage {
            interp,
            doc,
            registry,
            pipeline,
            layout,
            css: String::from(css),
            script_error,
            console,
        }
    }

    /// The current box tree to paint (reflects every dispatch so far).
    pub fn layout(&self) -> &LayoutBox {
        &self.layout
    }

    /// Whether the page registered any `click` listener on element `id`.
    pub fn has_click_listener(&self, id: &str) -> bool {
        self.registry.borrow().has(id, "click")
    }

    /// The `textContent` of element `id` in the live DOM (for tests/inspection).
    pub fn element_text(&self, id: &str) -> Option<String> {
        self.doc.borrow().get_element_text(id)
    }

    /// Re-lay-out the DOM if a dispatch dirtied it; returns whether a re-layout happened.
    fn relayout_if_dirty(&mut self) -> bool {
        if self.doc.borrow_mut().take_dirty() {
            self.layout = self.doc.borrow().render_to_layout();
            true
        } else {
            false
        }
    }

    /// Dispatch a synthetic `click` to element `id`, bubbling up its id-bearing ancestors,
    /// then re-lay-out if a handler mutated the DOM. This is the by-id entry point — no pixel
    /// coordinates needed. Returns the [`events::DispatchResult`] (handlers fired + any error).
    /// Console output a handler produced is appended to [`InteractivePage::console`].
    pub fn click_node(&mut self, id: &str) -> events::DispatchResult {
        let path = self.doc.borrow().ancestor_id_path(id);
        // ancestor_id_path is empty if the id is absent; dispatch to just `id` is then a no-op.
        let result = events::dispatch_click_by_id(&mut self.interp, &self.registry, &path);
        for line in self.interp.take_console_output() {
            self.console.push(line);
        }
        self.relayout_if_dirty();
        result
    }

    /// Hit-test the current layout at `(x, y)` to a DOM element id, then dispatch a `click` to
    /// it (bubbling). Returns `None` if the point hit no id-bearing element; otherwise the
    /// [`events::DispatchResult`]. This is the pixel entry point the live browser drives from a
    /// real pointer click.
    pub fn click_at(&mut self, x: f32, y: f32) -> Option<events::DispatchResult> {
        let hit = raeweb::hit_test(&self.layout, x, y)?;
        let id = hit.node_id?;
        Some(self.click_node(&id))
    }

    /// The display list for the current layout, for the live paint bridge. Re-renders the
    /// live (post-dispatch) DOM through the engine's public pipeline.
    pub fn display_list(&self) -> raeweb::DisplayList {
        self.pipeline
            .render(&self.doc.borrow().serialize(), &self.css)
    }
}

/// Pull the inline source of each `<script>` that is NOT `src=`-loaded.
fn extract_scripts(html: &str) -> Vec<String> {
    let mut scripts = Vec::new();
    let lower = to_ascii_lower(html);
    let bytes = html.as_bytes();
    let mut search_from = 0usize;
    while let Some(open) = find_from(&lower, "<script", search_from) {
        let after_open = match find_from(&lower, ">", open) {
            Some(gt) => gt + 1,
            None => break,
        };
        // The opening tag text (e.g. `<script src="x.js">`); skip external scripts.
        let open_tag = &lower[open..after_open];
        let is_external = open_tag.contains("src=");
        let close = find_from(&lower, "</script>", after_open).unwrap_or(bytes.len());
        if !is_external && after_open <= close {
            let body = html[after_open..close].trim();
            if !body.is_empty() {
                scripts.push(String::from(body));
            }
        }
        search_from = if close < bytes.len() {
            close + 9
        } else {
            bytes.len()
        };
    }
    scripts
}

/// ASCII-lowercase copy of `s` (so tag-name matching is case-insensitive without
/// touching the original bytes we slice out for content).
fn to_ascii_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Byte-offset of `needle` in `haystack` at or after `from`, or `None`. Operates on
/// the lowercased copy so offsets line up with the original (ASCII-lowercasing
/// preserves byte length for ASCII; non-ASCII bytes are unaffected by the search
/// terms here, which are all ASCII).
fn find_from(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..].find(needle).map(|i| i + from)
}

/// The visible text of the first layout box whose tag matches `tag`.
fn first_layout_text(node: &LayoutBox, tag: &str) -> Option<String> {
    if node.tag_name.as_deref() == Some(tag) {
        let text = collect_box_text(node);
        if !text.is_empty() {
            return Some(text);
        }
    }
    for child in &node.children {
        if let Some(t) = first_layout_text(child, tag) {
            return Some(t);
        }
    }
    None
}

/// All text under a layout box, in order.
fn collect_box_text(node: &LayoutBox) -> String {
    let mut out = String::new();
    collect_box_text_into(node, &mut out);
    out.trim().into()
}

fn collect_box_text_into(node: &LayoutBox, out: &mut String) {
    if let Some(t) = &node.text {
        out.push_str(t);
    }
    for child in &node.children {
        collect_box_text_into(child, out);
    }
}

/// The border-box of the first layout box with `node_id == id`.
fn first_box_of_id(node: &LayoutBox, id: &str) -> Option<(f32, f32, f32, f32)> {
    if node.node_id.as_deref() == Some(id) {
        let b = node.dimensions.border_box();
        return Some((b.x, b.y, b.width, b.height));
    }
    for child in &node.children {
        if let Some(r) = first_box_of_id(child, id) {
            return Some(r);
        }
    }
    None
}

/// Count every box in the tree.
fn count_boxes(node: &LayoutBox) -> usize {
    1 + node.children.iter().map(count_boxes).sum::<usize>()
}

// ===========================================================================
// Built-in local content (the offline "web" v1 ships).
// ===========================================================================

/// The home page address.
pub const HOME_URL: &str = "about:home";

/// The built-in home document — a real HTML page with CSS + an inline script,
/// rendered through the live engine. Doubles as the demo of "open a local .html
/// and render it" without depending on the filesystem being populated.
pub const HOME_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
<style>
body { background: #0a0e1a; color: #e8eaf0; padding: 24px; }
h1 { color: #6ea8fe; font-size: 28px; }
p { font-size: 16px; }
#tagline { color: #9aa3b2; }
.card { background: #12162a; padding: 16px; }
</style>
</head>
<body>
<h1 id="title">RaeWeb</h1>
<p id="tagline">The web, rendered natively.</p>
<div class="card">
<p>This page was parsed, styled, laid out and painted by the AthenaOS web engine,
then its script ran in the AthenaOS JavaScript interpreter.</p>
</div>
<script>
var greeting = "RaeWeb online: " + (6 * 7);
console.log(greeting);
</script>
</body>
</html>"#;

/// Resolve a built-in/`about:` URL to its document text, or `None` if it is not a
/// known built-in (the live app then tries the filesystem).
pub fn builtin_document(url: &str) -> Option<&'static str> {
    match url {
        HOME_URL => Some(HOME_HTML),
        _ => None,
    }
}

/// How the browser should source the document for a typed URL — the pure routing
/// decision, host-testable so the network/local split is provable without syscalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlKind {
    /// A built-in `about:` page (served from [`builtin_document`]).
    Builtin,
    /// An `http://` URL — fetched over the network via [`BrowserModel::fetch_and_render`].
    Network,
    /// An `https://` URL — network, but the HTTP/1.1 loader is plaintext-only, so this
    /// resolves to an honest "HTTPS not yet supported" error page (TLS is a follow-up).
    NetworkTls,
    /// A `file://` URL or a bare local path — read off the filesystem.
    Local,
}

/// Classify a typed address into its [`UrlKind`]. Case-insensitive on the scheme.
/// `about:` first (so `about:home` never looks like a file), then the network
/// schemes, then everything else is local.
pub fn classify_url(url: &str) -> UrlKind {
    let trimmed = url.trim();
    let lower = to_ascii_lower(trimmed);
    if lower.starts_with("about:") {
        UrlKind::Builtin
    } else if lower.starts_with("http://") {
        UrlKind::Network
    } else if lower.starts_with("https://") {
        UrlKind::NetworkTls
    } else {
        UrlKind::Local
    }
}

/// Build an honest error document for a transport/parse failure — a real HTML page
/// rendered through the live engine (NOT a panic, NOT a blank surface). The error
/// reason is mapped from the loader's [`raeweb::loader::LoadError`] into plain
/// language a user can act on.
pub fn load_error_doc(url: &str, err: &raeweb::loader::LoadError) -> String {
    use raeweb::loader::LoadError;
    let reason = match err {
        LoadError::BadUrl => "The address could not be understood (bad URL or unsupported scheme).",
        LoadError::Transport => {
            "Could not reach the server. The connection was refused, timed out, or no \
             network is available yet."
        }
        LoadError::BadResponse => {
            "The server's response was malformed or could not be displayed as a page."
        }
    };
    error_page("Can't load this page", url, reason)
}

/// Build an honest error document for a non-2xx HTTP status with no usable body.
fn http_status_error_doc(url: &str, status: u16) -> String {
    let mut headline = String::from("Server returned ");
    headline.push_str(&u16_str(status));
    error_page(
        &headline,
        url,
        "The server responded, but not with a page to show.",
    )
}

/// The shared error-page template — a minimal, self-styled HTML document. It is run
/// through the SAME parse→cascade→layout path as any other page, so an error renders
/// like a real page (with an `id="error"` heading the host KAT can assert against).
fn error_page(headline: &str, url: &str, reason: &str) -> String {
    let mut s = String::from(
        "<!DOCTYPE html><html><head><style>\
         body { background: #0a0e1a; color: #e8eaf0; padding: 32px; }\
         h1 { color: #ff6e6e; font-size: 24px; }\
         p { font-size: 15px; color: #9aa3b2; }\
         .url { color: #6ea8fe; }\
         </style></head><body>\
         <h1 id=\"error\">",
    );
    push_escaped(&mut s, headline);
    s.push_str("</h1><p id=\"reason\">");
    push_escaped(&mut s, reason);
    s.push_str("</p><p>Address: <span class=\"url\" id=\"failed-url\">");
    push_escaped(&mut s, url);
    s.push_str("</span></p></body></html>");
    s
}

/// Append `text` to `out`, escaping the HTML-significant characters so a hostile URL
/// or reason string can never inject markup into the error page.
fn push_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// Decimal string for a u16 status code — `no_std`-safe, no fmt machinery.
fn u16_str(mut n: u16) -> String {
    if n == 0 {
        return String::from("0");
    }
    let mut buf = [0u8; 5];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    String::from(core::str::from_utf8(&buf[i..]).unwrap_or("0"))
}

// ===========================================================================
// App state + render (live ELF only — syscall-touching).
// ===========================================================================

/// Which toolbar control the pointer is over / was clicked.
#[cfg(not(test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Back,
    Forward,
    Reload,
    Address,
    None,
}

/// The whole live app.
#[cfg(not(test))]
struct App {
    model: BrowserModel,
    /// The currently-rendered page (None until the first load).
    page: Option<LoadedPage>,
    /// The address bar text being edited.
    address: String,
    /// Whether the address bar has keyboard focus.
    editing_address: bool,
    /// Vertical scroll offset into the web view.
    scroll_y: f32,
    /// A short status line (script result / hover URL / errors).
    status: String,
    shift: bool,
}

#[cfg(not(test))]
impl App {
    fn new() -> App {
        let mut app = App {
            model: BrowserModel::new(),
            page: None,
            address: String::new(),
            editing_address: false,
            scroll_y: 0.0,
            status: String::new(),
            shift: false,
        };
        // The home page is a builtin (no network), so no progress painter needed.
        app.load(HOME_URL, None);
        app
    }

    /// Load `url`: resolve its document (built-in or local file), render it through
    /// the live engine, run its script, and record the navigation.
    fn load(&mut self, url: &str, progress: Option<&mut dyn FnMut(&str)>) {
        self.model.navigate(url);
        self.show(url, progress);
    }

    /// Resolve + render `url` WITHOUT recording navigation (used by back/forward/
    /// reload, which move the cursor themselves).
    ///
    /// `progress` is an optional painter the live loop passes so a blocking network
    /// fetch can show a "Loading…" frame BEFORE it blocks (the local/builtin paths
    /// never block, so it is unused there). It is called with the in-flight URL.
    fn show(&mut self, url: &str, progress: Option<&mut dyn FnMut(&str)>) {
        let owned = String::from(url);

        // Network URLs go through the live HTTP transport; local/builtin URLs read
        // their document synchronously. Either way the result is one LoadedPage.
        let page = match classify_url(&owned) {
            UrlKind::Network => {
                // Loading state: paint a "Loading…" frame before the (blocking) fetch
                // so the user gets feedback the navigation is in flight.
                self.editing_address = false;
                self.address.clear();
                self.address.push_str(&owned);
                self.status.clear();
                self.status.push_str("Loading ");
                self.status.push_str(&owned);
                self.status.push('…');
                if let Some(p) = progress {
                    p(&owned);
                }
                let mut transport = RaeSocketTransport::new();
                self.model.fetch_and_render(&owned, &mut transport)
            }
            UrlKind::NetworkTls => {
                // The verified TLS 1.3 fetch path is BUILT and proven: raenet's
                // `HttpsClient::get_over` completes the handshake and authenticates the
                // server (cert chain → trusted root, hostname → cert SAN,
                // CertificateVerify + server Finished) before any body is accepted, and
                // fails closed on every failure (host-KAT'd: `cargo test -p raenet
                // --features tls13`). It is NOT linked into this live ELF yet because
                // the RustCrypto TLS stack (sha2/aes-gcm/polyval) cannot be lowered by
                // LLVM for the soft-float, no-SSE `x86_64-unknown-none` userspace target
                // (CLAUDE.md pitfall #14 — the same SIMD-split class as the kernel's
                // sha2 force-soft fix; the kernel sidesteps it by not pulling those
                // crates into any ELF). Be HONEST: never fake security with a plaintext
                // fetch — show the real status.
                let doc = error_page(
                    "Secure pages aren't wired into the browser yet",
                    &owned,
                    "AthenaOS has a verified TLS 1.3 fetch path (it validates the \
                     certificate chain, the hostname, and the server's signatures before \
                     loading anything), but it isn't linked into this browser build yet. \
                     Loading an http:// address is the only network option for now — the \
                     browser will never fall back to an unverified secure connection.",
                );
                let css = extract_styles(&doc);
                self.model.render_page(&owned, &doc, &css)
            }
            UrlKind::Builtin | UrlKind::Local => {
                let html = self.resolve(&owned);
                let css = extract_styles(&html);
                self.model.render_page(&owned, &html, &css)
            }
        };
        self.address.clear();
        self.address.push_str(&owned);
        self.scroll_y = 0.0;
        // Surface the script's result (or error) in the status strip.
        self.status.clear();
        if let Some(err) = &page.script_error {
            self.status.push_str(err);
        } else if let Some(first) = page.console.first() {
            self.status.push_str("script: ");
            self.status.push_str(first);
        } else {
            let count = page.box_count();
            self.status.push_str("rendered ");
            self.status.push_str(&usize_str(count));
            self.status.push_str(" boxes");
        }
        self.page = Some(page);
    }

    /// Return the document text for `url`: a built-in page, else a local file read
    /// off disk, else a small "not found" document.
    fn resolve(&self, url: &str) -> String {
        if let Some(doc) = builtin_document(url) {
            return String::from(doc);
        }
        if let Some(doc) = read_local_html(url) {
            return doc;
        }
        let mut s =
            String::from("<html><body><h1>Not found</h1><p id=\"msg\">No local document at ");
        s.push_str(url);
        s.push_str("</p></body></html>");
        s
    }

    fn go_back(&mut self, progress: Option<&mut dyn FnMut(&str)>) {
        if let Some(url) = self.model.back() {
            let owned = String::from(url);
            self.show(&owned, progress);
        }
    }

    fn go_forward(&mut self, progress: Option<&mut dyn FnMut(&str)>) {
        if let Some(url) = self.model.forward() {
            let owned = String::from(url);
            self.show(&owned, progress);
        }
    }

    fn reload(&mut self, progress: Option<&mut dyn FnMut(&str)>) {
        if let Some(url) = self.model.current_url() {
            let owned = String::from(url);
            self.show(&owned, progress);
        }
    }

    /// Commit the address bar: navigate to whatever was typed.
    fn commit_address(&mut self, progress: Option<&mut dyn FnMut(&str)>) {
        let url = self.address.clone();
        self.editing_address = false;
        if !url.is_empty() {
            self.load(&url, progress);
        }
    }
}

// ── Live HTTP transport over raekit sockets (live ELF only) ─────────────────

/// The real [`raenet::http1::HttpTransport`] the live browser fetches through:
/// DNS-resolve the host, open a TCP socket, send the request, and drain the reply.
/// This is the production peer of the host KAT's `MockTransport` — same trait, so the
/// engine's fetch path is identical whether driven by a socket or a canned response.
///
/// Live connectivity is IRON-GATED on the kernel's networking reaching DHCP-Bound
/// (the RTL8125 RX path / DHCP lease). Until that lands on iron, a fetch resolves to
/// the honest error page (DNS/connect fails cleanly → `LoadError::Transport`); the
/// host proof uses the mock, never this transport.
#[cfg(not(test))]
struct RaeSocketTransport {
    fd: Option<u64>,
}

#[cfg(not(test))]
impl RaeSocketTransport {
    fn new() -> RaeSocketTransport {
        RaeSocketTransport { fd: None }
    }
}

#[cfg(not(test))]
impl Drop for RaeSocketTransport {
    fn drop(&mut self) {
        if let Some(fd) = self.fd.take() {
            raekit::sys::sock_close(fd);
        }
    }
}

#[cfg(not(test))]
impl raenet::http1::HttpTransport for RaeSocketTransport {
    fn connect(&mut self, host: &str, port: u16) -> raenet::http1::Http1Result<()> {
        use raenet::http1::Http1Error;
        // Accept a dotted-quad host directly; otherwise resolve via DNS.
        let ip = match parse_dotted_quad(host) {
            Some(quad) => quad,
            None => raekit::sys::dns_resolve(host)
                .ok_or_else(|| Http1Error::Transport(String::from("DNS resolution failed")))?,
        };
        let fd = raekit::sys::tcp_connect(ip, port)
            .ok_or_else(|| Http1Error::Transport(String::from("TCP connect failed")))?;
        self.fd = Some(fd);
        Ok(())
    }

    fn send(&mut self, buf: &[u8]) -> raenet::http1::Http1Result<()> {
        use raenet::http1::Http1Error;
        let fd = self
            .fd
            .ok_or_else(|| Http1Error::Transport(String::from("send before connect")))?;
        let mut off = 0usize;
        let mut idle = 0u32;
        while off < buf.len() {
            let n = raekit::sys::sock_send(fd, &buf[off..]);
            if n < 0 {
                return Err(Http1Error::Transport(String::from("socket send error")));
            }
            if n == 0 {
                // Non-blocking back-pressure: yield and retry a bounded number of times.
                idle += 1;
                if idle > 100_000 {
                    return Err(Http1Error::Transport(String::from("send timed out")));
                }
                raekit::sys::yield_now();
                continue;
            }
            idle = 0;
            off += n as usize;
        }
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> raenet::http1::Http1Result<usize> {
        use raenet::http1::Http1Error;
        let fd = self
            .fd
            .ok_or_else(|| Http1Error::Transport(String::from("recv before connect")))?;
        // The socket is non-blocking (0 = nothing yet). Spin-yield up to a bound so a
        // slow server eventually delivers, but a dead connection can't hang forever.
        let mut idle = 0u32;
        loop {
            let n = raekit::sys::sock_recv(fd, buf);
            if n < 0 {
                return Err(Http1Error::Transport(String::from("socket recv error")));
            }
            if n == 0 {
                idle += 1;
                if idle > 200_000 {
                    // Treat a stalled stream as EOF so send_request makes its final parse
                    // attempt rather than spinning forever.
                    return Ok(0);
                }
                raekit::sys::yield_now();
                continue;
            }
            return Ok(n as usize);
        }
    }
}

/// Parse a dotted-quad IPv4 literal (`"93.184.216.34"`) into octets, or `None` if it
/// is a hostname. Lets a typed `http://1.2.3.4/` skip DNS. Never panics.
#[cfg(not(test))]
fn parse_dotted_quad(host: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut parts = host.split('.');
    for slot in octets.iter_mut() {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 {
            return None;
        }
        let mut v: u16 = 0;
        for b in part.bytes() {
            if !b.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (b - b'0') as u16;
        }
        if v > 255 {
            return None;
        }
        *slot = v as u8;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(octets)
}

/// Read a local `.html` file off disk (a `file:` URL or a bare path). Returns the
/// full document text, or `None` if absent/unreadable. Pure file I/O — no network.
#[cfg(not(test))]
fn read_local_html(url: &str) -> Option<String> {
    // Accept `file:///path` and bare `/path` forms.
    let path = url.strip_prefix("file://").unwrap_or(url);
    if path.is_empty() || path.starts_with("about:") {
        return None;
    }
    let fd = raekit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if data.len() > 8 * 1024 * 1024 {
            break;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = raekit::sys::close(fd);
    if data.is_empty() {
        return None;
    }
    // Treat the bytes as UTF-8; lossily substitute anything invalid so a binary
    // file never crashes the parser.
    Some(alloc::string::String::from_utf8_lossy(&data).into_owned())
}

/// Decimal string for a small count — no alloc-heavy formatting, `no_std`-safe.
#[cfg(not(test))]
fn usize_str(mut n: usize) -> String {
    if n == 0 {
        return String::from("0");
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    String::from(core::str::from_utf8(&buf[i..]).unwrap_or("0"))
}

// ── Render ─────────────────────────────────────────────────────────────────

#[cfg(not(test))]
fn render(app: &App, canvas: &mut Canvas) {
    // Web view background (white — pages style their own body).
    canvas.fill_rect(0, 0, WIN_W, WIN_H, VIEW_BG);

    // Title bar.
    canvas.fill_rect(0, 0, WIN_W, TITLE_H, CHROME);
    canvas.draw_text_aa(
        10,
        ((TITLE_H - rae_tokens::TYPE_SUBTITLE.line_height as usize) / 2) as i32,
        "Browser",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_SECONDARY,
        FontFamily::Sans,
    );

    render_toolbar(app, canvas);
    render_webview(app, canvas);
    render_status(app, canvas);
}

#[cfg(not(test))]
fn render_toolbar(app: &App, canvas: &mut Canvas) {
    let y = TITLE_H;
    canvas.fill_rect(0, y, WIN_W, TOOLBAR_H, CHROME);

    let btn = |canvas: &mut Canvas, idx: usize, glyph: &str, enabled: bool| {
        let bx = 8 + idx * (BTN_W + 4);
        let by = y + 4;
        let bh = TOOLBAR_H - 8;
        canvas.fill_rounded_rect(bx, by, BTN_W, bh, rae_tokens::RADIUS_SM as usize, FIELD_BG);
        let fg = if enabled { TEXT_PRIMARY } else { TEXT_TERTIARY };
        let gw = canvas.measure_text_aa(glyph, rae_tokens::TYPE_BODY, FontFamily::Sans);
        canvas.draw_text_aa(
            (bx + BTN_W / 2) as i32 - gw / 2,
            (by + (bh - rae_tokens::TYPE_BODY.line_height as usize) / 2) as i32,
            glyph,
            rae_tokens::TYPE_BODY,
            fg,
            FontFamily::Sans,
        );
    };
    btn(canvas, 0, "<", app.model.can_go_back());
    btn(canvas, 1, ">", app.model.can_go_forward());
    btn(canvas, 2, "R", app.model.current_url().is_some());

    // Address bar fills the rest of the toolbar.
    let addr_x = 8 + 3 * (BTN_W + 4) + 4;
    let addr_w = WIN_W - addr_x - 8;
    let addr_y = y + 4;
    let addr_h = TOOLBAR_H - 8;
    canvas.fill_rounded_rect(
        addr_x,
        addr_y,
        addr_w,
        addr_h,
        rae_tokens::RADIUS_SM as usize,
        FIELD_BG,
    );
    if app.editing_address {
        canvas.fill_rect(addr_x, addr_y + addr_h - 2, addr_w, 2, accent());
    }
    let shown = if app.address.is_empty() {
        "Enter a local path or about:home"
    } else {
        app.address.as_str()
    };
    let fg = if app.address.is_empty() {
        TEXT_TERTIARY
    } else {
        TEXT_PRIMARY
    };
    canvas.draw_text_aa(
        addr_x as i32 + 10,
        addr_y as i32 + ((addr_h - rae_tokens::TYPE_BODY.line_height as usize) / 2) as i32,
        shown,
        rae_tokens::TYPE_BODY,
        fg,
        FontFamily::Sans,
    );
}

/// Paint the current page through the LIVE engine paint bridge
/// ([`raeweb::backend::paint_displaylist_to_canvas`]) — the SAME crisp-AA `raegfx`
/// path AthUI uses ("renders through AthUI"). The display list is in viewport
/// space; we offset it under the chrome by translating the bridge's scroll origin
/// up by `top` (content that lands above the chrome is clipped to the canvas), and
/// add the user's scroll. The chrome is redrawn ON TOP afterward by [`render`].
#[cfg(not(test))]
fn render_webview(app: &App, canvas: &mut Canvas) {
    let top = TITLE_H + TOOLBAR_H;
    let page = match &app.page {
        Some(p) => p,
        None => return,
    };
    let list = app.model.display_list_for(page);
    // scroll_y > 0 moves content up; subtracting `top` pushes the page DOWN so its
    // origin sits just below the toolbar.
    paint_displaylist_to_canvas(&list, canvas, 0.0, app.scroll_y - top as f32);
}

#[cfg(not(test))]
fn render_status(app: &App, canvas: &mut Canvas) {
    let sy = WIN_H - STATUS_H;
    canvas.fill_rect(0, sy, WIN_W, STATUS_H, CHROME);
    canvas.fill_rect(0, sy, WIN_W, 1, STROKE);
    let text = if app.status.is_empty() {
        "Ready"
    } else {
        app.status.as_str()
    };
    canvas.draw_text_aa(
        10,
        sy as i32 + ((STATUS_H - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32,
        text,
        rae_tokens::TYPE_CAPTION,
        TEXT_TERTIARY,
        FontFamily::Sans,
    );
}

/// Paint a lightweight "Loading…" frame and present it, BEFORE a blocking network
/// fetch. Captures only the canvas + surface id (no `App` borrow), so it can run as
/// the progress callback while `App::show` holds `&mut self`. Clears the web view,
/// draws a status line with the in-flight URL, and presents immediately so the user
/// sees the navigation took.
#[cfg(not(test))]
fn paint_loading(canvas: &mut Canvas, sid: u64, url: &str) {
    let top = TITLE_H + TOOLBAR_H;
    // Clear the content area (leave the chrome that's already painted).
    canvas.fill_rect(0, top, WIN_W, WIN_H - top - STATUS_H, VIEW_BG);
    canvas.draw_text_aa(
        24,
        (top + 24) as i32,
        "Loading…",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_SECONDARY,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        24,
        (top + 24 + rae_tokens::TYPE_SUBTITLE.line_height as usize + 6) as i32,
        url,
        rae_tokens::TYPE_CAPTION,
        TEXT_TERTIARY,
        FontFamily::Sans,
    );
    // Status strip echo.
    let sy = WIN_H - STATUS_H;
    canvas.fill_rect(0, sy, WIN_W, STATUS_H, CHROME);
    canvas.fill_rect(0, sy, WIN_W, 1, STROKE);
    canvas.draw_text_aa(
        10,
        sy as i32 + ((STATUS_H - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32,
        "Loading…",
        rae_tokens::TYPE_CAPTION,
        TEXT_TERTIARY,
        FontFamily::Sans,
    );
    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
}

// ── Input ──────────────────────────────────────────────────────────────────

/// Which toolbar control a surface-local click hit.
#[cfg(not(test))]
fn hit_control(lx: i32, ly: i32) -> Control {
    let y = TITLE_H as i32;
    if ly < y || ly >= y + TOOLBAR_H as i32 {
        return Control::None;
    }
    for (idx, ctrl) in [Control::Back, Control::Forward, Control::Reload]
        .iter()
        .enumerate()
    {
        let bx = (8 + idx * (BTN_W + 4)) as i32;
        if lx >= bx && lx < bx + BTN_W as i32 {
            return *ctrl;
        }
    }
    let addr_x = (8 + 3 * (BTN_W + 4) + 4) as i32;
    if lx >= addr_x {
        return Control::Address;
    }
    Control::None
}

/// Scancode → ASCII for the address bar (letters, digits, and path/URL punctuation
/// `: / . _ - ~`). Returns `None` for keys we do not type.
#[cfg(not(test))]
fn scancode_to_ascii(code: u8, shift: bool) -> Option<u8> {
    let base: u8 = match code {
        0x10 => b'q',
        0x11 => b'w',
        0x12 => b'e',
        0x13 => b'r',
        0x14 => b't',
        0x15 => b'y',
        0x16 => b'u',
        0x17 => b'i',
        0x18 => b'o',
        0x19 => b'p',
        0x1E => b'a',
        0x1F => b's',
        0x20 => b'd',
        0x21 => b'f',
        0x22 => b'g',
        0x23 => b'h',
        0x24 => b'j',
        0x25 => b'k',
        0x26 => b'l',
        0x2C => b'z',
        0x2D => b'x',
        0x2E => b'c',
        0x2F => b'v',
        0x30 => b'b',
        0x31 => b'n',
        0x32 => b'm',
        0x02 => return Some(if shift { b'!' } else { b'1' }),
        0x03 => return Some(b'2'),
        0x04 => return Some(b'3'),
        0x05 => return Some(b'4'),
        0x06 => return Some(b'5'),
        0x07 => return Some(b'6'),
        0x08 => return Some(b'7'),
        0x09 => return Some(b'8'),
        0x0A => return Some(b'9'),
        0x0B => return Some(b'0'),
        0x39 => return Some(b' '),
        0x0C => return Some(if shift { b'_' } else { b'-' }),
        0x27 => return Some(if shift { b':' } else { b';' }),
        0x34 => return Some(b'.'),
        0x35 => return Some(b'/'),
        0x29 => return Some(b'~'),
        _ => return None,
    };
    if shift {
        Some(base.to_ascii_uppercase())
    } else {
        Some(base)
    }
}

// ===========================================================================
// Live entry point.
// ===========================================================================

/// The freestanding userspace entry (called by the `_start` shim in `main.rs`).
/// Creates the window surface, loads the home page through the live engine, and
/// runs the event loop: toolbar clicks navigate, the address bar accepts typed
/// URLs, arrow keys scroll the web view.
#[cfg(not(test))]
pub fn run() -> ! {
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }
    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();
    render(&app, &mut canvas);
    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    loop {
        let mut dirty = false;

        // ── Mouse: toolbar clicks (back/forward/reload + focus the address bar).
        let mut left_down = false;
        let mut mouse_edge = false;
        loop {
            let ev = raekit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            let now_down = (ev & 0x01) != 0;
            if now_down && !left_down {
                mouse_edge = true;
            }
            left_down = now_down;
        }
        if mouse_edge {
            let (cx, cy, _btn) = raekit::sys::cursor_pos();
            let (ox, oy) =
                raekit::sys::surface_origin(sid).unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
            let lx = (cx as i32).saturating_sub(ox as i32);
            let ly = (cy as i32).saturating_sub(oy as i32);
            match hit_control(lx, ly) {
                Control::Back => {
                    let mut prog = |url: &str| paint_loading(&mut canvas, sid, url);
                    app.go_back(Some(&mut prog));
                    dirty = true;
                }
                Control::Forward => {
                    let mut prog = |url: &str| paint_loading(&mut canvas, sid, url);
                    app.go_forward(Some(&mut prog));
                    dirty = true;
                }
                Control::Reload => {
                    let mut prog = |url: &str| paint_loading(&mut canvas, sid, url);
                    app.reload(Some(&mut prog));
                    dirty = true;
                }
                Control::Address => {
                    app.editing_address = true;
                    dirty = true;
                }
                Control::None => {
                    if app.editing_address {
                        app.editing_address = false;
                        dirty = true;
                    }
                }
            }
        }

        // ── Keyboard.
        let key = raekit::sys::read_key();
        if key != 0 {
            let code = (key & 0xFF) as u8;
            let pressed = (key & 0x8000_0000) == 0;
            // Track shift.
            if code == 0x2A || code == 0x36 {
                app.shift = pressed;
            } else if pressed {
                if app.editing_address {
                    match code {
                        0x1C => {
                            let mut prog = |url: &str| paint_loading(&mut canvas, sid, url);
                            app.commit_address(Some(&mut prog));
                            dirty = true;
                        }
                        0x01 => {
                            app.editing_address = false;
                            dirty = true;
                        }
                        0x0E => {
                            app.address.pop();
                            dirty = true;
                        }
                        _ => {
                            if let Some(ch) = scancode_to_ascii(code, app.shift) {
                                if app.address.len() < 1024 {
                                    app.address.push(ch as char);
                                    dirty = true;
                                }
                            }
                        }
                    }
                } else {
                    match code {
                        0x01 => raekit::sys::exit(0),
                        // Up/Down arrows scroll the web view.
                        0x48 => {
                            app.scroll_y = (app.scroll_y - 40.0).max(0.0);
                            dirty = true;
                        }
                        0x50 => {
                            app.scroll_y += 40.0;
                            dirty = true;
                        }
                        // 'l' focuses the address bar (browser convention).
                        0x26 => {
                            app.editing_address = true;
                            dirty = true;
                        }
                        // Backspace navigates back.
                        0x0E => {
                            let mut prog = |url: &str| paint_loading(&mut canvas, sid, url);
                            app.go_back(Some(&mut prog));
                            dirty = true;
                        }
                        _ => {}
                    }
                }
            }
        }

        if dirty {
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
    }
}

// ===========================================================================
// Host KAT — links the LIVE raeweb + rae_js engines, no kernel.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A known document: CSS-styled heading + a tagline + a card, with an inline
    /// script that computes a value and logs it. Exercises the full
    /// parse→cascade→layout path AND the JS interpreter.
    const DOC: &str = r#"<!DOCTYPE html>
<html><head><style>
body { padding: 10px; }
h1 { font-size: 24px; }
#hero { background: #112233; }
</style></head>
<body>
<h1 id="heading">Hello RaeWeb</h1>
<div id="hero"><p>A laid-out card.</p></div>
<script>
var answer = 6 * 7;
console.log("answer=" + answer);
</script>
</body></html>"#;

    #[test]
    fn layout_carries_real_parsed_text() {
        // (a) The HTML/CSS engine actually parsed + laid out the page.
        let model = BrowserModel::new();
        let css = extract_styles(DOC);
        let page = model.render_page("about:test", DOC, &css);

        // The heading's text survived tokenize → parse → DOM → layout.
        let heading = page
            .first_text_of("h1")
            .expect("h1 should carry its text through layout");
        assert_eq!(heading, "Hello RaeWeb");

        // The styled element produced a real computed box with non-zero size.
        let (_x, _y, w, h) = page
            .box_of_id("hero")
            .expect("#hero should have a computed layout box");
        assert!(w > 0.0, "hero box should have a positive width, got {w}");
        assert!(h > 0.0, "hero box should have a positive height, got {h}");

        // The tree is more than a bare document node.
        assert!(
            page.box_count() > 3,
            "expected several layout boxes, got {}",
            page.box_count()
        );
    }

    #[test]
    fn inline_script_actually_executed() {
        // (b) The inline JS RAN in the real rae_js interpreter and produced an
        // observable result (captured console output).
        let model = BrowserModel::new();
        let page = model.render_page("about:test", DOC, "");
        assert!(
            page.script_error.is_none(),
            "script should run cleanly, got {:?}",
            page.script_error
        );
        assert_eq!(
            page.console,
            alloc::vec![String::from("answer=42")],
            "the inline script's console.log of (6*7) must be captured"
        );
    }

    #[test]
    fn home_page_renders_and_scripts() {
        // The shipped built-in home document is a real page that renders + scripts.
        let model = BrowserModel::new();
        let css = extract_styles(HOME_HTML);
        let page = model.render_page(HOME_URL, HOME_HTML, &css);
        assert_eq!(page.first_text_of("h1").as_deref(), Some("RaeWeb"));
        assert!(page.script_error.is_none());
        // The home script logs "RaeWeb online: 42".
        assert_eq!(page.console.len(), 1);
        assert!(
            page.console[0].contains("42"),
            "home script should compute 6*7, got {:?}",
            page.console
        );
    }

    #[test]
    fn navigation_history_back_forward() {
        let mut model = BrowserModel::new();
        assert!(!model.can_go_back());
        model.navigate("about:home");
        model.navigate("file:///a.html");
        model.navigate("file:///b.html");
        assert_eq!(model.current_url(), Some("file:///b.html"));
        assert!(model.can_go_back());
        assert!(!model.can_go_forward());

        assert_eq!(model.back(), Some("file:///a.html"));
        assert_eq!(model.back(), Some("about:home"));
        assert!(!model.can_go_back());
        assert_eq!(model.forward(), Some("file:///a.html"));

        // Navigating from the middle truncates forward history.
        model.navigate("file:///c.html");
        assert!(!model.can_go_forward());
        assert_eq!(model.current_url(), Some("file:///c.html"));
        assert_eq!(model.history_len(), 3);
    }

    #[test]
    fn style_and_script_extraction() {
        let css = extract_styles(DOC);
        assert!(css.contains("font-size: 24px"), "CSS body not extracted");
        let (out, err) = run_inline_scripts(DOC);
        assert!(err.is_none());
        assert_eq!(out, alloc::vec![String::from("answer=42")]);

        // A page with no script yields no output and no error.
        let (out2, err2) = run_inline_scripts("<html><body><p>hi</p></body></html>");
        assert!(out2.is_empty());
        assert!(err2.is_none());

        // An external script is skipped (no fetch in v1).
        let (out3, _err3) = run_inline_scripts("<script src=\"x.js\">console.log('nope')</script>");
        assert!(out3.is_empty(), "external <script src> must be skipped");
    }

    // ── ENGINE-GAP KAT: live JS → DOM mutation through the load+execute path ──

    /// The headline proof: an inline `<script>` that sets `textContent` on an element
    /// actually changes what RENDERS. Loads `<p id="out">old</p>` + a script that writes
    /// `'new'`, runs it through the real browser load+execute path, and asserts the
    /// laid-out text for `#out` is now `'new'` (NOT `'old'`).
    #[test]
    fn js_textcontent_write_changes_rendered_layout() {
        let doc = "<!DOCTYPE html><html><body>\
                   <p id=\"out\">old</p>\
                   <script>document.getElementById('out').textContent = 'new';</script>\
                   </body></html>";
        let model = BrowserModel::new();
        let page = model.render_page("about:test", doc, "");

        // The script ran cleanly (the DOM binding exists now — no ReferenceError).
        assert!(
            page.script_error.is_none(),
            "DOM-mutating script should run cleanly, got {:?}",
            page.script_error
        );

        // The RENDERED layout for the <p> now carries 'new', not the parsed 'old'.
        let rendered = page
            .first_text_of("p")
            .expect("the <p> must lay out with text");
        assert!(
            rendered.contains("new"),
            "layout must reflect the JS mutation, got {rendered:?}"
        );
        assert!(
            !rendered.contains("old"),
            "the original text must be GONE after the mutation, got {rendered:?}"
        );
    }

    /// FAIL-ability demonstration (kept as a live, passing test): asserting the rendered
    /// text is STILL 'old' after the mutation must NOT hold. This is the inverse of the
    /// headline assertion — it proves the test above can actually fail (it would fail if the
    /// binding were a no-op). We assert the negation here so the suite stays green while
    /// documenting the fail condition.
    #[test]
    fn fail_ability_old_text_is_not_what_renders() {
        let doc = "<!DOCTYPE html><html><body>\
                   <p id=\"out\">old</p>\
                   <script>document.getElementById('out').textContent = 'new';</script>\
                   </body></html>";
        let model = BrowserModel::new();
        let page = model.render_page("about:test", doc, "");
        let rendered = page.first_text_of("p").unwrap_or_default();
        // If the binding were inert, `rendered` would be "old" and THIS would fail —
        // demonstrating the headline KAT is genuinely FAIL-able.
        assert_ne!(
            rendered.trim(),
            "old",
            "if this held, the DOM binding did nothing (the headline KAT would be a false green)"
        );
    }

    /// The READ path: a script can read an element's original text content.
    #[test]
    fn js_textcontent_read_returns_original_text() {
        let doc = "<!DOCTYPE html><html><body>\
                   <p id=\"out\">hello world</p>\
                   <script>console.log(document.getElementById('out').textContent);</script>\
                   </body></html>";
        let model = BrowserModel::new();
        let page = model.render_page("about:test", doc, "");
        assert!(page.script_error.is_none(), "{:?}", page.script_error);
        assert_eq!(
            page.console,
            alloc::vec![String::from("hello world")],
            "reading textContent must return the live element text"
        );
    }

    /// A script that copies one element's text into another, observed in the layout —
    /// proves the binding is bidirectional (read + write) within one script.
    #[test]
    fn js_can_copy_text_between_elements() {
        let doc = "<!DOCTYPE html><html><body>\
                   <p id=\"src\">payload</p><p id=\"dst\">empty</p>\
                   <script>\
                   var s = document.getElementById('src');\
                   var d = document.getElementById('dst');\
                   d.textContent = s.textContent;\
                   </script>\
                   </body></html>";
        let model = BrowserModel::new();
        let page = model.render_page("about:test", doc, "");
        assert!(page.script_error.is_none(), "{:?}", page.script_error);
        // Both paragraphs now read 'payload' in the layout.
        let dst = first_box_with_id_text(&page.layout, "dst").unwrap_or_default();
        assert!(
            dst.contains("payload"),
            "the destination paragraph should have the copied text, got {dst:?}"
        );
    }

    /// A missing id degrades cleanly. `getElementById('nope')` returns JS `null`; READING a
    /// property of null is a TypeError (surfaced as a graceful script error), and the page
    /// still renders its real content without a panic.
    #[test]
    fn js_missing_id_read_surfaces_error_no_panic() {
        let doc = "<!DOCTYPE html><html><body>\
                   <p id=\"here\">present</p>\
                   <script>var t = document.getElementById('nope').textContent;</script>\
                   </body></html>";
        let model = BrowserModel::new();
        let page = model.render_page("about:test", doc, "");
        // The page still rendered its real content.
        assert_eq!(page.first_text_of("p").as_deref(), Some("present"));
        // Reading a property of the null element surfaced as a TypeError, not a crash.
        assert!(
            page.script_error.is_some(),
            "reading .textContent of a null element should surface a TypeError"
        );
    }

    /// Writing a property of the null returned for a missing id is silently ignored
    /// (sloppy-mode assignment-to-null) — no mutation, no panic, the page renders. Proves the
    /// engine-gap path never crashes the app on a missing-id write.
    #[test]
    fn js_missing_id_write_no_panic() {
        let doc = "<!DOCTYPE html><html><body>\
                   <p id=\"here\">present</p>\
                   <script>document.getElementById('nope').textContent = 'x';</script>\
                   </body></html>";
        let model = BrowserModel::new();
        let page = model.render_page("about:test", doc, "");
        // The page still rendered its real content; nothing was mutated.
        assert_eq!(page.first_text_of("p").as_deref(), Some("present"));
    }

    /// getElementById('x') === null check works (so guarded scripts run cleanly).
    #[test]
    fn js_can_guard_against_missing_id() {
        let doc = "<!DOCTYPE html><html><body>\
                   <p id=\"here\">present</p>\
                   <script>\
                   var el = document.getElementById('nope');\
                   if (el) { el.textContent = 'x'; } else { console.log('absent'); }\
                   </script>\
                   </body></html>";
        let model = BrowserModel::new();
        let page = model.render_page("about:test", doc, "");
        assert!(page.script_error.is_none(), "{:?}", page.script_error);
        assert_eq!(page.console, alloc::vec![String::from("absent")]);
    }

    // ── Interactivity: addEventListener + click dispatch ──────────────────────

    /// THE headline interactivity KAT. A page with a button and a paragraph reading 'before';
    /// the script registers a `click` listener on the button that sets the paragraph to
    /// 'after'. We load it as an `InteractivePage`, SIMULATE a click on the button by id, and
    /// assert the rendered text for `#out` is now 'after' (it was 'before' at load).
    #[test]
    fn click_dispatch_runs_handler_and_mutates_render() {
        let doc = "<!DOCTYPE html><html><body>\
                   <button id=\"b\">click me</button>\
                   <p id=\"out\">before</p>\
                   <script>\
                   document.getElementById('b').addEventListener('click', function() {\
                     document.getElementById('out').textContent = 'after';\
                   });\
                   </script>\
                   </body></html>";
        let mut page = InteractivePage::load(doc, "");
        assert!(
            page.script_error.is_none(),
            "load-time script should run cleanly, got {:?}",
            page.script_error
        );
        // The listener was registered against the button's id.
        assert!(
            page.has_click_listener("b"),
            "addEventListener('click', …) must register a listener on #b"
        );
        // Before the click: the rendered paragraph still says 'before'.
        let before = first_box_with_id_text(page.layout(), "out").unwrap_or_default();
        assert!(
            before.contains("before") && !before.contains("after"),
            "pre-click render must show 'before', got {before:?}"
        );

        // SIMULATE the click on node `b`.
        let result = page.click_node("b");
        assert_eq!(result.handlers_fired, 1, "exactly one handler should fire");
        assert!(
            result.error.is_none(),
            "handler must not error: {:?}",
            result.error
        );

        // After the click: the rendered paragraph now says 'after'.
        let after = first_box_with_id_text(page.layout(), "out").unwrap_or_default();
        assert!(
            after.contains("after"),
            "post-click render must reflect the handler's mutation, got {after:?}"
        );
        assert!(
            !after.contains("before"),
            "the original 'before' text must be gone after the click, got {after:?}"
        );
    }

    /// FAIL-ability demonstration (kept passing): if dispatch did nothing, the rendered text
    /// would still be 'before'. We assert the negation so the suite stays green while proving
    /// the headline KAT is genuinely fail-able.
    #[test]
    fn fail_ability_click_actually_changed_render() {
        let doc = "<!DOCTYPE html><html><body>\
                   <button id=\"b\">x</button><p id=\"out\">before</p>\
                   <script>\
                   document.getElementById('b').addEventListener('click', function() {\
                     document.getElementById('out').textContent = 'after';\
                   });\
                   </script></body></html>";
        let mut page = InteractivePage::load(doc, "");
        page.click_node("b");
        let after = first_box_with_id_text(page.layout(), "out").unwrap_or_default();
        // If dispatch were inert, `after` would still contain 'before' and THIS fails —
        // demonstrating the headline KAT genuinely catches a broken dispatch.
        assert!(
            !after.contains("before"),
            "if this held, click dispatch did nothing (the headline KAT would be a false green)"
        );
    }

    /// Pixel hit-testing → dispatch: a click at the button's laid-out coordinates resolves to
    /// the button node and fires its handler. Proves `click_at(x, y)` works, not just by-id.
    #[test]
    fn click_at_hit_tests_then_dispatches() {
        let doc = "<!DOCTYPE html><html><body>\
                   <button id=\"b\">click me</button><p id=\"out\">before</p>\
                   <script>\
                   document.getElementById('b').addEventListener('click', function() {\
                     document.getElementById('out').textContent = 'after';\
                   });\
                   </script></body></html>";
        let mut page = InteractivePage::load(doc, "");
        // Find the button's laid-out box, click at its center.
        let (bx, by, bw, bh) =
            box_rect_of_id(page.layout(), "b").expect("the button must have a layout box");
        let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
        let result = page
            .click_at(cx, cy)
            .expect("a click at the button's center must hit the button node");
        assert_eq!(result.handlers_fired, 1, "the button's handler should fire");
        let after = first_box_with_id_text(page.layout(), "out").unwrap_or_default();
        assert!(
            after.contains("after"),
            "pixel-hit-test dispatch must reflect the mutation, got {after:?}"
        );
    }

    /// A handler that THROWS must not panic or hang the browser: the error is surfaced and the
    /// page stays renderable.
    #[test]
    fn throwing_handler_does_not_panic_page_alive() {
        let doc = "<!DOCTYPE html><html><body>\
                   <button id=\"b\">x</button><p id=\"keep\">content</p>\
                   <script>\
                   document.getElementById('b').addEventListener('click', function() {\
                     throw new Error('boom');\
                   });\
                   </script></body></html>";
        let mut page = InteractivePage::load(doc, "");
        let result = page.click_node("b");
        assert_eq!(result.handlers_fired, 1);
        assert!(
            result.error.is_some(),
            "a throwing handler should surface an error"
        );
        // The page is still alive and renders its real content.
        let kept = first_box_with_id_text(page.layout(), "keep").unwrap_or_default();
        assert!(
            kept.contains("content"),
            "page must stay renderable, got {kept:?}"
        );
    }

    /// A handler that schedules a deferred mutation via `setTimeout(fn, 0)` lands after the
    /// dispatch drains the event loop — proving async handlers work end-to-end.
    #[test]
    fn handler_settimeout_mutation_visible_after_drain() {
        let doc = "<!DOCTYPE html><html><body>\
                   <button id=\"b\">x</button><p id=\"out\">before</p>\
                   <script>\
                   document.getElementById('b').addEventListener('click', function() {\
                     setTimeout(function() {\
                       document.getElementById('out').textContent = 'deferred';\
                     }, 0);\
                   });\
                   </script></body></html>";
        let mut page = InteractivePage::load(doc, "");
        page.click_node("b");
        let after = first_box_with_id_text(page.layout(), "out").unwrap_or_default();
        assert!(
            after.contains("deferred"),
            "the setTimeout mutation must be visible after the loop drains, got {after:?}"
        );
    }

    /// Bubbling: a `click` on a child fires a listener registered on an id-bearing ancestor.
    #[test]
    fn click_bubbles_to_ancestor_listener() {
        let doc = "<!DOCTYPE html><html><body>\
                   <div id=\"outer\"><button id=\"inner\">x</button></div>\
                   <p id=\"out\">before</p>\
                   <script>\
                   document.getElementById('outer').addEventListener('click', function() {\
                     document.getElementById('out').textContent = 'bubbled';\
                   });\
                   </script></body></html>";
        let mut page = InteractivePage::load(doc, "");
        // Click the inner button; the outer div's listener should fire via bubbling.
        let result = page.click_node("inner");
        assert_eq!(
            result.handlers_fired, 1,
            "the ancestor's handler should fire"
        );
        let after = first_box_with_id_text(page.layout(), "out").unwrap_or_default();
        assert!(
            after.contains("bubbled"),
            "the bubbled ancestor handler's mutation must show, got {after:?}"
        );
    }

    /// removeEventListener un-registers a handler so a subsequent click does nothing.
    #[test]
    fn remove_event_listener_stops_dispatch() {
        let doc = "<!DOCTYPE html><html><body>\
                   <button id=\"b\">x</button><p id=\"out\">before</p>\
                   <script>\
                   function h() { document.getElementById('out').textContent = 'fired'; }\
                   var el = document.getElementById('b');\
                   el.addEventListener('click', h);\
                   el.removeEventListener('click', h);\
                   </script></body></html>";
        let mut page = InteractivePage::load(doc, "");
        assert!(
            !page.has_click_listener("b"),
            "the listener should have been removed"
        );
        let result = page.click_node("b");
        assert_eq!(
            result.handlers_fired, 0,
            "no handler should fire after removal"
        );
        let after = first_box_with_id_text(page.layout(), "out").unwrap_or_default();
        assert!(
            after.contains("before"),
            "render must be unchanged, got {after:?}"
        );
    }

    /// A click on an element with no registered listener is a clean no-op (no panic, no error).
    #[test]
    fn click_with_no_listener_is_noop() {
        let doc = "<!DOCTYPE html><html><body>\
                   <button id=\"b\">x</button><p id=\"out\">before</p></body></html>";
        let mut page = InteractivePage::load(doc, "");
        let result = page.click_node("b");
        assert_eq!(result.handlers_fired, 0);
        assert!(result.error.is_none());
    }

    /// The (x, y, w, h) border box of the first layout node with `id`.
    fn box_rect_of_id(node: &LayoutBox, id: &str) -> Option<(f32, f32, f32, f32)> {
        if node.node_id.as_deref() == Some(id) {
            let b = node.dimensions.border_box();
            return Some((b.x, b.y, b.width, b.height));
        }
        for child in &node.children {
            if let Some(r) = box_rect_of_id(child, id) {
                return Some(r);
            }
        }
        None
    }

    /// Find the text of the first layout box with the given node_id.
    fn first_box_with_id_text(node: &LayoutBox, id: &str) -> Option<String> {
        if node.node_id.as_deref() == Some(id) {
            let t = collect_box_text(node);
            if !t.is_empty() {
                return Some(t);
            }
        }
        for child in &node.children {
            if let Some(t) = first_box_with_id_text(child, id) {
                return Some(t);
            }
        }
        None
    }

    // ── NETWORK KATs: address-bar fetch over a MOCK transport ─────────────────
    //
    // These prove the engine-gap close: a typed http:// URL is fetched over the
    // injectable HttpTransport, the response's HTML is run through the SAME
    // parse→cascade→layout + inline-<script> path local pages use, and failures
    // render an HONEST error page (never a panic, never blank). The transport is
    // raenet::http1::MockTransport — a canned HTTP/1.1 dialogue — so NO live network
    // is touched (live connectivity is iron-gated on DHCP-Bound). This mirrors how
    // rae_mail host-tests SMTP/IMAP against scripted server dialogs.

    use raenet::http1::MockTransport;

    /// Build a canned HTTP/1.1 200 response with the given HTML body + Content-Length.
    fn mock_200(body: &str) -> Vec<u8> {
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: ");
        raw.extend_from_slice(decimal(body.len()).as_bytes());
        raw.extend_from_slice(b"\r\n\r\n");
        raw.extend_from_slice(body.as_bytes());
        raw
    }

    /// Decimal string for a usize (test helper — no std fmt needed for the literal path).
    fn decimal(mut n: usize) -> String {
        if n == 0 {
            return String::from("0");
        }
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        while n > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        String::from(core::str::from_utf8(&buf[i..]).unwrap_or("0"))
    }

    /// HEADLINE NETWORK PROOF: navigate to http://example.test/ with a mock returning
    /// a 200 whose HTML has a known element + an inline script. Assert the transport
    /// was driven with the right host/path, the layout carries the page's text, AND
    /// the inline script executed (captured console output).
    #[test]
    fn network_fetch_renders_and_runs_script() {
        let body = "<!DOCTYPE html><html><head><style>h1{font-size:20px;}</style></head>\
                    <body><h1 id=\"banner\">Live from the network</h1>\
                    <p id=\"para\">fetched over HTTP</p>\
                    <script>console.log(\"net script ran: \" + (3 * 14));</script>\
                    </body></html>";
        let mut transport = MockTransport::new(mock_200(body));
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://example.test/index.html", &mut transport);

        // (1) The transport was actually driven: connected to the right host:port…
        assert_eq!(
            transport.connected_to,
            Some((String::from("example.test"), 80)),
            "fetch must connect to the URL's host on the default HTTP port"
        );
        // …and sent a well-formed GET for the right path.
        let sent = String::from_utf8_lossy(&transport.sent);
        assert!(
            sent.starts_with("GET /index.html HTTP/1.1\r\n"),
            "request line wrong: {sent:?}"
        );
        assert!(
            sent.contains("Host: example.test\r\n"),
            "Host header wrong: {sent:?}"
        );

        // (2) The fetched HTML was parsed + laid out: the heading text survived.
        assert_eq!(
            page.first_text_of("h1").as_deref(),
            Some("Live from the network"),
            "the network page's heading must render through layout"
        );

        // (3) The inline <script> executed in rae_js (observable via console).
        assert!(page.script_error.is_none(), "{:?}", page.script_error);
        assert_eq!(
            page.console,
            alloc::vec![String::from("net script ran: 42")],
            "the fetched page's inline script must run and log 3*14"
        );
    }

    /// FAIL-ABILITY (kept passing as the positive): the rendered heading must come
    /// from the FETCHED body. If `fetch_and_render` ignored the body (a no-op seam),
    /// `first_text_of("h1")` would be `None`/empty and this flips — demonstrating the
    /// headline KAT genuinely depends on the network fetch.
    #[test]
    fn network_fetch_fail_ability_depends_on_body() {
        let body = "<html><body><h1 id=\"banner\">Live from the network</h1></body></html>";
        let mut transport = MockTransport::new(mock_200(body));
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://example.test/", &mut transport);
        let h1 = page.first_text_of("h1").unwrap_or_default();
        assert_eq!(
            h1, "Live from the network",
            "the rendered heading must come from the fetched body, not a fallback"
        );
    }

    /// ERROR-PAGE PROOF (HTTP 404 with no body): a non-2xx status renders the honest
    /// error page through the real engine — an `id="error"` heading mentioning the
    /// status, NOT a panic, NOT a blank page.
    #[test]
    fn network_404_renders_honest_error_page() {
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec();
        let mut transport = MockTransport::new(raw);
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://example.test/missing", &mut transport);

        assert!(page.script_error.is_none());
        let err = first_box_with_id_text(&page.layout, "error").unwrap_or_default();
        assert!(
            err.contains("404"),
            "the error page must surface the 404 status, got {err:?}"
        );
        // The page is real (laid out), not empty.
        assert!(page.box_count() > 3, "error page should lay out real boxes");
    }

    /// ERROR-PAGE PROOF (transport/connection failure): a transport that delivers no
    /// bytes (connection refused / immediate close) makes the parser fail at EOF, which
    /// the loader maps to `LoadError::Transport` → the honest "Can't load this page"
    /// page, with the failed URL echoed back. No panic.
    #[test]
    fn network_connection_error_renders_honest_error_page() {
        let mut transport = MockTransport::new(Vec::new());
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://unreachable.test/", &mut transport);

        assert!(page.script_error.is_none());
        let err = first_box_with_id_text(&page.layout, "error").unwrap_or_default();
        assert!(
            err.to_lowercase().contains("load"),
            "a connection failure must render the load-error page, got {err:?}"
        );
        let failed = first_box_with_id_text(&page.layout, "failed-url").unwrap_or_default();
        assert!(
            failed.contains("unreachable.test"),
            "the error page should name the failed address, got {failed:?}"
        );
    }

    /// A malformed/garbage response degrades to the honest error page (never a panic)
    /// — proves the hostile-input safety of the whole fetch→render path.
    #[test]
    fn network_malformed_response_no_panic() {
        let mut transport = MockTransport::new(b"NOT-HTTP garbage \x00\xff bytes".to_vec());
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://bad.test/", &mut transport);
        // Contract: no panic, something always lays out.
        assert!(page.box_count() >= 1, "must always lay out something");
    }

    /// The pure URL-routing decision: about: → builtin, http:// → network,
    /// https:// → network-TLS (honest unsupported), everything else → local.
    #[test]
    fn url_classification_routes_correctly() {
        assert_eq!(classify_url("about:home"), UrlKind::Builtin);
        assert_eq!(classify_url("http://example.com/"), UrlKind::Network);
        assert_eq!(classify_url("HTTP://EXAMPLE.COM/"), UrlKind::Network);
        assert_eq!(classify_url("https://secure.test/"), UrlKind::NetworkTls);
        assert_eq!(classify_url("file:///home/page.html"), UrlKind::Local);
        assert_eq!(classify_url("/local/path.html"), UrlKind::Local);
        assert_eq!(classify_url("  http://trim.me/  "), UrlKind::Network);
    }

    /// A chunked 200 response renders too — proves the fetch path uses the real
    /// http1 framing (not just Content-Length), driven incrementally by the mock.
    #[test]
    fn network_chunked_response_renders() {
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n");
        raw.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n");
        let body = "<h1 id=\"x\">chunked page</h1>";
        raw.extend_from_slice(hex_lower(body.len()).as_bytes());
        raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(body.as_bytes());
        raw.extend_from_slice(b"\r\n0\r\n\r\n");

        let mut transport = MockTransport::new(raw).with_recv_chunk(4);
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://chunk.test/", &mut transport);
        assert_eq!(page.first_text_of("h1").as_deref(), Some("chunked page"));
    }

    // ── LINKED-STYLESHEET KATs: <link rel=stylesheet> fetch + cascade ─────────
    //
    // These prove the slice: a fetched page that LINKS an external stylesheet has
    // that sheet fetched over the SAME transport, resolved relative to the page URL,
    // and cascaded ALONGSIDE the inline <style> — so a linked rule changes a computed
    // box. A linked sheet that 404s / fails is skipped: the page still renders with
    // its inline styles. The transport routes by request path so one connection model
    // serves the page AND its sub-resources, exactly like the live socket transport.

    /// A multi-resource mock: routes the canned 200 response by the request path it
    /// captures in `send`. The path is read from the `GET <path> HTTP/1.1` line of the
    /// most recent request, so reusing ONE transport across the page + stylesheet
    /// fetches (as `fetch_and_render` does) serves the right body for each. An unknown
    /// path returns a 404 (the fail-soft path). Mirrors how the live transport reuses a
    /// connection for sub-resources.
    struct RoutingMock {
        routes: alloc::vec::Vec<(String, Vec<u8>)>,
        sent: Vec<u8>,
        connected_hosts: Vec<String>,
        cur: Vec<u8>,
        read_pos: usize,
    }

    impl RoutingMock {
        fn new(routes: alloc::vec::Vec<(&str, Vec<u8>)>) -> Self {
            RoutingMock {
                routes: routes
                    .into_iter()
                    .map(|(p, b)| (String::from(p), b))
                    .collect(),
                sent: Vec::new(),
                connected_hosts: Vec::new(),
                cur: Vec::new(),
                read_pos: 0,
            }
        }
        fn path_of_request(req: &[u8]) -> Option<String> {
            let s = String::from_utf8_lossy(req);
            let line = s.lines().next()?;
            let mut parts = line.split(' ');
            let _method = parts.next()?;
            Some(String::from(parts.next()?))
        }
    }

    impl raenet::http1::HttpTransport for RoutingMock {
        fn connect(&mut self, host: &str, _port: u16) -> raenet::http1::Http1Result<()> {
            self.connected_hosts.push(String::from(host));
            Ok(())
        }
        fn send(&mut self, buf: &[u8]) -> raenet::http1::Http1Result<()> {
            self.sent.extend_from_slice(buf);
            // Select the response body for THIS request's path; reset the read cursor.
            let path = Self::path_of_request(buf).unwrap_or_default();
            self.cur = self
                .routes
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, b)| b.clone())
                .unwrap_or_else(|| b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec());
            self.read_pos = 0;
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> raenet::http1::Http1Result<usize> {
            let remaining = self.cur.len().saturating_sub(self.read_pos);
            if remaining == 0 {
                return Ok(0);
            }
            let n = remaining.min(buf.len());
            buf[..n].copy_from_slice(&self.cur[self.read_pos..self.read_pos + n]);
            self.read_pos += n;
            Ok(n)
        }
    }

    /// Build a 200 response with an explicit Content-Type.
    fn mock_200_ct(body: &str, content_type: &str) -> Vec<u8> {
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Type: ");
        raw.extend_from_slice(content_type.as_bytes());
        raw.extend_from_slice(b"\r\nContent-Length: ");
        raw.extend_from_slice(decimal(body.len()).as_bytes());
        raw.extend_from_slice(b"\r\n\r\n");
        raw.extend_from_slice(body.as_bytes());
        raw
    }

    /// HEADLINE LINKED-CSS PROOF: a fetched page links an external stylesheet whose
    /// rule sets `#box { width: 100px }`. Assert the stylesheet was fetched (the
    /// routing mock served `/style.css`) AND that the LINKED rule shrank the box's
    /// computed width to ~100 — far below the unstyled, viewport-wide default.
    #[test]
    fn linked_stylesheet_is_fetched_and_cascaded() {
        let html = "<!DOCTYPE html><html><head>\
                    <link rel=\"stylesheet\" href=\"/style.css\">\
                    </head><body><div id=\"box\">styled by a linked sheet</div></body></html>";
        let css = "#box { width: 100px; }";
        let mut transport = RoutingMock::new(alloc::vec![
            ("/page.html", mock_200_ct(html, "text/html")),
            ("/style.css", mock_200_ct(css, "text/css")),
        ]);
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://example.test/page.html", &mut transport);

        // The stylesheet was actually requested over the same transport.
        let sent = String::from_utf8_lossy(&transport.sent);
        assert!(
            sent.contains("GET /style.css HTTP/1.1\r\n"),
            "the linked stylesheet must be fetched, requests were: {sent:?}"
        );

        // The LINKED rule applied: the box is ~100px wide, not the viewport width.
        let (_x, _y, w, _h) = page
            .box_of_id("box")
            .expect("#box should have a computed layout box");
        assert!(
            (w - 100.0).abs() < 1.0,
            "the linked `#box{{width:100px}}` rule must apply; width was {w} \
             (unstyled default would be ~{VIEWPORT_W})"
        );
        assert!(page.script_error.is_none(), "{:?}", page.script_error);
    }

    /// FAIL-ABILITY / BASELINE: the SAME page WITHOUT the linked sheet leaves the box
    /// at its full default width. This is the negative pole the headline test moves
    /// away from — if linked CSS were ignored, the headline test would see THIS width
    /// and fail.
    #[test]
    fn unlinked_baseline_box_is_full_width() {
        let html = "<!DOCTYPE html><html><head></head>\
                    <body><div id=\"box\">no linked sheet</div></body></html>";
        let mut transport =
            RoutingMock::new(alloc::vec![("/page.html", mock_200_ct(html, "text/html"))]);
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://example.test/page.html", &mut transport);
        let (_x, _y, w, _h) = page.box_of_id("box").expect("#box should lay out");
        assert!(
            w > 200.0,
            "without a width rule the box should be wide (~{VIEWPORT_W}), got {w}"
        );
    }

    /// ROBUSTNESS: a `<link>` whose href 404s must NOT break the page — the inline
    /// `<style>` still applies and the page renders. Proves a failed stylesheet fetch
    /// degrades gracefully (skip it, render with what loaded).
    #[test]
    fn linked_stylesheet_404_degrades_gracefully() {
        // Inline style sets the heading text-bearing box; the linked sheet 404s.
        let html = "<!DOCTYPE html><html><head>\
                    <style>#kept { width: 120px; }</style>\
                    <link rel=\"stylesheet\" href=\"/missing.css\">\
                    </head><body>\
                    <div id=\"kept\">inline-styled</div>\
                    <h1 id=\"hd\">still here</h1>\
                    </body></html>";
        // Only the page route exists; /missing.css falls through to the mock's 404.
        let mut transport =
            RoutingMock::new(alloc::vec![("/page.html", mock_200_ct(html, "text/html"))]);
        let model = BrowserModel::new();
        let page = model.fetch_and_render("http://example.test/page.html", &mut transport);

        // The page rendered (no panic) and the INLINE rule still applied.
        let (_x, _y, w, _h) = page
            .box_of_id("kept")
            .expect("inline-styled #kept must still lay out");
        assert!(
            (w - 120.0).abs() < 1.0,
            "inline style must survive a failed linked-sheet fetch; width was {w}"
        );
        // And the document content is intact.
        assert_eq!(page.first_text_of("h1").as_deref(), Some("still here"));
        // The failed fetch was attempted then skipped — request was sent, no crash.
        let sent = String::from_utf8_lossy(&transport.sent);
        assert!(
            sent.contains("GET /missing.css HTTP/1.1\r\n"),
            "the 404 sheet should have been attempted, requests: {sent:?}"
        );
    }

    /// The pure DOM-walk that finds linked stylesheet hrefs: rel token-matching,
    /// case-insensitivity, and skipping non-stylesheet / href-less links.
    #[test]
    fn link_stylesheet_href_extraction() {
        let html = "<html><head>\
            <link rel=\"stylesheet\" href=\"a.css\">\
            <link rel=\"icon\" href=\"favicon.ico\">\
            <link rel=\"alternate stylesheet\" href=\"b.css\">\
            <link rel=\"STYLESHEET\" href=\"c.css\">\
            <link rel=\"stylesheet\">\
            </head><body></body></html>";
        let hrefs = link_stylesheet_hrefs(html);
        assert_eq!(
            hrefs,
            alloc::vec![
                String::from("a.css"),
                String::from("b.css"),
                String::from("c.css")
            ],
            "must collect stylesheet hrefs (token-aware, case-insensitive), \
             skip rel=icon and href-less links"
        );
    }

    /// Lowercase hex of a small usize (for the chunked-test chunk size).
    fn hex_lower(mut n: usize) -> String {
        if n == 0 {
            return String::from("0");
        }
        let digits = b"0123456789abcdef";
        let mut buf = [0u8; 16];
        let mut i = buf.len();
        while n > 0 {
            i -= 1;
            buf[i] = digits[n % 16];
            n /= 16;
        }
        String::from(core::str::from_utf8(&buf[i..]).unwrap_or("0"))
    }
}
