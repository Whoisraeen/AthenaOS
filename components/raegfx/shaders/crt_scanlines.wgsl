// CRT-scanlines theme effect — a retro user-selectable surface treatment that
// runs over a backdrop: barrel curvature, horizontal scanline darkening, and an
// aperture-grille RGB column mask. Runs on the Phase 6.2 WGSL->SPIR-V path.
// (Concept §Customization / theme engine + §Language Stack — extended.)

struct CrtParams {
    resolution: vec2<f32>,  // sets scanline/grille frequency
    scanline: f32,          // 0..1 scanline darkness
    mask: f32,              // 0..1 aperture-grille tint strength
    curvature: f32,         // barrel distortion amount
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var<uniform> crt: CrtParams;

fn barrel(uv: vec2<f32>, k: f32) -> vec2<f32> {
    let c = uv - vec2<f32>(0.5, 0.5);
    let r2 = dot(c, c);
    return uv + c * r2 * k;
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let duv = barrel(uv, crt.curvature);

    // Outside the curved tube -> black border.
    if (duv.x < 0.0 || duv.x > 1.0 || duv.y < 0.0 || duv.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    // Explicit-LOD fetch so the conditional control flow stays validator-clean.
    var col = textureSampleLevel(src, src_samp, duv, 0.0).rgb;

    // Horizontal scanlines.
    let sl = sin(duv.y * crt.resolution.y * 3.14159265) * 0.5 + 0.5;
    col = col * mix(1.0, sl, crt.scanline);

    // Aperture-grille: tint by pixel column (R/G/B stripes).
    let col_idx = u32(floor(duv.x * crt.resolution.x)) % 3u;
    var tint = vec3<f32>(1.0, 1.0, 1.0);
    if (col_idx == 0u) {
        tint = vec3<f32>(1.0, 0.7, 0.7);
    } else if (col_idx == 1u) {
        tint = vec3<f32>(0.7, 1.0, 0.7);
    } else {
        tint = vec3<f32>(0.7, 0.7, 1.0);
    }
    col = col * mix(vec3<f32>(1.0, 1.0, 1.0), tint, crt.mask);

    return vec4<f32>(col, 1.0);
}
