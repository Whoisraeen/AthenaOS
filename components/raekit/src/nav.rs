//! Navigation and routing for AthKit apps.
//!
//! Provides three navigation paradigms:
//! - `NavigationStack` — push/pop like UINavigationController
//! - `TabView` — tabbed interface with named tabs
//! - `Router` — URL-style route-based navigation

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::view::ViewNode;

// ── NavigationStack ──────────────────────────────────────────────────────

/// A push/pop navigation stack. Each entry has a title and a view tree.
pub struct NavigationStack {
    stack: Vec<NavigationEntry>,
}

struct NavigationEntry {
    title: String,
    view: ViewNode,
}

impl NavigationStack {
    pub fn new(title: &str, root: ViewNode) -> Self {
        let mut stack = Vec::new();
        stack.push(NavigationEntry {
            title: String::from(title),
            view: root,
        });
        Self { stack }
    }

    pub fn push(&mut self, title: &str, view: ViewNode) {
        self.stack.push(NavigationEntry {
            title: String::from(title),
            view,
        });
    }

    pub fn pop(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            true
        } else {
            false
        }
    }

    pub fn pop_to_root(&mut self) {
        self.stack.truncate(1);
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    pub fn can_pop(&self) -> bool {
        self.stack.len() > 1
    }

    pub fn current_title(&self) -> &str {
        self.stack.last().map(|e| e.title.as_str()).unwrap_or("")
    }

    pub fn current_view(&self) -> &ViewNode {
        &self
            .stack
            .last()
            .expect("navigation stack is never empty")
            .view
    }

    pub fn replace_current(&mut self, view: ViewNode) {
        if let Some(entry) = self.stack.last_mut() {
            entry.view = view;
        }
    }

    /// Build the entire stack into a single NavigationView node.
    pub fn build(&self) -> ViewNode {
        let entry = self.stack.last().expect("navigation stack is never empty");
        ViewNode::NavigationView {
            title: entry.title.clone(),
            content: alloc::boxed::Box::new(ViewNode::Empty),
        }
    }
}

// ── TabView ──────────────────────────────────────────────────────────────

/// A tabbed container with named tabs. Tracks the active tab index.
pub struct TabViewNav {
    tabs: Vec<TabEntry>,
    active: usize,
}

struct TabEntry {
    label: String,
    icon_id: u32,
    view: ViewNode,
}

impl TabViewNav {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
        }
    }

    pub fn tab(mut self, label: &str, icon_id: u32, view: ViewNode) -> Self {
        self.tabs.push(TabEntry {
            label: String::from(label),
            icon_id,
            view,
        });
        self
    }

    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_label(&self) -> &str {
        self.tabs
            .get(self.active)
            .map(|t| t.label.as_str())
            .unwrap_or("")
    }

    pub fn active_view(&self) -> Option<&ViewNode> {
        self.tabs.get(self.active).map(|t| &t.view)
    }

    pub fn replace_tab_view(&mut self, index: usize, view: ViewNode) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.view = view;
        }
    }

    /// Build the tab container into a ViewNode tree.
    pub fn build(&self) -> ViewNode {
        let mut children = Vec::with_capacity(self.tabs.len());
        for tab in &self.tabs {
            children.push(ViewNode::TabItem {
                label: tab.label.clone(),
                icon_id: tab.icon_id,
                content: alloc::boxed::Box::new(ViewNode::Empty),
            });
        }
        ViewNode::Group { children }
    }
}

// ── Router ───────────────────────────────────────────────────────────────

/// URL-style route-based navigation. Routes are simple string paths like
/// `/settings/display`. The router maintains a history stack.
pub struct Router {
    routes: Vec<Route>,
    history: Vec<String>,
    current: usize,
}

struct Route {
    path: String,
    builder_id: u32,
}

impl Router {
    pub fn new() -> Self {
        let mut history = Vec::new();
        history.push(String::from("/"));
        Self {
            routes: Vec::new(),
            history,
            current: 0,
        }
    }

    pub fn route(mut self, path: &str, builder_id: u32) -> Self {
        self.routes.push(Route {
            path: String::from(path),
            builder_id,
        });
        self
    }

    pub fn push(&mut self, path: &str) {
        self.history.truncate(self.current + 1);
        self.history.push(String::from(path));
        self.current = self.history.len() - 1;
    }

    pub fn pop(&mut self) -> bool {
        if self.current > 0 {
            self.current -= 1;
            true
        } else {
            false
        }
    }

    pub fn forward(&mut self) -> bool {
        if self.current + 1 < self.history.len() {
            self.current += 1;
            true
        } else {
            false
        }
    }

    pub fn replace(&mut self, path: &str) {
        if let Some(entry) = self.history.get_mut(self.current) {
            *entry = String::from(path);
        }
    }

    pub fn current_path(&self) -> &str {
        self.history
            .get(self.current)
            .map(|s| s.as_str())
            .unwrap_or("/")
    }

    pub fn can_go_back(&self) -> bool {
        self.current > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.current + 1 < self.history.len()
    }

    pub fn history_depth(&self) -> usize {
        self.history.len()
    }

    /// Find the builder_id for the current route path. Supports exact
    /// match and single-segment wildcard (`*`).
    pub fn resolve(&self) -> Option<u32> {
        let current = self.current_path();
        for route in &self.routes {
            if route_matches(&route.path, current) {
                return Some(route.builder_id);
            }
        }
        None
    }
}

fn route_matches(pattern: &str, path: &str) -> bool {
    if pattern == path {
        return true;
    }

    let pat_segs: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if pat_segs.len() != path_segs.len() {
        return false;
    }

    for (p, s) in pat_segs.iter().zip(path_segs.iter()) {
        if *p != "*" && p != s {
            return false;
        }
    }
    true
}
