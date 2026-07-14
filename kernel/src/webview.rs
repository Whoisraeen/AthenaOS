//! RaeWeb browser surface — the kernel-drawn, compositor-presented web view.
//!
//! > "Native everywhere. No Electron tax. No web wrappers. Native rendering,
//! > native input, native audio — sub-frame latency end to end."
//! > — AthenaOS Concept §Core Principles #1
//!
//! > "Web apps via PWA support that actually feels native (renders through AthUI)."
//! > — AthenaOS Concept §Compatibility Strategy #3
//!
//! This is the Phase-2 browser surface (docs/research/web-engine.md): a minimal but
//! REAL browser that fetches → parses → lays out → paints a web document through the
//! `athweb` engine and presents the painted [`athgfx::Canvas`] via the compositor —
//! exactly like the live `Settings`/`GameOS` shell surfaces, NOT a userspace ELF
//! (the userspace `apps/web` build hits the known `poly1305`/`sha2` SIMD-split LLVM
//! error; the kernel's build-std/soft-float config already links `athnet` cleanly,
//! so the browser rides the kernel surface path for this slice — a standalone
//! `apps/web` crate with `-Z build-std` flags is a documented later slice).
//!
//! No new syscall / ABI: this is kernel-side rendering over the existing
//! `athnet`/`athgfx` APIs (web-engine.md §"Interface needs": Phase 1–3 needs no
//! syscall). `#![no_std]`-clean (the kernel is no_std), never panics on malformed
//! HTML or a network error — the engine is resilient and the surface renders an
//! error page rather than crashing.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use ath_tokens::{derive_accent, AccentRamp, Palette, DARK, RADIUS_SM, TYPE_BODY};
use athgfx::text::FontFamily;
use athgfx::Canvas;
use athweb::loader::{self, Mime};
use athweb::{build_display_list, count_dom_nodes, parse_html, DomNode, LayoutBox, RenderPipeline};
use spin::Mutex;

/// Active palette — the browser chrome is dark glass like the rest of the shell.
const PALETTE: &Palette = &DARK;

/// Chrome surfaces (token-driven so a Vibe-Mode re-skin flows here too).
const CHROME_BG: u32 = DARK.bg_raised;
const TOOLBAR_BG: u32 = DARK.bg_elevated;
const FIELD_BG: u32 = DARK.bg_base;
const FIELD_BORDER: u32 = DARK.stroke_subtle;
const FG: u32 = DARK.text_primary;
const FG_DIM: u32 = DARK.text_secondary;
const FG_MUTED: u32 = DARK.text_tertiary;
/// Page canvas background — content paints on a light surface (the default UA
/// `body` background) so dark-on-light author text is legible.
const PAGE_BG: u32 = 0xFF_FF_FF_FF;

/// Height of the chrome toolbar (address bar + nav buttons) in device pixels.
const TOOLBAR_H: usize = 56;

/// The LIVE accent ramp for the browser chrome — derived from the active theme/Vibe
/// seed so the toolbar accent + focus ring recolour on a one-tap re-skin, same as
/// every other surface (`login_ui::accent`, `notify::proof_accent`).
#[inline]
fn accent() -> AccentRamp {
    derive_accent(crate::theme_engine::active_accent(), PALETTE)
}

/// Public accent base for the cross-surface cohesion smoketest.
#[inline]
pub fn proof_accent() -> u32 {
    accent().base
}

// ─── Bundled pages (deterministic content; the loopback/offline corpus) ─────
//
// These render through the SAME real `athnet::http1::fetch_with` path as a live
// fetch — they're served from an in-memory `MockTransport` (a canned 200 OK), so
// `fetched=bundled` still exercises the genuine HTTP parse + engine pipeline. They
// also give link navigation a deterministic graph (home ↔ about) the smoketest
// asserts on, independent of whether the QEMU/iron net is up.

/// The author/UA stylesheet cascaded over every bundled + fetched document. Kept
/// tiny and resilient — the engine degrades gracefully on anything it can't parse.
const UA_CSS: &str = "body { color: #1a1a1a; font-size: 16px } \
    h1 { font-size: 28px; font-weight: bold; color: #0a0a0a } \
    h2 { font-size: 20px; font-weight: bold } \
    p { font-size: 16px } \
    a { color: #0a64c8; display: block; height: 26px; font-size: 16px }";

/// Resolve a bundled URL to its HTML body, or `None` if it isn't a known local page.
/// Accepts both the `rae://` home scheme and `http://localhost/...` loopback paths.
fn bundled_page(url: &str) -> Option<&'static str> {
    let path = url
        .strip_prefix("rae://")
        .or_else(|| url.strip_prefix("http://localhost/"))
        .or_else(|| url.strip_prefix("http://localhost"))
        .unwrap_or(url);
    let path = path.trim_start_matches('/');
    match path {
        "" | "home" | "index.html" | "start" => Some(PAGE_HOME),
        "about" | "about.html" => Some(PAGE_ABOUT),
        "welcome" => Some(PAGE_WELCOME),
        _ => None,
    }
}

const PAGE_HOME: &str = "<!DOCTYPE html><html><body>\
    <h1>RaeWeb</h1>\
    <p>The native web surface. No Electron tax.</p>\
    <p>Pages render through the RaeWeb engine straight to the compositor.</p>\
    <a href=\"rae://about\">About RaeWeb</a>\
    <a href=\"rae://welcome\">Welcome page</a>\
    </body></html>";

const PAGE_ABOUT: &str = "<!DOCTYPE html><html><body>\
    <h1>About</h1>\
    <p>RaeWeb is a from-scratch no_std HTML/CSS/layout/paint engine.</p>\
    <p>It renders through athgfx::Canvas, the same crisp-AA path as AthUI.</p>\
    <a href=\"rae://home\">Back home</a>\
    </body></html>";

const PAGE_WELCOME: &str = "<!DOCTYPE html><html><body>\
    <h1>Welcome</h1>\
    <p>Native everywhere. Native rendering, native input, native audio.</p>\
    <a href=\"rae://home\">Back home</a>\
    </body></html>";

/// Build a canned HTTP/1.1 200 response carrying `body` — fed to athnet's
/// `MockTransport` so a bundled navigation exercises the real fetch+parse path.
fn canned_http_response(body: &str) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: ");
    out.push_str(&alloc::format!("{}", body.len()));
    out.push_str("\r\n\r\n");
    out.push_str(body);
    out.into_bytes()
}

/// Render the standard error page (itself a trivial document the engine paints —
/// the spec's dogfood failure mode). Never leaves the user on a blank screen.
fn error_page(url: &str, reason: &str) -> String {
    alloc::format!(
        "<!DOCTYPE html><html><body>\
        <h1>Can't load page</h1>\
        <p>{}</p>\
        <p>{}</p>\
        <a href=\"rae://home\">Back home</a>\
        </body></html>",
        url,
        reason,
    )
}

// ─── The browser surface state ──────────────────────────────────────────────

/// One web-view surface (single-tab for this slice). Owns the render pipeline,
/// the current document (DOM + layout for hit-test/link-nav), the chrome state
/// (address bar, history), and the last render stats for `/proc/athena/web`.
pub struct WebView {
    pipeline: RenderPipeline,
    /// Current document URL (shown in the address bar).
    url: String,
    /// The parsed DOM for link navigation (anchors resolved against it).
    dom: DomNode,
    /// The laid-out tree for paint + hit-test.
    layout: LayoutBox,
    /// Vertical scroll offset (page space).
    scroll_y: f32,
    /// Back / forward history stacks (URLs).
    back: Vec<String>,
    forward: Vec<String>,
    /// Address-bar edit state.
    editing: bool,
    edit_buf: String,
    /// Last render stats (proof material for procfs + smoketest).
    dom_nodes: usize,
    paint_cmds: usize,
    last_text_draws: usize,
    /// How the last document was obtained.
    last_source: PageSource,
}

/// How a document was obtained — drives the `fetched=` field of the smoketest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSource {
    Bundled,
    Live,
    Error,
}

impl PageSource {
    fn as_str(&self) -> &'static str {
        match self {
            PageSource::Bundled => "bundled",
            PageSource::Live => "live",
            PageSource::Error => "error",
        }
    }
}

impl WebView {
    /// Create a web view sized to `(w, h)` device pixels (the surface size). The
    /// page viewport is the area below the chrome toolbar.
    pub fn new(w: usize, h: usize) -> Self {
        let page_w = w as f32;
        let page_h = (h.saturating_sub(TOOLBAR_H)) as f32;
        let pipeline = RenderPipeline::new(page_w, page_h);
        let mut v = Self {
            pipeline,
            url: String::new(),
            dom: parse_html(""),
            layout: LayoutBox::new(athweb::DisplayMode::Block),
            scroll_y: 0.0,
            back: Vec::new(),
            forward: Vec::new(),
            editing: false,
            edit_buf: String::new(),
            dom_nodes: 0,
            paint_cmds: 0,
            last_text_draws: 0,
            last_source: PageSource::Error,
        };
        v.load("rae://home", true);
        v
    }

    /// Current URL (for procfs / smoketest).
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn dom_nodes(&self) -> usize {
        self.dom_nodes
    }
    pub fn paint_cmds(&self) -> usize {
        self.paint_cmds
    }
    pub fn last_text_draws(&self) -> usize {
        self.last_text_draws
    }
    pub fn last_source(&self) -> PageSource {
        self.last_source
    }

    /// Fetch `url`, parse, lay out, and store as the current document. `push_history`
    /// records the previous URL on the back stack (false for back/forward replays).
    ///
    /// Bundled pages are served from an in-memory `MockTransport` (real fetch+parse,
    /// deterministic); a real `http://host/...` triggers a best-effort live
    /// `athnet` fetch. Any failure renders the error page — never a panic, never a
    /// blank surface.
    fn load(&mut self, url: &str, push_history: bool) {
        if push_history && !self.url.is_empty() && self.url != url {
            self.back.push(self.url.clone());
            self.forward.clear();
        }

        let (html, source) = self.fetch(url);
        self.set_document(url, &html, source);
    }

    /// Fetch the document body for `url`, returning `(html, source)`.
    fn fetch(&self, url: &str) -> (String, PageSource) {
        // 1. Bundled / loopback page → canned response over the real fetch path.
        //    `loader::fetch_document` → `fetch_with` → `HttpUrl::parse`, which only
        //    accepts `http://`, so the bundled fetch uses a loopback http URL (the
        //    MockTransport ignores the host and serves the canned bytes). The DISPLAY
        //    url stays `rae://…`; this is purely the wire form the parser accepts.
        if let Some(body) = bundled_page(url) {
            let mut transport = athnet::http1::MockTransport::new(canned_http_response(body));
            match loader::fetch_document("http://localhost/", &mut transport) {
                Ok(res) if res.mime == Mime::Html || res.mime == Mime::Other => {
                    return (res.as_text(), PageSource::Bundled);
                }
                _ => {
                    return (
                        error_page(url, "Bundled document failed to parse."),
                        PageSource::Error,
                    );
                }
            }
        }

        // 2. A real remote http:// URL → best-effort live fetch. (Plaintext only;
        //    HTTPS/TLS is a Phase-4 dependency, web-engine.md §security.) If the net
        //    is down (headless QEMU / iron RX gated — live fix #2), this returns an
        //    error page rather than hanging.
        if url.starts_with("http://") {
            match live_fetch(url) {
                Ok(html) => return (html, PageSource::Live),
                Err(reason) => return (error_page(url, reason), PageSource::Error),
            }
        }

        (
            error_page(url, "Unsupported address (try rae://home)."),
            PageSource::Error,
        )
    }

    /// Parse + lay out `html` and adopt it as the current document.
    fn set_document(&mut self, url: &str, html: &str, source: PageSource) {
        self.url = String::from(url);
        self.dom = parse_html(html);
        self.dom_nodes = count_dom_nodes(&self.dom);
        self.layout = self.pipeline.render_to_layout(html, UA_CSS);
        let dl = build_display_list(&self.layout, &self.pipeline.viewport);
        self.paint_cmds = dl.commands.len();
        self.scroll_y = 0.0;
        self.pipeline.viewport.scroll_y = 0.0;
        self.last_source = source;
        self.editing = false;
        self.edit_buf.clear();
    }

    /// Navigate to a URL (public entry — clears forward history).
    pub fn navigate(&mut self, url: &str) {
        self.load(url, true);
    }

    /// Go back one entry, if any. Returns true if it navigated.
    pub fn go_back(&mut self) -> bool {
        if let Some(prev) = self.back.pop() {
            self.forward.push(self.url.clone());
            let (html, source) = self.fetch(&prev);
            self.set_document(&prev, &html, source);
            true
        } else {
            false
        }
    }

    /// Go forward one entry, if any. Returns true if it navigated.
    pub fn go_forward(&mut self) -> bool {
        if let Some(next) = self.forward.pop() {
            self.back.push(self.url.clone());
            let (html, source) = self.fetch(&next);
            self.set_document(&next, &html, source);
            true
        } else {
            false
        }
    }

    /// Reload the current page.
    pub fn reload(&mut self) {
        let url = self.url.clone();
        let (html, source) = self.fetch(&url);
        self.set_document(&url, &html, source);
    }

    /// Scroll the page by `dy` device pixels (clamped to ≥0).
    pub fn scroll(&mut self, dy: f32) {
        self.scroll_y = (self.scroll_y + dy).max(0.0);
        self.pipeline.viewport.scroll_y = self.scroll_y;
    }

    /// Resolve a click in **surface** coordinates `(x, y)`. A click in the page
    /// region over a link navigates; a click in the toolbar focuses the address
    /// bar. Returns true if the surface needs a repaint.
    pub fn click(&mut self, x: f32, y: f32) -> bool {
        if (y as usize) < TOOLBAR_H {
            // Toolbar: focus the address bar for editing.
            self.editing = true;
            self.edit_buf = self.url.clone();
            return true;
        }
        // Page space: translate into document coordinates (add scroll, drop the
        // toolbar offset) and resolve a link.
        let page_x = x;
        let page_y = (y - TOOLBAR_H as f32) + self.scroll_y;
        if let Some(href) = loader::link_href_at(&self.dom, &self.layout, page_x, page_y) {
            let target = self.resolve_href(&href);
            self.navigate(&target);
            return true;
        }
        false
    }

    /// Activate a link by index (the keyboard/programmatic path — Enter on the Nth
    /// link). Returns the resolved target URL it navigated to, if any.
    pub fn activate_link(&mut self, index: usize) -> Option<String> {
        let mut hrefs: Vec<String> = Vec::new();
        collect_hrefs(&self.dom, &mut hrefs);
        let href = hrefs.get(index)?.clone();
        let target = self.resolve_href(&href);
        self.navigate(&target);
        Some(target)
    }

    /// Resolve an `href` (possibly relative) against the current URL. Minimal: absolute
    /// `rae://`/`http://` pass through; a leading-`/` path joins the current origin;
    /// anything else is treated as a loopback path.
    fn resolve_href(&self, href: &str) -> String {
        if href.starts_with("rae://") || href.starts_with("http://") || href.starts_with("https://")
        {
            return String::from(href);
        }
        if href.starts_with('/') {
            // Join to current origin if it's an http URL; else loopback.
            if let Some(rest) = self.url.strip_prefix("http://") {
                if let Some(slash) = rest.find('/') {
                    return alloc::format!("http://{}{}", &rest[..slash], href);
                }
                return alloc::format!("http://{}{}", rest, href);
            }
            return alloc::format!("rae:/{}", href);
        }
        // Bare name → bundled page.
        alloc::format!("rae://{}", href)
    }

    // ── Address-bar editing ────────────────────────────────────────────────

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// Begin editing the address bar (select-all semantics: start from current URL).
    pub fn begin_edit(&mut self) {
        self.editing = true;
        self.edit_buf = self.url.clone();
    }

    /// Type a character into the address bar. Returns true if accepted.
    pub fn type_char(&mut self, c: char) -> bool {
        if !self.editing || self.edit_buf.len() >= 256 {
            return false;
        }
        self.edit_buf.push(c);
        true
    }

    /// Backspace in the address bar.
    pub fn backspace(&mut self) {
        if self.editing {
            self.edit_buf.pop();
        }
    }

    /// Commit the address bar (Enter): navigate to the typed URL.
    pub fn commit_edit(&mut self) {
        if !self.editing {
            return;
        }
        let target = String::from(self.edit_buf.trim());
        self.editing = false;
        if !target.is_empty() {
            self.navigate(&target);
        }
    }

    /// Cancel editing (Esc): restore the displayed URL.
    pub fn cancel_edit(&mut self) {
        self.editing = false;
        self.edit_buf.clear();
    }

    // ── Render ─────────────────────────────────────────────────────────────

    /// Paint the whole surface (chrome + page) into `canvas`, returning the page
    /// paint stats (proof: a non-zero `text_draws` means the page actually drew).
    pub fn render(&mut self, canvas: &mut Canvas) -> athweb::backend::PaintStats {
        let w = canvas.width();
        let h = canvas.height();
        let acc = accent();

        // 1. Page background fills the whole surface first (so scroll past content
        //    shows the page color, not stale chrome pixels).
        canvas.fill_rect(0, 0, w, h.saturating_sub(0), PAGE_BG);

        // 2. Paint the page content under the toolbar. We paint into the full canvas
        //    but the engine's display list is offset by the toolbar via a translated
        //    scroll: the backend subtracts (scroll_x, scroll_y), so we add TOOLBAR_H
        //    worth of negative scroll to push content down.
        let dl = build_display_list(&self.layout, &self.pipeline.viewport);
        self.paint_cmds = dl.commands.len();
        let stats = athweb::backend::paint_displaylist_to_canvas(
            &dl,
            canvas,
            0.0,
            self.scroll_y - TOOLBAR_H as f32,
        );
        self.last_text_draws = stats.text_draws;

        // 3. Chrome toolbar on top (so it always covers the page top edge).
        canvas.fill_rect(0, 0, w, TOOLBAR_H, CHROME_BG);
        // Toolbar accent hairline at the bottom edge.
        canvas.fill_rect(0, TOOLBAR_H.saturating_sub(2), w, 2, acc.subtle);

        // Nav buttons: back / forward / reload (simple glyph chips).
        let pad = 12usize;
        let btn = 32usize;
        let by = (TOOLBAR_H - btn) / 2;
        let back_color = if self.back.is_empty() { FG_MUTED } else { FG };
        let fwd_color = if self.forward.is_empty() {
            FG_MUTED
        } else {
            FG
        };
        self.chip(canvas, pad, by, btn, "<", back_color);
        self.chip(canvas, pad * 2 + btn, by, btn, ">", fwd_color);
        self.chip(canvas, pad * 3 + btn * 2, by, btn, "R", FG_DIM);

        // Address bar field (the remaining toolbar width).
        let field_x = pad * 4 + btn * 3;
        let field_y = by;
        let field_w = w.saturating_sub(field_x + pad);
        let field_h = btn;
        canvas.fill_rounded_rect(
            field_x,
            field_y,
            field_w,
            field_h,
            RADIUS_SM as usize,
            FIELD_BG,
        );
        let border = if self.editing { acc.base } else { FIELD_BORDER };
        canvas.draw_rounded_rect_outline(
            field_x,
            field_y,
            field_w,
            field_h,
            RADIUS_SM as usize,
            border,
        );
        let shown = if self.editing {
            &self.edit_buf
        } else {
            &self.url
        };
        canvas.draw_text_aa(
            (field_x + 10) as i32,
            (field_y + (field_h - TYPE_BODY.line_height as usize) / 2) as i32,
            shown,
            TYPE_BODY,
            FG,
            FontFamily::Sans,
        );

        // 4. Title strip is implicit for this slice (the page <h1> is the title).
        stats
    }

    /// Draw a small toolbar chip with a centered glyph.
    fn chip(&self, canvas: &mut Canvas, x: usize, y: usize, size: usize, glyph: &str, fg: u32) {
        canvas.fill_rounded_rect(x, y, size, size, RADIUS_SM as usize, TOOLBAR_BG);
        let gw = canvas.measure_text_aa(glyph, TYPE_BODY, FontFamily::Sans);
        canvas.draw_text_aa(
            (x + size / 2) as i32 - gw / 2,
            (y + (size - TYPE_BODY.line_height as usize) / 2) as i32,
            glyph,
            TYPE_BODY,
            fg,
            FontFamily::Sans,
        );
    }
}

/// Collect anchor hrefs from a DOM in document order (mirrors the engine's
/// `link_href_at` collection; used by `activate_link`).
fn collect_hrefs(node: &DomNode, out: &mut Vec<String>) {
    if node.tag_name() == Some("a") {
        if let Some(href) = node.get_attribute("href") {
            out.push(String::from(href));
        }
    }
    for child in &node.children {
        collect_hrefs(child, out);
    }
}

// ─── Live fetch (best-effort, plaintext) ────────────────────────────────────

/// Best-effort live HTTP/1.1 GET over the kernel smoltcp socket stack.
///
/// This is the genuine network path the browser uses post-boot when the net is up
/// (DHCP bound). It is deliberately bounded: a fixed poll budget so a dead/headless
/// net surfaces as an error page rather than wedging the surface. HTTPS is out of
/// scope (TLS 1.3 is a Phase-4 dependency); only `http://` is attempted.
fn live_fetch(url: &str) -> Result<String, &'static str> {
    // HttpUrl::parse rejects any non-`http://` scheme (incl. https) — TLS 1.3 is a
    // Phase-4 dependency (web-engine.md §security), so an https:// URL fails here.
    let parsed = athnet::http1::HttpUrl::parse(url)
        .map_err(|_| "Unsupported URL (http:// only — HTTPS is a later phase).")?;
    let host = parsed.host.clone();
    let port = parsed.port;

    // Resolve host → IPv4. A dotted-quad passes through; a name goes through DNS.
    let ip = match parse_dotted_quad(&host) {
        Some(ip) => ip,
        None => crate::dns::resolve_blocking(&host).ok_or("DNS resolution failed.")?,
    };

    let pid = crate::scheduler::current_task_id()
        .map(|t| t.raw())
        .unwrap_or(0);

    let fd = crate::net::sys_net_socket(0, pid);
    if fd == u64::MAX {
        return Err("Socket create failed.");
    }
    let ip_packed =
        ((ip[0] as u64) << 24) | ((ip[1] as u64) << 16) | ((ip[2] as u64) << 8) | (ip[3] as u64);
    let res = (|| -> Result<String, &'static str> {
        if crate::net::sys_net_connect(fd, ip_packed, port as u64, pid) == u64::MAX {
            return Err("Connect failed.");
        }
        // Pump the stack until connected or the budget is exhausted.
        for _ in 0..2000 {
            crate::net::poll_full();
        }

        let req = alloc::format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: RaeWeb/0.1\r\n\r\n",
            parsed.path,
            host,
        );
        if crate::net::sys_net_send(fd, req.as_bytes(), pid) == u64::MAX {
            return Err("Send failed.");
        }

        let mut body: Vec<u8> = Vec::new();
        let mut buf = [0u8; 2048];
        let mut idle = 0u32;
        // Bounded receive loop: ~16k poll cycles total, stop on a clean EOF or after
        // a stretch of empty reads (peer done / nothing coming).
        for _ in 0..16000 {
            crate::net::poll_full();
            let n = crate::net::sys_net_recv(fd, &mut buf, pid);
            if n == u64::MAX {
                break;
            }
            if n == 0 {
                idle += 1;
                if idle > 4000 && !body.is_empty() {
                    break;
                }
                continue;
            }
            idle = 0;
            body.extend_from_slice(&buf[..n as usize]);
            if body.len() > 1 << 20 {
                break; // 1 MiB page budget
            }
        }

        if body.is_empty() {
            return Err("No response (is the network up?).");
        }
        // Strip HTTP headers: find the CRLFCRLF boundary.
        let html = match find_body_start(&body) {
            Some(start) => bytes_to_text(&body[start..]),
            None => bytes_to_text(&body),
        };
        Ok(html)
    })();

    crate::net::sys_net_close(fd, pid);
    res
}

/// Parse a dotted-quad IPv4 string, or `None` if it isn't one.
fn parse_dotted_quad(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = s.split('.');
    for slot in out.iter_mut() {
        let p = parts.next()?;
        *slot = p.parse::<u8>().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

/// Find the byte offset just past the `\r\n\r\n` header/body separator.
fn find_body_start(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Lossy bytes → text (drop non-printable/non-UTF8 rather than panic).
fn bytes_to_text(bytes: &[u8]) -> String {
    match core::str::from_utf8(bytes) {
        Ok(s) => String::from(s),
        Err(_) => bytes
            .iter()
            .filter(|&&b| b == b'\n' || b == b'\t' || (0x20..0x7f).contains(&b))
            .map(|&b| b as char)
            .collect(),
    }
}

// ─── R10 contract: status (procfs), boot smoketest, init ────────────────────

static LAST_DOM_NODES: AtomicUsize = AtomicUsize::new(0);
static LAST_PAINT_CMDS: AtomicUsize = AtomicUsize::new(0);
static SMOKETEST_PASSED: AtomicU64 = AtomicU64::new(0);
static LAST_URL: Mutex<String> = Mutex::new(String::new());
static LAST_SOURCE: AtomicU64 = AtomicU64::new(0); // 0=err 1=bundled 2=live

/// `/proc/athena/web` — engine status line (R10).
///
/// Format: `engine=athweb tabs=<n> last_url=<…> dom_nodes=<n> paint_cmds=<n> js=disabled`
pub fn dump_text() -> String {
    let url = LAST_URL.lock().clone();
    let dom_nodes = LAST_DOM_NODES.load(Ordering::Relaxed);
    let paint_cmds = LAST_PAINT_CMDS.load(Ordering::Relaxed);
    let source = match LAST_SOURCE.load(Ordering::Relaxed) {
        2 => "live",
        1 => "bundled",
        _ => "error",
    };
    let passed = SMOKETEST_PASSED.load(Ordering::Relaxed) == 1;
    let mut out = String::new();
    out.push_str("# RaeWeb — native web surface (renders through AthUI, no Electron tax)\n");
    out.push_str(&alloc::format!(
        "engine=athweb tabs=1 last_url={} dom_nodes={} paint_cmds={} source={} js=disabled\n",
        if url.is_empty() { "-" } else { url.as_str() },
        dom_nodes,
        paint_cmds,
        source,
    ));
    out.push_str(&alloc::format!(
        "boot_smoketest={}\n",
        if passed { "PASS" } else { "FAIL" }
    ));
    out
}

/// Initialize the web surface subsystem (R10 init). No heavy state — the WebView is
/// constructed lazily by the shell when the user opens the browser (F7). This just
/// records readiness for procfs and logs the Concept line it serves.
pub fn init() {
    *LAST_URL.lock() = String::new();
    crate::serial_println!(
        "[ OK ] RaeWeb surface ready (native web view; renders through AthUI, no Electron tax)"
    );
}

/// Boot smoketest (R10) — proves the full browser surface path end to end:
/// construct a WebView (home page bundled-fetched + parsed + laid out), paint it
/// into a real Canvas, then activate the first link and confirm the page changed.
///
/// FAIL conditions (the test can print FAIL): no DOM nodes, the page paints zero
/// text, or link navigation does not change the URL/DOM. Emits the house proof line:
///   `[web] browser smoketest: navigated=ok fetched=bundled dom_nodes=N painted=ok
///    presented=ok link_nav=ok -> PASS`
pub fn run_boot_smoketest() {
    const W: usize = 800;
    const H: usize = 600;

    // 1. Construct the view — this navigates to rae://home (real fetch+parse).
    let mut view = WebView::new(W, H);
    let navigated = !view.url().is_empty() && view.dom_nodes() > 0;
    let fetched = view.last_source();
    let dom_nodes = view.dom_nodes();

    // 2. Paint into a real athgfx Canvas backed by an owned buffer (exercises the
    //    genuine draw_text_aa / fill_* path, not a counting stub).
    let mut buf = alloc::vec![0u8; W * H * 4];
    // SAFETY: `buf` outlives the Canvas (both dropped at end of fn); the Canvas only
    // writes within W*H*4 bytes, which `buf` provides.
    let mut canvas = unsafe { Canvas::new(buf.as_mut_ptr(), W, H, 4) };
    let stats = view.render(&mut canvas);
    let painted = stats.total_commands >= 1 && stats.text_draws >= 1;
    // "presented" = the surface produced non-blank pixels (a sample of the page area
    // is non-background). This is the FAIL hook for "presents 0 painted content".
    let presented = canvas_has_content(&buf, W, H);

    // 3. Link navigation: home has a link to rae://about — activate link 0 and
    //    confirm the URL changed and the new DOM is non-empty.
    let before = String::from(view.url());
    let link_target = view.activate_link(0);
    let after = String::from(view.url());
    let link_nav = link_target.is_some() && after != before && view.dom_nodes() > 0;

    // Record for procfs.
    *LAST_URL.lock() = after.clone();
    LAST_DOM_NODES.store(view.dom_nodes(), Ordering::Relaxed);
    LAST_PAINT_CMDS.store(view.paint_cmds(), Ordering::Relaxed);
    LAST_SOURCE.store(
        match fetched {
            PageSource::Live => 2,
            PageSource::Bundled => 1,
            PageSource::Error => 0,
        },
        Ordering::Relaxed,
    );

    let pass = navigated && painted && presented && link_nav && fetched != PageSource::Error;
    SMOKETEST_PASSED.store(if pass { 1 } else { 0 }, Ordering::Relaxed);

    crate::serial_println!(
        "[web] browser smoketest: navigated={} fetched={} dom_nodes={} painted={} presented={} link_nav={} -> {}",
        if navigated { "ok" } else { "FAIL" },
        fetched.as_str(),
        dom_nodes,
        if painted { "ok" } else { "FAIL" },
        if presented { "ok" } else { "FAIL" },
        if link_nav { "ok" } else { "FAIL" },
        if pass { "PASS" } else { "FAIL" },
    );
}

/// True if the buffer has any non-background (non-white, non-zero) pixel — the
/// "did anything actually present?" check. Samples a stride to stay cheap.
fn canvas_has_content(buf: &[u8], w: usize, h: usize) -> bool {
    // Sample the page region (below the toolbar) every 37 pixels.
    let start = TOOLBAR_H * w * 4;
    let mut i = start;
    let end = (w * h * 4).min(buf.len());
    while i + 3 < end {
        let b = buf[i];
        let g = buf[i + 1];
        let r = buf[i + 2];
        // Not page-white and not transparent-black → real drawn content.
        if !(r == 0xFF && g == 0xFF && b == 0xFF) && !(r == 0 && g == 0 && b == 0) {
            return true;
        }
        i += 37 * 4;
    }
    false
}
