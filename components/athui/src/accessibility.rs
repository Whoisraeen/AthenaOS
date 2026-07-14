//! AthUI Accessibility Tree — auto-generated from the widget tree.
//!
//! Provides semantic accessibility nodes, focus management, screen reader
//! output, high-contrast mode, and reduced-motion support. The tree mirrors
//! the widget tree and is rebuilt each frame (or on structural change).

extern crate alloc;

use crate::layout::ComputedLayout;
use crate::tree::WidgetNode;
use alloc::string::String;
use alloc::vec::Vec;

// ── Accessibility Roles ─────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessibilityRole {
    Button,
    Label,
    TextField,
    Slider,
    Checkbox,
    Toggle,
    Image,
    Link,
    Heading,
    List,
    ListItem,
    Tab,
    TabBar,
    ScrollView,
    Dialog,
    Alert,
    Menu,
    MenuItem,
    ProgressBar,
    Switch,
    Toolbar,
    Window,
    Group,
    None,
}

// ── Accessibility Traits ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct AccessibilityTraits {
    pub focusable: bool,
    pub selected: bool,
    pub disabled: bool,
    pub expanded: bool,
    pub modal: bool,
    pub live_region: bool,
    pub hidden: bool,
}

// ── Accessibility Actions ───────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessibilityAction {
    Press,
    Increment,
    Decrement,
    SetValue,
    Focus,
    Scroll,
    Dismiss,
    Activate,
}

// ── Rect for bounds ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl From<ComputedLayout> for Rect {
    fn from(c: ComputedLayout) -> Self {
        Self {
            x: c.x,
            y: c.y,
            width: c.width,
            height: c.height,
        }
    }
}

// ── Accessibility Node ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AccessibilityNode {
    pub id: u32,
    pub role: AccessibilityRole,
    pub label: String,
    pub value: Option<String>,
    pub hint: Option<String>,
    pub traits: AccessibilityTraits,
    pub bounds: Rect,
    pub children: Vec<u32>,
    pub parent: Option<u32>,
    pub actions: Vec<AccessibilityAction>,
}

impl AccessibilityNode {
    pub fn new(id: u32, role: AccessibilityRole, label: String, bounds: Rect) -> Self {
        let traits = AccessibilityTraits {
            focusable: matches!(
                role,
                AccessibilityRole::Button
                    | AccessibilityRole::TextField
                    | AccessibilityRole::Slider
                    | AccessibilityRole::Checkbox
                    | AccessibilityRole::Toggle
                    | AccessibilityRole::Link
                    | AccessibilityRole::Tab
                    | AccessibilityRole::MenuItem
                    | AccessibilityRole::Switch
            ),
            ..Default::default()
        };
        let actions = match role {
            AccessibilityRole::Button | AccessibilityRole::Link | AccessibilityRole::MenuItem => {
                alloc::vec![AccessibilityAction::Press]
            }
            AccessibilityRole::Slider => {
                alloc::vec![
                    AccessibilityAction::Increment,
                    AccessibilityAction::Decrement,
                    AccessibilityAction::SetValue
                ]
            }
            AccessibilityRole::Checkbox | AccessibilityRole::Toggle | AccessibilityRole::Switch => {
                alloc::vec![AccessibilityAction::Press]
            }
            AccessibilityRole::TextField => {
                alloc::vec![AccessibilityAction::Focus, AccessibilityAction::SetValue]
            }
            AccessibilityRole::ScrollView => {
                alloc::vec![AccessibilityAction::Scroll]
            }
            AccessibilityRole::Dialog | AccessibilityRole::Alert => {
                alloc::vec![AccessibilityAction::Dismiss]
            }
            _ => Vec::new(),
        };
        Self {
            id,
            role,
            label,
            value: None,
            hint: None,
            traits,
            bounds,
            children: Vec::new(),
            parent: None,
            actions,
        }
    }
}

// ── Widget Accessibility Info (trait for widgets to implement) ───────────

/// Widgets implement this to provide accessibility metadata.
/// Default implementations infer role/label from widget type.
pub trait Accessible {
    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
    fn accessibility_label(&self) -> String {
        String::new()
    }
    fn accessibility_value(&self) -> Option<String> {
        None
    }
    fn accessibility_hint(&self) -> Option<String> {
        None
    }
    fn is_accessibility_hidden(&self) -> bool {
        false
    }
}

// ── Accessibility Tree ──────────────────────────────────────────────────

pub struct AccessibilityTree {
    pub nodes: Vec<AccessibilityNode>,
    pub focus_index: Option<usize>,
    pub high_contrast: bool,
    pub reduced_motion: bool,
}

impl AccessibilityTree {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            focus_index: None,
            high_contrast: false,
            reduced_motion: false,
        }
    }

    /// Rebuild the accessibility tree from the widget tree.
    /// Auto-generates nodes based on widget types and positions.
    pub fn build_from_widget_tree(&mut self, root: &WidgetNode) {
        self.nodes.clear();
        self.build_node(root, None, 0.0, 0.0);
        // Preserve focus if the focused node still exists
        if let Some(idx) = self.focus_index {
            if idx >= self.focusable_count() {
                self.focus_index = if self.focusable_count() > 0 {
                    Some(0)
                } else {
                    None
                };
            }
        }
    }

    fn build_node(
        &mut self,
        node: &WidgetNode,
        parent_id: Option<u32>,
        parent_x: f32,
        parent_y: f32,
    ) {
        let abs_x = parent_x + node.computed.x;
        let abs_y = parent_y + node.computed.y;
        let bounds = Rect {
            x: abs_x,
            y: abs_y,
            width: node.computed.width,
            height: node.computed.height,
        };

        let role = infer_role_from_widget_id(node.id);
        let label = infer_label(node);

        let mut a11y_node = AccessibilityNode::new(node.id, role, label, bounds);
        a11y_node.parent = parent_id;

        for child in &node.children {
            a11y_node.children.push(child.id);
        }

        self.nodes.push(a11y_node);

        for child in &node.children {
            self.build_node(child, Some(node.id), abs_x, abs_y);
        }
    }

    /// Get all focusable nodes in tree order.
    pub fn focusable_nodes(&self) -> Vec<&AccessibilityNode> {
        self.nodes
            .iter()
            .filter(|n| n.traits.focusable && !n.traits.hidden && !n.traits.disabled)
            .collect()
    }

    pub fn focusable_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.traits.focusable && !n.traits.hidden && !n.traits.disabled)
            .count()
    }

    /// Move focus to next focusable element (Tab key).
    pub fn focus_next(&mut self) {
        let count = self.focusable_count();
        if count == 0 {
            self.focus_index = None;
            return;
        }
        self.focus_index = Some(match self.focus_index {
            Some(i) => (i + 1) % count,
            None => 0,
        });
    }

    /// Move focus to previous focusable element (Shift+Tab).
    pub fn focus_previous(&mut self) {
        let count = self.focusable_count();
        if count == 0 {
            self.focus_index = None;
            return;
        }
        self.focus_index = Some(match self.focus_index {
            Some(0) => count - 1,
            Some(i) => i - 1,
            None => count - 1,
        });
    }

    /// Focus a specific node by widget ID.
    pub fn focus_node(&mut self, widget_id: u32) {
        let focusable: Vec<u32> = self.focusable_nodes().iter().map(|n| n.id).collect();
        if let Some(pos) = focusable.iter().position(|&id| id == widget_id) {
            self.focus_index = Some(pos);
        }
    }

    /// Get the currently focused node.
    pub fn focused_node(&self) -> Option<&AccessibilityNode> {
        let focusable = self.focusable_nodes();
        self.focus_index.and_then(|i| focusable.get(i).copied())
    }

    /// Get the widget ID of the currently focused element.
    pub fn focused_widget_id(&self) -> Option<u32> {
        self.focused_node().map(|n| n.id)
    }

    /// Generate a text description of the currently focused element for screen readers.
    pub fn describe_focused(&self) -> String {
        match self.focused_node() {
            Some(node) => {
                let role_str = role_to_string(node.role);
                let mut desc = String::new();
                desc.push_str(&node.label);
                if !node.label.is_empty() {
                    desc.push_str(", ");
                }
                desc.push_str(role_str);
                if let Some(ref val) = node.value {
                    desc.push_str(", value: ");
                    desc.push_str(val);
                }
                if node.traits.disabled {
                    desc.push_str(", disabled");
                }
                if node.traits.selected {
                    desc.push_str(", selected");
                }
                if node.traits.expanded {
                    desc.push_str(", expanded");
                }
                if let Some(ref hint) = node.hint {
                    desc.push_str(". ");
                    desc.push_str(hint);
                }
                desc
            }
            None => String::from("No element focused"),
        }
    }

    /// Get node by widget ID.
    pub fn get_node(&self, widget_id: u32) -> Option<&AccessibilityNode> {
        self.nodes.iter().find(|n| n.id == widget_id)
    }

    /// Get mutable node by widget ID.
    pub fn get_node_mut(&mut self, widget_id: u32) -> Option<&mut AccessibilityNode> {
        self.nodes.iter_mut().find(|n| n.id == widget_id)
    }

    /// Override colors for high contrast mode.
    pub fn high_contrast_colors(&self) -> Option<HighContrastPalette> {
        if self.high_contrast {
            Some(HighContrastPalette {
                background: 0xFF_00_00_00,
                foreground: 0xFF_FF_FF_FF,
                accent: 0xFF_FF_FF_00,
                focus_ring: 0xFF_00_FF_FF,
                error: 0xFF_FF_00_00,
                link: 0xFF_00_FF_00,
            })
        } else {
            None
        }
    }
}

// ── High Contrast Palette ───────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct HighContrastPalette {
    pub background: u32,
    pub foreground: u32,
    pub accent: u32,
    pub focus_ring: u32,
    pub error: u32,
    pub link: u32,
}

// ── Focus Ring Rendering Helper ─────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct FocusRing {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub thickness: f32,
    pub color: u32,
}

impl FocusRing {
    pub fn from_node(node: &AccessibilityNode, color: u32) -> Self {
        Self {
            x: node.bounds.x - 2.0,
            y: node.bounds.y - 2.0,
            width: node.bounds.width + 4.0,
            height: node.bounds.height + 4.0,
            thickness: 2.0,
            color,
        }
    }
}

/// The shared focus-ring stroke width (px) — matches the gameos couch ring so the
/// desktop and couch focus cues read identically.
pub const FOCUS_RING_W: usize = 3;

/// Draw the unified, reusable VISIBLE focus ring around a screen rect onto a
/// athgfx canvas. This is the single focus-ring renderer every shell chrome
/// surface (taskbar Start, taskbar items, tray, Control Center, dialogs) calls,
/// so the focus cue is consistent everywhere — the audit's "every chrome element
/// shows a focus ring (ring shape, never color-only)" requirement.
///
/// `normal_ring` is the surface's normal-mode ring color; this routes it through
/// [`ath_tokens::active_focus_ring`] so when high-contrast forced-colors mode is
/// on the ring becomes the HC cyan automatically — the surface never has to know
/// about HC. The ring is drawn as FOUR redundant signals (never color alone, the
/// a11y rule): an outer glow halo, a `FOCUS_RING_W`-thick accent ring, and a
/// bright inset top-edge highlight that survives a color-blind / low-contrast
/// viewer.
pub fn draw_focus_ring(
    canvas: &mut athgfx::Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    radius: usize,
    normal_ring: u32,
) {
    if w < 4 || h < 4 {
        return;
    }
    // HC-aware ring color: cyan under forced-colors, else the caller's accent.
    let ring = ath_tokens::active_focus_ring(normal_ring);
    // 1. Glow halo: a soft accent-tinted wash one ring-width OUTSIDE the rect, so
    //    the element reads as LIT (focus is glow, not a 1px border).
    let glow = (0x40 << 24) | (ring & 0x00FF_FFFF);
    let gw = FOCUS_RING_W * 2;
    canvas.draw_rounded_rect_outline(
        x.saturating_sub(gw),
        y.saturating_sub(gw),
        w + gw * 2,
        h + gw * 2,
        radius + gw,
        glow,
    );
    // 2. The thick accent ring.
    for r in 0..FOCUS_RING_W {
        canvas.draw_rounded_rect_outline(
            x + r,
            y + r,
            w.saturating_sub(r * 2),
            h.saturating_sub(r * 2),
            radius.saturating_sub(r),
            ring,
        );
    }
    // 3. Non-color cue: a bright inset top-edge highlight (a fourth signal that
    //    survives a color-blind / low-contrast viewer — shape + position, not hue).
    let inset = FOCUS_RING_W;
    let top_w = w.saturating_sub(inset * 2);
    if top_w >= 2 {
        canvas.fill_rect(x + inset, y + inset, top_w, 2, 0xFF_FF_FF_FF);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// The concrete AthUI widget kinds the layout tree (`tree.rs`) actually
/// instantiates, plus the richer kinds from `inputs.rs`/`containers.rs`/
/// `feedback.rs`. This is the bridge between the opaque `Widget2` boxes in the
/// `WidgetNode` tree (which carry no type tag) and a real accessibility role:
/// a widget registers its `(id, WidgetKind, label, value)` here when it is
/// built, and `build_from_widget_tree` reads it back to name the node. This
/// replaces the old "everything is a Group" placeholder — the tree now NAMES
/// things, which is the whole point of an accessibility tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetKind {
    Button,
    Label,
    Heading,
    TextField,
    Checkbox,
    Toggle,
    Switch,
    Slider,
    ProgressBar,
    Image,
    Link,
    List,
    ListItem,
    Tab,
    TabBar,
    Menu,
    MenuItem,
    ScrollView,
    Dialog,
    Alert,
    Toolbar,
    Group,
}

/// Map a concrete AthUI widget kind to its semantic accessibility role. This is
/// the real role inference that the old `infer_role_from_widget_id` stub faked.
pub fn role_for_widget_kind(kind: WidgetKind) -> AccessibilityRole {
    match kind {
        WidgetKind::Button => AccessibilityRole::Button,
        WidgetKind::Label => AccessibilityRole::Label,
        WidgetKind::Heading => AccessibilityRole::Heading,
        WidgetKind::TextField => AccessibilityRole::TextField,
        WidgetKind::Checkbox => AccessibilityRole::Checkbox,
        WidgetKind::Toggle => AccessibilityRole::Toggle,
        WidgetKind::Switch => AccessibilityRole::Switch,
        WidgetKind::Slider => AccessibilityRole::Slider,
        WidgetKind::ProgressBar => AccessibilityRole::ProgressBar,
        WidgetKind::Image => AccessibilityRole::Image,
        WidgetKind::Link => AccessibilityRole::Link,
        WidgetKind::List => AccessibilityRole::List,
        WidgetKind::ListItem => AccessibilityRole::ListItem,
        WidgetKind::Tab => AccessibilityRole::Tab,
        WidgetKind::TabBar => AccessibilityRole::TabBar,
        WidgetKind::Menu => AccessibilityRole::Menu,
        WidgetKind::MenuItem => AccessibilityRole::MenuItem,
        WidgetKind::ScrollView => AccessibilityRole::ScrollView,
        WidgetKind::Dialog => AccessibilityRole::Dialog,
        WidgetKind::Alert => AccessibilityRole::Alert,
        WidgetKind::Toolbar => AccessibilityRole::Toolbar,
        WidgetKind::Group => AccessibilityRole::Group,
    }
}

/// Per-widget semantic metadata AthUI publishes for the accessibility tree. A
/// widget contributes one of these (with its real kind/label/value) so the tree
/// can name it. `build_from_widget_tree` consults the registry keyed by widget
/// id; the kernel widget-provider seam (`a11y::set_widget_provider`) consumes
/// the same data via [`provider_nodes_for_window`].
#[derive(Clone, Debug)]
pub struct WidgetSemantics {
    pub id: u32,
    pub kind: WidgetKind,
    pub label: String,
    pub value: Option<String>,
    pub disabled: bool,
    pub checked: bool,
}

impl WidgetSemantics {
    pub fn new(id: u32, kind: WidgetKind, label: &str) -> Self {
        Self {
            id,
            kind,
            label: String::from(label),
            value: None,
            disabled: false,
            checked: false,
        }
    }

    pub fn with_value(mut self, value: &str) -> Self {
        self.value = Some(String::from(value));
        self
    }
}

// ── Widget semantics registry ───────────────────────────────────────────
// The `Widget2` boxes in the layout tree carry no type tag, so a widget that
// wants to be named registers its semantics here when built. This is a small
// process-local table (AthUI runs per-process in userspace), not a kernel
// shadow tree.

use core::cell::RefCell;

// A minimal single-threaded slot. AthUI's widget tree is single-threaded per
// process; we keep the registry in a plain static RefCell guarded by the
// build/read discipline (never re-entrant).
struct Registry(RefCell<Vec<WidgetSemantics>>);
// SAFETY: AthUI's widget tree is built and queried on one UI thread per
// process. The registry is only touched from `register_widget_semantics` /
// `clear_widget_semantics` / the lookups below, never re-entrantly and never
// across threads. This mirrors the existing single-thread `NEXT_ID` static in
// tree.rs.
unsafe impl Sync for Registry {}

static WIDGET_REGISTRY: Registry = Registry(RefCell::new(Vec::new()));

/// Register (or replace) the semantics for a widget id. Call this when building
/// a widget that should be named in the accessibility tree (Button/TextField/…).
pub fn register_widget_semantics(sem: WidgetSemantics) {
    let mut reg = WIDGET_REGISTRY.0.borrow_mut();
    if let Some(slot) = reg.iter_mut().find(|s| s.id == sem.id) {
        *slot = sem;
    } else {
        reg.push(sem);
    }
}

/// Clear the whole registry (e.g. when tearing a window down).
pub fn clear_widget_semantics() {
    WIDGET_REGISTRY.0.borrow_mut().clear();
}

fn lookup_semantics(id: u32) -> Option<WidgetSemantics> {
    WIDGET_REGISTRY
        .0
        .borrow()
        .iter()
        .find(|s| s.id == id)
        .cloned()
}

/// Real role inference: a registered widget reports its true role; an
/// unregistered structural node (a bare frame/container) is a Group. This is the
/// replacement for the old stub that returned Group unconditionally.
fn infer_role_from_widget_id(id: u32) -> AccessibilityRole {
    match lookup_semantics(id) {
        Some(sem) => role_for_widget_kind(sem.kind),
        None => AccessibilityRole::Group,
    }
}

/// Real label inference: a registered widget reports its label; unregistered
/// structural nodes have no label (correct — a layout frame is not announced).
fn infer_label(node: &WidgetNode) -> String {
    match lookup_semantics(node.id) {
        Some(sem) => sem.label,
        None => String::new(),
    }
}

// ── Kernel widget-provider seam (the userspace half) ────────────────────
// `a11y.rs::set_widget_provider(fn(window_id) -> Vec<AccessNode>)` is the kernel
// seam (foundation §6). AthUI is a separate (userspace) crate from the kernel
// `a11y` module and cannot construct kernel `AccessNode`s directly, so it emits
// a wire-shaped [`ProviderNode`] that maps 1:1 onto `ath_abi::A11yNode` via the
// `A11Y_ROLE_*` / `A11Y_STATE_*` / `A11Y_ACTIONBIT_*` numeric vocabulary. The
// kernel (or a future userspace AT bridge) copies these fields straight across.
// The numeric tags are inlined (not imported) so athui keeps its lean dep set;
// `provider_role_code_kat` asserts they stay in lockstep with the ABI.

// ath_abi A11Y_ROLE_* values (mirrored — kept in lockstep by the host KAT).
pub const ROLE_CODE_DESKTOP: u32 = 1;
pub const ROLE_CODE_WINDOW: u32 = 2;
pub const ROLE_CODE_BUTTON: u32 = 3;
pub const ROLE_CODE_LABEL: u32 = 4;
pub const ROLE_CODE_TEXT_FIELD: u32 = 5;
pub const ROLE_CODE_SLIDER: u32 = 6;
pub const ROLE_CODE_CHECKBOX: u32 = 7;
pub const ROLE_CODE_TOGGLE: u32 = 8;
pub const ROLE_CODE_IMAGE: u32 = 9;
pub const ROLE_CODE_LINK: u32 = 10;
pub const ROLE_CODE_HEADING: u32 = 11;
pub const ROLE_CODE_LIST: u32 = 12;
pub const ROLE_CODE_LIST_ITEM: u32 = 13;
pub const ROLE_CODE_TAB: u32 = 14;
pub const ROLE_CODE_TAB_BAR: u32 = 15;
pub const ROLE_CODE_SCROLL_VIEW: u32 = 16;
pub const ROLE_CODE_DIALOG: u32 = 17;
pub const ROLE_CODE_ALERT: u32 = 18;
pub const ROLE_CODE_MENU: u32 = 19;
pub const ROLE_CODE_MENU_ITEM: u32 = 20;
pub const ROLE_CODE_PROGRESS_BAR: u32 = 21;
pub const ROLE_CODE_SWITCH: u32 = 22;
pub const ROLE_CODE_TOOLBAR: u32 = 23;
pub const ROLE_CODE_GROUP: u32 = 24;

// ath_abi A11Y_STATE_* bits.
pub const STATE_CODE_FOCUSED: u32 = 1 << 0;
pub const STATE_CODE_VISIBLE: u32 = 1 << 1;
pub const STATE_CODE_DISABLED: u32 = 1 << 2;
pub const STATE_CODE_CHECKED: u32 = 1 << 3;
pub const STATE_CODE_FOCUSABLE: u32 = 1 << 9;

// ath_abi A11Y_ACTIONBIT_* bits.
pub const ACTIONBIT_CODE_FOCUS: u32 = 1 << 0;
pub const ACTIONBIT_CODE_ACTIVATE: u32 = 1 << 1;
pub const ACTIONBIT_CODE_SCROLL: u32 = 1 << 2;
pub const ACTIONBIT_CODE_SET_VALUE: u32 = 1 << 3;
pub const ACTIONBIT_CODE_INCREMENT: u32 = 1 << 4;
pub const ACTIONBIT_CODE_DECREMENT: u32 = 1 << 5;
pub const ACTIONBIT_CODE_DISMISS: u32 = 1 << 6;

/// The numeric ABI role code for a role (maps to `ath_abi::A11Y_ROLE_*`).
pub fn role_code(role: AccessibilityRole) -> u32 {
    match role {
        AccessibilityRole::Window => ROLE_CODE_WINDOW,
        AccessibilityRole::Button => ROLE_CODE_BUTTON,
        AccessibilityRole::Label => ROLE_CODE_LABEL,
        AccessibilityRole::TextField => ROLE_CODE_TEXT_FIELD,
        AccessibilityRole::Slider => ROLE_CODE_SLIDER,
        AccessibilityRole::Checkbox => ROLE_CODE_CHECKBOX,
        AccessibilityRole::Toggle => ROLE_CODE_TOGGLE,
        AccessibilityRole::Image => ROLE_CODE_IMAGE,
        AccessibilityRole::Link => ROLE_CODE_LINK,
        AccessibilityRole::Heading => ROLE_CODE_HEADING,
        AccessibilityRole::List => ROLE_CODE_LIST,
        AccessibilityRole::ListItem => ROLE_CODE_LIST_ITEM,
        AccessibilityRole::Tab => ROLE_CODE_TAB,
        AccessibilityRole::TabBar => ROLE_CODE_TAB_BAR,
        AccessibilityRole::ScrollView => ROLE_CODE_SCROLL_VIEW,
        AccessibilityRole::Dialog => ROLE_CODE_DIALOG,
        AccessibilityRole::Alert => ROLE_CODE_ALERT,
        AccessibilityRole::Menu => ROLE_CODE_MENU,
        AccessibilityRole::MenuItem => ROLE_CODE_MENU_ITEM,
        AccessibilityRole::ProgressBar => ROLE_CODE_PROGRESS_BAR,
        AccessibilityRole::Switch => ROLE_CODE_SWITCH,
        AccessibilityRole::Toolbar => ROLE_CODE_TOOLBAR,
        AccessibilityRole::Group => ROLE_CODE_GROUP,
        AccessibilityRole::None => ROLE_CODE_GROUP,
    }
}

/// The `A11Y_ACTIONBIT_*` bitfield a node with this role accepts — derived from
/// the same role->actions table `AccessibilityNode::new` uses, projected onto
/// the ABI action bits.
pub fn action_bits_for_role(role: AccessibilityRole) -> u32 {
    match role {
        AccessibilityRole::Button | AccessibilityRole::Link | AccessibilityRole::MenuItem => {
            ACTIONBIT_CODE_FOCUS | ACTIONBIT_CODE_ACTIVATE
        }
        AccessibilityRole::Checkbox | AccessibilityRole::Toggle | AccessibilityRole::Switch => {
            ACTIONBIT_CODE_FOCUS | ACTIONBIT_CODE_ACTIVATE
        }
        AccessibilityRole::Slider => {
            ACTIONBIT_CODE_FOCUS
                | ACTIONBIT_CODE_SET_VALUE
                | ACTIONBIT_CODE_INCREMENT
                | ACTIONBIT_CODE_DECREMENT
        }
        AccessibilityRole::TextField => ACTIONBIT_CODE_FOCUS | ACTIONBIT_CODE_SET_VALUE,
        AccessibilityRole::ScrollView => ACTIONBIT_CODE_FOCUS | ACTIONBIT_CODE_SCROLL,
        AccessibilityRole::Dialog | AccessibilityRole::Alert => ACTIONBIT_CODE_DISMISS,
        _ => 0,
    }
}

/// A wire-shaped widget node ready to be parented under a window in the kernel
/// accessibility tree. Mirrors the `ath_abi::A11yNode` field set (minus the
/// inline name buffer, kept as a `String` until the kernel copies it).
#[derive(Clone, Debug)]
pub struct ProviderNode {
    pub id: u64,
    pub parent: u64,
    pub role: u32,
    pub state: u32,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub actions: u32,
    pub name: String,
}

/// Produce the widget-tier provider nodes for a window from a built
/// `AccessibilityTree` (already walked from the live widget tree, so labels and
/// roles come from the registry via `infer_*`). Each node is parented under
/// `window_id` and converted to the ABI vocabulary. Structural Group/None nodes
/// with no label are skipped (they are not assistive-meaningful), so the
/// provider returns only NAMED, semantic widgets — never a bag of anonymous
/// groups. Returns an empty Vec for a window with no semantic widgets.
pub fn provider_nodes_for_window(
    window_id: u64,
    tree: &AccessibilityTree,
) -> alloc::vec::Vec<ProviderNode> {
    let mut out = alloc::vec::Vec::new();
    for n in &tree.nodes {
        // Skip purely-structural, unnamed nodes — they carry no semantics.
        if matches!(n.role, AccessibilityRole::Group | AccessibilityRole::None)
            && n.label.is_empty()
        {
            continue;
        }
        let mut state = STATE_CODE_VISIBLE;
        if n.traits.focusable {
            state |= STATE_CODE_FOCUSABLE;
        }
        if n.traits.disabled {
            state |= STATE_CODE_DISABLED;
        }
        if n.traits.selected {
            state |= STATE_CODE_CHECKED;
        }
        out.push(ProviderNode {
            id: n.id as u64,
            parent: window_id,
            role: role_code(n.role),
            state,
            x: n.bounds.x as i32,
            y: n.bounds.y as i32,
            w: n.bounds.width.max(0.0) as u32,
            h: n.bounds.height.max(0.0) as u32,
            actions: action_bits_for_role(n.role),
            name: n.label.clone(),
        });
    }
    out
}

fn role_to_string(role: AccessibilityRole) -> &'static str {
    match role {
        AccessibilityRole::Button => "button",
        AccessibilityRole::Label => "label",
        AccessibilityRole::TextField => "text field",
        AccessibilityRole::Slider => "slider",
        AccessibilityRole::Checkbox => "checkbox",
        AccessibilityRole::Toggle => "toggle",
        AccessibilityRole::Image => "image",
        AccessibilityRole::Link => "link",
        AccessibilityRole::Heading => "heading",
        AccessibilityRole::List => "list",
        AccessibilityRole::ListItem => "list item",
        AccessibilityRole::Tab => "tab",
        AccessibilityRole::TabBar => "tab bar",
        AccessibilityRole::ScrollView => "scroll view",
        AccessibilityRole::Dialog => "dialog",
        AccessibilityRole::Alert => "alert",
        AccessibilityRole::Menu => "menu",
        AccessibilityRole::MenuItem => "menu item",
        AccessibilityRole::ProgressBar => "progress bar",
        AccessibilityRole::Switch => "switch",
        AccessibilityRole::Toolbar => "toolbar",
        AccessibilityRole::Window => "window",
        AccessibilityRole::Group => "group",
        AccessibilityRole::None => "",
    }
}

// ── Host KATs (dev box, `cargo test -p athui`) ──────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_codes_match_documented_abi() {
        // Lockstep guard against the documented ath_abi A11Y_ROLE_* / _STATE_* /
        // _ACTIONBIT_* values (athui doesn't link ath_abi; the kernel a11y.rs
        // side asserts the same numbers against the real constants). If the ABI
        // renumbers, both this and the kernel smoketest catch the drift.
        assert_eq!(ROLE_CODE_WINDOW, 2);
        assert_eq!(ROLE_CODE_BUTTON, 3);
        assert_eq!(ROLE_CODE_LABEL, 4);
        assert_eq!(ROLE_CODE_TEXT_FIELD, 5);
        assert_eq!(ROLE_CODE_SLIDER, 6);
        assert_eq!(ROLE_CODE_CHECKBOX, 7);
        assert_eq!(STATE_CODE_FOCUSABLE, 1 << 9);
        assert_eq!(ACTIONBIT_CODE_ACTIVATE, 1 << 1);
        assert_eq!(ACTIONBIT_CODE_SET_VALUE, 1 << 3);
    }

    // NOTE: these exercises share the process-global `WIDGET_REGISTRY` (a
    // static `RefCell`, sound because AthUI runs one UI thread per process —
    // see the `unsafe impl Sync` safety note above). cargo runs `#[test]`s
    // multi-threaded, so EVERY registry-touching assertion must live inside
    // ONE `#[test]` or they race the shared `RefCell` ("already borrowed").
    // Keep registry exercises sequential here; parallel-safe asserts (constants,
    // pure functions) go in their own tests.
    #[test]
    fn registry_role_inference_and_provider() {
        // (1) infer role + label are real, not the old Group/"" stub.
        clear_widget_semantics();
        register_widget_semantics(WidgetSemantics::new(7, WidgetKind::Button, "Save"));
        register_widget_semantics(WidgetSemantics::new(8, WidgetKind::Label, "Name:"));
        assert_eq!(infer_role_from_widget_id(7), AccessibilityRole::Button);
        assert_eq!(infer_role_from_widget_id(8), AccessibilityRole::Label);
        // Unregistered structural node stays a Group with no label (correct).
        assert_eq!(infer_role_from_widget_id(999), AccessibilityRole::Group);

        // (2) provider emits named widgets only (drops the unnamed Group root).
        clear_widget_semantics();
        register_widget_semantics(WidgetSemantics::new(2, WidgetKind::Button, "Save"));
        register_widget_semantics(WidgetSemantics::new(3, WidgetKind::TextField, "Search"));

        let mut tree = AccessibilityTree::new();
        // window root (Group, no label) + a button + a textfield
        tree.nodes.push(AccessibilityNode::new(
            1,
            AccessibilityRole::Group,
            String::new(),
            Rect::default(),
        ));
        tree.nodes.push(AccessibilityNode::new(
            2,
            AccessibilityRole::Button,
            String::from("Save"),
            Rect {
                x: 10.0,
                y: 20.0,
                width: 80.0,
                height: 30.0,
            },
        ));
        tree.nodes.push(AccessibilityNode::new(
            3,
            AccessibilityRole::TextField,
            String::from("Search"),
            Rect::default(),
        ));

        let nodes = provider_nodes_for_window(17, &tree);
        // The unnamed structural Group is dropped; only the 2 real widgets remain.
        assert_eq!(nodes.len(), 2);
        let btn = nodes.iter().find(|n| n.id == 2).unwrap();
        assert_eq!(btn.parent, 17);
        assert_eq!(btn.role, ROLE_CODE_BUTTON);
        assert_eq!(btn.name, "Save");
        assert_eq!(btn.x, 10);
        assert_eq!(btn.w, 80);
        assert!(btn.actions & ACTIONBIT_CODE_ACTIVATE != 0);
        let tf = nodes.iter().find(|n| n.id == 3).unwrap();
        assert_eq!(tf.role, ROLE_CODE_TEXT_FIELD);
        assert!(tf.actions & ACTIONBIT_CODE_SET_VALUE != 0);
        clear_widget_semantics();
    }
}
