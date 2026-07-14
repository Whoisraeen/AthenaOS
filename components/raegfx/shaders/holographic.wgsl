// Holographic-foil theme effect — a user-selectable Vibe Mode / theme-engine
// surface treatment. Angle/position-dependent iridescent rainbow that drifts
// with time, brightening toward the surface center like holographic foil. Pure
// ALU, runs on the Phase 6.2 WGSL->SPIR-V path. (Concept §Customization / theme
// engine + §Language Stack — extended.)

struct HoloParams {
    time: f32,       // seconds, drives the hue sweep
    intensity: f32,  // 0..1 overall iridescence strength (premultiplied alpha)
    scale: f32,      // spatial frequency of the rainbow bands
    _pad: f32,
};

@group(0) @binding(0) var<uniform> holo: HoloParams;

// Rainbow ramp from a hue scalar (0..1), no HSV conversion needed.
fn rainbow(h: f32) -> vec3<f32> {
    let r = abs(h * 6.0 - 3.0) - 1.0;
    let g = 2.0 - abs(h * 6.0 - 2.0);
    let b = 2.0 - abs(h * 6.0 - 4.0);
    return clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // Diagonal interference pattern + slow time drift -> a sweeping hue.
    let phase = fract((uv.x + uv.y) * holo.scale + holo.time * 0.1);
    let col = rainbow(phase);

    // Fresnel-ish brighten toward the vertical center band.
    let edge = pow(1.0 - abs(uv.y - 0.5) * 2.0, 2.0);
    let a = holo.intensity * mix(0.4, 1.0, edge);

    return vec4<f32>(col, a);
}
