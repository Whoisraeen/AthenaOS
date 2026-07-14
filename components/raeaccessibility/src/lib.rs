#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ─── Accessibility Roles ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    Window,
    Dialog,
    Alert,
    Menu,
    MenuBar,
    MenuItem,
    MenuItemCheckBox,
    MenuItemRadio,
    ToolBar,
    StatusBar,
    TabList,
    Tab,
    TabPanel,
    ScrollBar,
    Slider,
    SpinButton,
    ProgressBar,
    Button,
    CheckBox,
    RadioButton,
    ComboBox,
    TextBox,
    PasswordText,
    StaticText,
    Image,
    Link,
    List,
    ListItem,
    Tree,
    TreeItem,
    Table,
    TableRow,
    TableCell,
    HeaderItem,
    Separator,
    Group,
    Pane,
    Document,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    Paragraph,
    BlockQuote,
    Article,
    Complementary,
    ContentInfo,
    Form,
    Main,
    Navigation,
    Region,
    Banner,
    Search,
    Note,
    Figure,
    Tooltip,
    Log,
    Marquee,
    Timer,
    Math,
    Grid,
    GridCell,
    Row,
    RowHeader,
    ColumnHeader,
    Application,
    Presentation,
}

impl Role {
    pub fn heading_level(self) -> Option<u8> {
        match self {
            Role::Heading1 => Some(1),
            Role::Heading2 => Some(2),
            Role::Heading3 => Some(3),
            Role::Heading4 => Some(4),
            Role::Heading5 => Some(5),
            Role::Heading6 => Some(6),
            _ => None,
        }
    }

    pub fn is_interactive(self) -> bool {
        matches!(
            self,
            Role::Button
                | Role::CheckBox
                | Role::RadioButton
                | Role::ComboBox
                | Role::TextBox
                | Role::PasswordText
                | Role::Slider
                | Role::SpinButton
                | Role::Link
                | Role::MenuItem
                | Role::MenuItemCheckBox
                | Role::MenuItemRadio
                | Role::Tab
                | Role::TreeItem
        )
    }

    pub fn is_landmark(self) -> bool {
        matches!(
            self,
            Role::Banner
                | Role::Complementary
                | Role::ContentInfo
                | Role::Form
                | Role::Main
                | Role::Navigation
                | Role::Region
                | Role::Search
        )
    }
}

// ─── Bitflags Macro ─────────────────────────────────────────────────────────

macro_rules! bitflags_manual {
    (pub struct $name:ident : $ty:ty { $(const $flag:ident = $val:expr;)* }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name { bits: $ty }

        impl $name {
            $(pub const $flag: Self = Self { bits: $val };)*

            pub const fn empty() -> Self { Self { bits: 0 } }
            pub const fn all() -> Self { Self { bits: !0 } }
            pub const fn bits(self) -> $ty { self.bits }
            pub const fn contains(self, other: Self) -> bool { (self.bits & other.bits) == other.bits }
            pub fn insert(&mut self, other: Self) { self.bits |= other.bits; }
            pub fn remove(&mut self, other: Self) { self.bits &= !other.bits; }
            pub fn toggle(&mut self, other: Self) { self.bits ^= other.bits; }
            pub const fn union(self, other: Self) -> Self { Self { bits: self.bits | other.bits } }
            pub const fn intersection(self, other: Self) -> Self { Self { bits: self.bits & other.bits } }
            pub const fn is_empty(self) -> bool { self.bits == 0 }
        }
    };
}

// ─── Accessibility States ───────────────────────────────────────────────────

bitflags_manual! {
    pub struct StateFlags: u64 {
        const FOCUSABLE       = 1 << 0;
        const FOCUSED         = 1 << 1;
        const SELECTED        = 1 << 2;
        const CHECKED         = 1 << 3;
        const EXPANDED        = 1 << 4;
        const COLLAPSED       = 1 << 5;
        const DISABLED        = 1 << 6;
        const READ_ONLY       = 1 << 7;
        const REQUIRED        = 1 << 8;
        const INVALID         = 1 << 9;
        const PRESSED         = 1 << 10;
        const BUSY            = 1 << 11;
        const MODAL           = 1 << 12;
        const MULTI_SELECTABLE = 1 << 13;
        const EDITABLE        = 1 << 14;
        const HAS_POPUP       = 1 << 15;
        const VISITED         = 1 << 16;
        const ACTIVE          = 1 << 17;
        const DEFAULT         = 1 << 18;
        const HOVERED         = 1 << 19;
        const DRAGGED         = 1 << 20;
        const DROP_TARGET     = 1 << 21;
    }
}

// ─── Actions ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Action {
    Click,
    Focus,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
    Expand,
    Collapse,
    Select,
    Activate,
    Press,
    Release,
    Check,
    Uncheck,
    SetValue,
    Increment,
    Decrement,
    ShowMenu,
    Dismiss,
    MoveToParent,
    ScrollIntoView,
}

// ─── Relations ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RelationType {
    LabelledBy,
    DescribedBy,
    ControlledBy,
    Controls,
    FlowsTo,
    FlowsFrom,
    MemberOf,
    Owns,
    ErrorMessage,
    Details,
}

#[derive(Debug, Clone)]
pub struct Relation {
    pub relation_type: RelationType,
    pub target_id: u32,
}

// ─── Bounds ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains_point(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.width as i32
            && self.x + self.width as i32 > other.x
            && self.y < other.y + other.height as i32
            && self.y + self.height as i32 > other.y
    }
}

// ─── Accessibility Tree Node ────────────────────────────────────────────────

#[derive(Clone)]
pub struct AccessibilityNode {
    pub id: u32,
    pub role: Role,
    pub name: String,
    pub description: String,
    pub value: String,
    pub states: StateFlags,
    pub bounds: Rect,
    pub actions: Vec<Action>,
    pub relations: Vec<Relation>,
    pub parent_id: Option<u32>,
    pub children_ids: Vec<u32>,
    pub properties: NodeProperties,
}

#[derive(Clone, Default)]
pub struct NodeProperties {
    pub placeholder: String,
    pub autocomplete: String,
    pub value_min: f32,
    pub value_max: f32,
    pub value_now: f32,
    pub value_text: String,
    pub row_index: u32,
    pub col_index: u32,
    pub row_span: u32,
    pub col_span: u32,
    pub level: u32,
    pub position_in_set: u32,
    pub set_size: u32,
    pub live_region: LiveRegion,
    pub relevant: String,
    pub atomic: bool,
    pub busy: bool,
    pub class_name: String,
    pub input_type: String,
    pub sort_direction: SortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveRegion {
    #[default]
    Off,
    Polite,
    Assertive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDirection {
    #[default]
    None,
    Ascending,
    Descending,
    Other,
}

impl AccessibilityNode {
    pub fn new(id: u32, role: Role, name: &str) -> Self {
        Self {
            id,
            role,
            name: String::from(name),
            description: String::new(),
            value: String::new(),
            states: StateFlags::empty(),
            bounds: Rect::default(),
            actions: Vec::new(),
            relations: Vec::new(),
            parent_id: None,
            children_ids: Vec::new(),
            properties: NodeProperties::default(),
        }
    }

    pub fn is_focusable(&self) -> bool {
        self.states.contains(StateFlags::FOCUSABLE)
    }

    pub fn is_visible(&self) -> bool {
        self.bounds.width > 0 && self.bounds.height > 0
    }
}

// ─── Accessibility Tree ─────────────────────────────────────────────────────

pub struct AccessibilityTree {
    nodes: Vec<AccessibilityNode>,
    root_id: u32,
    next_id: u32,
    focus_id: Option<u32>,
}

impl AccessibilityTree {
    pub fn new() -> Self {
        let root = AccessibilityNode::new(0, Role::Application, "RaeenOS");
        Self {
            nodes: vec![root],
            root_id: 0,
            next_id: 1,
            focus_id: None,
        }
    }

    pub fn add_node(&mut self, parent_id: u32, role: Role, name: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let mut node = AccessibilityNode::new(id, role, name);
        node.parent_id = Some(parent_id);

        if let Some(parent) = self.get_node_mut(parent_id) {
            parent.children_ids.push(id);
        }

        self.nodes.push(node);
        id
    }

    pub fn remove_node(&mut self, id: u32) {
        if let Some(node) = self.get_node(id) {
            let parent_id = node.parent_id;
            let children = node.children_ids.clone();
            for child_id in children {
                self.remove_node(child_id);
            }
            if let Some(pid) = parent_id {
                if let Some(parent) = self.get_node_mut(pid) {
                    parent.children_ids.retain(|&c| c != id);
                }
            }
        }
        self.nodes.retain(|n| n.id != id);
    }

    pub fn get_node(&self, id: u32) -> Option<&AccessibilityNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn get_node_mut(&mut self, id: u32) -> Option<&mut AccessibilityNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn find_at_point(&self, x: i32, y: i32) -> Option<u32> {
        let mut deepest: Option<u32> = None;
        for node in &self.nodes {
            if node.bounds.contains_point(x, y) && node.is_visible() {
                deepest = Some(node.id);
            }
        }
        deepest
    }

    pub fn set_focus(&mut self, id: u32) {
        if let Some(old_focus) = self.focus_id {
            if let Some(node) = self.get_node_mut(old_focus) {
                node.states.remove(StateFlags::FOCUSED);
            }
        }
        if let Some(node) = self.get_node_mut(id) {
            if node.states.contains(StateFlags::FOCUSABLE) {
                node.states.insert(StateFlags::FOCUSED);
                self.focus_id = Some(id);
            }
        }
    }

    pub fn focused_node(&self) -> Option<&AccessibilityNode> {
        self.focus_id.and_then(|id| self.get_node(id))
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn walk_depth_first(&self, start_id: u32) -> Vec<u32> {
        let mut result = Vec::new();
        let mut stack = vec![start_id];
        while let Some(id) = stack.pop() {
            result.push(id);
            if let Some(node) = self.get_node(id) {
                for &child in node.children_ids.iter().rev() {
                    stack.push(child);
                }
            }
        }
        result
    }
}

// ─── Screen Reader Engine ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseMode {
    VirtualCursor,
    FormsMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationType {
    NextElement,
    PreviousElement,
    NextHeading,
    PreviousHeading,
    NextLink,
    PreviousLink,
    NextButton,
    PreviousButton,
    NextFormField,
    PreviousFormField,
    NextLandmark,
    PreviousLandmark,
    NextTable,
    PreviousTable,
    NextListItem,
    PreviousListItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnouncementPriority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Clone)]
pub struct Announcement {
    pub text: String,
    pub priority: AnnouncementPriority,
    pub interrupt: bool,
}

pub struct ScreenReader {
    mode: BrowseMode,
    virtual_cursor_pos: u32,
    caret_position: u32,
    announcement_queue: Vec<Announcement>,
    reading_order: Vec<u32>,
    say_all_active: bool,
    say_all_position: usize,
}

impl ScreenReader {
    pub fn new() -> Self {
        Self {
            mode: BrowseMode::VirtualCursor,
            virtual_cursor_pos: 0,
            caret_position: 0,
            announcement_queue: Vec::new(),
            reading_order: Vec::new(),
            say_all_active: false,
            say_all_position: 0,
        }
    }

    pub fn set_mode(&mut self, mode: BrowseMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> BrowseMode {
        self.mode
    }

    pub fn navigate(&mut self, tree: &AccessibilityTree, nav_type: NavigationType) -> Option<u32> {
        let order = tree.walk_depth_first(0);
        if order.is_empty() {
            return None;
        }

        let current_pos = order
            .iter()
            .position(|&id| id == self.virtual_cursor_pos)
            .unwrap_or(0);

        match nav_type {
            NavigationType::NextElement => {
                let next = (current_pos + 1) % order.len();
                self.virtual_cursor_pos = order[next];
                Some(order[next])
            }
            NavigationType::PreviousElement => {
                let prev = if current_pos == 0 {
                    order.len() - 1
                } else {
                    current_pos - 1
                };
                self.virtual_cursor_pos = order[prev];
                Some(order[prev])
            }
            NavigationType::NextHeading => {
                self.find_next_by_role(tree, &order, current_pos, |r| r.heading_level().is_some())
            }
            NavigationType::PreviousHeading => {
                self.find_prev_by_role(tree, &order, current_pos, |r| r.heading_level().is_some())
            }
            NavigationType::NextLink => {
                self.find_next_by_role(tree, &order, current_pos, |r| r == Role::Link)
            }
            NavigationType::PreviousLink => {
                self.find_prev_by_role(tree, &order, current_pos, |r| r == Role::Link)
            }
            NavigationType::NextButton => {
                self.find_next_by_role(tree, &order, current_pos, |r| r == Role::Button)
            }
            NavigationType::PreviousButton => {
                self.find_prev_by_role(tree, &order, current_pos, |r| r == Role::Button)
            }
            NavigationType::NextFormField => {
                self.find_next_by_role(tree, &order, current_pos, |r| {
                    matches!(
                        r,
                        Role::TextBox
                            | Role::PasswordText
                            | Role::ComboBox
                            | Role::CheckBox
                            | Role::RadioButton
                    )
                })
            }
            NavigationType::PreviousFormField => {
                self.find_prev_by_role(tree, &order, current_pos, |r| {
                    matches!(
                        r,
                        Role::TextBox
                            | Role::PasswordText
                            | Role::ComboBox
                            | Role::CheckBox
                            | Role::RadioButton
                    )
                })
            }
            NavigationType::NextLandmark => {
                self.find_next_by_role(tree, &order, current_pos, |r| r.is_landmark())
            }
            NavigationType::PreviousLandmark => {
                self.find_prev_by_role(tree, &order, current_pos, |r| r.is_landmark())
            }
            NavigationType::NextTable => {
                self.find_next_by_role(tree, &order, current_pos, |r| r == Role::Table)
            }
            NavigationType::PreviousTable => {
                self.find_prev_by_role(tree, &order, current_pos, |r| r == Role::Table)
            }
            NavigationType::NextListItem => {
                self.find_next_by_role(tree, &order, current_pos, |r| r == Role::ListItem)
            }
            NavigationType::PreviousListItem => {
                self.find_prev_by_role(tree, &order, current_pos, |r| r == Role::ListItem)
            }
        }
    }

    fn find_next_by_role(
        &mut self,
        tree: &AccessibilityTree,
        order: &[u32],
        start: usize,
        pred: impl Fn(Role) -> bool,
    ) -> Option<u32> {
        for i in 1..order.len() {
            let idx = (start + i) % order.len();
            if let Some(node) = tree.get_node(order[idx]) {
                if pred(node.role) {
                    self.virtual_cursor_pos = order[idx];
                    return Some(order[idx]);
                }
            }
        }
        None
    }

    fn find_prev_by_role(
        &mut self,
        tree: &AccessibilityTree,
        order: &[u32],
        start: usize,
        pred: impl Fn(Role) -> bool,
    ) -> Option<u32> {
        for i in 1..order.len() {
            let idx = if start >= i {
                start - i
            } else {
                order.len() - (i - start)
            };
            if let Some(node) = tree.get_node(order[idx]) {
                if pred(node.role) {
                    self.virtual_cursor_pos = order[idx];
                    return Some(order[idx]);
                }
            }
        }
        None
    }

    pub fn announce(&mut self, text: &str, priority: AnnouncementPriority, interrupt: bool) {
        if interrupt {
            self.announcement_queue
                .retain(|a| matches!(a.priority, AnnouncementPriority::Critical));
        }
        self.announcement_queue.push(Announcement {
            text: String::from(text),
            priority,
            interrupt,
        });
        self.announcement_queue
            .sort_by(|a, b| (b.priority as u8).cmp(&(a.priority as u8)));
    }

    pub fn next_announcement(&mut self) -> Option<Announcement> {
        if self.announcement_queue.is_empty() {
            None
        } else {
            Some(self.announcement_queue.remove(0))
        }
    }

    pub fn start_say_all(&mut self, tree: &AccessibilityTree) {
        self.reading_order = tree.walk_depth_first(0);
        self.say_all_active = true;
        self.say_all_position = 0;
    }

    pub fn say_all_next(&mut self, tree: &AccessibilityTree) -> Option<String> {
        if !self.say_all_active {
            return None;
        }
        while self.say_all_position < self.reading_order.len() {
            let id = self.reading_order[self.say_all_position];
            self.say_all_position += 1;
            if let Some(node) = tree.get_node(id) {
                if !node.name.is_empty() {
                    return Some(node.name.clone());
                }
            }
        }
        self.say_all_active = false;
        None
    }

    pub fn stop_say_all(&mut self) {
        self.say_all_active = false;
    }

    pub fn describe_node(&self, node: &AccessibilityNode) -> String {
        let mut desc = String::new();
        desc.push_str(&format_role(node.role));
        if !node.name.is_empty() {
            desc.push_str(": ");
            desc.push_str(&node.name);
        }
        if !node.value.is_empty() {
            desc.push_str(", value: ");
            desc.push_str(&node.value);
        }
        if node.states.contains(StateFlags::DISABLED) {
            desc.push_str(", disabled");
        }
        if node.states.contains(StateFlags::CHECKED) {
            desc.push_str(", checked");
        }
        if node.states.contains(StateFlags::EXPANDED) {
            desc.push_str(", expanded");
        }
        if node.states.contains(StateFlags::COLLAPSED) {
            desc.push_str(", collapsed");
        }
        desc
    }
}

fn format_role(role: Role) -> String {
    match role {
        Role::Button => String::from("button"),
        Role::Link => String::from("link"),
        Role::TextBox => String::from("edit"),
        Role::CheckBox => String::from("check box"),
        Role::RadioButton => String::from("radio button"),
        Role::ComboBox => String::from("combo box"),
        Role::Heading1 => String::from("heading level 1"),
        Role::Heading2 => String::from("heading level 2"),
        Role::Heading3 => String::from("heading level 3"),
        Role::Heading4 => String::from("heading level 4"),
        Role::Heading5 => String::from("heading level 5"),
        Role::Heading6 => String::from("heading level 6"),
        Role::List => String::from("list"),
        Role::ListItem => String::from("list item"),
        Role::Table => String::from("table"),
        Role::Image => String::from("image"),
        Role::Slider => String::from("slider"),
        Role::ProgressBar => String::from("progress bar"),
        Role::Menu => String::from("menu"),
        Role::MenuItem => String::from("menu item"),
        Role::Dialog => String::from("dialog"),
        Role::Alert => String::from("alert"),
        Role::Tab => String::from("tab"),
        Role::TabPanel => String::from("tab panel"),
        Role::Tree => String::from("tree"),
        Role::TreeItem => String::from("tree item"),
        _ => String::from("element"),
    }
}

// ─── Speech Synthesis (TTS) ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceGender {
    Male,
    Female,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceAge {
    Child,
    Teen,
    Adult,
    Senior,
}

#[derive(Clone)]
pub struct Voice {
    pub name: String,
    pub language: String,
    pub gender: VoiceGender,
    pub age: VoiceAge,
    pub quality: u8,
}

#[derive(Clone)]
pub struct SpeechParams {
    pub rate: f32,
    pub pitch: f32,
    pub volume: f32,
}

impl Default for SpeechParams {
    fn default() -> Self {
        Self {
            rate: 1.0,
            pitch: 1.0,
            volume: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsmlElement {
    Speak,
    Break,
    Emphasis,
    Prosody,
    SayAs,
    Sub,
    Phoneme,
    Audio,
}

#[derive(Clone)]
pub struct SsmlNode {
    pub element: SsmlElement,
    pub text: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<SsmlNode>,
}

impl SsmlNode {
    pub fn text(content: &str) -> Self {
        Self {
            element: SsmlElement::Speak,
            text: String::from(content),
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn break_time(time_ms: u32) -> Self {
        Self {
            element: SsmlElement::Break,
            text: String::new(),
            attributes: vec![(String::from("time"), alloc::format!("{}ms", time_ms))],
            children: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Utterance {
    pub text: String,
    pub voice: Option<String>,
    pub params: SpeechParams,
    pub priority: u8,
    pub ssml: Option<SsmlNode>,
}

pub struct SpeechSynthesizer {
    voices: Vec<Voice>,
    current_voice: usize,
    params: SpeechParams,
    utterance_queue: Vec<Utterance>,
    speaking: bool,
    paused: bool,
}

impl SpeechSynthesizer {
    pub fn new() -> Self {
        Self {
            voices: vec![
                Voice {
                    name: String::from("Rae"),
                    language: String::from("en-US"),
                    gender: VoiceGender::Female,
                    age: VoiceAge::Adult,
                    quality: 90,
                },
                Voice {
                    name: String::from("Ren"),
                    language: String::from("en-US"),
                    gender: VoiceGender::Male,
                    age: VoiceAge::Adult,
                    quality: 85,
                },
                Voice {
                    name: String::from("Nova"),
                    language: String::from("en-GB"),
                    gender: VoiceGender::Female,
                    age: VoiceAge::Adult,
                    quality: 88,
                },
            ],
            current_voice: 0,
            params: SpeechParams::default(),
            utterance_queue: Vec::new(),
            speaking: false,
            paused: false,
        }
    }

    pub fn speak(&mut self, text: &str) {
        self.utterance_queue.push(Utterance {
            text: String::from(text),
            voice: None,
            params: self.params.clone(),
            priority: 5,
            ssml: None,
        });
    }

    pub fn speak_ssml(&mut self, ssml: SsmlNode) {
        self.utterance_queue.push(Utterance {
            text: ssml.text.clone(),
            voice: None,
            params: self.params.clone(),
            priority: 5,
            ssml: Some(ssml),
        });
    }

    pub fn speak_priority(&mut self, text: &str, priority: u8, interrupt: bool) {
        if interrupt {
            self.stop();
        }
        self.utterance_queue.push(Utterance {
            text: String::from(text),
            voice: None,
            params: self.params.clone(),
            priority,
            ssml: None,
        });
        self.utterance_queue
            .sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    pub fn stop(&mut self) {
        self.utterance_queue.clear();
        self.speaking = false;
    }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.params.rate = rate.max(0.1).min(10.0);
    }

    pub fn set_pitch(&mut self, pitch: f32) {
        self.params.pitch = pitch.max(0.1).min(3.0);
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.params.volume = volume.max(0.0).min(1.0);
    }

    pub fn select_voice(&mut self, name: &str) -> bool {
        if let Some(idx) = self.voices.iter().position(|v| v.name == name) {
            self.current_voice = idx;
            true
        } else {
            false
        }
    }

    pub fn available_voices(&self) -> &[Voice] {
        &self.voices
    }

    pub fn next_utterance(&mut self) -> Option<Utterance> {
        if self.paused || self.utterance_queue.is_empty() {
            None
        } else {
            self.speaking = true;
            Some(self.utterance_queue.remove(0))
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    pub fn queue_length(&self) -> usize {
        self.utterance_queue.len()
    }
}

// ─── Braille Display ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrailleGrade {
    Uncontracted,
    Contracted,
}

pub struct BrailleCell {
    pub dots: u8,
}

impl BrailleCell {
    pub fn empty() -> Self {
        Self { dots: 0 }
    }
    pub fn from_dots(dots: u8) -> Self {
        Self { dots }
    }
    pub fn dot(&self, n: u8) -> bool {
        (self.dots >> (n - 1)) & 1 == 1
    }
}

pub struct BrailleDisplay {
    cells: Vec<BrailleCell>,
    cell_count: usize,
    grade: BrailleGrade,
    cursor_position: usize,
    pan_offset: usize,
    status_cells: usize,
}

impl BrailleDisplay {
    pub fn new(cell_count: usize, status_cells: usize) -> Self {
        let mut cells = Vec::with_capacity(cell_count);
        for _ in 0..cell_count {
            cells.push(BrailleCell::empty());
        }
        Self {
            cells,
            cell_count,
            grade: BrailleGrade::Uncontracted,
            cursor_position: 0,
            pan_offset: 0,
            status_cells,
        }
    }

    pub fn set_text(&mut self, text: &str) {
        let braille = self.translate_to_braille(text);
        let display_start = self.status_cells;
        for (i, cell) in braille.iter().enumerate() {
            if display_start + i < self.cell_count {
                self.cells[display_start + i] = BrailleCell::from_dots(*cell);
            }
        }
    }

    pub fn translate_to_braille(&self, text: &str) -> Vec<u8> {
        text.bytes().map(|b| self.ascii_to_braille(b)).collect()
    }

    fn ascii_to_braille(&self, ch: u8) -> u8 {
        match ch {
            b'a' => 0b000001,
            b'b' => 0b000011,
            b'c' => 0b001001,
            b'd' => 0b011001,
            b'e' => 0b010001,
            b'f' => 0b001011,
            b'g' => 0b011011,
            b'h' => 0b010011,
            b'i' => 0b001010,
            b'j' => 0b011010,
            b'k' => 0b000101,
            b'l' => 0b000111,
            b'm' => 0b001101,
            b'n' => 0b011101,
            b'o' => 0b010101,
            b'p' => 0b001111,
            b'q' => 0b011111,
            b'r' => 0b010111,
            b's' => 0b001110,
            b't' => 0b011110,
            b'u' => 0b100101,
            b'v' => 0b100111,
            b'w' => 0b111010,
            b'x' => 0b101101,
            b'y' => 0b111101,
            b'z' => 0b110101,
            b' ' => 0b000000,
            _ => 0b111111,
        }
    }

    pub fn pan_left(&mut self) {
        let available = self.cell_count - self.status_cells;
        if self.pan_offset >= available {
            self.pan_offset -= available;
        } else {
            self.pan_offset = 0;
        }
    }

    pub fn pan_right(&mut self, text_len: usize) {
        let available = self.cell_count - self.status_cells;
        if self.pan_offset + available < text_len {
            self.pan_offset += available;
        }
    }

    pub fn cursor_route(&mut self, cell_index: usize) {
        self.cursor_position = self.pan_offset + cell_index;
    }

    pub fn set_grade(&mut self, grade: BrailleGrade) {
        self.grade = grade;
    }

    pub fn set_status(&mut self, status: &[u8]) {
        for (i, &dots) in status.iter().enumerate() {
            if i < self.status_cells {
                self.cells[i] = BrailleCell::from_dots(dots);
            }
        }
    }
}

// ─── Magnifier ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagnifierMode {
    FullScreen,
    Docked,
    Floating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFilter {
    None,
    Grayscale,
    Inverted,
    HighContrast,
    Deuteranopia,
    Protanopia,
    Tritanopia,
}

pub struct Magnifier {
    zoom_level: f32,
    min_zoom: f32,
    max_zoom: f32,
    mode: MagnifierMode,
    follow_focus: bool,
    follow_caret: bool,
    follow_mouse: bool,
    color_inversion: bool,
    color_filter: ColorFilter,
    smooth_tracking: bool,
    viewport_x: i32,
    viewport_y: i32,
    viewport_width: u32,
    viewport_height: u32,
    lens_width: u32,
    lens_height: u32,
}

impl Magnifier {
    pub fn new() -> Self {
        Self {
            zoom_level: 2.0,
            min_zoom: 1.0,
            max_zoom: 20.0,
            mode: MagnifierMode::FullScreen,
            follow_focus: true,
            follow_caret: true,
            follow_mouse: true,
            color_inversion: false,
            color_filter: ColorFilter::None,
            smooth_tracking: true,
            viewport_x: 0,
            viewport_y: 0,
            viewport_width: 1920,
            viewport_height: 1080,
            lens_width: 400,
            lens_height: 300,
        }
    }

    pub fn set_zoom(&mut self, level: f32) {
        self.zoom_level = level.max(self.min_zoom).min(self.max_zoom);
    }

    pub fn zoom_in(&mut self) {
        self.set_zoom(self.zoom_level * 1.25);
    }

    pub fn zoom_out(&mut self) {
        self.set_zoom(self.zoom_level / 1.25);
    }

    pub fn zoom_level(&self) -> f32 {
        self.zoom_level
    }

    pub fn set_mode(&mut self, mode: MagnifierMode) {
        self.mode = mode;
    }
    pub fn mode(&self) -> MagnifierMode {
        self.mode
    }

    pub fn set_follow_focus(&mut self, enabled: bool) {
        self.follow_focus = enabled;
    }
    pub fn set_follow_caret(&mut self, enabled: bool) {
        self.follow_caret = enabled;
    }
    pub fn set_follow_mouse(&mut self, enabled: bool) {
        self.follow_mouse = enabled;
    }

    pub fn set_color_inversion(&mut self, enabled: bool) {
        self.color_inversion = enabled;
    }
    pub fn set_color_filter(&mut self, filter: ColorFilter) {
        self.color_filter = filter;
    }

    pub fn move_to(&mut self, x: i32, y: i32) {
        if self.smooth_tracking {
            let dx = x - self.viewport_x;
            let dy = y - self.viewport_y;
            self.viewport_x += dx / 4;
            self.viewport_y += dy / 4;
        } else {
            self.viewport_x = x;
            self.viewport_y = y;
        }
    }

    pub fn focus_changed(&mut self, bounds: &Rect) {
        if self.follow_focus {
            let center_x = bounds.x + bounds.width as i32 / 2;
            let center_y = bounds.y + bounds.height as i32 / 2;
            self.move_to(center_x, center_y);
        }
    }

    pub fn caret_moved(&mut self, x: i32, y: i32) {
        if self.follow_caret {
            self.move_to(x, y);
        }
    }

    pub fn mouse_moved(&mut self, x: i32, y: i32) {
        if self.follow_mouse {
            self.move_to(x, y);
        }
    }
}

// ─── On-Screen Keyboard ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardLayout {
    Qwerty,
    Azerty,
    Dvorak,
    Colemak,
}

#[derive(Debug, Clone)]
pub struct KeyDef {
    pub label: String,
    pub keycode: u32,
    pub width_units: u8,
    pub is_modifier: bool,
}

pub struct OnScreenKeyboard {
    layout: KeyboardLayout,
    visible: bool,
    keys: Vec<Vec<KeyDef>>,
    shift_active: bool,
    caps_lock: bool,
    prediction_enabled: bool,
    predictions: Vec<String>,
    swipe_typing: bool,
    haptic_feedback: bool,
    dwell_click_enabled: bool,
    dwell_time_ms: u32,
    scanning_enabled: bool,
    key_size: u32,
}

impl OnScreenKeyboard {
    pub fn new(layout: KeyboardLayout) -> Self {
        let keys = Self::build_layout(layout);
        Self {
            layout,
            visible: false,
            keys,
            shift_active: false,
            caps_lock: false,
            prediction_enabled: true,
            predictions: Vec::new(),
            swipe_typing: false,
            haptic_feedback: true,
            dwell_click_enabled: false,
            dwell_time_ms: 1000,
            scanning_enabled: false,
            key_size: 48,
        }
    }

    fn build_layout(layout: KeyboardLayout) -> Vec<Vec<KeyDef>> {
        let rows = match layout {
            KeyboardLayout::Qwerty => vec!["qwertyuiop", "asdfghjkl", "zxcvbnm"],
            KeyboardLayout::Azerty => vec!["azertyuiop", "qsdfghjklm", "wxcvbn"],
            KeyboardLayout::Dvorak => vec!["pyfgcrl", "aoeuidhtns", "qjkxbmwvz"],
            KeyboardLayout::Colemak => vec!["qwfpgjluy", "arstdhneio", "zxcvbkm"],
        };

        rows.iter()
            .map(|row| {
                row.chars()
                    .map(|c| {
                        let mut label = String::new();
                        label.push(c);
                        KeyDef {
                            label,
                            keycode: c as u32,
                            width_units: 1,
                            is_modifier: false,
                        }
                    })
                    .collect()
            })
            .collect()
    }

    pub fn show(&mut self) {
        self.visible = true;
    }
    pub fn hide(&mut self) {
        self.visible = false;
    }
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn toggle_shift(&mut self) {
        self.shift_active = !self.shift_active;
    }
    pub fn toggle_caps(&mut self) {
        self.caps_lock = !self.caps_lock;
    }

    pub fn set_layout(&mut self, layout: KeyboardLayout) {
        self.layout = layout;
        self.keys = Self::build_layout(layout);
    }

    pub fn press_key(&mut self, row: usize, col: usize) -> Option<u32> {
        if row < self.keys.len() && col < self.keys[row].len() {
            let keycode = self.keys[row][col].keycode;
            if self.shift_active {
                self.shift_active = false;
            }
            Some(keycode)
        } else {
            None
        }
    }

    pub fn update_predictions(&mut self, text: &str) {
        self.predictions.clear();
        if self.prediction_enabled && !text.is_empty() {
            self.predictions.push(String::from(text));
        }
    }

    pub fn get_predictions(&self) -> &[String] {
        &self.predictions
    }

    pub fn set_dwell_click(&mut self, enabled: bool, time_ms: u32) {
        self.dwell_click_enabled = enabled;
        self.dwell_time_ms = time_ms;
    }
}

// ─── Switch Access ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    RowColumn,
    Linear,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchAction {
    Select,
    Next,
    Previous,
    Back,
}

pub struct SwitchAccess {
    enabled: bool,
    scan_mode: ScanMode,
    scan_speed_ms: u32,
    auto_scan: bool,
    current_group: usize,
    current_item: usize,
    groups: Vec<Vec<u32>>,
    switch_debounce_ms: u32,
    last_switch_time: u64,
    highlight_color: u32,
}

impl SwitchAccess {
    pub fn new() -> Self {
        Self {
            enabled: false,
            scan_mode: ScanMode::RowColumn,
            scan_speed_ms: 1000,
            auto_scan: true,
            current_group: 0,
            current_item: 0,
            groups: Vec::new(),
            switch_debounce_ms: 100,
            last_switch_time: 0,
            highlight_color: 0xFF00FF00,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }
    pub fn disable(&mut self) {
        self.enabled = false;
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_scan_mode(&mut self, mode: ScanMode) {
        self.scan_mode = mode;
    }
    pub fn set_scan_speed(&mut self, ms: u32) {
        self.scan_speed_ms = ms.max(100).min(10000);
    }
    pub fn set_auto_scan(&mut self, enabled: bool) {
        self.auto_scan = enabled;
    }

    pub fn build_groups(&mut self, tree: &AccessibilityTree) {
        self.groups.clear();
        let all_ids = tree.walk_depth_first(0);
        let interactive: Vec<u32> = all_ids
            .into_iter()
            .filter(|&id| {
                tree.get_node(id)
                    .map(|n| n.role.is_interactive())
                    .unwrap_or(false)
            })
            .collect();

        match self.scan_mode {
            ScanMode::Linear => {
                for id in interactive {
                    self.groups.push(vec![id]);
                }
            }
            ScanMode::RowColumn => {
                let chunk_size = 5;
                for chunk in interactive.chunks(chunk_size) {
                    self.groups.push(chunk.to_vec());
                }
            }
            ScanMode::Group => {
                let chunk_size = 10;
                for chunk in interactive.chunks(chunk_size) {
                    self.groups.push(chunk.to_vec());
                }
            }
        }
    }

    pub fn handle_switch(&mut self, action: SwitchAction, current_time: u64) -> Option<u32> {
        if current_time - self.last_switch_time < self.switch_debounce_ms as u64 {
            return None;
        }
        self.last_switch_time = current_time;

        if self.groups.is_empty() {
            return None;
        }

        match action {
            SwitchAction::Next => {
                self.current_item += 1;
                if self.current_item >= self.groups[self.current_group].len() {
                    self.current_item = 0;
                    self.current_group = (self.current_group + 1) % self.groups.len();
                }
                Some(self.groups[self.current_group][self.current_item])
            }
            SwitchAction::Previous => {
                if self.current_item == 0 {
                    if self.current_group == 0 {
                        self.current_group = self.groups.len() - 1;
                    } else {
                        self.current_group -= 1;
                    }
                    self.current_item = self.groups[self.current_group].len() - 1;
                } else {
                    self.current_item -= 1;
                }
                Some(self.groups[self.current_group][self.current_item])
            }
            SwitchAction::Select => Some(self.groups[self.current_group][self.current_item]),
            SwitchAction::Back => {
                self.current_item = 0;
                if self.current_group == 0 {
                    self.current_group = self.groups.len() - 1;
                } else {
                    self.current_group -= 1;
                }
                Some(self.groups[self.current_group][self.current_item])
            }
        }
    }

    pub fn current_highlighted(&self) -> Option<u32> {
        if self.groups.is_empty() {
            return None;
        }
        Some(self.groups[self.current_group][self.current_item])
    }
}

// ─── Eye Tracking ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct GazePoint {
    pub x: f32,
    pub y: f32,
    pub confidence: f32,
    pub timestamp_ms: u64,
}

pub struct EyeTracker {
    enabled: bool,
    calibrated: bool,
    calibration_points: Vec<(f32, f32)>,
    current_gaze: GazePoint,
    fixation_threshold_ms: u64,
    fixation_radius_px: f32,
    dwell_click_enabled: bool,
    dwell_time_ms: u64,
    dwell_start: u64,
    dwell_point: GazePoint,
    gaze_scroll_enabled: bool,
    scroll_zone_height: u32,
    smoothing_factor: f32,
    history: Vec<GazePoint>,
    history_max: usize,
}

impl EyeTracker {
    pub fn new() -> Self {
        Self {
            enabled: false,
            calibrated: false,
            calibration_points: Vec::new(),
            current_gaze: GazePoint::default(),
            fixation_threshold_ms: 100,
            fixation_radius_px: 30.0,
            dwell_click_enabled: true,
            dwell_time_ms: 800,
            dwell_start: 0,
            dwell_point: GazePoint::default(),
            gaze_scroll_enabled: true,
            scroll_zone_height: 100,
            smoothing_factor: 0.3,
            history: Vec::new(),
            history_max: 30,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }
    pub fn disable(&mut self) {
        self.enabled = false;
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn start_calibration(&mut self) {
        self.calibration_points.clear();
        self.calibrated = false;
    }

    pub fn add_calibration_point(&mut self, screen_x: f32, screen_y: f32) {
        self.calibration_points.push((screen_x, screen_y));
        if self.calibration_points.len() >= 9 {
            self.calibrated = true;
        }
    }

    pub fn is_calibrated(&self) -> bool {
        self.calibrated
    }

    pub fn update_gaze(&mut self, raw_x: f32, raw_y: f32, confidence: f32, timestamp_ms: u64) {
        let smoothed_x =
            self.current_gaze.x * (1.0 - self.smoothing_factor) + raw_x * self.smoothing_factor;
        let smoothed_y =
            self.current_gaze.y * (1.0 - self.smoothing_factor) + raw_y * self.smoothing_factor;

        self.current_gaze = GazePoint {
            x: smoothed_x,
            y: smoothed_y,
            confidence,
            timestamp_ms,
        };

        self.history.push(self.current_gaze);
        if self.history.len() > self.history_max {
            self.history.remove(0);
        }
    }

    pub fn detect_fixation(&self) -> bool {
        if self.history.len() < 5 {
            return false;
        }
        let recent = &self.history[self.history.len() - 5..];
        let avg_x: f32 = recent.iter().map(|g| g.x).sum::<f32>() / 5.0;
        let avg_y: f32 = recent.iter().map(|g| g.y).sum::<f32>() / 5.0;

        recent.iter().all(|g| {
            let dx = g.x - avg_x;
            let dy = g.y - avg_y;
            let dist_sq = dx * dx + dy * dy;
            let mut s = dist_sq / 2.0;
            if s > 0.0 {
                for _ in 0..15 {
                    s = (s + dist_sq / s) * 0.5;
                }
            }
            s < self.fixation_radius_px
        })
    }

    pub fn check_dwell_click(&mut self, timestamp_ms: u64) -> Option<GazePoint> {
        if !self.dwell_click_enabled {
            return None;
        }
        if !self.detect_fixation() {
            self.dwell_start = timestamp_ms;
            self.dwell_point = self.current_gaze;
            return None;
        }

        if timestamp_ms - self.dwell_start >= self.dwell_time_ms {
            self.dwell_start = timestamp_ms;
            Some(self.current_gaze)
        } else {
            None
        }
    }

    pub fn check_gaze_scroll(&self, screen_height: u32) -> Option<i32> {
        if !self.gaze_scroll_enabled {
            return None;
        }
        let y = self.current_gaze.y as u32;
        if y < self.scroll_zone_height {
            Some(-1)
        } else if y > screen_height - self.scroll_zone_height {
            Some(1)
        } else {
            None
        }
    }

    pub fn gaze_point(&self) -> GazePoint {
        self.current_gaze
    }
}

// ─── Voice Control ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceControlMode {
    Command,
    Dictation,
    Sleep,
}

#[derive(Clone)]
pub struct VoiceCommand {
    pub phrase: String,
    pub action: VoiceCommandAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCommandAction {
    Click,
    DoubleClick,
    RightClick,
    ScrollUp,
    ScrollDown,
    GoBack,
    GoForward,
    GoHome,
    SwitchApp,
    Close,
    Minimize,
    Maximize,
    ShowNumbers,
    HideNumbers,
    MouseGrid,
    StartDictation,
    StopDictation,
    Sleep,
    Wake,
    Undo,
    Redo,
    SelectAll,
    Copy,
    Paste,
    Cut,
    Delete,
    PressEnter,
    PressTab,
    PressEscape,
}

pub struct NumberedOverlay {
    pub entries: Vec<(u32, Rect, u32)>,
    pub visible: bool,
    next_number: u32,
}

impl NumberedOverlay {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            visible: false,
            next_number: 1,
        }
    }

    pub fn generate(&mut self, tree: &AccessibilityTree) {
        self.entries.clear();
        self.next_number = 1;
        let all_ids = tree.walk_depth_first(0);
        for id in all_ids {
            if let Some(node) = tree.get_node(id) {
                if node.role.is_interactive() && node.is_visible() {
                    self.entries.push((self.next_number, node.bounds, id));
                    self.next_number += 1;
                }
            }
        }
        self.visible = true;
    }

    pub fn find_by_number(&self, number: u32) -> Option<u32> {
        self.entries
            .iter()
            .find(|(n, _, _)| *n == number)
            .map(|(_, _, id)| *id)
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }
}

pub struct MouseGrid {
    pub active: bool,
    pub level: u8,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl MouseGrid {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            active: false,
            level: 0,
            x: 0,
            y: 0,
            width: screen_width,
            height: screen_height,
        }
    }

    pub fn show(&mut self) {
        self.active = true;
        self.level = 0;
    }

    pub fn select_cell(&mut self, cell: u8) -> (i32, i32) {
        let row = (cell - 1) / 3;
        let col = (cell - 1) % 3;
        let cell_w = self.width / 3;
        let cell_h = self.height / 3;

        self.x += col as i32 * cell_w as i32;
        self.y += row as i32 * cell_h as i32;
        self.width = cell_w;
        self.height = cell_h;
        self.level += 1;

        (
            self.x + self.width as i32 / 2,
            self.y + self.height as i32 / 2,
        )
    }

    pub fn reset(&mut self, screen_width: u32, screen_height: u32) {
        self.x = 0;
        self.y = 0;
        self.width = screen_width;
        self.height = screen_height;
        self.level = 0;
    }

    pub fn hide(&mut self) {
        self.active = false;
    }
}

pub struct VoiceControl {
    mode: VoiceControlMode,
    commands: Vec<VoiceCommand>,
    numbered_overlay: NumberedOverlay,
    mouse_grid: MouseGrid,
    listening: bool,
    last_phrase: String,
    confidence_threshold: f32,
}

impl VoiceControl {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        let mut vc = Self {
            mode: VoiceControlMode::Command,
            commands: Vec::new(),
            numbered_overlay: NumberedOverlay::new(),
            mouse_grid: MouseGrid::new(screen_width, screen_height),
            listening: false,
            last_phrase: String::new(),
            confidence_threshold: 0.7,
        };
        vc.register_default_commands();
        vc
    }

    fn register_default_commands(&mut self) {
        let defaults = [
            ("click", VoiceCommandAction::Click),
            ("double click", VoiceCommandAction::DoubleClick),
            ("right click", VoiceCommandAction::RightClick),
            ("scroll up", VoiceCommandAction::ScrollUp),
            ("scroll down", VoiceCommandAction::ScrollDown),
            ("go back", VoiceCommandAction::GoBack),
            ("go forward", VoiceCommandAction::GoForward),
            ("go home", VoiceCommandAction::GoHome),
            ("switch app", VoiceCommandAction::SwitchApp),
            ("close", VoiceCommandAction::Close),
            ("minimize", VoiceCommandAction::Minimize),
            ("maximize", VoiceCommandAction::Maximize),
            ("show numbers", VoiceCommandAction::ShowNumbers),
            ("hide numbers", VoiceCommandAction::HideNumbers),
            ("mouse grid", VoiceCommandAction::MouseGrid),
            ("start dictation", VoiceCommandAction::StartDictation),
            ("stop dictation", VoiceCommandAction::StopDictation),
            ("go to sleep", VoiceCommandAction::Sleep),
            ("wake up", VoiceCommandAction::Wake),
            ("undo", VoiceCommandAction::Undo),
            ("redo", VoiceCommandAction::Redo),
            ("select all", VoiceCommandAction::SelectAll),
            ("copy that", VoiceCommandAction::Copy),
            ("paste that", VoiceCommandAction::Paste),
            ("cut that", VoiceCommandAction::Cut),
            ("delete that", VoiceCommandAction::Delete),
            ("press enter", VoiceCommandAction::PressEnter),
            ("press tab", VoiceCommandAction::PressTab),
            ("press escape", VoiceCommandAction::PressEscape),
        ];

        for (phrase, action) in defaults {
            self.commands.push(VoiceCommand {
                phrase: String::from(phrase),
                action,
            });
        }
    }

    pub fn set_mode(&mut self, mode: VoiceControlMode) {
        self.mode = mode;
    }
    pub fn mode(&self) -> VoiceControlMode {
        self.mode
    }

    pub fn start_listening(&mut self) {
        self.listening = true;
    }
    pub fn stop_listening(&mut self) {
        self.listening = false;
    }
    pub fn is_listening(&self) -> bool {
        self.listening
    }

    pub fn process_phrase(&mut self, phrase: &str, confidence: f32) -> Option<VoiceCommandAction> {
        if confidence < self.confidence_threshold {
            return None;
        }
        self.last_phrase = String::from(phrase);

        match self.mode {
            VoiceControlMode::Sleep => {
                if phrase == "wake up" {
                    self.mode = VoiceControlMode::Command;
                    return Some(VoiceCommandAction::Wake);
                }
                None
            }
            VoiceControlMode::Command => {
                let lower = phrase.to_ascii_lowercase();
                for cmd in &self.commands {
                    if lower.contains(&cmd.phrase) {
                        return Some(cmd.action);
                    }
                }
                None
            }
            VoiceControlMode::Dictation => {
                if phrase == "stop dictation" {
                    self.mode = VoiceControlMode::Command;
                    Some(VoiceCommandAction::StopDictation)
                } else {
                    None
                }
            }
        }
    }

    pub fn add_command(&mut self, phrase: &str, action: VoiceCommandAction) {
        self.commands.push(VoiceCommand {
            phrase: String::from(phrase),
            action,
        });
    }

    pub fn show_numbers(&mut self, tree: &AccessibilityTree) {
        self.numbered_overlay.generate(tree);
    }

    pub fn select_number(&self, number: u32) -> Option<u32> {
        self.numbered_overlay.find_by_number(number)
    }

    pub fn show_mouse_grid(&mut self) {
        self.mouse_grid.show();
    }

    pub fn grid_select(&mut self, cell: u8) -> (i32, i32) {
        self.mouse_grid.select_cell(cell)
    }
}

// ─── High Contrast Theme ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighContrastTheme {
    None,
    HighContrastBlack,
    HighContrastWhite,
    Custom,
}

#[derive(Clone)]
pub struct ContrastColors {
    pub background: u32,
    pub foreground: u32,
    pub hyperlink: u32,
    pub disabled: u32,
    pub selected_background: u32,
    pub selected_foreground: u32,
    pub button_face: u32,
    pub button_text: u32,
}

impl ContrastColors {
    pub fn high_contrast_black() -> Self {
        Self {
            background: 0xFF000000,
            foreground: 0xFFFFFFFF,
            hyperlink: 0xFF00FFFF,
            disabled: 0xFF00FF00,
            selected_background: 0xFF1AEBFF,
            selected_foreground: 0xFF000000,
            button_face: 0xFF000000,
            button_text: 0xFFFFFFFF,
        }
    }

    pub fn high_contrast_white() -> Self {
        Self {
            background: 0xFFFFFFFF,
            foreground: 0xFF000000,
            hyperlink: 0xFF0000FF,
            disabled: 0xFF808080,
            selected_background: 0xFF000080,
            selected_foreground: 0xFFFFFFFF,
            button_face: 0xFFFFFFFF,
            button_text: 0xFF000000,
        }
    }
}

// ─── Global Accessibility Service ───────────────────────────────────────────

pub struct AccessibilityService {
    pub initialized: bool,
    pub tree: AccessibilityTree,
    pub screen_reader: ScreenReader,
    pub speech: SpeechSynthesizer,
    pub braille: BrailleDisplay,
    pub magnifier: Magnifier,
    pub keyboard: OnScreenKeyboard,
    pub switch_access: SwitchAccess,
    pub eye_tracker: EyeTracker,
    pub voice_control: VoiceControl,
    pub high_contrast: HighContrastTheme,
    pub contrast_colors: ContrastColors,
    pub animations_reduced: bool,
    pub captions_enabled: bool,
    pub sticky_keys: bool,
    pub filter_keys: bool,
    pub filter_keys_delay_ms: u32,
    pub bounce_keys: bool,
    pub bounce_keys_delay_ms: u32,
    pub mouse_keys: bool,
    pub cursor_size: u8,
    pub cursor_blink_rate_ms: u32,
}

impl AccessibilityService {
    pub const fn uninit() -> Self {
        Self {
            initialized: false,
            tree: AccessibilityTree {
                nodes: Vec::new(),
                root_id: 0,
                next_id: 1,
                focus_id: None,
            },
            screen_reader: ScreenReader {
                mode: BrowseMode::VirtualCursor,
                virtual_cursor_pos: 0,
                caret_position: 0,
                announcement_queue: Vec::new(),
                reading_order: Vec::new(),
                say_all_active: false,
                say_all_position: 0,
            },
            speech: SpeechSynthesizer {
                voices: Vec::new(),
                current_voice: 0,
                params: SpeechParams {
                    rate: 1.0,
                    pitch: 1.0,
                    volume: 1.0,
                },
                utterance_queue: Vec::new(),
                speaking: false,
                paused: false,
            },
            braille: BrailleDisplay {
                cells: Vec::new(),
                cell_count: 0,
                grade: BrailleGrade::Uncontracted,
                cursor_position: 0,
                pan_offset: 0,
                status_cells: 0,
            },
            magnifier: Magnifier {
                zoom_level: 2.0,
                min_zoom: 1.0,
                max_zoom: 20.0,
                mode: MagnifierMode::FullScreen,
                follow_focus: true,
                follow_caret: true,
                follow_mouse: true,
                color_inversion: false,
                color_filter: ColorFilter::None,
                smooth_tracking: true,
                viewport_x: 0,
                viewport_y: 0,
                viewport_width: 1920,
                viewport_height: 1080,
                lens_width: 400,
                lens_height: 300,
            },
            keyboard: OnScreenKeyboard {
                layout: KeyboardLayout::Qwerty,
                visible: false,
                keys: Vec::new(),
                shift_active: false,
                caps_lock: false,
                prediction_enabled: true,
                predictions: Vec::new(),
                swipe_typing: false,
                haptic_feedback: true,
                dwell_click_enabled: false,
                dwell_time_ms: 1000,
                scanning_enabled: false,
                key_size: 48,
            },
            switch_access: SwitchAccess {
                enabled: false,
                scan_mode: ScanMode::RowColumn,
                scan_speed_ms: 1000,
                auto_scan: true,
                current_group: 0,
                current_item: 0,
                groups: Vec::new(),
                switch_debounce_ms: 100,
                last_switch_time: 0,
                highlight_color: 0xFF00FF00,
            },
            eye_tracker: EyeTracker {
                enabled: false,
                calibrated: false,
                calibration_points: Vec::new(),
                current_gaze: GazePoint {
                    x: 0.0,
                    y: 0.0,
                    confidence: 0.0,
                    timestamp_ms: 0,
                },
                fixation_threshold_ms: 100,
                fixation_radius_px: 30.0,
                dwell_click_enabled: true,
                dwell_time_ms: 800,
                dwell_start: 0,
                dwell_point: GazePoint {
                    x: 0.0,
                    y: 0.0,
                    confidence: 0.0,
                    timestamp_ms: 0,
                },
                gaze_scroll_enabled: true,
                scroll_zone_height: 100,
                smoothing_factor: 0.3,
                history: Vec::new(),
                history_max: 30,
            },
            voice_control: VoiceControl {
                mode: VoiceControlMode::Command,
                commands: Vec::new(),
                numbered_overlay: NumberedOverlay {
                    entries: Vec::new(),
                    visible: false,
                    next_number: 1,
                },
                mouse_grid: MouseGrid {
                    active: false,
                    level: 0,
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                listening: false,
                last_phrase: String::new(),
                confidence_threshold: 0.7,
            },
            high_contrast: HighContrastTheme::None,
            contrast_colors: ContrastColors {
                background: 0xFF000000,
                foreground: 0xFFFFFFFF,
                hyperlink: 0xFF00FFFF,
                disabled: 0xFF00FF00,
                selected_background: 0xFF1AEBFF,
                selected_foreground: 0xFF000000,
                button_face: 0xFF000000,
                button_text: 0xFFFFFFFF,
            },
            animations_reduced: false,
            captions_enabled: false,
            sticky_keys: false,
            filter_keys: false,
            filter_keys_delay_ms: 300,
            bounce_keys: false,
            bounce_keys_delay_ms: 300,
            mouse_keys: false,
            cursor_size: 1,
            cursor_blink_rate_ms: 500,
        }
    }
}

static INIT: AtomicBool = AtomicBool::new(false);

pub static mut ACCESSIBILITY_SERVICE: AccessibilityService = AccessibilityService::uninit();

pub fn init() {
    if INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        ACCESSIBILITY_SERVICE.initialized = true;
        ACCESSIBILITY_SERVICE.tree = AccessibilityTree::new();
        ACCESSIBILITY_SERVICE.screen_reader = ScreenReader::new();
        ACCESSIBILITY_SERVICE.speech = SpeechSynthesizer::new();
        ACCESSIBILITY_SERVICE.braille = BrailleDisplay::new(40, 4);
        ACCESSIBILITY_SERVICE.magnifier = Magnifier::new();
        ACCESSIBILITY_SERVICE.keyboard = OnScreenKeyboard::new(KeyboardLayout::Qwerty);
        ACCESSIBILITY_SERVICE.switch_access = SwitchAccess::new();
        ACCESSIBILITY_SERVICE.eye_tracker = EyeTracker::new();
        ACCESSIBILITY_SERVICE.voice_control = VoiceControl::new(1920, 1080);
        ACCESSIBILITY_SERVICE.high_contrast = HighContrastTheme::None;
        ACCESSIBILITY_SERVICE.contrast_colors = ContrastColors::high_contrast_black();
    }
}

// ─── Helper trait for lowercase (no_std) ────────────────────────────────────

trait AsciiLowercase {
    fn to_ascii_lowercase(&self) -> String;
}

impl AsciiLowercase for str {
    fn to_ascii_lowercase(&self) -> String {
        let mut s = String::with_capacity(self.len());
        for c in self.chars() {
            if c.is_ascii_uppercase() {
                s.push((c as u8 + 32) as char);
            } else {
                s.push(c);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Rect geometry ──────────────────────────────────────────────────────
    #[test]
    fn rect_contains_point_is_half_open() {
        let r = Rect::new(10, 20, 100, 50); // x:[10,110) y:[20,70)
        assert!(r.contains_point(10, 20)); // top-left inclusive
        assert!(r.contains_point(109, 69)); // last inside pixel
        assert!(!r.contains_point(110, 20)); // right edge exclusive
        assert!(!r.contains_point(10, 70)); // bottom edge exclusive
        assert!(!r.contains_point(9, 20)); // left of
    }

    #[test]
    fn rect_intersects() {
        let a = Rect::new(0, 0, 100, 100);
        assert!(a.intersects(&Rect::new(50, 50, 100, 100))); // overlap
        assert!(!a.intersects(&Rect::new(100, 0, 10, 10))); // edge-adjacent = no overlap
        assert!(!a.intersects(&Rect::new(200, 200, 10, 10))); // disjoint
    }

    // ─── Role semantics ─────────────────────────────────────────────────────
    #[test]
    fn role_queries() {
        assert_eq!(Role::Heading3.heading_level(), Some(3));
        assert_eq!(Role::Button.heading_level(), None);
        assert!(Role::Button.is_interactive());
        assert!(Role::Link.is_interactive());
        assert!(!Role::StaticText.is_interactive());
        assert!(Role::Navigation.is_landmark());
        assert!(Role::Main.is_landmark());
        assert!(!Role::Button.is_landmark());
    }

    // ─── StateFlags bitset ──────────────────────────────────────────────────
    #[test]
    fn state_flags_set_ops() {
        let mut s = StateFlags::empty();
        assert!(!s.contains(StateFlags::FOCUSED));
        s.insert(StateFlags::FOCUSABLE);
        s.insert(StateFlags::FOCUSED);
        assert!(s.contains(StateFlags::FOCUSABLE));
        assert!(s.contains(StateFlags::FOCUSED));
        s.remove(StateFlags::FOCUSED);
        assert!(!s.contains(StateFlags::FOCUSED));
        assert!(s.contains(StateFlags::FOCUSABLE));
        s.toggle(StateFlags::CHECKED);
        assert!(s.contains(StateFlags::CHECKED));
        let u = StateFlags::FOCUSABLE.union(StateFlags::DISABLED);
        assert!(u.contains(StateFlags::FOCUSABLE) && u.contains(StateFlags::DISABLED));
    }

    // ─── Node visibility / focusability ─────────────────────────────────────
    #[test]
    fn node_visibility_and_focusability() {
        let mut n = AccessibilityNode::new(1, Role::Button, "OK");
        assert!(!n.is_visible()); // default zero-size bounds
        assert!(!n.is_focusable());
        n.bounds = Rect::new(0, 0, 80, 24);
        n.states.insert(StateFlags::FOCUSABLE);
        assert!(n.is_visible());
        assert!(n.is_focusable());
    }

    // ─── Tree construction + traversal ──────────────────────────────────────
    #[test]
    fn tree_root_is_application() {
        let t = AccessibilityTree::new();
        let root = t.get_node(0).unwrap();
        assert_eq!(root.role, Role::Application);
        assert_eq!(t.node_count(), 1);
    }

    #[test]
    fn tree_add_links_parent_and_child() {
        let mut t = AccessibilityTree::new();
        let a = t.add_node(0, Role::Group, "A");
        let b = t.add_node(0, Role::Group, "B");
        let a1 = t.add_node(a, Role::Button, "A1");
        assert_ne!(a, b); // ids are unique + monotonic
        assert_eq!(t.get_node(a1).unwrap().parent_id, Some(a));
        assert!(t.get_node(a).unwrap().children_ids.contains(&a1));
        assert_eq!(t.get_node(0).unwrap().children_ids, alloc::vec![a, b]);
    }

    #[test]
    fn tree_walk_is_preorder_depth_first() {
        let mut t = AccessibilityTree::new();
        let a = t.add_node(0, Role::Group, "A");
        let b = t.add_node(0, Role::Group, "B");
        let a1 = t.add_node(a, Role::Button, "A1");
        assert_eq!(t.walk_depth_first(0), alloc::vec![0, a, a1, b]);
    }

    #[test]
    fn tree_remove_takes_the_whole_subtree() {
        let mut t = AccessibilityTree::new();
        let a = t.add_node(0, Role::Group, "A");
        let a1 = t.add_node(a, Role::Button, "A1");
        let _a2 = t.add_node(a, Role::Button, "A2");
        let count_before = t.node_count();
        t.remove_node(a);
        assert!(t.get_node(a).is_none());
        assert!(t.get_node(a1).is_none()); // child gone too
        assert!(!t.get_node(0).unwrap().children_ids.contains(&a)); // unlinked from parent
        assert_eq!(t.node_count(), count_before - 3);
    }

    #[test]
    fn tree_focus_requires_focusable() {
        let mut t = AccessibilityTree::new();
        let btn = t.add_node(0, Role::Button, "OK");
        // Not focusable yet → set_focus is a no-op.
        t.set_focus(btn);
        assert!(t.focused_node().is_none());
        t.get_node_mut(btn)
            .unwrap()
            .states
            .insert(StateFlags::FOCUSABLE);
        t.set_focus(btn);
        assert_eq!(t.focused_node().map(|n| n.id), Some(btn));
        assert!(t
            .get_node(btn)
            .unwrap()
            .states
            .contains(StateFlags::FOCUSED));
    }

    #[test]
    fn tree_find_at_point_returns_visible_hit() {
        let mut t = AccessibilityTree::new();
        let btn = t.add_node(0, Role::Button, "OK");
        t.get_node_mut(btn).unwrap().bounds = Rect::new(10, 10, 100, 30);
        assert_eq!(t.find_at_point(50, 20), Some(btn));
        assert_eq!(t.find_at_point(500, 500), None); // miss
                                                     // Zero-size (invisible) nodes are never hit.
        let ghost = t.add_node(0, Role::Group, "ghost");
        assert!(t.get_node(ghost).unwrap().bounds.width == 0);
        assert_eq!(t.find_at_point(0, 0), None);
    }

    // ─── Braille translation ────────────────────────────────────────────────
    #[test]
    fn braille_maps_ascii_letters() {
        let d = BrailleDisplay::new(40, 0);
        assert_eq!(d.translate_to_braille("a"), alloc::vec![0b000001]);
        assert_eq!(
            d.translate_to_braille("abc"),
            alloc::vec![0b000001, 0b000011, 0b001001]
        );
        assert_eq!(d.translate_to_braille(" "), alloc::vec![0b000000]);
        // An unmapped char yields the full-cell fallback, never a panic.
        assert_eq!(d.translate_to_braille("@"), alloc::vec![0b111111]);
    }

    // ─── Magnifier zoom ─────────────────────────────────────────────────────
    #[test]
    fn magnifier_zoom_clamps_to_range() {
        let mut m = Magnifier::new();
        assert_eq!(m.zoom_level(), 2.0); // default
        m.set_zoom(100.0);
        assert_eq!(m.zoom_level(), 20.0); // clamped to max
        m.set_zoom(0.1);
        assert_eq!(m.zoom_level(), 1.0); // clamped to min
        m.set_zoom(2.0);
        m.zoom_in();
        assert_eq!(m.zoom_level(), 2.5); // 2.0 * 1.25
        for _ in 0..50 {
            m.zoom_in();
        }
        assert_eq!(m.zoom_level(), 20.0); // saturates, never exceeds max
        for _ in 0..50 {
            m.zoom_out();
        }
        assert_eq!(m.zoom_level(), 1.0); // saturates at min, never below
    }

    // ─── Screen-reader spoken descriptions ──────────────────────────────────
    #[test]
    fn screen_reader_describes_role_name_and_state() {
        let sr = ScreenReader::new();
        let btn = AccessibilityNode::new(1, Role::Button, "OK");
        assert_eq!(sr.describe_node(&btn), "button: OK");

        let mut cb = AccessibilityNode::new(2, Role::CheckBox, "Agree");
        cb.states.insert(StateFlags::CHECKED);
        assert_eq!(sr.describe_node(&cb), "check box: Agree, checked");

        let mut disabled = AccessibilityNode::new(3, Role::Button, "Submit");
        disabled.states.insert(StateFlags::DISABLED);
        assert_eq!(sr.describe_node(&disabled), "button: Submit, disabled");
    }
}
