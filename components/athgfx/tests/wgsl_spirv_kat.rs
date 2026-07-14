//! Host KAT for the WGSL -> SPIR-V toolchain (Phase 6.2), GPU-free.
//!
//! Proof ladder for MasterChecklist "WGSL → SPIR-V shader toolchain":
//!   1. Each bundled effect shader (`athgfx/shaders/*.wgsl`) compiles to SPIR-V.
//!   2. The emitted blob has the correct magic, and round-trips through the
//!      crate's own `SpirVModule::parse` with the expected entry-point stage —
//!      i.e. the two halves of the shader pipeline (emitter + the existing
//!      parser the submit path uses) agree.
//!   3. When `spirv-val` is available (Vulkan SDK on the dev box), the blob is
//!      externally validated — the authoritative oracle, same as the DXBC KAT.
//!
//! Negative cases prove the test can actually FAIL: malformed WGSL returns Err
//! (never panics), a wrong entry point returns Err, and a hand-corrupted SPIR-V
//! word is rejected by spirv-val (so the oracle gate is real, not decorative).
//!
//! Run: `cargo test -p athgfx --features wgsl`
//!      (force the oracle with `RAEEN_SPIRV_VAL=1`).
#![cfg(feature = "wgsl")]

use athgfx::shader::{ShaderStage, SpirVExecutionModel, SpirVModule};
use athgfx::wgsl::{
    compile_effect_shaders, compile_theme_shaders, compile_wgsl, compile_wgsl_bytes, reflect_wgsl,
    BindingKind, EFFECT_SHADERS, THEME_SHADERS,
};

const SPIRV_MAGIC: u32 = 0x0723_0203;

fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(words.len() * 4);
    for w in words {
        b.extend_from_slice(&w.to_le_bytes());
    }
    b
}

fn expected_model(stage: ShaderStage) -> SpirVExecutionModel {
    match stage {
        ShaderStage::Vertex => SpirVExecutionModel::Vertex,
        ShaderStage::Fragment => SpirVExecutionModel::Fragment,
        ShaderStage::Compute => SpirVExecutionModel::GLCompute,
    }
}

// ── spirv-val oracle (optional; mirrors the DXBC KAT pattern) ────────────────
fn try_spirv_val(spirv: &[u8], label: &str) {
    use std::io::Write;
    use std::process::Command;

    let require = std::env::var("RAEEN_SPIRV_VAL").ok().as_deref() == Some("1");

    let mut tmp = std::env::temp_dir();
    // Per-process unique name so concurrent test binaries never race on one .spv.
    tmp.push(format!("athena_wgsl_kat_{label}_{}.spv", std::process::id()));
    {
        let mut f = match std::fs::File::create(&tmp) {
            Ok(f) => f,
            Err(_) => {
                if require {
                    panic!("could not write temp SPIR-V for spirv-val");
                }
                return;
            }
        };
        f.write_all(spirv).expect("write spirv temp");
    }

    // Search PATH first, then the known Vulkan SDK install location on the dev box.
    let mut candidates: Vec<String> = vec!["spirv-val".into(), "spirv-val.exe".into()];
    if let Ok(sdk) = std::env::var("VULKAN_SDK") {
        candidates.push(format!("{sdk}/Bin/spirv-val.exe"));
        candidates.push(format!("{sdk}/Bin/spirv-val"));
    }
    candidates.push("/c/VulkanSDK/1.4.341.1/Bin/spirv-val.exe".into());

    let mut ran = false;
    for c in &candidates {
        match Command::new(c).arg(&tmp).output() {
            Ok(out) => {
                ran = true;
                let stderr = String::from_utf8_lossy(&out.stderr);
                assert!(
                    out.status.success(),
                    "spirv-val FAILED for {label}:\n{stderr}"
                );
                eprintln!("[kat] spirv-val OK for {label}");
                break;
            }
            Err(_) => continue,
        }
    }
    let _ = std::fs::remove_file(&tmp);
    if require && !ran {
        panic!("RAEEN_SPIRV_VAL=1 but spirv-val not found");
    }
    if !ran {
        eprintln!("[kat] spirv-val not found; structural asserts only for {label}");
    }
}

/// Every bundled effect shader compiles, has a valid SPIR-V header, round-trips
/// through the crate's own parser at the right stage, and validates externally.
#[test]
fn effect_shaders_compile_and_validate() {
    for s in EFFECT_SHADERS {
        let words = compile_wgsl(s.source, s.entry_point, s.stage)
            .unwrap_or_else(|e| panic!("compile {} failed: {e}", s.name));

        assert!(
            words.len() >= 5,
            "{}: SPIR-V too short for a header",
            s.name
        );
        assert_eq!(words[0], SPIRV_MAGIC, "{}: wrong SPIR-V magic", s.name);
        assert!(
            words[3] > 1,
            "{}: bound must be > 1, got {}",
            s.name,
            words[3]
        );

        // Round-trip through the parser the submit path actually uses.
        let bytes = words_to_bytes(&words);
        let parsed = SpirVModule::parse(&bytes)
            .unwrap_or_else(|| panic!("{}: SpirVModule::parse rejected our own output", s.name));
        let ep = parsed
            .entry_points
            .iter()
            .find(|ep| ep.name == s.entry_point)
            .unwrap_or_else(|| panic!("{}: entry '{}' missing in parse", s.name, s.entry_point));
        assert_eq!(
            ep.execution_model,
            expected_model(s.stage),
            "{}: execution model mismatch",
            s.name
        );

        try_spirv_val(&bytes, s.name);
    }
}

/// `compile_effect_shaders()` (the toolchain's batch entry) succeeds for all
/// bundled shaders — this is what a smoketest / submit-path warmup would call.
#[test]
fn compile_all_effect_shaders_ok() {
    let compiled = compile_effect_shaders().expect("batch compile of effect shaders");
    assert_eq!(compiled.len(), EFFECT_SHADERS.len());
    for (name, words) in compiled {
        assert_eq!(
            words[0], SPIRV_MAGIC,
            "{name}: bad magic from batch compile"
        );
    }
}

/// Every theme-engine effect shader (frosted glass / holographic / CRT scanlines)
/// compiles, round-trips through the parser, and validates under spirv-val —
/// proving the Concept "theme effects authored as WGSL" set runs on the 6.2 path.
#[test]
fn theme_shaders_compile_and_validate() {
    assert!(THEME_SHADERS.len() >= 3, "expect >= 3 theme effects");
    for s in THEME_SHADERS {
        let words = compile_wgsl(s.source, s.entry_point, s.stage)
            .unwrap_or_else(|e| panic!("compile {} failed: {e}", s.name));
        assert_eq!(words[0], SPIRV_MAGIC, "{}: wrong SPIR-V magic", s.name);

        let bytes = words_to_bytes(&words);
        let parsed = SpirVModule::parse(&bytes)
            .unwrap_or_else(|| panic!("{}: SpirVModule::parse rejected our own output", s.name));
        assert!(
            parsed
                .entry_points
                .iter()
                .any(|ep| ep.name == s.entry_point),
            "{}: entry '{}' missing in parse",
            s.name,
            s.entry_point
        );

        try_spirv_val(&bytes, s.name);
    }

    // The batch entry the theme engine would call also succeeds.
    let compiled = compile_theme_shaders().expect("batch compile of theme shaders");
    assert_eq!(compiled.len(), THEME_SHADERS.len());
}

/// The byte-blob helper produces a whole number of little-endian words matching
/// the word form (what `vkCreateShaderModule` ingests).
#[test]
fn bytes_helper_matches_words() {
    let s = &EFFECT_SHADERS[0];
    let words = compile_wgsl(s.source, s.entry_point, s.stage).unwrap();
    let bytes = compile_wgsl_bytes(s.source, s.entry_point, s.stage).unwrap();
    assert_eq!(bytes.len(), words.len() * 4);
    assert_eq!(bytes, words_to_bytes(&words));
}

// ── Reflection: the interface the submit path builds a pipeline layout from ──

/// glass.fs declares exactly texture@(0,0), sampler@(0,1), uniform@(0,2) and one
/// fragment entry point — what a `vkPipelineLayout` for it must match.
#[test]
fn reflect_reports_glass_bindings() {
    let r = reflect_wgsl(athgfx::wgsl::GLASS_FS_WGSL).expect("reflect glass.fs");

    // One fragment entry point.
    assert_eq!(r.entry_points.len(), 1);
    assert_eq!(r.entry_points[0].name, "fs_main");
    assert_eq!(r.entry_points[0].stage, ShaderStage::Fragment);

    // Three bindings, sorted, with the right kinds.
    assert_eq!(r.bindings.len(), 3, "glass.fs should declare 3 resources");
    let kinds: Vec<(u32, u32, BindingKind)> = r
        .bindings
        .iter()
        .map(|b| (b.group, b.binding, b.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (0, 0, BindingKind::Texture),
            (0, 1, BindingKind::Sampler),
            (0, 2, BindingKind::UniformBuffer),
        ]
    );
}

/// The fullscreen vertex shader binds no resources (a pipeline-layout no-op).
#[test]
fn reflect_fullscreen_has_no_bindings() {
    let r = reflect_wgsl(athgfx::wgsl::FULLSCREEN_VS_WGSL).expect("reflect fullscreen.vs");
    assert!(
        r.bindings.is_empty(),
        "fullscreen VS has no @group/@binding"
    );
    assert_eq!(r.entry_points.len(), 1);
    assert_eq!(r.entry_points[0].stage, ShaderStage::Vertex);
}

/// vibe_tint.fs declares a single uniform buffer.
#[test]
fn reflect_vibe_single_uniform() {
    let r = reflect_wgsl(athgfx::wgsl::VIBE_TINT_FS_WGSL).expect("reflect vibe_tint.fs");
    assert_eq!(r.bindings.len(), 1);
    assert_eq!(r.bindings[0].kind, BindingKind::UniformBuffer);
    assert_eq!((r.bindings[0].group, r.bindings[0].binding), (0, 0));
}

/// Reflection of malformed WGSL is an error, never a panic.
#[test]
fn reflect_malformed_is_error() {
    assert!(reflect_wgsl("not wgsl at all").is_err());
}

// ── Negative cases: the test must be able to FAIL ────────────────────────────

/// Malformed WGSL returns a parse error — never a panic.
#[test]
fn malformed_wgsl_is_error_not_panic() {
    let bad = "@fragment fn fs_main() -> @location(0) vec4<f32> { this is not wgsl }";
    let r = compile_wgsl(bad, "fs_main", ShaderStage::Fragment);
    assert!(r.is_err(), "garbage WGSL must not compile");
    assert_eq!(r.unwrap_err().phase, "parse");
}

/// A valid module but a wrong entry-point name fails cleanly at the entry check.
#[test]
fn wrong_entry_point_is_error() {
    let r = compile_wgsl(
        athgfx::wgsl::GLASS_FS_WGSL,
        "does_not_exist",
        ShaderStage::Fragment,
    );
    assert!(r.is_err(), "unknown entry point must fail");
    assert_eq!(r.unwrap_err().phase, "entry");
}

/// Asking for the wrong stage of an existing entry point also fails (glass.fs is
/// a fragment entry, not a vertex one).
#[test]
fn wrong_stage_is_error() {
    let r = compile_wgsl(athgfx::wgsl::GLASS_FS_WGSL, "fs_main", ShaderStage::Vertex);
    assert!(r.is_err(), "fragment entry requested as vertex must fail");
}

/// Proves the spirv-val oracle is real: corrupt a word in otherwise-valid SPIR-V
/// and confirm spirv-val rejects it. Skipped (not failed) when the tool is absent.
#[test]
fn corrupted_spirv_is_rejected_by_oracle() {
    use std::process::Command;

    // Locate spirv-val; if unavailable, this negative proof is a no-op.
    let mut candidates: Vec<String> = vec!["spirv-val".into(), "spirv-val.exe".into()];
    if let Ok(sdk) = std::env::var("VULKAN_SDK") {
        candidates.push(format!("{sdk}/Bin/spirv-val.exe"));
    }
    candidates.push("/c/VulkanSDK/1.4.341.1/Bin/spirv-val.exe".into());
    let tool = candidates
        .into_iter()
        .find(|c| Command::new(c).arg("--version").output().is_ok());
    let Some(tool) = tool else {
        eprintln!("[kat] spirv-val absent; skipping corruption negative-proof");
        return;
    };

    let s = &EFFECT_SHADERS[1]; // glass.fs
    let words = compile_wgsl(s.source, s.entry_point, s.stage).unwrap();
    let mut bytes = words_to_bytes(&words);
    // Smash a word deep in the instruction stream (past the 20-byte header) to a
    // bogus opcode/operand — must break validation without breaking the header.
    let off = bytes.len() / 2 & !3; // word-aligned, mid-module
    bytes[off] ^= 0xFF;
    bytes[off + 1] ^= 0xFF;

    let mut tmp = std::env::temp_dir();
    tmp.push(format!("athena_wgsl_kat_corrupt_{}.spv", std::process::id()));
    std::fs::write(&tmp, &bytes).unwrap();
    let out = Command::new(&tool).arg(&tmp).output().unwrap();
    let _ = std::fs::remove_file(&tmp);
    assert!(
        !out.status.success(),
        "spirv-val ACCEPTED corrupted SPIR-V — the oracle gate is not real"
    );
    eprintln!("[kat] spirv-val correctly rejected corrupted SPIR-V");
}
