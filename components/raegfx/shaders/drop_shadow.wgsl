// Drop-shadow fragment shader — soft elevation shadow behind windows/popovers.
// Reads the source window's alpha (a coverage mask), offsets it toward the light
// direction, softens it with a 3x3 average, and emits the shadow color modulated
// by that coverage. The compositor draws this beneath the window to give the
// glass surfaces real depth. (Concept §RaeUI elevation / §Language Stack.)

struct ShadowParams {
    offset: vec2<f32>,   // shadow displacement, in texels (light direction)
    color: vec4<f32>,    // rgb = shadow color, a = max opacity
    softness: f32,       // edge blur reach, in texels
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var mask: texture_2d<f32>;  // window coverage in .a
@group(0) @binding(1) var mask_samp: sampler;
@group(0) @binding(2) var<uniform> sh: ShadowParams;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(mask, 0));
    let texel = vec2<f32>(1.0, 1.0) / dims;
    let base = uv - sh.offset * texel;

    var cover = 0.0;
    for (var i = -1; i <= 1; i = i + 1) {
        for (var j = -1; j <= 1; j = j + 1) {
            let o = vec2<f32>(f32(i), f32(j)) * texel * sh.softness;
            cover = cover + textureSampleLevel(mask, mask_samp, base + o, 0.0).a;
        }
    }
    cover = cover / 9.0;

    return vec4<f32>(sh.color.rgb, cover * sh.color.a);
}
