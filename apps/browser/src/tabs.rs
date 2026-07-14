//! Multi-tab browsing — *"the web is the universal app runtime; PWAs that feel
//! native"* (LEGACY_GAMING_CONCEPT.md §"3. Web apps via PWA support that actually feels
//! native"). A daily-driver browser needs tabs, and each tab must be an
//! INDEPENDENT browsing context: its own navigation history, current page, title,
//! and scroll position. Navigating, going back, or reloading in one tab must never
//! touch another.
//!
//! ## Design — syscall-free, exactly like [`crate::BrowserModel`]
//! A [`Tab`] is a thin wrapper around ONE [`crate::BrowserModel`] (the existing,
//! host-KAT'd per-document navigation + render core) plus the tab's view state
//! (current [`crate::LoadedPage`], title, scroll). Reusing `BrowserModel` as the
//! per-tab unit means every tab inherits the proven history/back/forward/fetch
//! logic for free, and the whole [`TabManager`] stays linkable in the host KAT
//! (`cargo test -p browser --features host`) with no kernel — the model feeds on
//! in-memory HTML/CSS strings and an injectable [`raenet::http1::HttpTransport`].
//!
//! The [`TabManager`] owns N tabs and an active index. It guarantees the invariant
//! a tab strip relies on: **there is always at least one tab** (closing the last
//! tab replaces it with a fresh empty one — never zero), and the count is bounded
//! by [`MAX_TABS`] so nothing is unbounded.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::{BrowserModel, LoadedPage};

/// The hard cap on open tabs. A daily-driver browser opens many tabs, but an
/// unbounded count is a memory-exhaustion vector (each tab retains a rendered
/// [`LoadedPage`] and a history stack), so [`TabManager::new_tab`] refuses to grow
/// past this. Chosen generously — far more than a human keeps open — while still
/// being a real ceiling.
pub const MAX_TABS: usize = 100;

/// The placeholder title shown for a tab that has not loaded a page yet.
pub const EMPTY_TAB_TITLE: &str = "New Tab";

/// One browsing context: its OWN navigation core ([`BrowserModel`]), the page it is
/// currently showing, a cached title for the chrome, and its scroll offset. Two tabs
/// share NOTHING — operating on one never affects another.
pub struct Tab {
    /// The per-tab navigation core (history stack + render/fetch pipeline). Private so
    /// callers go through the tab's own navigate/back/forward/reload methods, which keep
    /// `page`, `title`, and `scroll_y` in sync with the model's cursor.
    model: BrowserModel,
    /// The page currently shown in this tab (`None` until the first load).
    page: Option<LoadedPage>,
    /// The tab's display title — the loaded page's `<title>`, falling back to the URL's
    /// host, falling back to [`EMPTY_TAB_TITLE`]. Cached so the tab strip can render
    /// without re-parsing each frame.
    title: String,
    /// This tab's vertical scroll offset into its web view. Per-tab so switching tabs
    /// restores where each one was scrolled to.
    pub scroll_y: f32,
}

impl Default for Tab {
    fn default() -> Self {
        Self::new()
    }
}

impl Tab {
    /// A fresh, empty tab (no page loaded, no history).
    pub fn new() -> Tab {
        Tab {
            model: BrowserModel::new(),
            page: None,
            title: String::from(EMPTY_TAB_TITLE),
            scroll_y: 0.0,
        }
    }

    /// The tab's display title (page `<title>`, else host, else "New Tab").
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The page this tab currently shows, if any.
    pub fn page(&self) -> Option<&LoadedPage> {
        self.page.as_ref()
    }

    /// The URL this tab is currently at (its reload target), or `None` if empty.
    pub fn current_url(&self) -> Option<&str> {
        self.model.current_url()
    }

    /// Whether a [`back`](Tab::back) is possible in THIS tab.
    pub fn can_go_back(&self) -> bool {
        self.model.can_go_back()
    }

    /// Whether a [`forward`](Tab::forward) is possible in THIS tab.
    pub fn can_go_forward(&self) -> bool {
        self.model.can_go_forward()
    }

    /// The number of entries in THIS tab's history.
    pub fn history_len(&self) -> usize {
        self.model.history_len()
    }

    /// Direct (read-only) access to this tab's navigation core, for callers that need
    /// the lower-level model (e.g. the live ELF's display-list bridge).
    pub fn model(&self) -> &BrowserModel {
        &self.model
    }

    /// Render `html`+`css` for `url`, run its script, record the navigation in THIS
    /// tab's history, and make it the shown page. Truncates this tab's forward history
    /// (standard browser model). Resets this tab's scroll to the top.
    ///
    /// Pure w.r.t. syscalls: the caller supplies the document bytes (the live app reads
    /// them from disk/builtins; the host KAT passes in-memory strings), so the whole
    /// path is host-testable. For a network URL use [`Tab::navigate_fetch`].
    pub fn navigate(&mut self, url: &str, html: &str, css: &str) {
        let page = self.model.render_page(url, html, css);
        self.model.navigate(url);
        self.adopt(page);
    }

    /// Fetch `url` over `transport` and make the result this tab's shown page, recording
    /// the navigation. Mirrors [`Tab::navigate`] but sources the document from the
    /// network ([`BrowserModel::fetch_and_render`], which always yields a renderable
    /// page — an honest error page on failure, never a panic). The host KAT drives this
    /// with a [`raenet::http1::MockTransport`]/`RoutingMock`.
    pub fn navigate_fetch<T: raenet::http1::HttpTransport>(
        &mut self,
        url: &str,
        transport: &mut T,
    ) -> &LoadedPage {
        let page = self.model.fetch_and_render(url, transport);
        self.model.navigate(url);
        self.adopt(page);
        // `adopt` set `self.page`; unwrap is safe.
        self.page.as_ref().unwrap()
    }

    /// Go back one entry in THIS tab's history, re-rendering the previous page from the
    /// caller-supplied document bytes (the caller re-resolves the URL the same way it
    /// did on the forward navigation). Returns the URL navigated to, or `None` if there
    /// is no back entry. The caller supplies `html`/`css` for that URL.
    ///
    /// Two-step shape (peek the URL, then commit the render) keeps the document-sourcing
    /// outside the model so the model stays syscall-free. See [`Tab::back_url`] +
    /// [`Tab::show`] for the lower-level split the live app uses.
    pub fn back(&mut self, resolve: impl FnOnce(&str) -> (String, String)) -> Option<String> {
        let url = self.model.back()?.to_string();
        let (html, css) = resolve(&url);
        let page = self.model.render_page(&url, &html, &css);
        self.adopt(page);
        Some(url)
    }

    /// Go forward one entry in THIS tab's history. Symmetric to [`Tab::back`].
    pub fn forward(&mut self, resolve: impl FnOnce(&str) -> (String, String)) -> Option<String> {
        let url = self.model.forward()?.to_string();
        let (html, css) = resolve(&url);
        let page = self.model.render_page(&url, &html, &css);
        self.adopt(page);
        Some(url)
    }

    /// Reload the current page in THIS tab from freshly-supplied document bytes (the
    /// history cursor does not move). Returns the reloaded URL, or `None` if the tab is
    /// empty.
    pub fn reload(&mut self, resolve: impl FnOnce(&str) -> (String, String)) -> Option<String> {
        let url = self.model.current_url()?.to_string();
        let (html, css) = resolve(&url);
        let page = self.model.render_page(&url, &html, &css);
        self.adopt(page);
        Some(url)
    }

    /// Replace this tab's shown page with `page` and refresh the cached title + scroll.
    fn adopt(&mut self, page: LoadedPage) {
        self.title = derive_title(&page);
        self.scroll_y = 0.0;
        self.page = Some(page);
    }

    /// Lower-level: render `url`/`html`/`css` and show it WITHOUT touching history (used
    /// by the live app's back/forward/reload, which manage the cursor on the model
    /// directly). Exposed for the live ELF; the host KAT uses the higher-level methods.
    pub fn show(&mut self, url: &str, html: &str, css: &str) {
        let page = self.model.render_page(url, html, css);
        self.adopt(page);
    }
}

/// Derive a tab's title from its loaded page: the document `<title>` (parsed via
/// raeweb's public DOM), falling back to the URL's host, falling back to the URL
/// itself, falling back to [`EMPTY_TAB_TITLE`]. Never empty.
fn derive_title(page: &LoadedPage) -> String {
    let dom = raeweb::parse_html(&page.html);
    if let Some(t) = raeweb::document_title(&dom) {
        let t = t.trim();
        if !t.is_empty() {
            return String::from(t);
        }
    }
    if let Some(host) = host_of(&page.url) {
        if !host.is_empty() {
            return host;
        }
    }
    if !page.url.is_empty() {
        return page.url.clone();
    }
    String::from(EMPTY_TAB_TITLE)
}

/// Extract the host portion of a URL for the title fallback: the authority after
/// `scheme://`, up to the next `/`, `?`, or `#`. For an `about:` URL returns the part
/// after the colon (`about:home` → `home`). For a bare local path returns `None`
/// (the caller falls back to the full URL). Never panics.
fn host_of(url: &str) -> Option<String> {
    let url = url.trim();
    if let Some(rest) = url.split_once("://") {
        let authority = rest.1;
        let end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
        let host = &authority[..end];
        // Strip userinfo and port for a clean display host.
        let host = host.rsplit('@').next().unwrap_or(host);
        let host = host.split(':').next().unwrap_or(host);
        if host.is_empty() {
            return None;
        }
        return Some(String::from(host));
    }
    if let Some(rest) = url.strip_prefix("about:") {
        if !rest.is_empty() {
            return Some(String::from(rest));
        }
    }
    None
}

/// Owns the open tabs and which one is active. Always holds at least one tab (the
/// invariant the tab strip + the live web view rely on). The active tab is the one
/// whose page the chrome shows and whose navigation the toolbar drives.
pub struct TabManager {
    tabs: Vec<Tab>,
    active: usize,
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TabManager {
    /// A new manager with exactly one empty tab, which is active.
    pub fn new() -> TabManager {
        TabManager {
            tabs: alloc::vec![Tab::new()],
            active: 0,
        }
    }

    /// The number of open tabs (always ≥ 1).
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// The index of the active tab.
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The active tab (read-only).
    pub fn active_tab(&self) -> &Tab {
        // Invariant: `active` is always a valid index and `tabs` is never empty.
        &self.tabs[self.active]
    }

    /// The active tab (mutable) — the target of navigate/back/forward/reload.
    pub fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// A tab by index, or `None` if out of range. Lets the chrome render the strip.
    pub fn tab(&self, index: usize) -> Option<&Tab> {
        self.tabs.get(index)
    }

    /// A mutable tab by index, or `None` if out of range.
    pub fn tab_mut(&mut self, index: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(index)
    }

    /// Every tab's title, in tab order — what a tab strip renders.
    pub fn titles(&self) -> Vec<String> {
        self.tabs.iter().map(|t| String::from(t.title())).collect()
    }

    /// Open a new, empty tab and make it active. Returns its index, or `None` if the
    /// [`MAX_TABS`] cap is reached (the caller can surface "too many tabs"; nothing is
    /// opened and the active tab is unchanged).
    pub fn new_tab(&mut self) -> Option<usize> {
        if self.tabs.len() >= MAX_TABS {
            return None;
        }
        self.tabs.push(Tab::new());
        self.active = self.tabs.len() - 1;
        Some(self.active)
    }

    /// Open a new tab already navigated to `url` (rendering `html`+`css`) and make it
    /// active. Returns its index, or `None` if at the cap. Convenience over
    /// `new_tab()` + `active_tab_mut().navigate(...)`.
    pub fn new_tab_with(&mut self, url: &str, html: &str, css: &str) -> Option<usize> {
        let idx = self.new_tab()?;
        self.tabs[idx].navigate(url, html, css);
        Some(idx)
    }

    /// Switch the active tab to `index`. No-op (returns `false`) if `index` is out of
    /// range; returns `true` on a successful switch.
    pub fn switch_to(&mut self, index: usize) -> bool {
        if index < self.tabs.len() {
            self.active = index;
            true
        } else {
            false
        }
    }

    /// Close the tab at `index`. The active tab is chosen sensibly afterward:
    ///   * Closing a tab BEFORE the active one shifts the active index down by one so the
    ///     same tab stays active.
    ///   * Closing the active tab activates its right neighbor if one exists, else its
    ///     left neighbor (i.e. the new tab at the same index, clamped).
    ///   * Closing the LAST remaining tab does NOT empty the manager: it is replaced with
    ///     a fresh empty tab, so there is always ≥ 1 tab and a valid active index.
    ///
    /// Returns `true` if a tab was closed (always, for an in-range index), `false` if
    /// `index` was out of range.
    pub fn close_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() {
            return false;
        }

        // Closing the last remaining tab: never go to zero. Replace it with a fresh one.
        if self.tabs.len() == 1 {
            self.tabs[0] = Tab::new();
            self.active = 0;
            return true;
        }

        self.tabs.remove(index);

        // Recompute the active index so it still points at a sensible tab.
        if index < self.active {
            // A tab to the left went away — the active tab shifted down one slot.
            self.active -= 1;
        } else if index == self.active {
            // The active tab went away. Activate the tab now sitting at `index` (its old
            // right neighbor), clamped to the new last index (so closing the rightmost
            // active tab activates the new rightmost tab).
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
            // else: `self.active` already indexes the former right neighbor — keep it.
        }
        // index > self.active: tabs to the right shifted, active index unchanged.

        true
    }

    /// Close the active tab (see [`close_tab`](TabManager::close_tab)).
    pub fn close_active_tab(&mut self) -> bool {
        self.close_tab(self.active)
    }
}

// ===========================================================================
// Host KAT — links the LIVE raeweb + rae_js engines through BrowserModel, no
// kernel. Each test asserts CONCRETE values so it can actually FAIL.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract_styles;

    /// A page with a known `<title>`, a heading, and an inline script. Parameterized by
    /// a marker so two tabs can load distinguishable documents.
    fn page_doc(title: &str, marker: &str) -> String {
        let mut s = String::from("<!DOCTYPE html><html><head><title>");
        s.push_str(title);
        s.push_str("</title></head><body><h1 id=\"hd\">");
        s.push_str(marker);
        s.push_str("</h1></body></html>");
        s
    }

    /// Navigate the active tab to an in-memory document (sources its own CSS).
    fn nav_active(tm: &mut TabManager, url: &str, html: &str) {
        let css = extract_styles(html);
        tm.active_tab_mut().navigate(url, html, &css);
    }

    /// A trivial resolver for back/forward tests: maps a URL to a fresh page document so
    /// re-rendering on history navigation produces a deterministic, identifiable page.
    fn resolver_for(url: &str) -> (String, String) {
        // Title = the URL's last path segment; marker = "page:<url>".
        let title = url.rsplit('/').next().unwrap_or(url);
        let html = page_doc(title, url);
        let css = extract_styles(&html);
        (html, css)
    }

    #[test]
    fn starts_with_one_active_empty_tab() {
        let tm = TabManager::new();
        assert_eq!(tm.tab_count(), 1, "a new manager must have exactly one tab");
        assert_eq!(tm.active_index(), 0);
        assert!(
            tm.active_tab().page().is_none(),
            "the initial tab must be empty (no page loaded)"
        );
        assert_eq!(
            tm.active_tab().title(),
            EMPTY_TAB_TITLE,
            "an empty tab shows the placeholder title"
        );
    }

    /// CROSS-TAB ISOLATION (the headline): two tabs navigated to different pages keep
    /// INDEPENDENT current pages, histories, and titles. Navigating tab A must not change
    /// tab B at all.
    #[test]
    fn two_tabs_have_independent_navigation_state() {
        let mut tm = TabManager::new();

        // Tab 0 (the initial tab) → page A.
        nav_active(&mut tm, "about:a", &page_doc("Alpha", "AAA"));
        assert_eq!(tm.active_index(), 0);

        // Open tab 1 → page B.
        let b = tm.new_tab().expect("under the cap, a new tab opens");
        assert_eq!(b, 1);
        assert_eq!(tm.active_index(), 1, "the new tab becomes active");
        nav_active(&mut tm, "about:b", &page_doc("Beta", "BBB"));

        // Each tab's CURRENT page is its own.
        let a_page = tm.tab(0).unwrap().page().expect("tab 0 has a page");
        let b_page = tm.tab(1).unwrap().page().expect("tab 1 has a page");
        assert_eq!(a_page.url, "about:a");
        assert_eq!(b_page.url, "about:b");
        assert_eq!(
            a_page.first_text_of("h1").as_deref(),
            Some("AAA"),
            "tab 0 must still render its own heading"
        );
        assert_eq!(
            b_page.first_text_of("h1").as_deref(),
            Some("BBB"),
            "tab 1 renders its own heading"
        );

        // Titles are per-tab (from each page's own <title>).
        assert_eq!(tm.tab(0).unwrap().title(), "Alpha");
        assert_eq!(tm.tab(1).unwrap().title(), "Beta");

        // Now navigate tab 1 AGAIN; tab 0 must be untouched.
        nav_active(&mut tm, "about:b2", &page_doc("Beta2", "BBB2"));
        assert_eq!(
            tm.tab(0).unwrap().page().unwrap().url,
            "about:a",
            "navigating tab 1 must NOT change tab 0's current page"
        );
        assert_eq!(
            tm.tab(0).unwrap().title(),
            "Alpha",
            "navigating tab 1 must NOT change tab 0's title"
        );
        assert_eq!(
            tm.tab(0).unwrap().history_len(),
            1,
            "tab 0's history must be untouched by tab 1's navigation"
        );
        assert_eq!(
            tm.tab(1).unwrap().history_len(),
            2,
            "tab 1 now has two history entries"
        );
    }

    /// CROSS-TAB ISOLATION of back/forward: going back in tab A must not move tab B's
    /// history cursor or change tab B's page.
    #[test]
    fn back_forward_is_per_tab() {
        let mut tm = TabManager::new();

        // Tab 0: visit two pages so it has a back entry.
        nav_active(&mut tm, "about:a1", &page_doc("A1", "a1"));
        nav_active(&mut tm, "about:a2", &page_doc("A2", "a2"));

        // Tab 1: visit two different pages.
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:b1", &page_doc("B1", "b1"));
        nav_active(&mut tm, "about:b2", &page_doc("B2", "b2"));

        // Both tabs can go back; neither can go forward (at the newest entry).
        assert!(tm.tab(0).unwrap().can_go_back());
        assert!(tm.tab(1).unwrap().can_go_back());
        assert!(!tm.tab(1).unwrap().can_go_forward());

        // Go back in the ACTIVE tab (tab 1) only.
        let url = tm
            .active_tab_mut()
            .back(|u| resolver_for(u))
            .expect("tab 1 can go back");
        assert_eq!(url, "about:b1", "tab 1 went back to its own previous page");
        assert_eq!(tm.tab(1).unwrap().current_url(), Some("about:b1"));
        assert!(
            tm.tab(1).unwrap().can_go_forward(),
            "tab 1 now has a forward entry"
        );

        // Tab 0 is COMPLETELY unaffected by tab 1's back navigation.
        assert_eq!(
            tm.tab(0).unwrap().current_url(),
            Some("about:a2"),
            "tab 0's cursor must NOT move when tab 1 goes back"
        );
        assert!(
            !tm.tab(0).unwrap().can_go_forward(),
            "tab 0 must still be at its newest entry"
        );
        assert_eq!(
            tm.tab(0).unwrap().page().unwrap().url,
            "about:a2",
            "tab 0's shown page must be unchanged by tab 1's back"
        );
    }

    #[test]
    fn switch_to_changes_active_tab() {
        let mut tm = TabManager::new();
        nav_active(&mut tm, "about:a", &page_doc("Alpha", "AAA"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:b", &page_doc("Beta", "BBB"));

        assert_eq!(tm.active_index(), 1);
        assert_eq!(tm.active_tab().page().unwrap().url, "about:b");

        assert!(tm.switch_to(0), "switching to a valid index succeeds");
        assert_eq!(tm.active_index(), 0);
        assert_eq!(
            tm.active_tab().page().unwrap().url,
            "about:a",
            "active_tab() must reflect the switch"
        );
        assert_eq!(tm.active_tab().title(), "Alpha");

        // Out-of-range switch is a no-op that returns false.
        assert!(!tm.switch_to(7), "switching to an invalid index fails");
        assert_eq!(
            tm.active_index(),
            0,
            "active index unchanged on failed switch"
        );
    }

    /// Closing the active tab activates a neighbor and decrements the count.
    #[test]
    fn close_active_tab_activates_neighbor_and_decrements() {
        let mut tm = TabManager::new();
        nav_active(&mut tm, "about:a", &page_doc("Alpha", "AAA"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:b", &page_doc("Beta", "BBB"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:c", &page_doc("Gamma", "CCC"));
        assert_eq!(tm.tab_count(), 3);
        assert_eq!(tm.active_index(), 2);

        // Switch to the MIDDLE tab and close it; the right neighbor (old index 2) becomes
        // active, now sitting at index 1.
        tm.switch_to(1);
        assert!(tm.close_active_tab());
        assert_eq!(tm.tab_count(), 2, "count decremented");
        assert_eq!(tm.active_index(), 1, "the right neighbor is now active");
        assert_eq!(
            tm.active_tab().page().unwrap().url,
            "about:c",
            "the activated neighbor is the former tab to the right (Gamma)"
        );

        // Close the active (rightmost) tab; the left neighbor becomes active.
        assert!(tm.close_active_tab());
        assert_eq!(tm.tab_count(), 1);
        assert_eq!(tm.active_index(), 0);
        assert_eq!(
            tm.active_tab().page().unwrap().url,
            "about:a",
            "closing the rightmost active tab activates the left neighbor (Alpha)"
        );
    }

    /// Closing a tab BEFORE the active one keeps the same tab active (its index shifts).
    #[test]
    fn close_left_of_active_keeps_active_tab() {
        let mut tm = TabManager::new();
        nav_active(&mut tm, "about:a", &page_doc("Alpha", "AAA"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:b", &page_doc("Beta", "BBB"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:c", &page_doc("Gamma", "CCC"));
        // Active is tab 2 (Gamma). Close tab 0 (Alpha).
        assert_eq!(tm.active_index(), 2);
        assert!(tm.close_tab(0));
        assert_eq!(tm.tab_count(), 2);
        assert_eq!(
            tm.active_index(),
            1,
            "active index shifted down so the same tab (Gamma) stays active"
        );
        assert_eq!(tm.active_tab().page().unwrap().url, "about:c");
    }

    /// THE never-zero invariant: closing down to one tab, then closing again, leaves
    /// exactly one (fresh, empty) tab — never zero.
    #[test]
    fn closing_last_tab_never_yields_zero() {
        let mut tm = TabManager::new();
        nav_active(&mut tm, "about:a", &page_doc("Alpha", "AAA"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:b", &page_doc("Beta", "BBB"));

        // Close down to one tab.
        assert!(tm.close_active_tab());
        assert_eq!(tm.tab_count(), 1);
        assert!(
            tm.active_tab().page().is_some(),
            "the surviving tab still has its page"
        );

        // Close the LAST tab: it must be replaced with a fresh empty tab, NOT removed.
        assert!(tm.close_active_tab());
        assert_eq!(
            tm.tab_count(),
            1,
            "closing the last tab must leave exactly one tab, never zero"
        );
        assert_eq!(tm.active_index(), 0, "the fresh tab is active");
        assert!(
            tm.active_tab().page().is_none(),
            "the replacement tab is fresh and empty"
        );
        assert_eq!(tm.active_tab().title(), EMPTY_TAB_TITLE);
    }

    /// Each tab's title reflects its OWN page's `<title>`.
    #[test]
    fn title_per_tab_reflects_own_page_title() {
        let mut tm = TabManager::new();
        nav_active(&mut tm, "about:a", &page_doc("First Page Title", "x"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:b", &page_doc("Second Page Title", "y"));

        let titles = tm.titles();
        assert_eq!(
            titles,
            alloc::vec![
                String::from("First Page Title"),
                String::from("Second Page Title")
            ],
            "the tab strip must show each tab's own <title>"
        );
    }

    /// A page with NO `<title>` falls back to the URL's host for the tab title.
    #[test]
    fn title_falls_back_to_host_when_no_title_element() {
        let mut tm = TabManager::new();
        let html = "<!DOCTYPE html><html><body><p>no title element here</p></body></html>";
        let css = extract_styles(html);
        tm.active_tab_mut()
            .navigate("http://news.example.com/story", html, &css);
        assert_eq!(
            tm.active_tab().title(),
            "news.example.com",
            "with no <title>, the tab title falls back to the URL host"
        );
    }

    /// FAIL-ABILITY: switching to tab 1 must show tab 1's page, NOT tab 0's. If
    /// switch_to/active_tab were broken (always returned tab 0), this flips.
    #[test]
    fn fail_ability_active_tab_is_the_switched_one() {
        let mut tm = TabManager::new();
        nav_active(&mut tm, "about:a", &page_doc("Alpha", "AAA"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:b", &page_doc("Beta", "BBB"));
        tm.switch_to(1);
        // If active tracking were broken and returned tab 0, this would read "AAA".
        assert_eq!(
            tm.active_tab()
                .page()
                .unwrap()
                .first_text_of("h1")
                .as_deref(),
            Some("BBB"),
            "active_tab must be the switched-to tab, not a fixed/first tab"
        );
    }

    /// The tab count is bounded: new_tab refuses past MAX_TABS and reports it.
    #[test]
    fn new_tab_is_bounded_by_max_tabs() {
        let mut tm = TabManager::new();
        // One tab already exists; open up to the cap.
        while tm.tab_count() < MAX_TABS {
            assert!(
                tm.new_tab().is_some(),
                "opening tabs up to the cap must succeed"
            );
        }
        assert_eq!(tm.tab_count(), MAX_TABS);
        assert!(
            tm.new_tab().is_none(),
            "new_tab past MAX_TABS must refuse (return None)"
        );
        assert_eq!(
            tm.tab_count(),
            MAX_TABS,
            "a refused new_tab must not change the count"
        );
    }

    /// Tabs can be opened pre-navigated, and that page is independent too.
    #[test]
    fn new_tab_with_navigates_the_new_tab() {
        let mut tm = TabManager::new();
        nav_active(&mut tm, "about:a", &page_doc("Alpha", "AAA"));
        let html = page_doc("Created", "ZZZ");
        let css = extract_styles(&html);
        let idx = tm
            .new_tab_with("about:created", &html, &css)
            .expect("under the cap");
        assert_eq!(idx, 1);
        assert_eq!(tm.active_index(), 1);
        assert_eq!(tm.active_tab().title(), "Created");
        assert_eq!(
            tm.active_tab()
                .page()
                .unwrap()
                .first_text_of("h1")
                .as_deref(),
            Some("ZZZ")
        );
        // Tab 0 untouched.
        assert_eq!(tm.tab(0).unwrap().title(), "Alpha");
    }

    /// Reload re-renders the current page from fresh bytes without moving the cursor,
    /// and only affects the active tab.
    #[test]
    fn reload_affects_only_active_tab() {
        let mut tm = TabManager::new();
        nav_active(&mut tm, "about:a", &page_doc("Alpha", "AAA"));
        tm.new_tab().unwrap();
        nav_active(&mut tm, "about:b", &page_doc("Beta", "BBB"));

        // Reload tab 1 with mutated bytes (title changes); cursor stays put.
        let url = tm
            .active_tab_mut()
            .reload(|u| {
                let html = page_doc("Beta Reloaded", "BBB-v2");
                let css = extract_styles(&html);
                let _ = u;
                (html, css)
            })
            .expect("reload returns the current url");
        assert_eq!(url, "about:b");
        assert_eq!(tm.active_tab().title(), "Beta Reloaded");
        assert_eq!(
            tm.active_tab().history_len(),
            1,
            "reload must NOT add a history entry"
        );
        // Tab 0 untouched.
        assert_eq!(tm.tab(0).unwrap().title(), "Alpha");
    }
}
