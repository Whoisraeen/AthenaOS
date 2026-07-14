//! Vector path tessellation — Concept §"AthUI: glassmorphic, GPU-accelerated".
//!
//! Rounded rectangles, strokes, and arbitrary UI paths must become triangle
//! meshes to hit the GPU (or the SW rasterizer). `lyon` is the pure-Rust
//! tessellator that does it; this module wraps the fill path the compositor and
//! AthUI use. Behind the `tessellate` feature so the bare Canvas/font path stays
//! lean.

extern crate alloc;
use alloc::vec::Vec;
use lyon::math::point;
use lyon::path::Path;
use lyon::tessellation::{BuffersBuilder, FillOptions, FillTessellator, FillVertex, VertexBuffers};

/// A tessellated mesh: positions + a triangle index list.
pub struct Mesh {
    pub vertices: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Tessellate the fill of an axis-aligned rectangle into a triangle mesh.
pub fn tessellate_rect(x: f32, y: f32, w: f32, h: f32) -> Mesh {
    let mut builder = Path::builder();
    builder.begin(point(x, y));
    builder.line_to(point(x + w, y));
    builder.line_to(point(x + w, y + h));
    builder.line_to(point(x, y + h));
    builder.end(true);
    let path = builder.build();

    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let mut tess = FillTessellator::new();
    let ok = tess
        .tessellate_path(
            &path,
            &FillOptions::default(),
            &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| {
                let p = v.position();
                [p.x, p.y]
            }),
        )
        .is_ok();
    if !ok {
        return Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
        };
    }
    Mesh {
        vertices: buffers.vertices,
        indices: buffers.indices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_tessellates_to_two_triangles() {
        let m = tessellate_rect(0.0, 0.0, 100.0, 50.0);
        // A filled rect is two triangles: 4 unique verts, 6 indices.
        assert_eq!(m.vertices.len(), 4, "verts={}", m.vertices.len());
        assert_eq!(m.indices.len(), 6, "indices={}", m.indices.len());
        assert_eq!(m.indices.len() % 3, 0);
        // Every index must point at a real vertex.
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
    }
}
