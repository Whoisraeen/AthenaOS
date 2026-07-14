//! WGSL → SPIR-V shader toolchain (Concept §Language Stack — extended:
//! "WGSL → SPIR-V — the shader language for the theme engine and compositor
//! effects (glassmorphism, live wallpapers, Vibe Mode). Authored in WGSL,
//! compiled to SPIR-V for the RaeGFX submit path.").
//!
//! RaeGFX/RaeUI effect and theme shaders are authored in WGSL (see
//! `raegfx/shaders/*.wgsl`) and compiled here to SPIR-V words for the submit
//! path — this feeds Phase 6.3's "Loads SPIR-V shader on the live demo path".
//!
//! Like Skia/wgpu, the translator itself is a battle-tested library (naga, the
//! same shader compiler wgpu uses) rather than a from-scratch reimplementation;
//! this module is the thin RaeGFX-facing seam over it: parse → validate → emit,
//! returning a typed [`WgslError`] instead of panicking on malformed input.
//!
//! Gated behind the `wgsl` feature: it links naga (which needs `std`), so the
//! bare-metal kernel Canvas/font path never pulls it in. Userspace RaeGFX/RaeUI
//! turn it on. The emitted SPIR-V is round-trip-parseable by
//! [`crate::shader::SpirVModule`] and validates under `spirv-val`
//! (see `tests/wgsl_spirv_kat.rs`).

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::shader::ShaderStage;

/// A failure compiling WGSL to SPIR-V. `phase` localizes where it went wrong so
/// callers (and the shader author) get an actionable message rather than a panic.
#[derive(Debug, Clone)]
pub struct WgslError {
    /// "parse" | "validate" | "emit" | "entry".
    pub phase: &'static str,
    /// Human-readable diagnostic (naga's own rendered error where available).
    pub message: String,
}

impl core::fmt::Display for WgslError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "wgsl {}: {}", self.phase, self.message)
    }
}

fn to_naga_stage(stage: ShaderStage) -> naga::ShaderStage {
    match stage {
        ShaderStage::Vertex => naga::ShaderStage::Vertex,
        ShaderStage::Fragment => naga::ShaderStage::Fragment,
        ShaderStage::Compute => naga::ShaderStage::Compute,
    }
}

/// Compile a WGSL source string to SPIR-V words for the named entry point at the
/// given stage. Targets SPIR-V 1.0 (broad Vulkan compatibility — naga's default
/// `lang_version`).
///
/// Returns the SPIR-V as a `Vec<u32>` (logical words, host byte order). Use
/// [`compile_wgsl_bytes`] for the little-endian byte blob the submit path uploads.
pub fn compile_wgsl(
    source: &str,
    entry_point: &str,
    stage: ShaderStage,
) -> Result<Vec<u32>, WgslError> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| WgslError {
        phase: "parse",
        // naga renders a caret-annotated diagnostic against the source.
        message: e.emit_to_string(source),
    })?;

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| WgslError {
        phase: "validate",
        message: format!("{e:?}"),
    })?;

    // Confirm the requested entry point actually exists at the requested stage so
    // a typo fails here with a clear message rather than deep inside the backend.
    let has_entry = module
        .entry_points
        .iter()
        .any(|ep| ep.name == entry_point && ep.stage == to_naga_stage(stage));
    if !has_entry {
        return Err(WgslError {
            phase: "entry",
            message: format!(
                "no {:?} entry point named '{}' (have: {})",
                stage,
                entry_point,
                entry_point_list(&module)
            ),
        });
    }

    let options = naga::back::spv::Options::default();
    let pipeline = naga::back::spv::PipelineOptions {
        shader_stage: to_naga_stage(stage),
        entry_point: entry_point.to_string(),
    };

    naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline)).map_err(|e| WgslError {
        phase: "emit",
        message: format!("{e:?}"),
    })
}

/// Same as [`compile_wgsl`] but returns the little-endian SPIR-V byte blob that
/// the RaeGFX submit path / `vkCreateShaderModule` consumes.
pub fn compile_wgsl_bytes(
    source: &str,
    entry_point: &str,
    stage: ShaderStage,
) -> Result<Vec<u8>, WgslError> {
    let words = compile_wgsl(source, entry_point, stage)?;
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    Ok(bytes)
}

fn entry_point_list(module: &naga::Module) -> String {
    let mut s = String::new();
    for (i, ep) in module.entry_points.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("{}({:?})", ep.name, ep.stage));
    }
    s
}

// ═══════════════════════════════════════════════════════════════════════════
// Bundled RaeGFX effect / theme shaders (Concept §Language Stack — extended)
// ═══════════════════════════════════════════════════════════════════════════

/// The fullscreen-triangle vertex stage every compositor effect pass shares.
pub const FULLSCREEN_VS_WGSL: &str = include_str!("../shaders/fullscreen.wgsl");
/// Glassmorphism (frosted-glass backdrop) fragment shader — the UI signature.
pub const GLASS_FS_WGSL: &str = include_str!("../shaders/glass.wgsl");
/// Vibe Mode procedural background fragment shader.
pub const VIBE_TINT_FS_WGSL: &str = include_str!("../shaders/vibe_tint.wgsl");
/// Separable Gaussian blur fragment shader (run horizontal then vertical).
pub const BLUR_FS_WGSL: &str = include_str!("../shaders/blur.wgsl");
/// Drop-shadow (soft elevation shadow) fragment shader.
pub const DROP_SHADOW_FS_WGSL: &str = include_str!("../shaders/drop_shadow.wgsl");
/// Live wallpaper (animated aurora) fragment shader.
pub const LIVE_WALLPAPER_FS_WGSL: &str = include_str!("../shaders/live_wallpaper.wgsl");
/// Holographic-foil theme effect fragment shader.
pub const HOLOGRAPHIC_FS_WGSL: &str = include_str!("../shaders/holographic.wgsl");
/// CRT-scanlines theme effect fragment shader.
pub const CRT_SCANLINES_FS_WGSL: &str = include_str!("../shaders/crt_scanlines.wgsl");

/// A bundled effect shader: its source, entry point, and stage.
pub struct EffectShader {
    pub name: &'static str,
    pub source: &'static str,
    pub entry_point: &'static str,
    pub stage: ShaderStage,
}

/// The built-in RaeGFX/RaeUI effect + theme shaders, in compile order
/// (vertex stage first). Compiling all of these is the toolchain smoketest.
pub const EFFECT_SHADERS: &[EffectShader] = &[
    EffectShader {
        name: "fullscreen.vs",
        source: FULLSCREEN_VS_WGSL,
        entry_point: "vs_main",
        stage: ShaderStage::Vertex,
    },
    EffectShader {
        name: "glass.fs",
        source: GLASS_FS_WGSL,
        entry_point: "fs_main",
        stage: ShaderStage::Fragment,
    },
    EffectShader {
        name: "vibe_tint.fs",
        source: VIBE_TINT_FS_WGSL,
        entry_point: "fs_main",
        stage: ShaderStage::Fragment,
    },
    EffectShader {
        name: "blur.fs",
        source: BLUR_FS_WGSL,
        entry_point: "fs_main",
        stage: ShaderStage::Fragment,
    },
    EffectShader {
        name: "drop_shadow.fs",
        source: DROP_SHADOW_FS_WGSL,
        entry_point: "fs_main",
        stage: ShaderStage::Fragment,
    },
    EffectShader {
        name: "live_wallpaper.fs",
        source: LIVE_WALLPAPER_FS_WGSL,
        entry_point: "fs_main",
        stage: ShaderStage::Fragment,
    },
];

/// User-selectable theme-engine effect shaders (Concept §Customization). These
/// are surface treatments a theme/Vibe Mode applies — distinct from the always-on
/// compositor [`EFFECT_SHADERS`]. "Frosted glass" reuses [`GLASS_FS_WGSL`]; the
/// rest are the named theme effects. All run on this same WGSL→SPIR-V path.
pub const THEME_SHADERS: &[EffectShader] = &[
    EffectShader {
        name: "theme.frosted_glass.fs",
        source: GLASS_FS_WGSL,
        entry_point: "fs_main",
        stage: ShaderStage::Fragment,
    },
    EffectShader {
        name: "theme.holographic.fs",
        source: HOLOGRAPHIC_FS_WGSL,
        entry_point: "fs_main",
        stage: ShaderStage::Fragment,
    },
    EffectShader {
        name: "theme.crt_scanlines.fs",
        source: CRT_SCANLINES_FS_WGSL,
        entry_point: "fs_main",
        stage: ShaderStage::Fragment,
    },
];

/// Compile every bundled effect shader, returning `(name, spirv_words)` pairs.
/// The first error short-circuits with the offending shader's name prefixed so a
/// regression in any bundled shader is immediately attributable.
pub fn compile_effect_shaders() -> Result<Vec<(&'static str, Vec<u32>)>, WgslError> {
    compile_shader_set(EFFECT_SHADERS)
}

/// Compile every bundled theme-engine effect shader (the [`THEME_SHADERS`] set).
pub fn compile_theme_shaders() -> Result<Vec<(&'static str, Vec<u32>)>, WgslError> {
    compile_shader_set(THEME_SHADERS)
}

fn compile_shader_set(set: &[EffectShader]) -> Result<Vec<(&'static str, Vec<u32>)>, WgslError> {
    let mut out = Vec::with_capacity(set.len());
    for s in set {
        let words = compile_wgsl(s.source, s.entry_point, s.stage).map_err(|e| WgslError {
            phase: e.phase,
            message: format!("[{}] {}", s.name, e.message),
        })?;
        out.push((s.name, words));
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════
// Reflection — the shader's interface, for building a pipeline / descriptor layout
// ═══════════════════════════════════════════════════════════════════════════

/// What kind of resource a binding is, so the submit path can pick the matching
/// descriptor type when it builds the pipeline layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    UniformBuffer,
    StorageBuffer,
    Texture,
    Sampler,
    Other,
}

/// One `@group(g) @binding(b)` resource the shader declares.
#[derive(Debug, Clone)]
pub struct ResourceBindingInfo {
    pub group: u32,
    pub binding: u32,
    pub kind: BindingKind,
    pub name: Option<String>,
}

/// One entry point the module exposes (name + stage).
#[derive(Debug, Clone)]
pub struct EntryPointInfo {
    pub name: String,
    pub stage: ShaderStage,
}

/// The interface a WGSL module exposes — enough to build a `vkPipelineLayout`
/// (descriptor-set / binding layout) for the submit path without re-parsing.
#[derive(Debug, Clone)]
pub struct ShaderReflection {
    pub entry_points: Vec<EntryPointInfo>,
    /// Bound resources, sorted by `(group, binding)`.
    pub bindings: Vec<ResourceBindingInfo>,
}

fn from_naga_stage(s: naga::ShaderStage) -> ShaderStage {
    match s {
        naga::ShaderStage::Vertex => ShaderStage::Vertex,
        naga::ShaderStage::Fragment => ShaderStage::Fragment,
        naga::ShaderStage::Compute => ShaderStage::Compute,
    }
}

/// Parse + validate WGSL and report its interface (entry points + bound
/// resources) without emitting SPIR-V. The RaeGFX submit path uses this to build
/// the descriptor-set / pipeline layout that matches the shader's `@group/@binding`s.
pub fn reflect_wgsl(source: &str) -> Result<ShaderReflection, WgslError> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| WgslError {
        phase: "parse",
        message: e.emit_to_string(source),
    })?;
    // Validate so reflection only ever describes a well-formed module.
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| WgslError {
        phase: "validate",
        message: format!("{e:?}"),
    })?;

    let entry_points = module
        .entry_points
        .iter()
        .map(|ep| EntryPointInfo {
            name: ep.name.clone(),
            stage: from_naga_stage(ep.stage),
        })
        .collect();

    let mut bindings: Vec<ResourceBindingInfo> = Vec::new();
    for (_, gv) in module.global_variables.iter() {
        let Some(rb) = &gv.binding else { continue };
        let kind = match gv.space {
            naga::AddressSpace::Uniform => BindingKind::UniformBuffer,
            naga::AddressSpace::Storage { .. } => BindingKind::StorageBuffer,
            naga::AddressSpace::Handle => match &module.types[gv.ty].inner {
                naga::TypeInner::Image { .. } => BindingKind::Texture,
                naga::TypeInner::Sampler { .. } => BindingKind::Sampler,
                _ => BindingKind::Other,
            },
            _ => BindingKind::Other,
        };
        bindings.push(ResourceBindingInfo {
            group: rb.group,
            binding: rb.binding,
            kind,
            name: gv.name.clone(),
        });
    }
    bindings.sort_by_key(|b| (b.group, b.binding));

    Ok(ShaderReflection {
        entry_points,
        bindings,
    })
}
