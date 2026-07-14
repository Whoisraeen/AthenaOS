// Separable Gaussian blur fragment shader — the "blur" effect the Concept names
// alongside glassmorphism. Run twice per frame: a horizontal pass (direction
// (1,0)) then a vertical pass (direction (0,1)); the two 1D passes compose into
// a full 2D Gaussian at O(2n) instead of O(n^2) taps. Feeds the frosted-glass
// backdrop and the menu/overlay blur. (Concept §RaeUI / §Language Stack.)

struct BlurParams {
    direction: vec2<f32>,  // (1,0) horizontal pass, (0,1) vertical pass
    radius: f32,           // blur reach, in texels
    _pad: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var<uniform> blur: BlurParams;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(src, 0));
    let step = blur.direction / dims * blur.radius;

    // Canonical 9-tap Gaussian (sigma ~ radius/3), weights sum to 1.
    let w0 = 0.227027;
    let w1 = 0.194594;
    let w2 = 0.121622;
    let w3 = 0.054054;
    let w4 = 0.016216;

    var col = textureSampleLevel(src, src_samp, uv, 0.0) * w0;
    col = col + textureSampleLevel(src, src_samp, uv + step * 1.0, 0.0) * w1;
    col = col + textureSampleLevel(src, src_samp, uv - step * 1.0, 0.0) * w1;
    col = col + textureSampleLevel(src, src_samp, uv + step * 2.0, 0.0) * w2;
    col = col + textureSampleLevel(src, src_samp, uv - step * 2.0, 0.0) * w2;
    col = col + textureSampleLevel(src, src_samp, uv + step * 3.0, 0.0) * w3;
    col = col + textureSampleLevel(src, src_samp, uv - step * 3.0, 0.0) * w3;
    col = col + textureSampleLevel(src, src_samp, uv + step * 4.0, 0.0) * w4;
    col = col + textureSampleLevel(src, src_samp, uv - step * 4.0, 0.0) * w4;
    return col;
}
