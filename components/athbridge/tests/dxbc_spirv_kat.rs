//! Host KAT for the DXBC -> SPIR-V translator (slice 1), GPU-free.
//!
//! Fixtures `tests/fixtures/*.dxbc` were compiled on the dev box with the
//! Windows SDK `fxc` (vs_5_0 / ps_5_0) from the two HLSL files alongside them:
//!   passthrough_vs.hlsl: float4 main(float4 pos:POSITION):SV_Position{return pos;}
//!   solidcolor_ps.hlsl:  float4 main():SV_Target{return float4(1,0,0,1);}
//!
//! Proof is structural (decode the emitted SPIR-V word stream and check the
//! contract) plus, when `spirv-val` is on PATH, external validation. The negative
//! cases prove the test can actually FAIL (truncated -> Err, unsupported opcode
//! -> Err), never a panic.

use athbridge::dxbc_spirv::{translate, ShaderError, SpirvBuiltIn, TranslateOpts};
use athgfx::ShaderStage;

const PASSTHROUGH_VS: &[u8] = include_bytes!("fixtures/passthrough_vs.dxbc");
const SOLIDCOLOR_PS: &[u8] = include_bytes!("fixtures/solidcolor_ps.dxbc");
// Slice-2 fixtures (fxc ps_5_0):
//   alu_ps:  dp3_sat + negate + mad + write-mask + temp + swizzle (lighting-ish)
//   alu2_ps: add/mul/div/min/max/dp2/dp4/sqrt/rsq/frc/round + abs modifier
//   rcp_ps:  rcp opcode (compiled /Od so fxc does not fuse it into div)
const ALU_PS: &[u8] = include_bytes!("fixtures/alu_ps.dxbc");
const ALU2_PS: &[u8] = include_bytes!("fixtures/alu2_ps.dxbc");
const RCP_PS: &[u8] = include_bytes!("fixtures/rcp_ps.dxbc");
const MUL_PS: &[u8] = include_bytes!("fixtures/mul_ps.dxbc");
//   exp_log_ps: exp2 (`exp`) + log2 (`log`) — float transcendentals
const EXP_LOG_PS: &[u8] = include_bytes!("fixtures/exp_log_ps.dxbc");
//   int_ps: iadd/and/or/xor/ishl/ushr/not/ishr/ineg — integer ALU (bitcast model)
const INT_PS: &[u8] = include_bytes!("fixtures/int_ps.dxbc");
//   conv_ps: ftoi (float->int) + itof (int->float) conversions
const CONV_PS: &[u8] = include_bytes!("fixtures/conv_ps.dxbc");
//   cmp_ps: ge/lt/eq/ne (-> uint mask) + movc (per-lane select)
const CMP_PS: &[u8] = include_bytes!("fixtures/cmp_ps.dxbc");
//   conv2_ps: ftou (float->uint) + utof (uint->float)
const CONV2_PS: &[u8] = include_bytes!("fixtures/conv2_ps.dxbc");
//   if_ps: structured if/else/endif (basic blocks)
const IF_PS: &[u8] = include_bytes!("fixtures/if_ps.dxbc");
//   loop_ps: loop/breakc/endloop (OpLoopMerge + back-edge)
const LOOP_PS: &[u8] = include_bytes!("fixtures/loop_ps.dxbc");
//   sincos_ps: sincos (two-dst; fxc splits into sin-only + cos-only, null dsts)
const SINCOS_PS: &[u8] = include_bytes!("fixtures/sincos_ps.dxbc");
//   tex_ps: Texture2D.Sample (image/sampler binding model)
const TEX_PS: &[u8] = include_bytes!("fixtures/tex_ps.dxbc");
//   tex_lod_ps: Texture2D.SampleLevel -> sample_l (explicit LOD)
const TEX_LOD_PS: &[u8] = include_bytes!("fixtures/tex_lod_ps.dxbc");
//   tex_array_ps: Texture2DArray.Sample -> Arrayed image + 3-component coord
const TEX_ARRAY_PS: &[u8] = include_bytes!("fixtures/tex_array_ps.dxbc");
//   tex_cube_ps: TextureCube.Sample -> Cube image + 3-component direction coord
const TEX_CUBE_PS: &[u8] = include_bytes!("fixtures/tex_cube_ps.dxbc");
//   discard_ps: clip()/alpha-test -> discard_nz -> structured OpKill
const DISCARD_PS: &[u8] = include_bytes!("fixtures/discard_ps.dxbc");
//   deriv_ps: ddx/ddy -> OpDPdx/OpDPdy (screen-space derivatives)
const DERIV_PS: &[u8] = include_bytes!("fixtures/deriv_ps.dxbc");
//   cbuffer_ps: cb0[n] read -> uniform block + OpAccessChain/OpLoad
const CBUFFER_PS: &[u8] = include_bytes!("fixtures/cbuffer_ps.dxbc");
//   transform_vs: canonical VS — position * MVP matrix (cbuffer) -> SV_Position
const TRANSFORM_VS: &[u8] = include_bytes!("fixtures/transform_vs.dxbc");
//   cbuffer_dyn_ps: cb0[r#] dynamic (register-indexed) read -> runtime OpAccessChain
const CBUFFER_DYN_PS: &[u8] = include_bytes!("fixtures/cbuffer_dyn_ps.dxbc");
//   samplecmp_ps: SampleCmp (shadow) -> depth image + OpImageSampleDrefImplicitLod
const SAMPLECMP_PS: &[u8] = include_bytes!("fixtures/samplecmp_ps.dxbc");
//   gather_ps: Texture2D.Gather -> gather4 -> OpImageGather
const GATHER_PS: &[u8] = include_bytes!("fixtures/gather_ps.dxbc");
//   ld_ps: Texture2D.Load -> ld -> OpImageFetch (+ Lod mip)
const LD_PS: &[u8] = include_bytes!("fixtures/ld_ps.dxbc");
//   ldms_ps: Texture2DMS.Load -> ld_ms -> OpImageFetch (+ Sample) on an MS image
const LDMS_PS: &[u8] = include_bytes!("fixtures/ldms_ps.dxbc");
//   intbit_ps: imin/imax/umin/umax + imad + bfrev/countbits/firstbit_{lo,hi,shi}
const INTBIT_PS: &[u8] = include_bytes!("fixtures/intbit_ps.dxbc");
//   bitfield_ps: ubfe/ibfe/bfi (per-lane bitfield extract/insert, scalar-decomposed)
const BITFIELD_PS: &[u8] = include_bytes!("fixtures/bitfield_ps.dxbc");
//   half_ps: f32tof16/f16tof32 (per-lane Pack/UnpackHalf2x16 half-precision conv)
const HALF_PS: &[u8] = include_bytes!("fixtures/half_ps.dxbc");

const SPIRV_MAGIC: u32 = 0x0723_0203;

// SPIR-V opcodes we assert against.
const OP_ENTRY_POINT: u16 = 15;
const OP_EXECUTION_MODE: u16 = 16;
const OP_DECORATE: u16 = 71;
const OP_CONSTANT_COMPOSITE: u16 = 44;
const OP_EXT_INST: u16 = 12;
const OP_VECTOR_SHUFFLE: u16 = 79;
const OP_F_NEGATE: u16 = 127;
const OP_F_ADD: u16 = 129;
const OP_F_MUL: u16 = 133;
const OP_F_DIV: u16 = 136;
const OP_DOT: u16 = 148;
const OP_BITCAST: u16 = 124;
const OP_I_ADD: u16 = 128;
const OP_I_MUL: u16 = 132;
const OP_BIT_FIELD_INSERT: u16 = 201;
const OP_BIT_FIELD_S_EXTRACT: u16 = 202;
const OP_BIT_FIELD_U_EXTRACT: u16 = 203;
const OP_BIT_REVERSE: u16 = 204;
const OP_BIT_COUNT: u16 = 205;
const OP_SHIFT_LEFT_LOGICAL: u16 = 196;
const OP_BITWISE_AND: u16 = 199;
const OP_CONVERT_F_TO_U: u16 = 109;
const OP_CONVERT_F_TO_S: u16 = 110;
const OP_CONVERT_S_TO_F: u16 = 111;
const OP_CONVERT_U_TO_F: u16 = 112;
const OP_SELECT: u16 = 169;
const OP_F_ORD_GREATER_THAN_EQUAL: u16 = 190;
const OP_SELECTION_MERGE: u16 = 247;
const OP_LOOP_MERGE: u16 = 246;
const OP_BRANCH_CONDITIONAL: u16 = 250;
const OP_KILL: u16 = 252;
const OP_DPDX: u16 = 207;
const OP_DPDY: u16 = 208;
const OP_TYPE_IMAGE: u16 = 25;
const OP_TYPE_STRUCT: u16 = 30;
const OP_ACCESS_CHAIN: u16 = 65;
const OP_IMAGE_SAMPLE_IMPLICIT_LOD: u16 = 87;
const OP_IMAGE_SAMPLE_EXPLICIT_LOD: u16 = 88;
const OP_IMAGE_SAMPLE_DREF_IMPLICIT_LOD: u16 = 89;
const OP_IMAGE_GATHER: u16 = 96;
const OP_IMAGE_FETCH: u16 = 95;
const OP_FUNCTION: u16 = 54;
const OP_RETURN: u16 = 253;
const OP_FUNCTION_END: u16 = 56;

// GLSL.std.450 ext-inst numbers asserted against (the OpExtInst 4th operand).
const GLSL_FABS: u32 = 4;
const GLSL_FCLAMP: u32 = 43;
const GLSL_FMA: u32 = 50;
const GLSL_INVERSE_SQRT: u32 = 32;
const GLSL_SQRT: u32 = 31;
const GLSL_EXP2: u32 = 29;
const GLSL_LOG2: u32 = 30;
const GLSL_SIN: u32 = 13;
const GLSL_COS: u32 = 14;
const GLSL_UMIN: u32 = 38;
const GLSL_SMIN: u32 = 39;
const GLSL_UMAX: u32 = 41;
const GLSL_SMAX: u32 = 42;
const GLSL_FIND_ILSB: u32 = 73;
const GLSL_FIND_SMSB: u32 = 74;
const GLSL_FIND_UMSB: u32 = 75;
const GLSL_PACK_HALF_2X16: u32 = 58;
const GLSL_UNPACK_HALF_2X16: u32 = 62;

const EXECMODEL_VERTEX: u32 = 0;
const EXECMODEL_FRAGMENT: u32 = 4;
const EXECMODE_ORIGIN_UPPER_LEFT: u32 = 7;
const DECOR_BUILTIN: u32 = 11;
const DECOR_LOCATION: u32 = 30;
const BUILTIN_POSITION: u32 = 0;

fn words(spirv: &[u8]) -> Vec<u32> {
    assert_eq!(spirv.len() % 4, 0, "SPIR-V must be a whole number of words");
    spirv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Iterate SPIR-V instructions after the 5-word header: yields (opcode, operands).
fn instructions(spirv: &[u8]) -> Vec<(u16, Vec<u32>)> {
    let w = words(spirv);
    let mut out = Vec::new();
    let mut i = 5usize;
    while i < w.len() {
        let wc = (w[i] >> 16) as usize;
        let op = (w[i] & 0xFFFF) as u16;
        assert!(wc >= 1, "malformed instruction word count at {i}");
        let operands = w[i + 1..(i + wc).min(w.len())].to_vec();
        out.push((op, operands));
        i += wc;
    }
    out
}

fn assert_header(spirv: &[u8]) -> u32 {
    let w = words(spirv);
    assert!(w.len() >= 5, "SPIR-V too short for a header");
    assert_eq!(w[0], SPIRV_MAGIC, "wrong SPIR-V magic");
    let bound = w[3];
    assert!(
        bound > 1,
        "bound must be > 1 (stub had bound == 1), got {bound}"
    );
    bound
}

fn has_op_with_first(instrs: &[(u16, Vec<u32>)], op: u16, first: u32) -> bool {
    instrs
        .iter()
        .any(|(o, ops)| *o == op && ops.first() == Some(&first))
}

fn has_decorate(instrs: &[(u16, Vec<u32>)], decor: u32, value: u32) -> bool {
    instrs.iter().any(|(o, ops)| {
        *o == OP_DECORATE && ops.get(1) == Some(&decor) && ops.get(2) == Some(&value)
    })
}

/// Count instructions of a given opcode.
fn count_op(instrs: &[(u16, Vec<u32>)], op: u16) -> usize {
    instrs.iter().filter(|(o, _)| *o == op).count()
}

/// Does an `OpExtInst` with the given GLSL.std.450 instruction number appear?
/// (Operands: result-type, result-id, set-id, instruction-number, args...)
fn has_ext_inst(instrs: &[(u16, Vec<u32>)], glsl_inst: u32) -> bool {
    instrs
        .iter()
        .any(|(o, ops)| *o == OP_EXT_INST && ops.get(3) == Some(&glsl_inst))
}

// ── spirv-val oracle (optional) ──────────────────────────────────────────────

fn try_spirv_val(spirv: &[u8], label: &str) {
    use std::io::Write;
    use std::process::Command;

    // Always run if the tool is present; force-require it with RAEEN_SPIRV_VAL=1.
    let require = std::env::var("RAEEN_SPIRV_VAL").ok().as_deref() == Some("1");

    let mut tmp = std::env::temp_dir();
    // Per-process unique temp path so concurrent test binaries / re-runs can
    // never race on a shared .spv (a flaky-FAIL source — CLAUDE.md rule 16).
    tmp.push(format!("athena_dxbc_kat_{label}_{}.spv", std::process::id()));
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

    let candidates = ["spirv-val", "spirv-val.exe"];
    let mut ran = false;
    for c in candidates {
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
        panic!("RAEEN_SPIRV_VAL=1 but spirv-val not found on PATH");
    }
    if !ran {
        eprintln!("[kat] spirv-val not on PATH; structural asserts only for {label}");
    }
}

// ── Positive cases ───────────────────────────────────────────────────────────

#[test]
fn dxbc_spirv_passthrough_vs() {
    let t = translate(PASSTHROUGH_VS, TranslateOpts::default()).expect("VS should translate");
    assert_eq!(t.stage, ShaderStage::Vertex);

    let bound = assert_header(&t.spirv);
    eprintln!("[kat] passthrough VS bound={bound}");

    let instrs = instructions(&t.spirv);

    assert!(
        has_op_with_first(&instrs, OP_ENTRY_POINT, EXECMODEL_VERTEX),
        "missing OpEntryPoint Vertex"
    );
    assert!(
        has_decorate(&instrs, DECOR_BUILTIN, BUILTIN_POSITION),
        "missing BuiltIn Position decoration (SV_Position)"
    );
    assert!(
        instrs.iter().any(|(o, _)| *o == OP_FUNCTION),
        "missing OpFunction"
    );
    assert!(
        instrs.iter().any(|(o, _)| *o == OP_RETURN),
        "missing OpReturn"
    );
    assert!(
        instrs.iter().any(|(o, _)| *o == OP_FUNCTION_END),
        "missing OpFunctionEnd"
    );

    // I/O map: SV_Position output as a builtin; POSITION input present.
    assert!(
        t.io.outputs
            .iter()
            .any(|b| b.builtin == Some(SpirvBuiltIn::Position)),
        "SignatureMap missing Position builtin output"
    );
    assert!(
        t.io.inputs.iter().any(|b| b.semantic == "POSITION"),
        "SignatureMap missing POSITION input"
    );

    try_spirv_val(&t.spirv, "passthrough_vs");

    eprintln!("dxbc_spirv_passthrough_vs -> PASS");
}

#[test]
fn dxbc_spirv_solidcolor_ps() {
    let t = translate(SOLIDCOLOR_PS, TranslateOpts::default()).expect("PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);

    let bound = assert_header(&t.spirv);
    eprintln!("[kat] solidcolor PS bound={bound}");

    let instrs = instructions(&t.spirv);

    assert!(
        has_op_with_first(&instrs, OP_ENTRY_POINT, EXECMODEL_FRAGMENT),
        "missing OpEntryPoint Fragment"
    );
    assert!(
        instrs
            .iter()
            .any(|(o, ops)| *o == OP_EXECUTION_MODE
                && ops.get(1) == Some(&EXECMODE_ORIGIN_UPPER_LEFT)),
        "missing OriginUpperLeft execution mode"
    );
    assert!(
        has_decorate(&instrs, DECOR_LOCATION, 0),
        "missing Location 0 output decoration (SV_Target)"
    );
    assert!(
        instrs.iter().any(|(o, _)| *o == OP_CONSTANT_COMPOSITE),
        "missing OpConstantComposite for the solid color"
    );
    assert!(
        instrs.iter().any(|(o, _)| *o == OP_RETURN),
        "missing OpReturn"
    );

    // SV_Target output -> Location 0 in the signature map.
    assert!(
        t.io.outputs
            .iter()
            .any(|b| b.semantic.eq_ignore_ascii_case("SV_Target") && b.location == 0),
        "SignatureMap missing SV_Target at Location 0"
    );

    try_spirv_val(&t.spirv, "solidcolor_ps");

    eprintln!("dxbc_spirv_solidcolor_ps -> PASS");
}

// ── Slice 2: ALU + swizzle/mask/modifier ─────────────────────────────────────

#[test]
fn dxbc_spirv_alu_lighting_ps() {
    // dp3_sat r0.x, v0.xyzx, -v1.xyzx
    // mad o0.xyz, v2.xyzx, r0.xxxx, l(0.05,..)
    // mov o0.w, l(1.0)
    let t = translate(ALU_PS, TranslateOpts::default()).expect("alu PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);

    let bound = assert_header(&t.spirv);
    eprintln!("[kat] alu lighting PS bound={bound}");

    let instrs = instructions(&t.spirv);

    // Swizzles (.xyzx, .xxxx) must lower to OpVectorShuffle.
    assert!(
        count_op(&instrs, OP_VECTOR_SHUFFLE) >= 2,
        "expected OpVectorShuffle for the source swizzles + masked write"
    );
    // Negate modifier on -v1 -> OpFNegate.
    assert!(
        count_op(&instrs, OP_F_NEGATE) >= 1,
        "missing OpFNegate for the negate source modifier"
    );
    // dp3 -> OpDot.
    assert!(count_op(&instrs, OP_DOT) >= 1, "missing OpDot for dp3");
    // _sat -> OpExtInst FClamp.
    assert!(
        has_ext_inst(&instrs, GLSL_FCLAMP),
        "missing FClamp for the _sat result modifier"
    );
    // mad -> OpExtInst Fma.
    assert!(
        has_ext_inst(&instrs, GLSL_FMA),
        "missing Fma for the mad instruction"
    );

    assert!(
        has_op_with_first(&instrs, OP_ENTRY_POINT, EXECMODEL_FRAGMENT),
        "missing OpEntryPoint Fragment"
    );

    try_spirv_val(&t.spirv, "alu_ps");
    eprintln!("dxbc_spirv_alu_lighting_ps -> PASS");
}

#[test]
fn dxbc_spirv_alu_coverage_ps() {
    // add/mul/div/min/max/dp2/dp4/sqrt/rsq/frc/round + abs modifier.
    let t = translate(ALU2_PS, TranslateOpts::default()).expect("alu2 PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);

    let bound = assert_header(&t.spirv);
    eprintln!("[kat] alu coverage PS bound={bound}");

    let instrs = instructions(&t.spirv);

    assert!(count_op(&instrs, OP_F_ADD) >= 1, "missing OpFAdd (add)");
    assert!(count_op(&instrs, OP_F_DIV) >= 1, "missing OpFDiv (div)");
    assert!(count_op(&instrs, OP_DOT) >= 2, "missing OpDot for dp2+dp4");
    assert!(has_ext_inst(&instrs, GLSL_SQRT), "missing Sqrt ext-inst");
    assert!(
        has_ext_inst(&instrs, GLSL_INVERSE_SQRT),
        "missing InverseSqrt (rsq) ext-inst"
    );
    assert!(
        has_ext_inst(&instrs, GLSL_FABS),
        "missing FAbs for the abs source modifier"
    );

    try_spirv_val(&t.spirv, "alu2_ps");
    eprintln!("dxbc_spirv_alu_coverage_ps -> PASS");
}

#[test]
fn dxbc_spirv_rcp_ps() {
    // rcp r0.xyz, v0.xxxx ; mov o0.xyz, r0 ; mov o0.w, l(1)
    let t = translate(RCP_PS, TranslateOpts::default()).expect("rcp PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);

    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);

    // rcp lowers to 1.0 / x -> OpFDiv with a constant numerator.
    assert!(count_op(&instrs, OP_F_DIV) >= 1, "missing OpFDiv for rcp");
    // .xyz write-mask on o0 -> a masked-write OpVectorShuffle (value vs old dest).
    assert!(
        count_op(&instrs, OP_VECTOR_SHUFFLE) >= 1,
        "missing OpVectorShuffle for swizzle / masked write"
    );

    try_spirv_val(&t.spirv, "rcp_ps");
    eprintln!("dxbc_spirv_rcp_ps -> PASS");
}

#[test]
fn dxbc_spirv_mul_ps() {
    // mul o0.xyzw, v0, v1 -> a full-width OpFMul, no write-mask merge needed.
    let t = translate(MUL_PS, TranslateOpts::default()).expect("mul PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(count_op(&instrs, OP_F_MUL) >= 1, "missing OpFMul (mul)");
    try_spirv_val(&t.spirv, "mul_ps");
    eprintln!("dxbc_spirv_mul_ps -> PASS");
}

#[test]
fn dxbc_spirv_exp_log_ps() {
    // exp2(v) -> SM `exp` -> OpExtInst Exp2; log2(v+1) -> SM `log` -> OpExtInst Log2.
    // Completes the float-transcendental ALU set (sqrt/rsq/rcp/frc/round already
    // covered); sincos (two-dst) is the remaining float transcendental.
    let t = translate(EXP_LOG_PS, TranslateOpts::default()).expect("exp/log PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        has_ext_inst(&instrs, GLSL_EXP2),
        "missing Exp2 ext-inst for the exp opcode"
    );
    assert!(
        has_ext_inst(&instrs, GLSL_LOG2),
        "missing Log2 ext-inst for the log opcode"
    );
    // The log source is `v + 1.0`, and the result is `e + l` -> at least two OpFAdd.
    assert!(
        count_op(&instrs, OP_F_ADD) >= 2,
        "missing OpFAdd (the +1.0 and the e+l sum)"
    );
    try_spirv_val(&t.spirv, "exp_log_ps");
    eprintln!("dxbc_spirv_exp_log_ps -> PASS");
}

#[test]
fn dxbc_spirv_int_alu_ps() {
    // Integer ALU (iadd/and/or/xor/ishl/ushr/not/ishr/ineg) lowered via the
    // bitcast model: each op bitcasts the float-vec4 register to int-vec4, applies
    // the SPIR-V int op, and bitcasts back. spirv-val is the real proof.
    let t = translate(INT_PS, TranslateOpts::default()).expect("int PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);

    assert!(count_op(&instrs, OP_I_ADD) >= 1, "missing OpIAdd (iadd)");
    assert!(
        count_op(&instrs, OP_BITWISE_AND) >= 1,
        "missing OpBitwiseAnd (and)"
    );
    assert!(
        count_op(&instrs, OP_SHIFT_LEFT_LOGICAL) >= 1,
        "missing OpShiftLeftLogical (ishl)"
    );
    // The float<->int bridge: every int op is wrapped in OpBitcast both ways, so
    // many bitcasts must appear (>=2 per int op).
    assert!(
        count_op(&instrs, OP_BITCAST) >= 4,
        "missing OpBitcast float<->int bridges for the integer ops"
    );

    try_spirv_val(&t.spirv, "int_ps");
    eprintln!("dxbc_spirv_int_alu_ps -> PASS");
}

#[test]
fn dxbc_spirv_conv_ps() {
    // ftoi -> OpConvertFToS, itof -> OpConvertSToF (both wrapped in the bitcast
    // bridge so the int bits live in the float-vec4 register). spirv-val proves it.
    let t = translate(CONV_PS, TranslateOpts::default()).expect("conv PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);

    assert!(
        count_op(&instrs, OP_CONVERT_F_TO_S) >= 1,
        "missing OpConvertFToS (ftoi)"
    );
    assert!(
        count_op(&instrs, OP_CONVERT_S_TO_F) >= 1,
        "missing OpConvertSToF (itof)"
    );

    try_spirv_val(&t.spirv, "conv_ps");
    eprintln!("dxbc_spirv_conv_ps -> PASS");
}

#[test]
fn dxbc_spirv_cmp_movc_ps() {
    // ge/lt/eq/ne lower to OpFOrd* -> OpSelect(mask) ; movc lowers to
    // OpINotEqual -> OpSelect. spirv-val is the real proof the typed plumbing
    // (bool vec4, int-mask consts, select) is well-formed.
    let t = translate(CMP_PS, TranslateOpts::default()).expect("cmp PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);

    assert!(
        count_op(&instrs, OP_F_ORD_GREATER_THAN_EQUAL) >= 1,
        "missing OpFOrdGreaterThanEqual (ge)"
    );
    // Every comparison's mask-materialization + every movc is an OpSelect, so
    // many must appear (4 comparisons + 4 movc in the fixture).
    assert!(
        count_op(&instrs, OP_SELECT) >= 5,
        "missing OpSelect for comparison masks + movc"
    );

    try_spirv_val(&t.spirv, "cmp_ps");
    eprintln!("dxbc_spirv_cmp_movc_ps -> PASS");
}

#[test]
fn dxbc_spirv_conv2_ps() {
    // ftou -> OpConvertFToU, utof -> OpConvertUToF (unsigned typed view).
    let t = translate(CONV2_PS, TranslateOpts::default()).expect("conv2 PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_CONVERT_F_TO_U) >= 1,
        "missing OpConvertFToU (ftou)"
    );
    assert!(
        count_op(&instrs, OP_CONVERT_U_TO_F) >= 1,
        "missing OpConvertUToF (utof)"
    );
    try_spirv_val(&t.spirv, "conv2_ps");
    eprintln!("dxbc_spirv_conv2_ps -> PASS");
}

#[test]
fn dxbc_spirv_if_else_ps() {
    // if_nz/else/endif -> structured SPIR-V blocks (OpSelectionMerge +
    // OpBranchConditional + the then/else/merge labels). spirv-val is the real
    // proof the basic blocks are well-formed (every block terminated, merge
    // reachable) — a malformed CFG fails it loudly.
    let t = translate(IF_PS, TranslateOpts::default()).expect("if PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_SELECTION_MERGE) >= 1,
        "missing OpSelectionMerge (structured if)"
    );
    assert!(
        count_op(&instrs, OP_BRANCH_CONDITIONAL) >= 1,
        "missing OpBranchConditional (if branch)"
    );
    try_spirv_val(&t.spirv, "if_ps");
    eprintln!("dxbc_spirv_if_else_ps -> PASS");
}

#[test]
fn dxbc_spirv_loop_ps() {
    // loop/breakc/endloop -> a structured SPIR-V loop: OpLoopMerge in the header,
    // a conditional break to the merge, and a back-edge from continue to header.
    // spirv-val proves the loop CFG is well-formed (merge + continue reachable,
    // single back-edge) — the real correctness gate for loops.
    let t = translate(LOOP_PS, TranslateOpts::default()).expect("loop PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_LOOP_MERGE) >= 1,
        "missing OpLoopMerge (structured loop)"
    );
    assert!(
        count_op(&instrs, OP_BRANCH_CONDITIONAL) >= 1,
        "missing OpBranchConditional (breakc)"
    );
    try_spirv_val(&t.spirv, "loop_ps");
    eprintln!("dxbc_spirv_loop_ps -> PASS");
}

#[test]
fn dxbc_spirv_sincos_ps() {
    // sincos (two-dst; null dsts) -> GLSL.std.450 Sin + Cos ext-insts.
    let t = translate(SINCOS_PS, TranslateOpts::default()).expect("sincos PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(has_ext_inst(&instrs, GLSL_SIN), "missing Sin ext-inst");
    assert!(has_ext_inst(&instrs, GLSL_COS), "missing Cos ext-inst");
    try_spirv_val(&t.spirv, "sincos_ps");
    eprintln!("dxbc_spirv_sincos_ps -> PASS");
}

#[test]
fn dxbc_spirv_texture_sample_ps() {
    // Texture2D.Sample -> OpTypeImage + a descriptor-bound image/sampler +
    // OpSampledImage + OpImageSampleImplicitLod. spirv-val proves the binding
    // model (image/sampler types, descriptor decorations) is well-formed.
    let t = translate(TEX_PS, TranslateOpts::default()).expect("texture PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_TYPE_IMAGE) >= 1,
        "missing OpTypeImage (Texture2D)"
    );
    assert!(
        count_op(&instrs, OP_IMAGE_SAMPLE_IMPLICIT_LOD) >= 1,
        "missing OpImageSampleImplicitLod (sample)"
    );
    try_spirv_val(&t.spirv, "tex_ps");
    eprintln!("dxbc_spirv_texture_sample_ps -> PASS");
}

#[test]
fn dxbc_spirv_texture_sample_lod_ps() {
    // Texture2D.SampleLevel -> sample_l -> OpImageSampleExplicitLod with the Lod
    // image operand. Reuses the binding model; spirv-val proves the explicit-LOD
    // operand encoding.
    let t = translate(TEX_LOD_PS, TranslateOpts::default()).expect("tex-lod PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_IMAGE_SAMPLE_EXPLICIT_LOD) >= 1,
        "missing OpImageSampleExplicitLod (sample_l)"
    );
    try_spirv_val(&t.spirv, "tex_lod_ps");
    eprintln!("dxbc_spirv_texture_sample_lod_ps -> PASS");
}

#[test]
fn dxbc_spirv_texture_array_sample_ps() {
    // Texture2DArray.Sample -> the translator reads dcl_resource_texture2darray
    // (resource dimension) and emits an Arrayed OpTypeImage + a 3-component
    // sample coord (uv + array slice). spirv-val is the authoritative proof: a
    // vec2 coord on an Arrayed image (the off-by-one if the dimension were
    // ignored) FAILS validation, so a clean pass proves coord width matches.
    let t =
        translate(TEX_ARRAY_PS, TranslateOpts::default()).expect("tex-array PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    // An OpTypeImage with the Arrayed operand (index 4) set to 1.
    assert!(
        instrs
            .iter()
            .any(|(o, ops)| *o == OP_TYPE_IMAGE && ops.get(4) == Some(&1)),
        "missing Arrayed OpTypeImage (Texture2DArray)"
    );
    assert!(
        count_op(&instrs, OP_IMAGE_SAMPLE_IMPLICIT_LOD) >= 1,
        "missing OpImageSampleImplicitLod (array sample)"
    );
    try_spirv_val(&t.spirv, "tex_array_ps");
    eprintln!("dxbc_spirv_texture_array_sample_ps -> PASS");
}

#[test]
fn dxbc_spirv_texture_cube_sample_ps() {
    // TextureCube.Sample -> the translator reads dcl_resource_texturecube and
    // emits a Cube OpTypeImage (Dim=Cube) + a 3-component direction coord.
    // spirv-val proves the cube image + coord are well-formed.
    let t = translate(TEX_CUBE_PS, TranslateOpts::default()).expect("tex-cube PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    // An OpTypeImage with the Dim operand (index 2) == Cube (3).
    assert!(
        instrs
            .iter()
            .any(|(o, ops)| *o == OP_TYPE_IMAGE && ops.get(2) == Some(&3)),
        "missing Cube OpTypeImage (TextureCube)"
    );
    assert!(
        count_op(&instrs, OP_IMAGE_SAMPLE_IMPLICIT_LOD) >= 1,
        "missing OpImageSampleImplicitLod (cube sample)"
    );
    try_spirv_val(&t.spirv, "tex_cube_ps");
    eprintln!("dxbc_spirv_texture_cube_sample_ps -> PASS");
}

#[test]
fn dxbc_spirv_discard_ps() {
    // clip()/alpha-test -> discard_nz -> a structured selection whose taken
    // branch is an OpKill block. High fan-out: clip() is in a large fraction of
    // real pixel shaders. spirv-val proves the OpKill block + merge are
    // well-formed (an unterminated/misplaced OpKill would FAIL).
    let t = translate(DISCARD_PS, TranslateOpts::default()).expect("discard PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(count_op(&instrs, OP_KILL) >= 1, "missing OpKill (discard)");
    // The discard is a structured selection (OpSelectionMerge guards the kill).
    assert!(
        count_op(&instrs, OP_SELECTION_MERGE) >= 1,
        "discard must be structured (OpSelectionMerge)"
    );
    try_spirv_val(&t.spirv, "discard_ps");
    eprintln!("dxbc_spirv_discard_ps -> PASS");
}

#[test]
fn dxbc_spirv_deriv_ps() {
    // ddx()/ddy() -> deriv_rtx_coarse/deriv_rty_coarse -> OpDPdx/OpDPdy (the
    // coarse/fine hint folds to the plain derivative, needing only the Shader
    // capability). Common in real PS (mip selection, screen-space AA).
    let t = translate(DERIV_PS, TranslateOpts::default()).expect("deriv PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(count_op(&instrs, OP_DPDX) >= 1, "missing OpDPdx (ddx)");
    assert!(count_op(&instrs, OP_DPDY) >= 1, "missing OpDPdy (ddy)");
    try_spirv_val(&t.spirv, "deriv_ps");
    eprintln!("dxbc_spirv_deriv_ps -> PASS");
}

#[test]
fn dxbc_spirv_cbuffer_ps() {
    // cb0[n] reads -> a uniform block (struct{ vec4[N] }, Block + ArrayStride 16 +
    // member offset 0) accessed via OpAccessChain/OpLoad. THE most common shader
    // input (matrices/material/light params); previously a cb operand failed to
    // translate at all. spirv-val is the authoritative proof — uniform-block
    // layout decorations are strict, so a missing/wrong ArrayStride/Offset/Block
    // FAILS validation.
    let t = translate(CBUFFER_PS, TranslateOpts::default()).expect("cbuffer PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_TYPE_STRUCT) >= 1,
        "missing OpTypeStruct (uniform block)"
    );
    assert!(
        count_op(&instrs, OP_ACCESS_CHAIN) >= 1,
        "missing OpAccessChain (cb read)"
    );
    // The block carries a Block decoration (decoration 2, no value operand).
    assert!(
        instrs
            .iter()
            .any(|(o, ops)| *o == OP_DECORATE && ops.get(1) == Some(&2) && ops.len() == 2),
        "missing Block decoration"
    );
    // spirv-val rigorously checks the uniform-block layout (ArrayStride/Offset).
    try_spirv_val(&t.spirv, "cbuffer_ps");
    eprintln!("dxbc_spirv_cbuffer_ps -> PASS");
}

#[test]
fn dxbc_spirv_transform_vs() {
    // THE canonical vertex shader: o.pos = mul(i.pos, mvp), the MVP matrix read
    // from a cbuffer. Every rendered object runs this. Validates that the cbuffer
    // + 4x dp4 matrix multiply + SV_Position output compose into a real,
    // spirv-val-clean VS — i.e. the translator handles realistic shaders, not
    // just isolated opcodes.
    let t =
        translate(TRANSFORM_VS, TranslateOpts::default()).expect("transform VS should translate");
    assert_eq!(t.stage, ShaderStage::Vertex);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    // 4 cbuffer rows read (OpAccessChain) and 4 dp4 (OpDot) for the matrix mul.
    assert!(
        count_op(&instrs, OP_ACCESS_CHAIN) >= 4,
        "expected 4 cb-row reads, got {}",
        count_op(&instrs, OP_ACCESS_CHAIN)
    );
    assert!(
        count_op(&instrs, OP_DOT) >= 4,
        "expected 4 dp4 (matrix mul), got {}",
        count_op(&instrs, OP_DOT)
    );
    // SV_Position output carries the BuiltIn Position decoration.
    assert!(
        has_decorate(&instrs, DECOR_BUILTIN, BUILTIN_POSITION),
        "missing BuiltIn Position (SV_Position)"
    );
    try_spirv_val(&t.spirv, "transform_vs");
    eprintln!("dxbc_spirv_transform_vs -> PASS");
}

#[test]
fn dxbc_spirv_cbuffer_dyn_ps() {
    // DYNAMIC cb index: colors[idx] with idx a runtime value (skinning/instancing
    // idiom). The cb element index is a register lane, so the OpAccessChain index
    // is a runtime SSA value (not a constant). spirv-val proves the runtime-indexed
    // access into the uniform-block array is well-formed.
    let t = translate(CBUFFER_DYN_PS, TranslateOpts::default())
        .expect("dynamic-cbuffer PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_TYPE_STRUCT) >= 1,
        "missing OpTypeStruct (uniform block)"
    );
    assert!(
        count_op(&instrs, OP_ACCESS_CHAIN) >= 1,
        "missing OpAccessChain (dynamic cb read)"
    );
    try_spirv_val(&t.spirv, "cbuffer_dyn_ps");
    eprintln!("dxbc_spirv_cbuffer_dyn_ps -> PASS");
}

#[test]
fn dxbc_spirv_samplecmp_ps() {
    // Texture2D.SampleCmp (shadow mapping) -> sample_c -> a 2D DEPTH OpTypeImage +
    // OpImageSampleDrefImplicitLod (scalar comparison result). In nearly every 3D
    // scene with shadows. spirv-val proves the Dref sample on a depth image is
    // well-formed.
    let t =
        translate(SAMPLECMP_PS, TranslateOpts::default()).expect("SampleCmp PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    // A depth OpTypeImage: the Depth operand (index 3) is 1.
    assert!(
        instrs
            .iter()
            .any(|(o, ops)| *o == OP_TYPE_IMAGE && ops.get(3) == Some(&1)),
        "missing depth OpTypeImage (Depth=1)"
    );
    assert!(
        count_op(&instrs, OP_IMAGE_SAMPLE_DREF_IMPLICIT_LOD) >= 1,
        "missing OpImageSampleDrefImplicitLod (sample_c)"
    );
    try_spirv_val(&t.spirv, "samplecmp_ps");
    eprintln!("dxbc_spirv_samplecmp_ps -> PASS");
}

#[test]
fn dxbc_spirv_gather_ps() {
    // Texture2D.Gather -> gather4 -> OpImageGather (the 4 bilinear texels of a
    // channel; PCF/SSAO). spirv-val proves the gather on a sampled image is
    // well-formed.
    let t = translate(GATHER_PS, TranslateOpts::default()).expect("gather PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_IMAGE_GATHER) >= 1,
        "missing OpImageGather (gather4)"
    );
    try_spirv_val(&t.spirv, "gather_ps");
    eprintln!("dxbc_spirv_gather_ps -> PASS");
}

#[test]
fn dxbc_spirv_ld_ps() {
    // Texture2D.Load -> ld -> OpImageFetch (integer coord + Lod mip, no sampler).
    // The post-processing / deferred-G-buffer texel-read idiom. spirv-val proves
    // the integer-coord fetch on a sampled image is well-formed.
    let t = translate(LD_PS, TranslateOpts::default()).expect("ld PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    assert!(
        count_op(&instrs, OP_IMAGE_FETCH) >= 1,
        "missing OpImageFetch (ld)"
    );
    try_spirv_val(&t.spirv, "ld_ps");
    eprintln!("dxbc_spirv_ld_ps -> PASS");
}

#[test]
fn dxbc_spirv_ldms_ps() {
    // Texture2DMS.Load -> ld_ms -> OpImageFetch with the Sample operand on a
    // multisampled (MS=1) OpTypeImage. The MSAA-resolve idiom.
    let t = translate(LDMS_PS, TranslateOpts::default()).expect("ld_ms PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);
    // A multisampled OpTypeImage: the MS operand (index 5) is 1.
    assert!(
        instrs
            .iter()
            .any(|(o, ops)| *o == OP_TYPE_IMAGE && ops.get(5) == Some(&1)),
        "missing multisampled OpTypeImage (MS=1)"
    );
    assert!(
        count_op(&instrs, OP_IMAGE_FETCH) >= 1,
        "missing OpImageFetch (ld_ms)"
    );
    try_spirv_val(&t.spirv, "ldms_ps");
    eprintln!("dxbc_spirv_ldms_ps -> PASS");
}

#[test]
fn dxbc_spirv_int_minmax_bit_ps() {
    // Integer min/max + multiply-add + SM5 bit-manipulation:
    //   imin/imax -> GLSL SMin/SMax, umin/umax -> UMin/UMax (unsigned typed view),
    //   imad -> OpIMul + OpIAdd (int lanes), bfrev -> OpBitReverse,
    //   countbits -> OpBitCount, firstbit_lo -> FindILsb, firstbit_hi ->
    //   FindUMsb (31 - x with the -1 sentinel preserved), firstbit_shi ->
    //   FindSMsb. spirv-val is the authoritative proof the signed/unsigned typed
    //   views + the ext-inst result types are well-formed.
    let t = translate(INTBIT_PS, TranslateOpts::default()).expect("intbit PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);

    assert!(has_ext_inst(&instrs, GLSL_SMIN), "missing SMin (imin)");
    assert!(has_ext_inst(&instrs, GLSL_SMAX), "missing SMax (imax)");
    assert!(has_ext_inst(&instrs, GLSL_UMIN), "missing UMin (umin)");
    assert!(has_ext_inst(&instrs, GLSL_UMAX), "missing UMax (umax)");
    assert!(
        has_ext_inst(&instrs, GLSL_FIND_ILSB),
        "missing FindILsb (firstbit_lo)"
    );
    assert!(
        has_ext_inst(&instrs, GLSL_FIND_UMSB),
        "missing FindUMsb (firstbit_hi)"
    );
    assert!(
        has_ext_inst(&instrs, GLSL_FIND_SMSB),
        "missing FindSMsb (firstbit_shi)"
    );
    assert!(count_op(&instrs, OP_I_MUL) >= 1, "missing OpIMul (imad)");
    assert!(
        count_op(&instrs, OP_BIT_REVERSE) >= 1,
        "missing OpBitReverse (bfrev)"
    );
    assert!(
        count_op(&instrs, OP_BIT_COUNT) >= 1,
        "missing OpBitCount (countbits)"
    );

    try_spirv_val(&t.spirv, "intbit_ps");
    eprintln!("dxbc_spirv_int_minmax_bit_ps -> PASS");
}

#[test]
fn dxbc_spirv_bitfield_ps() {
    // SM5 bitfield opcodes lowered to SPIR-V:
    //   ubfe -> OpBitFieldUExtract, ibfe -> OpBitFieldSExtract, bfi ->
    //   OpBitFieldInsert. SPIR-V requires the Offset/Count operands to be
    //   scalar integers, so the vec4 D3D ops are decomposed per-component and
    //   re-composited (OpCompositeExtract/Construct). D3D masks width/offset to
    //   the low 5 bits; the translator mirrors that with OpBitwiseAnd(_, 31).
    //   spirv-val is the authoritative proof the scalar decomposition + typed
    //   views are well-formed.
    let t = translate(BITFIELD_PS, TranslateOpts::default()).expect("bitfield PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);

    assert!(
        count_op(&instrs, OP_BIT_FIELD_U_EXTRACT) >= 4,
        "missing OpBitFieldUExtract per lane (ubfe)"
    );
    assert!(
        count_op(&instrs, OP_BIT_FIELD_S_EXTRACT) >= 4,
        "missing OpBitFieldSExtract per lane (ibfe)"
    );
    assert!(
        count_op(&instrs, OP_BIT_FIELD_INSERT) >= 4,
        "missing OpBitFieldInsert per lane (bfi)"
    );

    try_spirv_val(&t.spirv, "bitfield_ps");
    eprintln!("dxbc_spirv_bitfield_ps -> PASS");
}

#[test]
fn dxbc_spirv_half_ps() {
    // SM5 half-precision conversions lowered via GLSL.std.450:
    //   f32tof16 -> PackHalf2x16(vec2(lane, 0.0)) per lane (fp16 bits in low 16),
    //   f16tof32 -> UnpackHalf2x16(uint).x per lane. The vec4 D3D ops are
    //   decomposed to scalar Pack/Unpack (the ext-insts are scalar<->vec2), and
    //   re-composited. spirv-val is the authoritative proof the mixed
    //   uint/float/vec2 typed views + ext-inst result types are well-formed.
    let t = translate(HALF_PS, TranslateOpts::default()).expect("half PS should translate");
    assert_eq!(t.stage, ShaderStage::Fragment);
    let _bound = assert_header(&t.spirv);
    let instrs = instructions(&t.spirv);

    assert!(
        has_ext_inst(&instrs, GLSL_PACK_HALF_2X16),
        "missing PackHalf2x16 (f32tof16)"
    );
    assert!(
        has_ext_inst(&instrs, GLSL_UNPACK_HALF_2X16),
        "missing UnpackHalf2x16 (f16tof32)"
    );

    try_spirv_val(&t.spirv, "half_ps");
    eprintln!("dxbc_spirv_half_ps -> PASS");
}

#[test]
fn dxbc_spirv_unsupported_stage_clean_err() {
    // A geometry/compute/hull stage is out of scope -> clean Err, never panic.
    // Find the SHEX chunk via the container's chunk-offset table (not a byte
    // search, which would collide with the value 0x00000050 elsewhere) and flip
    // the program_type nibble in its version token. program_type 2 = geometry.
    let mut blob = ALU_PS.to_vec();
    let rd = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let chunk_count = rd(&blob, 28) as usize;
    const FOURCC_SHEX: u32 = 0x5845_4853; // "SHEX"
    let mut shex_body = None;
    for i in 0..chunk_count {
        let off = rd(&blob, 32 + i * 4) as usize;
        if rd(&blob, off) == FOURCC_SHEX {
            shex_body = Some(off + 8);
            break;
        }
    }
    let ver_off = shex_body.expect("SHEX chunk present");
    let ver = rd(&blob, ver_off);
    // program_type is the high 16 bits; set it to 2 (geometry).
    let patched = (ver & 0x0000_FFFF) | (2 << 16);
    blob[ver_off..ver_off + 4].copy_from_slice(&patched.to_le_bytes());

    let r = translate(&blob, TranslateOpts::default());
    assert!(
        matches!(r, Err(ShaderError::UnsupportedShaderModel(_))),
        "geometry stage must be UnsupportedShaderModel, got {r:?}"
    );
    eprintln!("dxbc_spirv_unsupported_stage_clean_err -> PASS");
}

// ── Negative cases (prove FAIL works, never panic) ───────────────────────────

#[test]
fn dxbc_spirv_rejects_truncated() {
    // Take a valid fixture and cut it short mid-container.
    let truncated = &PASSTHROUGH_VS[..40];
    let r = translate(truncated, TranslateOpts::default());
    assert!(
        matches!(r, Err(ShaderError::InvalidBytecode)),
        "truncated DXBC must return InvalidBytecode, got {r:?}"
    );

    // Empty + garbage inputs must also error, not panic.
    assert!(translate(&[], TranslateOpts::default()).is_err());
    assert!(translate(&[0u8; 16], TranslateOpts::default()).is_err());
    assert!(translate(b"NOPE", TranslateOpts::default()).is_err());

    eprintln!("dxbc_spirv_rejects_truncated -> PASS");
}

#[test]
fn dxbc_spirv_rejects_unsupported_opcode() {
    // Patch the passthrough VS SHEX so the `mov` opcode (54) becomes an
    // out-of-subset opcode (e.g. `add` = 0). We must locate the mov opcode word
    // in the SHEX chunk and rewrite its low 11 bits. The SHEX chunk's mov token
    // is 0x05000036; change opcode bits to a value not in the supported subset.
    let mut blob = PASSTHROUGH_VS.to_vec();
    let mov_token = 0x0500_0036u32.to_le_bytes();
    let pos = blob
        .windows(4)
        .position(|w| w == mov_token)
        .expect("mov token present in fixture");
    // New opcode 0 (ADD) keeps the same length field -> still parses, unsupported.
    let patched = 0x0500_0000u32.to_le_bytes();
    blob[pos..pos + 4].copy_from_slice(&patched);

    let r = translate(&blob, TranslateOpts::default());
    assert!(
        matches!(r, Err(ShaderError::UnsupportedInstruction(_))),
        "patched shader must return UnsupportedInstruction, got {r:?}"
    );

    eprintln!("dxbc_spirv_rejects_unsupported_opcode -> PASS");
}

#[test]
fn dxbc_spirv_self_test_passes() {
    // The embedded boot self-test (R10) must agree with the file fixture.
    assert!(athbridge::dxbc_spirv::run_self_test());
    assert!(athbridge::dxbc_spirv::self_test_bound() > 1);
    eprintln!("dxbc_spirv_self_test_passes -> PASS");
}
