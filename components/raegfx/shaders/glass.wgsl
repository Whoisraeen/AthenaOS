// Glassmorphism fragment shader — the RaeenOS visual signature.
// Samples the composited backdrop behind a window, frosts it with a cheap
// separable-ish 9-tap blur, tints it toward the glass color, and adds a soft
// rim highlight at the surface edge. This is the "looks like Metal" frosted
// panel the Concept's UI identity is built on, expressed in WGSL and compiled
// to SPIR-V for the RaeGFX submit path. (Concept §RaeUI / §Language Stack.)

struct GlassParams {
    tint: vec4<f32>,     // rgb = glass color, a = frost strength (0..1)
    blur_radius: f32,    // backdrop blur reach, in texels
    rim: f32,            // edge highlight intensity
    _pad0: f32,
    _pad1: f32,
};

@group(0) @binding(0) var backdrop: texture_2d<f32>;
@group(0) @binding(1) var backdrop_samp: sampler;
@group(0) @binding(2) var<uniform> params: GlassParams;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(backdrop, 0));
    let texel = vec2<f32>(1.0, 1.0) / dims;

    var acc = vec3<f32>(0.0, 0.0, 0.0);
    var wsum = 0.0;
    // 3x3 distance-weighted tap kernel — frosted, allocation-free, uniform bounds
    // so the texture fetch stays uniformly controlled (validator-clean).
    for (var i = -1; i <= 1; i = i + 1) {
        for (var j = -1; j <= 1; j = j + 1) {
            let off = vec2<f32>(f32(i), f32(j)) * texel * params.blur_radius;
            let w = 1.0 / (1.0 + f32(i * i + j * j));
            acc = acc + textureSampleLevel(backdrop, backdrop_samp, uv + off, 0.0).rgb * w;
            wsum = wsum + w;
        }
    }
    let blurred = acc / wsum;

    // Frost: blend the blurred backdrop toward the glass tint.
    let frosted = mix(blurred, params.tint.rgb, params.tint.a);

    // Rim: brighten a thin band near the panel edge (uv square border).
    let edge_dist = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y));
    let edge = 1.0 - smoothstep(0.0, 0.08, edge_dist);
    let lit = frosted + vec3<f32>(edge * params.rim, edge * params.rim, edge * params.rim);

    return vec4<f32>(lit, 1.0);
}
