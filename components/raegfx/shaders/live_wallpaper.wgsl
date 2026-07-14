// Live wallpaper fragment shader — an animated aurora the compositor can run as
// a GPU-accelerated background (and pause when fully occluded). Pure ALU, driven
// by a time uniform: a vertical gradient base with two drifting aurora ribbons
// in the accent color. This is the "live wallpapers GPU-accelerated" effect the
// Concept names. (Concept §Customization / live wallpapers + §Language Stack.)

struct WallpaperParams {
    base_a: vec4<f32>,  // gradient top (rgb)
    base_b: vec4<f32>,  // gradient bottom (rgb)
    accent: vec4<f32>,  // aurora ribbon color (rgb)
    time: f32,          // seconds, drives the drift
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> wp: WallpaperParams;

fn band(uv: vec2<f32>, t: f32, freq: f32, speed: f32) -> f32 {
    return sin(uv.x * freq + t * speed) * 0.5 + 0.5;
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let t = wp.time;

    // Vertical gradient base.
    var col = mix(wp.base_a.rgb, wp.base_b.rgb, uv.y);

    // Two drifting aurora ribbons in the accent color.
    let b1 = band(uv, t, 6.2831, 0.35);
    let b2 = band(uv + vec2<f32>(0.3, 0.0), t, 9.4248, -0.22);
    let glow = smoothstep(0.45, 1.0, b1) * smoothstep(0.40, 1.0, b2);
    let ribbon = exp(-abs(uv.y - (0.4 + 0.15 * b1)) * 8.0);
    col = mix(col, wp.accent.rgb, glow * ribbon);

    return vec4<f32>(col, 1.0);
}
