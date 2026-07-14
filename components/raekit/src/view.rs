//! Declarative view tree — the core abstraction for RaeKit UIs.
//!
//! Every RaeKit app builds its interface by composing `ViewNode` values.
//! `ViewNode` is a pure data description of what should appear on screen;
//! the framework diffs and renders it against the compositor surface.

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

// ── Color ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn from_hex(hex: u32) -> Self {
        Self {
            a: ((hex >> 24) & 0xFF) as u8,
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
        }
    }

    pub const fn to_argb(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    // ── Named palette ────────────────────────────────────────────────
    pub const fn white() -> Self {
        Self::rgb(255, 255, 255)
    }
    pub const fn black() -> Self {
        Self::rgb(0, 0, 0)
    }
    pub const fn clear() -> Self {
        Self::rgba(0, 0, 0, 0)
    }
    pub const fn red() -> Self {
        Self::rgb(255, 59, 48)
    }
    pub const fn orange() -> Self {
        Self::rgb(255, 149, 0)
    }
    pub const fn yellow() -> Self {
        Self::rgb(255, 204, 0)
    }
    pub const fn green() -> Self {
        Self::rgb(52, 199, 89)
    }
    pub const fn blue() -> Self {
        Self::rgb(0, 122, 255)
    }
    pub const fn purple() -> Self {
        Self::rgb(175, 82, 222)
    }

    // ── RaeenOS semantic colors ──────────────────────────────────────
    pub const fn surface() -> Self {
        Self::from_hex(0xFF_1A_1A_22)
    }
    pub const fn surface_secondary() -> Self {
        Self::from_hex(0xFF_0A_0E_1A)
    }
    pub const fn accent() -> Self {
        Self::from_hex(0xFF_4E_9C_FF)
    }
    pub const fn text_primary() -> Self {
        Self::from_hex(0xFF_E0_E0_FF)
    }
    pub const fn text_secondary() -> Self {
        Self::from_hex(0xFF_99_99_BB)
    }
    pub const fn destructive() -> Self {
        Self::from_hex(0xFF_FF_2E_88)
    }
}

// ── Text style ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
    pub weight: FontWeight,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 14.0,
            color: Color::text_primary(),
            bold: false,
            italic: false,
            weight: FontWeight::Regular,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontWeight {
    Thin,
    Light,
    Regular,
    Medium,
    Semibold,
    Bold,
    Heavy,
    Black,
}

// ── Supporting enums ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Vertical,
    Horizontal,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSource {
    Asset(u64),
    Handle(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFit {
    Fill,
    Fit,
    Cover,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const fn all(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub const fn symmetric(h: f32, v: f32) -> Self {
        Self {
            top: v,
            right: h,
            bottom: v,
            left: h,
        }
    }

    pub const fn zero() -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    pub const fn top_only(v: f32) -> Self {
        Self {
            top: v,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    pub const fn bottom_only(v: f32) -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: v,
            left: 0.0,
        }
    }

    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    TopLeading,
    Top,
    TopTrailing,
    Leading,
    Center,
    Trailing,
    BottomLeading,
    Bottom,
    BottomTrailing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Destructive,
    Ghost,
}

// ── View trait ───────────────────────────────────────────────────────────

pub trait View {
    fn body(&self) -> ViewNode;
}

// ── ViewNode ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ViewNode {
    Empty,

    Text {
        content: String,
        style: TextStyle,
    },

    Rect {
        width: f32,
        height: f32,
        fill: Color,
        corner_radius: f32,
    },

    Image {
        source: ImageSource,
        fit: ImageFit,
        width: Option<f32>,
        height: Option<f32>,
    },

    Stack {
        direction: StackDirection,
        spacing: f32,
        alignment: Alignment,
        children: Vec<ViewNode>,
    },

    ZStack {
        alignment: Alignment,
        children: Vec<ViewNode>,
    },

    Button {
        label: Box<ViewNode>,
        action_id: u32,
        variant: ButtonVariant,
        disabled: bool,
    },

    List {
        items: Vec<ViewNode>,
        item_height: f32,
        separator: bool,
    },

    ScrollView {
        content: Box<ViewNode>,
        direction: ScrollDirection,
    },

    Spacer {
        min_size: f32,
    },

    Divider {
        thickness: f32,
        color: Color,
    },

    Toggle {
        is_on: bool,
        label: String,
        action_id: u32,
    },

    Slider {
        value: f32,
        range: (f32, f32),
        action_id: u32,
    },

    TextField {
        text: String,
        placeholder: String,
        action_id: u32,
    },

    ForEach {
        count: usize,
        builder_id: u32,
    },

    If {
        condition: bool,
        then_view: Box<ViewNode>,
        else_view: Option<Box<ViewNode>>,
    },

    Overlay {
        base: Box<ViewNode>,
        overlay: Box<ViewNode>,
    },

    Padding {
        edges: Edges,
        child: Box<ViewNode>,
    },

    Frame {
        width: Option<f32>,
        height: Option<f32>,
        alignment: Alignment,
        child: Box<ViewNode>,
    },

    Background {
        child: Box<ViewNode>,
        background: Box<ViewNode>,
    },

    NavigationView {
        title: String,
        content: Box<ViewNode>,
    },

    Sheet {
        is_presented: bool,
        content: Box<ViewNode>,
    },

    TabItem {
        label: String,
        icon_id: u32,
        content: Box<ViewNode>,
    },

    Group {
        children: Vec<ViewNode>,
    },
}

impl ViewNode {
    pub fn is_empty(&self) -> bool {
        matches!(self, ViewNode::Empty)
    }

    pub fn padding(self, insets: Edges) -> Self {
        ViewNode::Padding {
            edges: insets,
            child: Box::new(self),
        }
    }

    pub fn padding_all(self, v: f32) -> Self {
        self.padding(Edges::all(v))
    }

    pub fn frame(self, width: Option<f32>, height: Option<f32>) -> Self {
        ViewNode::Frame {
            width,
            height,
            alignment: Alignment::Center,
            child: Box::new(self),
        }
    }

    pub fn frame_aligned(
        self,
        width: Option<f32>,
        height: Option<f32>,
        alignment: Alignment,
    ) -> Self {
        ViewNode::Frame {
            width,
            height,
            alignment,
            child: Box::new(self),
        }
    }

    pub fn background(self, bg: ViewNode) -> Self {
        ViewNode::Background {
            child: Box::new(self),
            background: Box::new(bg),
        }
    }

    pub fn background_color(self, color: Color) -> Self {
        self.background(ViewNode::Rect {
            width: 0.0,
            height: 0.0,
            fill: color,
            corner_radius: 0.0,
        })
    }

    pub fn overlay(self, over: ViewNode) -> Self {
        ViewNode::Overlay {
            base: Box::new(self),
            overlay: Box::new(over),
        }
    }
}
