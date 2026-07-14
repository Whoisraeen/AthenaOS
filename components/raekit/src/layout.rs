//! Flexbox-style layout engine for RaeKit.
//!
//! Takes a `ViewNode` tree and calculates absolute positions and sizes
//! based on parent constraints and child preferences.

extern crate alloc;
use crate::view::{StackDirection, ViewNode};
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub node: ViewNode,
    pub frame: Rect,
    pub children: Vec<LayoutNode>,
}

pub struct Constraints {
    pub min_width: f32,
    pub max_width: f32,
    pub min_height: f32,
    pub max_height: f32,
}

impl Constraints {
    pub fn tight(w: f32, h: f32) -> Self {
        Self {
            min_width: w,
            max_width: w,
            min_height: h,
            max_height: h,
        }
    }

    pub fn loose(w: f32, h: f32) -> Self {
        Self {
            min_width: 0.0,
            max_width: w,
            min_height: 0.0,
            max_height: h,
        }
    }
}

pub fn compute_layout(node: &ViewNode, constraints: &Constraints) -> LayoutNode {
    match node {
        ViewNode::Empty => LayoutNode {
            node: ViewNode::Empty,
            frame: Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            children: Vec::new(),
        },
        ViewNode::Text { .. } => LayoutNode {
            node: node.clone(),
            frame: Rect {
                x: 0.0,
                y: 0.0,
                width: constraints.max_width.min(120.0),
                height: 20.0,
            },
            children: Vec::new(),
        },
        ViewNode::Rect { width, height, .. } => LayoutNode {
            node: node.clone(),
            frame: Rect {
                x: 0.0,
                y: 0.0,
                width: if *width > 0.0 {
                    *width
                } else {
                    constraints.max_width
                },
                height: if *height > 0.0 {
                    *height
                } else {
                    constraints.max_height
                },
            },
            children: Vec::new(),
        },
        ViewNode::Frame {
            width,
            height,
            alignment: _,
            child,
        } => {
            let child_w = width.unwrap_or(constraints.max_width);
            let child_h = height.unwrap_or(constraints.max_height);
            let tight_constraints = Constraints::loose(child_w, child_h);
            let mut child_layout = compute_layout(child, &tight_constraints);

            let frame_w = if let Some(w) = width {
                *w
            } else {
                child_layout.frame.width
            };
            let frame_h = if let Some(h) = height {
                *h
            } else {
                child_layout.frame.height
            };

            // Apply alignment here in a full implementation, for now center/top-left
            child_layout.frame.x = 0.0;
            child_layout.frame.y = 0.0;

            LayoutNode {
                node: node.clone(),
                frame: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: frame_w,
                    height: frame_h,
                },
                children: alloc::vec![child_layout],
            }
        }
        ViewNode::Padding { edges, child } => {
            let child_constraints = Constraints::loose(
                (constraints.max_width - edges.horizontal()).max(0.0),
                (constraints.max_height - edges.vertical()).max(0.0),
            );
            let mut child_layout = compute_layout(child, &child_constraints);
            child_layout.frame.x = edges.left;
            child_layout.frame.y = edges.top;

            LayoutNode {
                node: node.clone(),
                frame: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: child_layout.frame.width + edges.horizontal(),
                    height: child_layout.frame.height + edges.vertical(),
                },
                children: alloc::vec![child_layout],
            }
        }
        ViewNode::Background { child, background } => {
            let mut child_layout = compute_layout(child, constraints);
            let bg_constraints =
                Constraints::tight(child_layout.frame.width, child_layout.frame.height);
            let bg_layout = compute_layout(background, &bg_constraints);

            LayoutNode {
                node: node.clone(),
                frame: child_layout.frame.clone(),
                children: alloc::vec![bg_layout, child_layout],
            }
        }
        ViewNode::Overlay { base, overlay } => {
            let mut base_layout = compute_layout(base, constraints);
            let overlay_constraints =
                Constraints::tight(base_layout.frame.width, base_layout.frame.height);
            let overlay_layout = compute_layout(overlay, &overlay_constraints);

            LayoutNode {
                node: node.clone(),
                frame: base_layout.frame.clone(),
                children: alloc::vec![base_layout, overlay_layout],
            }
        }
        ViewNode::ZStack {
            children: node_children,
            ..
        } => {
            let mut children = Vec::new();
            let mut max_w = 0.0f32;
            let mut max_h = 0.0f32;

            for child_node in node_children {
                let child_layout = compute_layout(child_node, constraints);
                max_w = max_w.max(child_layout.frame.width);
                max_h = max_h.max(child_layout.frame.height);
                children.push(child_layout);
            }

            LayoutNode {
                node: node.clone(),
                frame: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: max_w,
                    height: max_h,
                },
                children,
            }
        }
        ViewNode::Spacer { min_size } => {
            LayoutNode {
                node: node.clone(),
                frame: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: constraints.max_width, // Expands fully
                    height: constraints.max_height,
                },
                children: Vec::new(),
            }
        }
        ViewNode::Stack {
            direction,
            spacing,
            children: node_children,
            ..
        } => {
            let mut children = Vec::new();
            let mut current_offset: f32 = 0.0;
            let mut max_cross: f32 = 0.0;

            for child_node in node_children {
                let child_constraints = match direction {
                    StackDirection::Vertical => Constraints::loose(
                        constraints.max_width,
                        constraints.max_height - current_offset,
                    ),
                    StackDirection::Horizontal => Constraints::loose(
                        constraints.max_width - current_offset,
                        constraints.max_height,
                    ),
                };

                let mut child_layout = compute_layout(child_node, &child_constraints);

                match direction {
                    StackDirection::Vertical => {
                        child_layout.frame.y = current_offset;
                        current_offset += child_layout.frame.height + spacing;
                        max_cross = if child_layout.frame.width > max_cross {
                            child_layout.frame.width
                        } else {
                            max_cross
                        };
                    }
                    StackDirection::Horizontal => {
                        child_layout.frame.x = current_offset;
                        current_offset += child_layout.frame.width + spacing;
                        max_cross = if child_layout.frame.height > max_cross {
                            child_layout.frame.height
                        } else {
                            max_cross
                        };
                    }
                }
                children.push(child_layout);
            }

            let (w, h) = match direction {
                StackDirection::Vertical => (max_cross, (current_offset - spacing).max(0.0)),
                StackDirection::Horizontal => ((current_offset - spacing).max(0.0), max_cross),
            };

            LayoutNode {
                node: node.clone(),
                frame: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: w,
                    height: h,
                },
                children,
            }
        }
        _ => LayoutNode {
            node: node.clone(),
            frame: Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            children: Vec::new(),
        },
    }
}
