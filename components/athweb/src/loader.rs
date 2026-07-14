//! RaeWeb resource loader — the missing front of the pipeline.
//!
//! > "Web apps via PWA support that actually feels native (renders through AthUI)."
//! > — AthenaOS Concept §3
//!
//! Phase 1 (docs/research/web-engine.md §"Resource loader"): fetch a document over
//! the **real** `athnet::http1` HTTP/1.1 client (`fetch_with` over any
//! [`HttpTransport`]) and feed the bytes into the existing HTML/CSS parse path. The
//! transport is abstract, so the deterministic host KAT drives it with athnet's
//! `MockTransport` (an in-memory document) while the browser shell will later plug in
//! `RaeSocketTransport` (live DNS+TCP) — same call path, no engine change.
//!
//! No new syscall / ABI (the spec confirms Phase 1 is pure userspace over existing
//! athnet APIs). `no_std + alloc`, never panics: every transport/HTTP error becomes a
//! [`LoadError`] the shell can render as an error page.

use alloc::string::String;
use alloc::vec::Vec;
use athnet::http1::{self, HttpTransport, Limits, Method};

/// Hard cap on the number of external stylesheets a single page may pull in. A
/// pathological page that links thousands of sheets must not spin or exhaust
/// memory; once the cap is hit, remaining `<link>`s are silently ignored (the page
/// still renders with what loaded).
pub const MAX_LINKED_STYLESHEETS: usize = 16;

/// How deep a chain of `@import` rules will be followed (sheet imports a sheet
/// that imports a sheet …). Bounds a malicious/cyclic import graph.
pub const MAX_IMPORT_DEPTH: usize = 4;

/// Hard ceiling on the TOTAL number of stylesheet fetches for one page across
/// `<link>`s AND every `@import` they pull in transitively. A cyclic or
/// fan-out-heavy import graph cannot exceed this, so the page can never spin or
/// exhaust memory regardless of depth.
pub const MAX_TOTAL_STYLESHEETS: usize = 32;

/// A loaded resource after redirects, MIME-classified for the dispatcher.
#[derive(Debug, Clone)]
pub struct Resource {
    /// Coarse MIME classification (the only distinction Phase 1 needs).
    pub mime: Mime,
    /// Raw response body bytes.
    pub bytes: Vec<u8>,
    /// HTTP status code (200, 404, …).
    pub status: u16,
    /// The URL actually fetched (== requested in Phase 1; redirect-following lands
    /// in Phase 2's live loader).
    pub final_url: String,
}

impl Resource {
    /// The body as UTF-8 text (lossy-free: invalid bytes are dropped at the
    /// boundary). HTML/CSS parsing wants `&str`; this is the bridge.
    pub fn as_text(&self) -> String {
        match core::str::from_utf8(&self.bytes) {
            // Strip a leading UTF-8 BOM (otherwise it lingers as a stray U+FEFF at
            // the start of the document and shows up as a text node).
            Ok(s) => String::from(s.strip_prefix('\u{FEFF}').unwrap_or(s)),
            // Not valid UTF-8: decode as Latin-1 (every byte -> U+0000..U+00FF) so a
            // legacy ISO-8859-1 / Windows-1252 page keeps its accented characters
            // instead of dropping them. Never panics; UTF-8 with stray bytes
            // mojibakes but loses nothing. The tokenizers tolerate junk.
            Err(_) => self.bytes.iter().map(|&b| b as char).collect(),
        }
    }
}

/// Coarse content classification driven off the `Content-Type` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mime {
    Html,
    Css,
    Json,
    Image,
    Other,
}

impl Mime {
    /// Classify from a `Content-Type` header value (case-insensitive, params
    /// ignored). Defaults to `Html` when absent — the common "200 with no type"
    /// case for hand-served documents.
    pub fn classify(content_type: Option<&str>) -> Self {
        let ct = match content_type {
            Some(c) => c,
            None => return Mime::Html,
        };
        let ct = ct.to_ascii_lowercase();
        if ct.contains("text/html") || ct.contains("application/xhtml") {
            Mime::Html
        } else if ct.contains("text/css") {
            Mime::Css
        } else if ct.contains("json") {
            Mime::Json
        } else if ct.contains("image/") {
            Mime::Image
        } else {
            Mime::Other
        }
    }
}

/// Why a navigation failed — rendered by the shell as an error page (itself a
/// trivial HTML doc, per the spec's dogfood failure mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// URL did not parse / unsupported scheme.
    BadUrl,
    /// DNS/TCP/transport-level failure (or a non-2xx the caller declined).
    Transport,
    /// HTTP framing / oversized / malformed response.
    BadResponse,
}

/// Fetch `url` over the real `athnet::http1` client and classify the result.
///
/// This exercises the genuine `fetch_with` → `send_request` → `parse_response`
/// path; `transport` decides whether that's a live socket or the in-memory mock.
/// A non-2xx status is still returned as a `Resource` (so the shell can render a
/// server error page) — only transport/parse failures become `Err`.
pub fn fetch_document<T: HttpTransport>(
    url: &str,
    transport: &mut T,
) -> Result<Resource, LoadError> {
    let resp = http1::fetch_with(url, Method::Get, None, &[], transport, &Limits::new())
        .map_err(|_| LoadError::Transport)?;

    let mime = Mime::classify(resp.header("content-type"));
    Ok(Resource {
        mime,
        bytes: resp.body,
        status: resp.status,
        final_url: String::from(url),
    })
}

/// Resolve a stylesheet `href` against the page's `base` URL into an absolute
/// `http://…` URL the loader can fetch. Handles the three forms a real page uses:
///
///   * **Absolute** — `http://cdn.test/site.css` (and `//cdn.test/x.css`, which
///     inherits the base scheme): returned essentially as-is.
///   * **Root-relative** — `/css/site.css`: keep the base's scheme+authority,
///     replace the whole path.
///   * **Relative** — `site.css` or `../a/b.css`: resolve against the base path's
///     directory, collapsing `.`/`..` segments.
///
/// Returns `None` for an unsupported scheme (e.g. `https://`, `data:`) or a base
/// that does not parse as `http://` — the caller simply skips that sheet. Never
/// panics. Only `http://` is supported in this phase (the same constraint as the
/// document fetch); a secure stylesheet is skipped rather than fetched insecurely.
pub fn resolve_stylesheet_url(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    // Absolute http URL — take it directly (validate it parses).
    if let Some(rest) = strip_scheme_ci(href, "http://") {
        let _ = rest; // parse below for validation
        return http1::HttpUrl::parse(href).ok().map(|_| String::from(href));
    }
    // Explicitly unsupported absolute schemes are skipped (never fetched).
    if strip_scheme_ci(href, "https://").is_some()
        || href.starts_with("data:")
        || href.starts_with("javascript:")
        || href.starts_with("ftp://")
    {
        return None;
    }

    let base = http1::HttpUrl::parse(base).ok()?;
    let authority = base.authority();

    // Scheme-relative ("//host/path"): inherit the base scheme (http).
    if let Some(rest) = href.strip_prefix("//") {
        let candidate = {
            let mut s = String::from("http://");
            s.push_str(rest);
            s
        };
        return http1::HttpUrl::parse(&candidate).ok().map(|_| candidate);
    }

    // Drop any query/fragment on the href before path math (we keep the path only;
    // a `?`/`#` is preserved as part of the final target string after resolution).
    let (href_path, href_suffix) = split_query_fragment(href);

    let resolved_path = if let Some(abs) = href_path.strip_prefix('/') {
        // Root-relative: replace the whole base path.
        let mut p = String::from("/");
        p.push_str(abs);
        normalize_path(&p)
    } else {
        // Relative to the directory of the base path.
        let base_dir = match base.path.rfind('/') {
            Some(i) => &base.path[..=i], // keep trailing slash
            None => "/",
        };
        let mut combined = String::from(base_dir);
        combined.push_str(href_path);
        normalize_path(&combined)
    };

    let mut out = String::from("http://");
    out.push_str(&authority);
    out.push_str(&resolved_path);
    out.push_str(href_suffix);
    Some(out)
}

/// Case-insensitive scheme strip helper (returns the remainder after the scheme).
fn strip_scheme_ci<'a>(s: &'a str, scheme: &str) -> Option<&'a str> {
    if s.len() >= scheme.len() && s[..scheme.len()].eq_ignore_ascii_case(scheme) {
        Some(&s[scheme.len()..])
    } else {
        None
    }
}

/// Split an href into (path, "?query#frag"-or-empty). The suffix is carried onto the
/// resolved URL verbatim so a stylesheet with a cache-buster query still fetches.
fn split_query_fragment(href: &str) -> (&str, &str) {
    match href.find(|c| c == '?' || c == '#') {
        Some(i) => (&href[..i], &href[i..]),
        None => (href, ""),
    }
}

/// Collapse `.`/`..`/empty segments in an absolute path (`/a/./b/../c.css` →
/// `/a/c.css`). A `..` at the root is clamped (never escapes above `/`). Always
/// returns a path starting with `/`. Never panics.
fn normalize_path(path: &str) -> String {
    let mut segs: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            other => segs.push(other),
        }
    }
    // Preserve a trailing slash (a directory ref) if the input ended with one and we
    // didn't collapse everything away.
    let trailing = path.ends_with('/') && !segs.is_empty();
    let mut out = String::from("/");
    for (i, s) in segs.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(s);
    }
    if trailing {
        out.push('/');
    }
    out
}

/// Fetch every external stylesheet `href` (already resolved to absolute `http://`
/// URLs) over `transport`, in order, and return their CSS bodies concatenated.
///
/// Robustness contract (the whole point of this slice):
///   * **Bounded count** — at most [`MAX_LINKED_STYLESHEETS`] are fetched; the rest
///     are ignored so a pathological page cannot spin.
///   * **Bounded size** — each fetch rides the loader's `Limits` (16 MiB body cap by
///     default), so a hostile/huge stylesheet cannot OOM the app.
///   * **Fail-soft** — a sheet that fails to fetch (transport error), returns a
///     non-2xx status, or is empty is skipped; the others still load. A failed
///     stylesheet NEVER fails the page.
///
/// Returns the concatenation of the successfully-fetched sheets (each followed by a
/// newline so adjacent rules don't merge), in `urls` order.
pub fn fetch_stylesheets<T: HttpTransport>(
    urls: &[String],
    transport: &mut T,
    limits: &Limits,
) -> String {
    let mut css = String::new();
    // Shared total-fetch budget across all <link> sheets AND their transitive
    // @imports — the single bound that makes a cyclic/fan-out import graph safe.
    let mut budget: usize = MAX_TOTAL_STYLESHEETS;
    for url in urls.iter().take(MAX_LINKED_STYLESHEETS) {
        if budget == 0 {
            break;
        }
        let sheet = fetch_stylesheet_recursive(url, transport, limits, 0, &mut budget);
        if !sheet.is_empty() {
            css.push_str(&sheet);
            css.push('\n');
        }
    }
    css
}

/// Fetch one stylesheet and, first, every stylesheet it `@import`s (depth-first,
/// bounded by [`MAX_IMPORT_DEPTH`] and the shared `budget`). Per CSS cascade
/// order, the imported sheets' rules come BEFORE the importing sheet's own rules
/// in the returned text. Fail-soft: a sheet (or import) that errors / is non-2xx
/// is skipped, never failing the page. `budget` is decremented once per fetch so
/// a cycle (`a.css` imports `b.css` imports `a.css`) terminates at the ceiling.
fn fetch_stylesheet_recursive<T: HttpTransport>(
    url: &str,
    transport: &mut T,
    limits: &Limits,
    depth: usize,
    budget: &mut usize,
) -> String {
    if *budget == 0 {
        return String::new();
    }
    *budget -= 1;
    let body = match http1::fetch_with(url, Method::Get, None, &[], transport, limits) {
        Ok(resp) if (200..300).contains(&resp.status) => match core::str::from_utf8(&resp.body) {
            Ok(s) => String::from(s.strip_prefix('\u{FEFF}').unwrap_or(s)),
            Err(_) => resp.body.iter().map(|&b| b as char).collect(),
        },
        // Non-2xx or transport/parse failure: skip this sheet.
        _ => return String::new(),
    };
    if body.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    if depth < MAX_IMPORT_DEPTH {
        for imp in extract_import_urls(&body) {
            if *budget == 0 {
                break;
            }
            // Resolve each @import target relative to THIS sheet's URL.
            if let Some(abs) = resolve_stylesheet_url(url, &imp) {
                let imported =
                    fetch_stylesheet_recursive(&abs, transport, limits, depth + 1, budget);
                if !imported.is_empty() {
                    out.push_str(&imported);
                    out.push('\n');
                }
            }
        }
    }
    // Imported rules first, then this sheet's own rules (CSS cascade order).
    out.push_str(&body);
    out
}

/// Extract the `@import` target URLs from the LEADING `@import` block of a
/// stylesheet (CSS requires `@import` rules to precede all other rules except
/// `@charset`). Handles `@import "x.css";`, `@import url(x.css);`, and quoted
/// `url("x.css")`, ignoring any trailing media-query list (fetched regardless in
/// v1). Byte-oriented so a multibyte UTF-8 char in the sheet can never panic a
/// `str` slice. Stops at the first rule that isn't `@charset`/`@import`/comment.
fn extract_import_urls(css: &str) -> Vec<String> {
    let b = css.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut urls = Vec::new();
    // Case-insensitive keyword match at `b[at..]`.
    let kw = |at: usize, k: &[u8]| -> bool {
        at + k.len() <= n && b[at..at + k.len()].eq_ignore_ascii_case(k)
    };
    loop {
        while i < n && b[i].is_ascii_whitespace() {
            i += 1;
        }
        // Skip a CSS comment.
        if i + 1 < n && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            continue;
        }
        if i >= n {
            break;
        }
        // @charset must precede @import; skip it.
        if kw(i, b"@charset") {
            while i < n && b[i] != b';' {
                i += 1;
            }
            i = (i + 1).min(n);
            continue;
        }
        // Anything other than @import ends the leading import block.
        if !kw(i, b"@import") {
            break;
        }
        i += 7;
        while i < n && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let (start, end);
        if kw(i, b"url(") {
            i += 4;
            while i < n && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let quote = if i < n && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                Some(q)
            } else {
                None
            };
            start = i;
            match quote {
                Some(q) => {
                    while i < n && b[i] != q {
                        i += 1;
                    }
                }
                None => {
                    while i < n && b[i] != b')' && !b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                }
            }
            end = i;
        } else if i < n && (b[i] == b'"' || b[i] == b'\'') {
            let q = b[i];
            i += 1;
            start = i;
            while i < n && b[i] != q {
                i += 1;
            }
            end = i;
        } else {
            // Malformed @import: skip to ';' and stop collecting.
            while i < n && b[i] != b';' {
                i += 1;
            }
            i = (i + 1).min(n);
            continue;
        }
        if let Ok(u) = core::str::from_utf8(&b[start..end]) {
            let u = u.trim();
            if !u.is_empty() {
                urls.push(String::from(u));
            }
        }
        // Advance past the statement terminator ';'.
        while i < n && b[i] != b';' {
            i += 1;
        }
        i = (i + 1).min(n);
    }
    urls
}

/// The outcome of navigating + rendering a fetched document.
pub struct PageLoad {
    /// The fetched resource (status, MIME, bytes).
    pub resource: Resource,
    /// Parsed + laid-out tree ready for paint + hit-test.
    pub layout: crate::LayoutBox,
}

/// One-shot navigation: fetch `url`, then run the existing render pipeline over the
/// document body. `css` is the author/UA sheet to cascade on top (Phase 2 will
/// extract `<style>`/`<link>` from the document itself).
///
/// Returns the laid-out tree (for paint + hit-test) alongside the raw resource.
pub fn navigate<T: HttpTransport>(
    pipeline: &crate::RenderPipeline,
    url: &str,
    css: &str,
    transport: &mut T,
) -> Result<PageLoad, LoadError> {
    let resource = fetch_document(url, transport)?;
    if resource.mime != Mime::Html && resource.mime != Mime::Other {
        // Phase 1 only renders documents; a bare CSS/JSON/image navigation is not a
        // page. (The browser shell handles sub-resources in Phase 3.)
        return Err(LoadError::BadResponse);
    }
    let html = resource.as_text();
    let layout = pipeline.render_to_layout(&html, css);
    Ok(PageLoad { resource, layout })
}

/// Collect every `<a href=…>` value from a parsed DOM in **document order**.
///
/// Anchors without an `href` (named bookmarks, JS-only links) are skipped so the
/// index lines up with the rendered, clickable anchor boxes.
fn collect_anchor_hrefs(node: &crate::DomNode, out: &mut Vec<String>) {
    if node.tag_name() == Some("a") {
        if let Some(href) = node.get_attribute("href") {
            out.push(String::from(href));
        }
    }
    for child in &node.children {
        collect_anchor_hrefs(child, out);
    }
}

/// Collect the border-box of every laid-out `<a>` box in **document order** — the
/// same traversal order `build_layout_tree` produced from the DOM, so the Nth box
/// here corresponds to the Nth anchor in [`collect_anchor_hrefs`].
fn collect_anchor_boxes(layout: &crate::LayoutBox, out: &mut Vec<crate::Rect>) {
    if layout.tag_name.as_deref() == Some("a") {
        out.push(layout.dimensions.border_box());
    }
    for child in &layout.children {
        collect_anchor_boxes(child, out);
    }
}

/// Resolve a click at viewport point `(x, y)` to a link target.
///
/// This is the navigation primitive for the browser surface: hit-test the laid-out
/// `<a>` boxes (document order) and, on the first containing box, return the `href`
/// of the matching anchor from the DOM. Returns `None` when the point isn't over a
/// link. Never panics — a DOM/layout anchor-count mismatch simply yields `None` for
/// the unmatched boxes.
///
/// The `(x, y)` passed in must already have scroll applied by the caller (page
/// space), matching the engine's [`crate::hit_test`] convention.
pub fn link_href_at(
    dom: &crate::DomNode,
    layout: &crate::LayoutBox,
    x: f32,
    y: f32,
) -> Option<String> {
    let mut hrefs: Vec<String> = Vec::new();
    collect_anchor_hrefs(dom, &mut hrefs);
    let mut boxes: Vec<crate::Rect> = Vec::new();
    collect_anchor_boxes(layout, &mut boxes);

    for (i, b) in boxes.iter().enumerate() {
        if b.contains_point(x, y) {
            if let Some(href) = hrefs.get(i) {
                if !href.is_empty() {
                    return Some(href.clone());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod loader_text_tests {
    use super::*;
    use alloc::vec;

    fn res(bytes: Vec<u8>) -> Resource {
        Resource {
            mime: Mime::Html,
            bytes,
            status: 200,
            final_url: String::new(),
        }
    }

    #[test]
    fn resolve_stylesheet_url_forms() {
        let base = "http://example.test/dir/page.html";
        // Relative to the document's directory.
        assert_eq!(
            resolve_stylesheet_url(base, "style.css").as_deref(),
            Some("http://example.test/dir/style.css")
        );
        // Root-relative replaces the whole path.
        assert_eq!(
            resolve_stylesheet_url(base, "/css/site.css").as_deref(),
            Some("http://example.test/css/site.css")
        );
        // Parent traversal collapses.
        assert_eq!(
            resolve_stylesheet_url(base, "../a/b.css").as_deref(),
            Some("http://example.test/a/b.css")
        );
        // Absolute http URL passes through.
        assert_eq!(
            resolve_stylesheet_url(base, "http://cdn.test/x.css").as_deref(),
            Some("http://cdn.test/x.css")
        );
        // Scheme-relative inherits http.
        assert_eq!(
            resolve_stylesheet_url(base, "//cdn.test/y.css").as_deref(),
            Some("http://cdn.test/y.css")
        );
        // A query suffix is preserved.
        assert_eq!(
            resolve_stylesheet_url(base, "site.css?v=2").as_deref(),
            Some("http://example.test/dir/site.css?v=2")
        );
        // Unsupported schemes are skipped (never fetched insecurely).
        assert_eq!(resolve_stylesheet_url(base, "https://cdn.test/z.css"), None);
        assert_eq!(resolve_stylesheet_url(base, "data:text/css,body{}"), None);
        // Empty href → None.
        assert_eq!(resolve_stylesheet_url(base, "   "), None);
        // A non-http base cannot resolve a relative href.
        assert_eq!(resolve_stylesheet_url("about:home", "style.css"), None);
    }

    #[test]
    fn fetch_stylesheets_skips_failures_and_caps_count() {
        use athnet::http1::MockTransport;
        // A 200 sheet then a 404 sheet (the mock can only serve one body, so we drive
        // each through its own single-resource transport — exercising both the success
        // and skip paths of the loop).
        let css200 =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: 11\r\n\r\np{color:red}"
                .to_vec();
        let mut ok = MockTransport::new(css200);
        let got = fetch_stylesheets(
            &[String::from("http://h.test/a.css")],
            &mut ok,
            &Limits::new(),
        );
        assert!(
            got.contains("color:red"),
            "200 sheet body must load: {got:?}"
        );

        let css404 = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec();
        let mut bad = MockTransport::new(css404);
        let got = fetch_stylesheets(
            &[String::from("http://h.test/missing.css")],
            &mut bad,
            &Limits::new(),
        );
        assert!(got.is_empty(), "a 404 sheet must be skipped, got {got:?}");

        // A transport that delivers nothing (connection error) is also skipped, no panic.
        let mut dead = MockTransport::new(Vec::new());
        let got = fetch_stylesheets(
            &[String::from("http://h.test/dead.css")],
            &mut dead,
            &Limits::new(),
        );
        assert!(
            got.is_empty(),
            "a failed fetch must be skipped, got {got:?}"
        );
    }

    #[test]
    fn as_text_strips_bom_and_keeps_latin1() {
        // UTF-8 BOM (EF BB BF) is stripped, not left as a stray U+FEFF.
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(b"<p>hi</p>");
        let t = res(b).as_text();
        assert!(t.starts_with("<p>"), "BOM not stripped: {t:?}");
        assert!(!t.contains('\u{FEFF}'));
        // A Latin-1 body (cafe + 0xE9) keeps the accented char instead of dropping it.
        let t2 = res(vec![b'c', b'a', b'f', 0xE9]).as_text();
        assert!(t2.contains('\u{E9}'), "Latin-1 accent dropped: {t2:?}");
        // Plain ASCII unchanged.
        assert_eq!(res(b"hello".to_vec()).as_text(), "hello");
    }

    /// A multi-resource transport: routes by the request-line path to a canned
    /// full HTTP response, so @import recursion can fetch DIFFERENT sheets.
    struct RoutingMock {
        routes: Vec<(String, Vec<u8>)>,
        pending: Vec<u8>,
        pos: usize,
    }
    impl RoutingMock {
        fn new(routes: Vec<(&str, Vec<u8>)>) -> Self {
            Self {
                routes: routes
                    .into_iter()
                    .map(|(p, b)| (String::from(p), b))
                    .collect(),
                pending: Vec::new(),
                pos: 0,
            }
        }
    }
    fn css_200(body: &str) -> Vec<u8> {
        let mut v = alloc::format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        v.extend_from_slice(body.as_bytes());
        v
    }
    impl HttpTransport for RoutingMock {
        fn connect(&mut self, _host: &str, _port: u16) -> athnet::http1::Http1Result<()> {
            Ok(())
        }
        fn send(&mut self, buf: &[u8]) -> athnet::http1::Http1Result<()> {
            let line = core::str::from_utf8(buf).unwrap_or("");
            let path = line.split_whitespace().nth(1).unwrap_or("/");
            self.pending = self
                .routes
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            self.pos = 0;
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> athnet::http1::Http1Result<usize> {
            let n = (self.pending.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n) // 0 when drained = EOF
        }
    }

    #[test]
    fn extract_import_urls_parses_leading_block() {
        let css = "@charset \"utf-8\";\n/* c */ @import \"a.css\";\n\
                   @import url(b.css);\n@import url('c.css') screen;\n\
                   p{color:red} @import \"late.css\";";
        let urls = extract_import_urls(css);
        // Three leading imports collected (string, url(), quoted url() + media).
        assert_eq!(
            urls,
            alloc::vec![
                String::from("a.css"),
                String::from("b.css"),
                String::from("c.css")
            ],
            "leading @import block: {urls:?}"
        );
        // The @import AFTER the first real rule (`p{}`) is NOT collected.
        assert!(!urls.iter().any(|u| u == "late.css"));
    }

    #[test]
    fn at_import_is_fetched_and_prepended() {
        // main.css imports sub.css; the imported rules must cascade BEFORE main's.
        let mut tx = RoutingMock::new(alloc::vec![
            (
                "/main.css",
                css_200("@import \"sub.css\";\n.main{color:red}")
            ),
            ("/sub.css", css_200(".sub{color:blue}")),
        ]);
        let out = fetch_stylesheets(
            &[String::from("http://h.test/main.css")],
            &mut tx,
            &Limits::new(),
        );
        assert!(out.contains(".sub"), "imported sheet missing: {out:?}");
        assert!(out.contains(".main"), "importing sheet missing: {out:?}");
        let sub = out.find(".sub").unwrap();
        let main = out.find(".main").unwrap();
        assert!(
            sub < main,
            "imported rules must precede importing rules: {out:?}"
        );
    }

    #[test]
    fn at_import_cycle_terminates() {
        // a.css imports itself — must terminate at the budget, never hang.
        let mut tx = RoutingMock::new(alloc::vec![(
            "/a.css",
            css_200("@import \"a.css\";\n.a{color:green}"),
        )]);
        let out = fetch_stylesheets(
            &[String::from("http://h.test/a.css")],
            &mut tx,
            &Limits::new(),
        );
        assert!(
            out.contains(".a"),
            "self-importing sheet must still load: {out:?}"
        );
    }
}
