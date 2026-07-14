// Fullscreen-triangle vertex shader for RaeGFX compositor effect passes.
// One draw of 3 vertices covers the whole render target with no vertex buffer:
// the clip-space positions and the matching [0,1] UVs are derived from the
// vertex index. Every glassmorphism / blur / Vibe-Mode fragment pass binds this
// as its vertex stage. (Concept §Language Stack — extended: effects authored in WGSL.)

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var out: VsOut;
    // vid 0 -> (0,0), 1 -> (2,0), 2 -> (0,2): a triangle that overdraws the quad.
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    out.uv = vec2<f32>(x, y);
    // Map UV [0,2] to clip [-1,3], flipping Y so (0,0) is top-left.
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}
