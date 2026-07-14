//! SwiftUI-style builder chain API.
//!
//! Each builder struct constructs a `ViewNode` via method chaining:
//!
//! ```ignore
//! VStack::new()
//!     .spacing(12.0)
//!     .child(Text::new("Hello").font_size(24.0).bold())
//!     .child(Button::label("OK").action(1))
//!     .padding(16.0)
//!     .build()
//! ```

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::view::*;

// ── Text ─────────────────────────────────────────────────────────────────

pub struct Text {
    content: String,
    style: TextStyle,
}

impl Text {
    pub fn new(s: &str) -> Self {
        Self {
            content: String::from(s),
            style: TextStyle::default(),
        }
    }

    pub fn font_size(mut self, size: f32) -> Self {
        self.style.size = size;
        self
    }

    pub fn color(mut self, c: Color) -> Self {
        self.style.color = c;
        self
    }

    pub fn bold(mut self) -> Self {
        self.style.bold = true;
        self.style.weight = FontWeight::Bold;
        self
    }

    pub fn italic(mut self) -> Self {
        self.style.italic = true;
        self
    }

    pub fn weight(mut self, w: FontWeight) -> Self {
        self.style.weight = w;
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Text {
            content: self.content,
            style: self.style,
        }
    }
}

impl From<Text> for ViewNode {
    fn from(t: Text) -> Self {
        t.build()
    }
}

// ── VStack ───────────────────────────────────────────────────────────────

pub struct VStack {
    spacing: f32,
    alignment: Alignment,
    children: Vec<ViewNode>,
}

impl VStack {
    pub fn new() -> Self {
        Self {
            spacing: 8.0,
            alignment: Alignment::Leading,
            children: Vec::new(),
        }
    }

    pub fn spacing(mut self, s: f32) -> Self {
        self.spacing = s;
        self
    }

    pub fn alignment(mut self, a: Alignment) -> Self {
        self.alignment = a;
        self
    }

    pub fn child(mut self, node: impl Into<ViewNode>) -> Self {
        self.children.push(node.into());
        self
    }

    pub fn children(mut self, nodes: Vec<ViewNode>) -> Self {
        self.children = nodes;
        self
    }

    pub fn padding(self, v: f32) -> PaddingBuilder {
        PaddingBuilder {
            edges: Edges::all(v),
            child: self.build(),
        }
    }

    pub fn background(self, color: Color) -> ViewNode {
        self.build().background_color(color)
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Stack {
            direction: StackDirection::Vertical,
            spacing: self.spacing,
            alignment: self.alignment,
            children: self.children,
        }
    }
}

impl From<VStack> for ViewNode {
    fn from(v: VStack) -> Self {
        v.build()
    }
}

// ── HStack ───────────────────────────────────────────────────────────────

pub struct HStack {
    spacing: f32,
    alignment: Alignment,
    children: Vec<ViewNode>,
}

impl HStack {
    pub fn new() -> Self {
        Self {
            spacing: 8.0,
            alignment: Alignment::Center,
            children: Vec::new(),
        }
    }

    pub fn spacing(mut self, s: f32) -> Self {
        self.spacing = s;
        self
    }

    pub fn alignment(mut self, a: Alignment) -> Self {
        self.alignment = a;
        self
    }

    pub fn child(mut self, node: impl Into<ViewNode>) -> Self {
        self.children.push(node.into());
        self
    }

    pub fn children(mut self, nodes: Vec<ViewNode>) -> Self {
        self.children = nodes;
        self
    }

    pub fn padding(self, v: f32) -> PaddingBuilder {
        PaddingBuilder {
            edges: Edges::all(v),
            child: self.build(),
        }
    }

    pub fn background(self, color: Color) -> ViewNode {
        self.build().background_color(color)
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Stack {
            direction: StackDirection::Horizontal,
            spacing: self.spacing,
            alignment: self.alignment,
            children: self.children,
        }
    }
}

impl From<HStack> for ViewNode {
    fn from(h: HStack) -> Self {
        h.build()
    }
}

// ── ZStackBuilder ────────────────────────────────────────────────────────

pub struct ZStackBuilder {
    alignment: Alignment,
    children: Vec<ViewNode>,
}

impl ZStackBuilder {
    pub fn new() -> Self {
        Self {
            alignment: Alignment::Center,
            children: Vec::new(),
        }
    }

    pub fn alignment(mut self, a: Alignment) -> Self {
        self.alignment = a;
        self
    }

    pub fn child(mut self, node: impl Into<ViewNode>) -> Self {
        self.children.push(node.into());
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::ZStack {
            alignment: self.alignment,
            children: self.children,
        }
    }
}

impl From<ZStackBuilder> for ViewNode {
    fn from(z: ZStackBuilder) -> Self {
        z.build()
    }
}

// ── ButtonBuilder ────────────────────────────────────────────────────────

pub struct ButtonBuilder {
    label_node: ViewNode,
    action_id: u32,
    variant: ButtonVariant,
    disabled: bool,
}

impl ButtonBuilder {
    pub fn new(label: &str) -> Self {
        Self {
            label_node: Text::new(label).color(Color::white()).build(),
            action_id: 0,
            variant: ButtonVariant::Primary,
            disabled: false,
        }
    }

    pub fn label_view(mut self, node: impl Into<ViewNode>) -> Self {
        self.label_node = node.into();
        self
    }

    pub fn action(mut self, id: u32) -> Self {
        self.action_id = id;
        self
    }

    pub fn style(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Button {
            label: Box::new(self.label_node),
            action_id: self.action_id,
            variant: self.variant,
            disabled: self.disabled,
        }
    }
}

impl From<ButtonBuilder> for ViewNode {
    fn from(b: ButtonBuilder) -> Self {
        b.build()
    }
}

// ── ImageBuilder ─────────────────────────────────────────────────────────

pub struct ImageBuilder {
    source: ImageSource,
    fit: ImageFit,
    width: Option<f32>,
    height: Option<f32>,
}

impl ImageBuilder {
    pub fn asset(id: u64) -> Self {
        Self {
            source: ImageSource::Asset(id),
            fit: ImageFit::Fit,
            width: None,
            height: None,
        }
    }

    pub fn handle(h: u64) -> Self {
        Self {
            source: ImageSource::Handle(h),
            fit: ImageFit::Fit,
            width: None,
            height: None,
        }
    }

    pub fn fit(mut self, f: ImageFit) -> Self {
        self.fit = f;
        self
    }

    pub fn size(mut self, w: f32, h: f32) -> Self {
        self.width = Some(w);
        self.height = Some(h);
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Image {
            source: self.source,
            fit: self.fit,
            width: self.width,
            height: self.height,
        }
    }
}

impl From<ImageBuilder> for ViewNode {
    fn from(i: ImageBuilder) -> Self {
        i.build()
    }
}

// ── ListBuilder ──────────────────────────────────────────────────────────

pub struct ListBuilder {
    items: Vec<ViewNode>,
    item_height: f32,
    separator: bool,
}

impl ListBuilder {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            item_height: 44.0,
            separator: true,
        }
    }

    pub fn item(mut self, node: impl Into<ViewNode>) -> Self {
        self.items.push(node.into());
        self
    }

    pub fn item_height(mut self, h: f32) -> Self {
        self.item_height = h;
        self
    }

    pub fn separator(mut self, show: bool) -> Self {
        self.separator = show;
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::List {
            items: self.items,
            item_height: self.item_height,
            separator: self.separator,
        }
    }
}

impl From<ListBuilder> for ViewNode {
    fn from(l: ListBuilder) -> Self {
        l.build()
    }
}

// ── ScrollViewBuilder ────────────────────────────────────────────────────

pub struct ScrollViewBuilder {
    content: ViewNode,
    direction: ScrollDirection,
}

impl ScrollViewBuilder {
    pub fn vertical(content: impl Into<ViewNode>) -> Self {
        Self {
            content: content.into(),
            direction: ScrollDirection::Vertical,
        }
    }

    pub fn horizontal(content: impl Into<ViewNode>) -> Self {
        Self {
            content: content.into(),
            direction: ScrollDirection::Horizontal,
        }
    }

    pub fn both(content: impl Into<ViewNode>) -> Self {
        Self {
            content: content.into(),
            direction: ScrollDirection::Both,
        }
    }

    pub fn build(self) -> ViewNode {
        ViewNode::ScrollView {
            content: Box::new(self.content),
            direction: self.direction,
        }
    }
}

impl From<ScrollViewBuilder> for ViewNode {
    fn from(s: ScrollViewBuilder) -> Self {
        s.build()
    }
}

// ── ToggleBuilder ────────────────────────────────────────────────────────

pub struct ToggleBuilder {
    label: String,
    is_on: bool,
    action_id: u32,
}

impl ToggleBuilder {
    pub fn new(label: &str, is_on: bool) -> Self {
        Self {
            label: String::from(label),
            is_on,
            action_id: 0,
        }
    }

    pub fn action(mut self, id: u32) -> Self {
        self.action_id = id;
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Toggle {
            is_on: self.is_on,
            label: self.label,
            action_id: self.action_id,
        }
    }
}

impl From<ToggleBuilder> for ViewNode {
    fn from(t: ToggleBuilder) -> Self {
        t.build()
    }
}

// ── SliderBuilder ────────────────────────────────────────────────────────

pub struct SliderBuilder {
    value: f32,
    min: f32,
    max: f32,
    action_id: u32,
}

impl SliderBuilder {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            min: 0.0,
            max: 1.0,
            action_id: 0,
        }
    }

    pub fn range(mut self, min: f32, max: f32) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn action(mut self, id: u32) -> Self {
        self.action_id = id;
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Slider {
            value: self.value,
            range: (self.min, self.max),
            action_id: self.action_id,
        }
    }
}

impl From<SliderBuilder> for ViewNode {
    fn from(s: SliderBuilder) -> Self {
        s.build()
    }
}

// ── TextFieldBuilder ─────────────────────────────────────────────────────

pub struct TextFieldBuilder {
    text: String,
    placeholder: String,
    action_id: u32,
}

impl TextFieldBuilder {
    pub fn new(text: &str) -> Self {
        Self {
            text: String::from(text),
            placeholder: String::new(),
            action_id: 0,
        }
    }

    pub fn placeholder(mut self, p: &str) -> Self {
        self.placeholder = String::from(p);
        self
    }

    pub fn action(mut self, id: u32) -> Self {
        self.action_id = id;
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::TextField {
            text: self.text,
            placeholder: self.placeholder,
            action_id: self.action_id,
        }
    }
}

impl From<TextFieldBuilder> for ViewNode {
    fn from(t: TextFieldBuilder) -> Self {
        t.build()
    }
}

// ── NavigationViewBuilder ────────────────────────────────────────────────

pub struct NavigationViewBuilder {
    title: String,
    content: ViewNode,
}

impl NavigationViewBuilder {
    pub fn new(title: &str, content: impl Into<ViewNode>) -> Self {
        Self {
            title: String::from(title),
            content: content.into(),
        }
    }

    pub fn build(self) -> ViewNode {
        ViewNode::NavigationView {
            title: self.title,
            content: Box::new(self.content),
        }
    }
}

impl From<NavigationViewBuilder> for ViewNode {
    fn from(n: NavigationViewBuilder) -> Self {
        n.build()
    }
}

// ── SheetBuilder ─────────────────────────────────────────────────────────

pub struct SheetBuilder {
    is_presented: bool,
    content: ViewNode,
}

impl SheetBuilder {
    pub fn new(presented: bool, content: impl Into<ViewNode>) -> Self {
        Self {
            is_presented: presented,
            content: content.into(),
        }
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Sheet {
            is_presented: self.is_presented,
            content: Box::new(self.content),
        }
    }
}

impl From<SheetBuilder> for ViewNode {
    fn from(s: SheetBuilder) -> Self {
        s.build()
    }
}

// ── Spacer / Divider convenience ─────────────────────────────────────────

pub struct Spacer;

impl Spacer {
    pub fn new() -> ViewNode {
        ViewNode::Spacer { min_size: 0.0 }
    }
    pub fn min(size: f32) -> ViewNode {
        ViewNode::Spacer { min_size: size }
    }
}

pub struct Divider;

impl Divider {
    pub fn new() -> ViewNode {
        ViewNode::Divider {
            thickness: 1.0,
            color: Color::accent(),
        }
    }

    pub fn thick(t: f32) -> ViewNode {
        ViewNode::Divider {
            thickness: t,
            color: Color::accent(),
        }
    }

    pub fn colored(t: f32, c: Color) -> ViewNode {
        ViewNode::Divider {
            thickness: t,
            color: c,
        }
    }
}

// ── Rect convenience ─────────────────────────────────────────────────────

pub struct RectBuilder {
    width: f32,
    height: f32,
    fill: Color,
    corner_radius: f32,
}

impl RectBuilder {
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            width: w,
            height: h,
            fill: Color::surface(),
            corner_radius: 0.0,
        }
    }

    pub fn fill(mut self, c: Color) -> Self {
        self.fill = c;
        self
    }

    pub fn corner_radius(mut self, r: f32) -> Self {
        self.corner_radius = r;
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::Rect {
            width: self.width,
            height: self.height,
            fill: self.fill,
            corner_radius: self.corner_radius,
        }
    }
}

impl From<RectBuilder> for ViewNode {
    fn from(r: RectBuilder) -> Self {
        r.build()
    }
}

// ── PaddingBuilder ───────────────────────────────────────────────────────

pub struct PaddingBuilder {
    pub(crate) edges: Edges,
    pub(crate) child: ViewNode,
}

impl PaddingBuilder {
    pub fn build(self) -> ViewNode {
        ViewNode::Padding {
            edges: self.edges,
            child: Box::new(self.child),
        }
    }

    pub fn background(self, color: Color) -> ViewNode {
        self.build().background_color(color)
    }
}

impl From<PaddingBuilder> for ViewNode {
    fn from(p: PaddingBuilder) -> Self {
        p.build()
    }
}

// ── Conditional builder ──────────────────────────────────────────────────

pub fn if_view(condition: bool, then_view: impl Into<ViewNode>) -> IfBuilder {
    IfBuilder {
        condition,
        then_view: then_view.into(),
        else_view: None,
    }
}

pub struct IfBuilder {
    condition: bool,
    then_view: ViewNode,
    else_view: Option<ViewNode>,
}

impl IfBuilder {
    pub fn else_view(mut self, v: impl Into<ViewNode>) -> Self {
        self.else_view = Some(v.into());
        self
    }

    pub fn build(self) -> ViewNode {
        ViewNode::If {
            condition: self.condition,
            then_view: Box::new(self.then_view),
            else_view: self.else_view.map(Box::new),
        }
    }
}

impl From<IfBuilder> for ViewNode {
    fn from(i: IfBuilder) -> Self {
        i.build()
    }
}
