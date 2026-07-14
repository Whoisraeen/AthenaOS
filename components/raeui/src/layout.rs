//! RaeUI Flexbox Layout Engine
//!
//! CSS Flexbox-inspired layout system for RaeUI widgets. Resolves dimensions,
//! distributes space among flex children, handles main/cross axis alignment,
//! and recursively computes positions for the entire widget tree.

extern crate alloc;
use alloc::vec::Vec;

// ── Dimension Types ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dimension {
    Auto,
    Points(f32),
    Percent(f32),
}

impl Default for Dimension {
    fn default() -> Self {
        Dimension::Auto
    }
}

impl Dimension {
    pub fn resolve(self, parent: f32) -> Option<f32> {
        match self {
            Dimension::Auto => None,
            Dimension::Points(v) => Some(v),
            Dimension::Percent(p) => Some(parent * p / 100.0),
        }
    }
}

// ── Edge Insets ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const fn zero() -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    pub const fn uniform(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub const fn symmetric(vertical: f32, horizontal: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

// ── Enumerations ────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Flex,
    Block,
    None,
}

impl Default for Display {
    fn default() -> Self {
        Display::Flex
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    Column,
}

impl Default for FlexDirection {
    fn default() -> Self {
        FlexDirection::Row
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JustifyContent {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

impl Default for JustifyContent {
    fn default() -> Self {
        JustifyContent::Start
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignItems {
    Start,
    Center,
    End,
    Stretch,
}

impl Default for AlignItems {
    fn default() -> Self {
        AlignItems::Stretch
    }
}

// ── Layout Style ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct LayoutStyle {
    pub display: Display,
    pub flex_direction: FlexDirection,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Dimension,
    pub width: Dimension,
    pub height: Dimension,
    pub min_width: Dimension,
    pub max_width: Dimension,
    pub min_height: Dimension,
    pub max_height: Dimension,
    pub padding: Edges,
    pub margin: Edges,
    pub gap: f32,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Start,
            align_items: AlignItems::Stretch,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dimension::Auto,
            width: Dimension::Auto,
            height: Dimension::Auto,
            min_width: Dimension::Points(0.0),
            max_width: Dimension::Points(f32::MAX),
            min_height: Dimension::Points(0.0),
            max_height: Dimension::Points(f32::MAX),
            padding: Edges::zero(),
            margin: Edges::zero(),
            gap: 0.0,
        }
    }
}

// ── Computed Layout ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct ComputedLayout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

// ── Layout Node ─────────────────────────────────────────────────────────

pub struct LayoutNode {
    pub style: LayoutStyle,
    pub children: Vec<LayoutNode>,
    pub computed: ComputedLayout,
    /// Intrinsic content size (e.g. from text measurement). If set, used as
    /// the basis when flex_basis is Auto and no explicit width/height.
    pub intrinsic_width: Option<f32>,
    pub intrinsic_height: Option<f32>,
}

impl LayoutNode {
    pub fn new(style: LayoutStyle) -> Self {
        Self {
            style,
            children: Vec::new(),
            computed: ComputedLayout::default(),
            intrinsic_width: None,
            intrinsic_height: None,
        }
    }

    pub fn leaf(style: LayoutStyle, w: f32, h: f32) -> Self {
        Self {
            style,
            children: Vec::new(),
            computed: ComputedLayout::default(),
            intrinsic_width: Some(w),
            intrinsic_height: Some(h),
        }
    }

    pub fn add_child(&mut self, child: LayoutNode) {
        self.children.push(child);
    }
}

// ── Layout Computation ──────────────────────────────────────────────────

pub fn compute_layout(node: &mut LayoutNode, available_width: f32, available_height: f32) {
    compute_node(node, available_width, available_height);
}

fn compute_node(node: &mut LayoutNode, avail_w: f32, avail_h: f32) {
    if node.style.display == Display::None {
        node.computed = ComputedLayout::default();
        return;
    }

    let padding = &node.style.padding;
    let pad_h = padding.horizontal();
    let pad_v = padding.vertical();

    // Resolve own width/height
    let resolved_w = resolve_size(
        node.style.width,
        node.style.min_width,
        node.style.max_width,
        avail_w,
        node.intrinsic_width.map(|w| w + pad_h),
    );
    let resolved_h = resolve_size(
        node.style.height,
        node.style.min_height,
        node.style.max_height,
        avail_h,
        node.intrinsic_height.map(|h| h + pad_v),
    );

    node.computed.width = resolved_w;
    node.computed.height = resolved_h;

    if node.children.is_empty() || node.style.display == Display::Block {
        // Block layout: stack children vertically, full width
        if !node.children.is_empty() {
            let inner_w = resolved_w - pad_h;
            let mut cursor_y = padding.top;
            for child in &mut node.children {
                let child_margin = child.style.margin;
                cursor_y += child_margin.top;
                compute_node(child, inner_w - child_margin.horizontal(), avail_h);
                child.computed.x = padding.left + child_margin.left;
                child.computed.y = cursor_y;
                cursor_y += child.computed.height + child_margin.bottom;
            }
            // Auto-height: grow to fit children
            if node.style.height == Dimension::Auto {
                node.computed.height = clamp_size(
                    cursor_y + padding.bottom,
                    node.style.min_height,
                    node.style.max_height,
                    avail_h,
                );
            }
        }
        return;
    }

    // Flex layout
    let inner_w = resolved_w - pad_h;
    let inner_h = resolved_h - pad_v;

    let is_row = node.style.flex_direction == FlexDirection::Row;
    let main_size = if is_row { inner_w } else { inner_h };
    let cross_size = if is_row { inner_h } else { inner_w };

    let child_count = node.children.len();
    let total_gap = if child_count > 1 {
        node.style.gap * (child_count as f32 - 1.0)
    } else {
        0.0
    };
    let available_main = main_size - total_gap;

    // Phase 1: Determine hypothetical main sizes
    let mut hypo_mains: Vec<f32> = Vec::with_capacity(child_count);
    for child in &node.children {
        let basis = child_basis(child, is_row, available_main);
        hypo_mains.push(basis);
    }

    // Phase 2: Flex grow/shrink distribution
    let total_hypo: f32 = hypo_mains.iter().copied().sum();
    let free_space = available_main - total_hypo;

    let mut final_mains: Vec<f32> = Vec::with_capacity(child_count);
    if free_space > 0.0 {
        let total_grow: f32 = node.children.iter().map(|c| c.style.flex_grow).sum();
        if total_grow > 0.0 {
            for (i, child) in node.children.iter().enumerate() {
                let grow_share = (child.style.flex_grow / total_grow) * free_space;
                final_mains.push(hypo_mains[i] + grow_share);
            }
        } else {
            final_mains = hypo_mains.clone();
        }
    } else if free_space < 0.0 {
        let total_shrink: f32 = node
            .children
            .iter()
            .enumerate()
            .map(|(i, c)| c.style.flex_shrink * hypo_mains[i])
            .sum();
        if total_shrink > 0.0 {
            for (i, child) in node.children.iter().enumerate() {
                let shrink_factor = child.style.flex_shrink * hypo_mains[i] / total_shrink;
                let shrink_amount = shrink_factor * (-free_space);
                final_mains.push((hypo_mains[i] - shrink_amount).max(0.0));
            }
        } else {
            final_mains = hypo_mains.clone();
        }
    } else {
        final_mains = hypo_mains;
    }

    // Apply min/max constraints on main axis
    for (i, child) in node.children.iter().enumerate() {
        let (min_dim, max_dim) = if is_row {
            (child.style.min_width, child.style.max_width)
        } else {
            (child.style.min_height, child.style.max_height)
        };
        let min_v = min_dim.resolve(main_size).unwrap_or(0.0);
        let max_v = max_dim.resolve(main_size).unwrap_or(f32::MAX);
        final_mains[i] = final_mains[i].max(min_v).min(max_v);
    }

    // Phase 3: Recursively compute children with resolved sizes
    let mut cross_sizes: Vec<f32> = Vec::with_capacity(child_count);
    for (i, child) in node.children.iter_mut().enumerate() {
        let child_main = final_mains[i];
        let (cw, ch) = if is_row {
            (child_main, cross_size)
        } else {
            (cross_size, child_main)
        };
        let margin_h = child.style.margin.horizontal();
        let margin_v = child.style.margin.vertical();
        compute_node(child, cw - margin_h, ch - margin_v);

        // Override main-axis size from flex
        if is_row {
            child.computed.width = child_main - margin_h;
        } else {
            child.computed.height = child_main - margin_v;
        }

        let child_cross = if is_row {
            child.computed.height + child.style.margin.vertical()
        } else {
            child.computed.width + child.style.margin.horizontal()
        };
        cross_sizes.push(child_cross);
    }

    // Phase 4: Position children along the main axis (justify_content)
    let actual_main_used: f32 = final_mains.iter().copied().sum();
    let remaining = available_main - actual_main_used;

    let (mut main_cursor, between_gap) = compute_justification(
        node.style.justify_content,
        remaining,
        child_count,
        node.style.gap,
    );

    main_cursor += if is_row {
        node.style.padding.left
    } else {
        node.style.padding.top
    };

    for (i, child) in node.children.iter_mut().enumerate() {
        let child_margin_start = if is_row {
            child.style.margin.left
        } else {
            child.style.margin.top
        };
        let child_margin_end = if is_row {
            child.style.margin.right
        } else {
            child.style.margin.bottom
        };

        main_cursor += child_margin_start;

        // Cross-axis alignment
        let cross_start = if is_row {
            node.style.padding.top
        } else {
            node.style.padding.left
        };
        let child_cross_margin_start = if is_row {
            child.style.margin.top
        } else {
            child.style.margin.left
        };
        let child_cross_size = cross_sizes[i]
            - if is_row {
                child.style.margin.vertical()
            } else {
                child.style.margin.horizontal()
            };

        let cross_pos = compute_cross_position(
            node.style.align_items,
            cross_start,
            cross_size,
            child_cross_size,
            child_cross_margin_start,
        );

        if is_row {
            child.computed.x = main_cursor;
            child.computed.y = cross_pos;
            // Handle stretch
            if node.style.align_items == AlignItems::Stretch
                && child.style.height == Dimension::Auto
            {
                child.computed.height = cross_size - child.style.margin.vertical();
            }
        } else {
            child.computed.y = main_cursor;
            child.computed.x = cross_pos;
            if node.style.align_items == AlignItems::Stretch && child.style.width == Dimension::Auto
            {
                child.computed.width = cross_size - child.style.margin.horizontal();
            }
        }

        main_cursor += final_mains[i] - child_margin_start + child_margin_end + between_gap;
    }

    // Auto-size the container if needed
    if node.style.width == Dimension::Auto && is_row {
        let used = main_cursor + node.style.padding.right;
        node.computed.width = clamp_size(used, node.style.min_width, node.style.max_width, avail_w);
    }
    if node.style.height == Dimension::Auto && !is_row {
        let used = main_cursor + node.style.padding.bottom;
        node.computed.height =
            clamp_size(used, node.style.min_height, node.style.max_height, avail_h);
    }
    if node.style.height == Dimension::Auto && is_row {
        let max_cross: f32 = cross_sizes.iter().copied().fold(0.0f32, f32::max);
        let used = max_cross + node.style.padding.vertical();
        node.computed.height =
            clamp_size(used, node.style.min_height, node.style.max_height, avail_h);
    }
    if node.style.width == Dimension::Auto && !is_row {
        let max_cross: f32 = cross_sizes.iter().copied().fold(0.0f32, f32::max);
        let used = max_cross + node.style.padding.horizontal();
        node.computed.width = clamp_size(used, node.style.min_width, node.style.max_width, avail_w);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn child_basis(child: &LayoutNode, is_row: bool, available_main: f32) -> f32 {
    // flex_basis takes priority, then width/height, then intrinsic
    let basis_dim = child.style.flex_basis;
    if let Some(v) = basis_dim.resolve(available_main) {
        return v;
    }
    let size_dim = if is_row {
        child.style.width
    } else {
        child.style.height
    };
    if let Some(v) = size_dim.resolve(available_main) {
        return v;
    }
    let intrinsic = if is_row {
        child.intrinsic_width
    } else {
        child.intrinsic_height
    };
    let base = intrinsic.unwrap_or(0.0);
    base + child.style.padding.horizontal() * if is_row { 1.0 } else { 0.0 }
        + child.style.padding.vertical() * if is_row { 0.0 } else { 1.0 }
}

fn resolve_size(
    dim: Dimension,
    min: Dimension,
    max: Dimension,
    parent: f32,
    fallback: Option<f32>,
) -> f32 {
    let base = dim.resolve(parent).or(fallback).unwrap_or(parent);
    let min_v = min.resolve(parent).unwrap_or(0.0);
    let max_v = max.resolve(parent).unwrap_or(f32::MAX);
    base.max(min_v).min(max_v)
}

fn clamp_size(value: f32, min: Dimension, max: Dimension, parent: f32) -> f32 {
    let min_v = min.resolve(parent).unwrap_or(0.0);
    let max_v = max.resolve(parent).unwrap_or(f32::MAX);
    value.max(min_v).min(max_v)
}

fn compute_justification(
    justify: JustifyContent,
    remaining: f32,
    count: usize,
    gap: f32,
) -> (f32, f32) {
    if count == 0 {
        return (0.0, 0.0);
    }
    let remaining = remaining.max(0.0);
    match justify {
        JustifyContent::Start => (0.0, gap),
        JustifyContent::End => (remaining, gap),
        JustifyContent::Center => (remaining / 2.0, gap),
        JustifyContent::SpaceBetween => {
            if count <= 1 {
                (0.0, 0.0)
            } else {
                (0.0, remaining / (count as f32 - 1.0))
            }
        }
        JustifyContent::SpaceAround => {
            let space = remaining / (count as f32);
            (space / 2.0, space)
        }
        JustifyContent::SpaceEvenly => {
            let space = remaining / (count as f32 + 1.0);
            (space, space)
        }
    }
}

fn compute_cross_position(
    align: AlignItems,
    cross_start: f32,
    cross_size: f32,
    child_size: f32,
    child_margin_start: f32,
) -> f32 {
    match align {
        AlignItems::Start => cross_start + child_margin_start,
        AlignItems::End => cross_start + cross_size - child_size - child_margin_start,
        AlignItems::Center => cross_start + (cross_size - child_size) / 2.0,
        AlignItems::Stretch => cross_start + child_margin_start,
    }
}

// ── Convenience Builders ────────────────────────────────────────────────

impl LayoutStyle {
    pub fn row() -> Self {
        Self {
            flex_direction: FlexDirection::Row,
            ..Default::default()
        }
    }

    pub fn column() -> Self {
        Self {
            flex_direction: FlexDirection::Column,
            ..Default::default()
        }
    }

    pub fn with_padding(mut self, padding: Edges) -> Self {
        self.padding = padding;
        self
    }

    pub fn with_gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    pub fn with_justify(mut self, j: JustifyContent) -> Self {
        self.justify_content = j;
        self
    }

    pub fn with_align(mut self, a: AlignItems) -> Self {
        self.align_items = a;
        self
    }

    pub fn with_size(mut self, w: Dimension, h: Dimension) -> Self {
        self.width = w;
        self.height = h;
        self
    }

    pub fn with_flex_grow(mut self, g: f32) -> Self {
        self.flex_grow = g;
        self
    }

    pub fn with_margin(mut self, margin: Edges) -> Self {
        self.margin = margin;
        self
    }
}
