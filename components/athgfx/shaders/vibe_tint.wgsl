// Vibe Mode background fragment shader — the customization-engine "visual
// personality" surface. Procedurally grades a diagonal two-color gradient that
// drifts with time, dithers to kill banding (the Apple-detail touch), and
// applies a vignette. Pure ALU (no texture) — exercises the math path of the
// WGSL->SPIR-V toolchain. (Concept §Customization / Vibe Mode + §Language Stack.)

struct VibeParams {
    color_a: vec4<f32>,  // gradient start (rgb)
    color_b: vec4<f32>,  // gradient end (rgb)
    time: f32,           // seconds, drives the drift
    vignette: f32,       // 0 = none, 1 = strong corner darkening
    _pad0: f32,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> vibe: VibeParams;

// Cheap hash for ordered-noise dithering.
fn hash21(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453);
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // Diagonal gradient parameter, gently animated.
    let t = clamp(uv.x * 0.6 + uv.y * 0.4 + sin(vibe.time) * 0.05, 0.0, 1.0);
    var col = mix(vibe.color_a.rgb, vibe.color_b.rgb, t);

    // 1/255 dither to break up gradient banding.
    let n = (hash21(uv * 512.0) - 0.5) * (1.0 / 255.0);
    col = col + vec3<f32>(n, n, n);

    // Radial vignette.
    let d = distance(uv, vec2<f32>(0.5, 0.5));
    let vig = 1.0 - smoothstep(0.4, 0.85, d) * vibe.vignette;

    return vec4<f32>(col * vig, 1.0);
}
