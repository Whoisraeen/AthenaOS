//! AthBridge DXBC -> SPIR-V shader translator (slice 1).
//!
//! Concept promise served (LEGACY_GAMING_CONCEPT.md, §Gaming-First / Compatibility):
//!   "DirectX 11/12 -> AthGFX translation at the driver level (DXVK/VKD3D-Proton
//!    lineage, but integrated and signed)" and
//!   "Steam works day one via AthBridge -- non-negotiable; without Steam there is
//!    no PC gaming OS."
//!
//! A Direct3D game cannot draw a single triangle on AthGFX/Vulkan until its
//! HLSL-compiled shaders (shipped as DXBC for D3D9/10/11) become SPIR-V. This
//! module is that translator. It is the FIRST real DirectX shader translation in
//! AthBridge -- everything before it was a 5-word SPIR-V header stub.
//!
//! ## The CPU-side vs GPU-gated boundary (load-bearing)
//! Translation is pure CPU-side compute, host-KAT-provable NOW: parse DXBC ->
//! decode the SM4/SM5 token stream -> emit a structurally-valid SPIR-V module.
//! The eventual GPU *submit* of that SPIR-V (`vkCreateShaderModule` /
//! `vkQueueSubmit` / scanout) is GPU-gated behind the owner's amdgpu bring-up and
//! is explicitly OUT of scope. The claim here is ONLY: "DXBC -> SPIR-V translation
//! produces valid SPIR-V for minimal shaders, host-proven." It does NOT render.
//!
//! ## Slice 1 scope
//! Exactly enough to translate two minimal shaders, host-proven against `fxc`-
//! compiled fixtures:
//!   1. passthrough vertex shader: `dcl_input v0`, `dcl_output_siv o0, position`,
//!      `mov o0, v0`, `ret`  -> SV_Position -> BuiltIn Position.
//!   2. solid-color pixel shader: `dcl_output o0`, `mov o0, l(r,g,b,a)`, `ret`
//!      -> SV_Target -> Location 0.
//! Instruction subset: `mov`, `ret`, `dcl_*`. Anything else returns a clean
//! `ShaderError::UnsupportedInstruction` -- never a panic, never garbage SPIR-V.
//!
//! ## Slice 2 scope (this file)
//! Adds the float-ALU + swizzle/mask/modifier subset that nearly every real
//! shader needs, still pure CPU and host-proven against `fxc`-compiled fixtures
//! (`alu_ps`, `alu2_ps`):
//!   * Source-operand swizzle (`.xyzw`/`.xxxx`/...) -> `OpVectorShuffle` (or a
//!     broadcast `OpCompositeExtract`+construct for scalar selects).
//!   * Dest write-mask (`.xy`/`.w`/...) -> load dest, overwrite only the masked
//!     lanes via `OpVectorShuffle`, store back (the standard masked-write idiom).
//!   * Source modifiers: negate (`-`) -> `OpFNegate`, absolute (`abs`/`|..|`) ->
//!     `OpExtInst FAbs`, and the instruction `_sat` result modifier -> clamp 0..1
//!     (`OpExtInst FClamp`).
//!   * Float ALU (lane-wise over the result vec4): `add` (0x00), `mul` (0x38),
//!     `mad` (0x32 -> `OpExtInst Fma`), `div` (0x0e), `min` (0x34), `max` (0x33),
//!     `dp2`/`dp3`/`dp4` (0x0f/0x10/0x11 -> `OpDot` over 2/3/4 lanes, broadcast),
//!     `sqrt` (0x4b), `rsq` (0x44 -> `InverseSqrt`), `rcp` (reciprocal),
//!     `frc` (0x1a -> `Fract`), `round_ne/_ni/_pi/_z` (0x40-0x43), `mov` (0x36).
//!   * Temp registers `r#` as Function-storage vec4 OpVariables, load/store with
//!     swizzle/mask.
//! Anything outside this set (control flow, texture sampling, integer ALU,
//! geometry/compute stages) returns a clean `ShaderError::UnsupportedInstruction`
//! / `UnsupportedShaderModel` -- never a panic, never garbage SPIR-V. It still
//! does NOT render: GPU submit stays gated behind amdgpu bring-up.
//!
//! ## Security
//! Input DXBC is untrusted attacker-controlled (it ships inside game files). Every
//! token read is bounds-checked; on any malformed token the translator returns an
//! `Err`, never panics, never reads out of bounds. (Matches the SEH engine's
//! "no OOB panic on hostile bytes" bar.)
//!
//! SSA model harvested from DXVK (`doitsujin/dxvk`, zlib; see
//! docs/OSS_RECOMMENDATIONS.md): a DXBC temp `r#` is a Function-storage 4-vector,
//! `mov` lowers to load+store/shuffle. Re-expressed in no_std Rust (no C++
//! transplant).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// Re-export the shared shader error type so callers/KATs can name it through the
// translator module (it is defined once in `dxgi`, the single error authority).
pub use crate::dxgi::ShaderError;

const DXBC_MAGIC: u32 = 0x4342_5844; // "DXBC"
const FOURCC_SHEX: u32 = 0x5845_4853; // "SHEX"
const FOURCC_SHDR: u32 = 0x5244_4853; // "SHDR"
const FOURCC_ISGN: u32 = 0x4E47_5349; // "ISGN"
const FOURCC_OSGN: u32 = 0x4E47_534F; // "OSGN"
const FOURCC_ISG1: u32 = 0x3147_5349; // "ISG1"
const FOURCC_OSG1: u32 = 0x3147_534F; // "OSG1"

const SPIRV_MAGIC: u32 = 0x0723_0203;

// ── SM4/SM5 opcodes (D3D10_SB_OPCODE_*) ──────────────────────────────────────
// Slice 1 subset.
const OP_MOV: u32 = 54; // 0x36
const OP_RET: u32 = 62; // 0x3e
const OP_DCL_INPUT: u32 = 95; // 0x5f
const OP_DCL_INPUT_PS: u32 = 98; // 0x62  (pixel-shader interpolated input)
const OP_DCL_OUTPUT: u32 = 101; // 0x65
const OP_DCL_OUTPUT_SIV: u32 = 103; // 0x67  (system-interpreted value, e.g. position)
const OP_DCL_INPUT_SIV: u32 = 97; // 0x61
const OP_DCL_INPUT_PS_SIV: u32 = 99; // 0x63
const OP_DCL_INPUT_PS_SGV: u32 = 100; // 0x64
const OP_DCL_INPUT_SGV: u32 = 96; // 0x60
const OP_DCL_OUTPUT_SGV: u32 = 102; // 0x66
const OP_DCL_GLOBAL_FLAGS: u32 = 106; // 0x6a
const OP_DCL_TEMPS: u32 = 104; // 0x68
const OP_CUSTOMDATA: u32 = 0x35; // extended-length declaration block

// Slice 2 float-ALU subset (values confirmed against `fxc`-emitted bytecode).
const OP_ADD: u32 = 0x00;
const OP_DIV: u32 = 0x0e;
const OP_DP2: u32 = 0x0f;
const OP_DP3: u32 = 0x10;
const OP_DP4: u32 = 0x11;
const OP_FRC: u32 = 0x1a;
const OP_MAD: u32 = 0x32;
const OP_MAX: u32 = 0x33;
const OP_MIN: u32 = 0x34;
const OP_MUL: u32 = 0x38;
const OP_RCP: u32 = 0x81; // D3D11_SB_OPCODE_RCP (SM5)
const OP_F32TOF16: u32 = 0x82; // float -> fp16 bits in low 16 (per lane)
const OP_F16TOF32: u32 = 0x83; // fp16 bits in low 16 -> float (per lane)
const OP_ROUND_NE: u32 = 0x40;
const OP_ROUND_NI: u32 = 0x41;
const OP_ROUND_PI: u32 = 0x42;
const OP_ROUND_Z: u32 = 0x43;
const OP_RSQ: u32 = 0x44;
const OP_EXP: u32 = 0x19; // D3D10_SB_OPCODE_EXP — base-2 exponential (exp2)
const OP_LOG: u32 = 0x2f; // D3D10_SB_OPCODE_LOG — base-2 logarithm (log2)
                          // Integer ALU (D3D10_SB_OPCODE_*). Operate on register bits as int32.
const OP_IADD: u32 = 0x1e;
const OP_INEG: u32 = 0x28;
const OP_AND: u32 = 0x01;
const OP_OR: u32 = 0x3c;
const OP_XOR: u32 = 0x57;
const OP_NOT: u32 = 0x3b;
const OP_ISHL: u32 = 0x29;
const OP_ISHR: u32 = 0x2a;
const OP_USHR: u32 = 0x55;
const OP_FTOI: u32 = 0x1b; // float -> signed int
const OP_ITOF: u32 = 0x2b; // signed int -> float
const OP_FTOU: u32 = 0x1c; // float -> unsigned int
const OP_UTOF: u32 = 0x56; // unsigned int -> float
const OP_GE: u32 = 0x1d; // float >= -> uint mask (D3D10_SB_OPCODE_GE = 29)
const OP_LT: u32 = 0x31; // float <  -> uint mask
const OP_EQ: u32 = 0x18; // float == -> uint mask
const OP_NE: u32 = 0x39; // float != -> uint mask
const OP_MOVC: u32 = 0x37; // dst = src0 ? src1 : src2 (per-lane)
const OP_IGE: u32 = 0x20; // int >= -> mask (32)
const OP_IEQ: u32 = 0x21; // int == (33)
const OP_ILT: u32 = 0x22; // int <  (34)
const OP_IMAD: u32 = 0x23; // int multiply-add a*b+c (35)
const OP_IMAX: u32 = 0x24; // signed int max (36)
const OP_IMIN: u32 = 0x25; // signed int min (37)
const OP_INE: u32 = 0x27; // int != (39)
const OP_UMAX: u32 = 0x53; // unsigned int max (83)
const OP_UMIN: u32 = 0x54; // unsigned int min (84)
                           // SM5 bit-manipulation (D3D11_SB_OPCODE_*): population/scan/reverse.
const OP_COUNTBITS: u32 = 0x86; // popcount per lane (134)
const OP_FIRSTBIT_HI: u32 = 0x87; // first set bit from MSB, unsigned (135)
const OP_FIRSTBIT_LO: u32 = 0x88; // first set bit from LSB (136)
const OP_FIRSTBIT_SHI: u32 = 0x89; // first sign-differing bit from MSB (137)
const OP_UBFE: u32 = 0x8a; // unsigned bitfield extract (138)
const OP_IBFE: u32 = 0x8b; // signed bitfield extract (139)
const OP_BFI: u32 = 0x8c; // bitfield insert (140)
const OP_BFREV: u32 = 0x8d; // reverse bit order per lane (141)
const OP_IF: u32 = 0x1f; // D3D10_SB_OPCODE_IF (31); test-nonzero bit @ token bit 18
const OP_ELSE: u32 = 0x12; // 18
const OP_ENDIF: u32 = 0x15; // 21
const OP_LOOP: u32 = 0x30; // 48
const OP_ENDLOOP: u32 = 0x16; // 22
const OP_BREAK: u32 = 0x02; // 2 (unconditional)
const OP_BREAKC: u32 = 0x03; // 3 (conditional; test bit @ 18)
const OP_DISCARD: u32 = 0x0d; // 13: discard_nz/_z src (clip / alpha-test); test bit @ 18
                              // Screen-space derivatives (ddx/ddy). SM4 legacy (RTX=11, RTY=12) + SM5
                              // coarse/fine; coarse/fine map to the plain derivative (a precision hint).
const OP_DERIV_RTX: u32 = 0x0b; // 11
const OP_DERIV_RTY: u32 = 0x0c; // 12
const OP_DERIV_RTX_COARSE: u32 = 0x7a; // 122
const OP_DERIV_RTX_FINE: u32 = 0x7b; // 123
const OP_DERIV_RTY_COARSE: u32 = 0x7c; // 124
const OP_DERIV_RTY_FINE: u32 = 0x7d; // 125
const OP_SINCOS: u32 = 0x4d; // 77: sincos dest_sin, dest_cos, src (dsts may be null)
const OP_SAMPLE: u32 = 0x45; // 69: sample dst, coord, resource(t#), sampler(s#)
const OP_SAMPLE_C: u32 = 0x46; // 70: sample_c (depth comparison — shadow maps)
const OP_SAMPLE_C_LZ: u32 = 0x47; // 71: sample_c_lz (comparison at LOD 0)
const OP_SAMPLE_L: u32 = 0x48; // 72: sample_l (adds an explicit LOD source)
const OP_GATHER4: u32 = 0x6d; // 109: gather4 (Texture2D.Gather — PCF/SSAO)
const OP_LD: u32 = 0x2d; // 45: ld (Texture2D.Load — exact texel, .z = mip)
const OP_LD_MS: u32 = 0x2e; // 46: ld_ms (Texture2DMS.Load — MSAA sample)
const OP_DCL_RESOURCE: u32 = 0x58; // 88
const OP_DCL_CONSTANT_BUFFER: u32 = 0x59; // 89: dcl_constantbuffer cbN[size]
const OP_DCL_SAMPLER: u32 = 0x5a; // 90
/// D3D resource dimensions encoded in dcl_resource opcode-token bits [15:11].
const RES_DIM_TEXTURECUBE: u32 = 6;
const RES_DIM_TEXTURE2DARRAY: u32 = 8;

/// The texture shape a SAMPLE targets — selects the OpTypeImage and the sample
/// coordinate width. From the resource's `dcl_resource` dimension.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TexKind {
    Tex2D,
    Tex2DArray,
    Cube,
    /// A 2D depth/comparison image — used by `sample_c` (shadow mapping). Not from
    /// `dcl_resource` (which says texture2d); forced by the sample_c opcode.
    Depth2D,
    /// A 2D multisampled image — used by `ld_ms` (MSAA texel load). Forced by the
    /// ld_ms opcode (the dcl says texture2dms).
    Tex2DMS,
}

impl TexKind {
    fn from_res_dim(dim: u32) -> Self {
        match dim {
            RES_DIM_TEXTURE2DARRAY => TexKind::Tex2DArray,
            RES_DIM_TEXTURECUBE => TexKind::Cube,
            _ => TexKind::Tex2D,
        }
    }
    /// Sample-coordinate components: 2 for plain 2D / depth-2D, 3 for an array
    /// (uv + slice) or a cube (xyz direction).
    fn coord_components(self) -> u32 {
        match self {
            TexKind::Tex2D | TexKind::Depth2D | TexKind::Tex2DMS => 2,
            _ => 3,
        }
    }
}
const OP_SQRT: u32 = 0x4b;

// ── Operand register files (D3D10_SB_OPERAND_TYPE_*) ──
const OPERAND_TEMP: u32 = 0;
const OPERAND_INPUT: u32 = 1;
const OPERAND_OUTPUT: u32 = 2;
const OPERAND_IMMEDIATE32: u32 = 4;
const OPERAND_SAMPLER: u32 = 6; // D3D10_SB_OPERAND_TYPE_SAMPLER (s#)
const OPERAND_RESOURCE: u32 = 7; // D3D10_SB_OPERAND_TYPE_RESOURCE (t#)
const OPERAND_CONSTANT_BUFFER: u32 = 8; // D3D10_SB_OPERAND_TYPE_CONSTANT_BUFFER (cb#[i])
const OPERAND_NULL: u32 = 13; // D3D10_SB_OPERAND_TYPE_NULL (e.g. an unused sincos dst)

// ── DXBC signature system-value codes (D3D_NAME_*) ──
// SV_NONE == 0 (user semantic / no system value); SV_POSITION is the only
// builtin slice 1 maps explicitly. Others land in later slices.
const SV_POSITION: u32 = 1;

/// Where a translated shader's inputs/outputs bind on the Vulkan side. Returned
/// alongside the SPIR-V so the (GPU-gated) pipeline-create side can build the
/// matching interface. Slice 1 only records position/target/location, but the
/// shape is the load-bearing seam to the GPU submit path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignatureMap {
    pub inputs: Vec<SemanticBinding>,
    pub outputs: Vec<SemanticBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticBinding {
    pub register: u32,
    pub semantic: String,
    pub semantic_index: u32,
    /// Vulkan `Location` (user varyings / SV_Target) or `u32::MAX` for builtins.
    pub location: u32,
    pub builtin: Option<SpirvBuiltIn>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpirvBuiltIn {
    Position,
    FragCoord,
}

/// Reserved descriptor-binding bases (DXVK convention, namespaced per resource
/// class). Unused in slice 1 (no resources) but recorded so slices 2+ extend
/// rather than reinvent the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingLayout {
    pub cb_base: u32,
    pub t_base: u32,
    pub s_base: u32,
    pub u_base: u32,
}

impl Default for BindingLayout {
    fn default() -> Self {
        Self {
            cb_base: 0,
            t_base: 64,
            s_base: 128,
            u_base: 192,
        }
    }
}

/// The result of a successful translation. `spirv` is little-endian SPIR-V words
/// as bytes (ready for `vkCreateShaderModule`, once that path exists).
#[derive(Debug, Clone)]
pub struct Translated {
    pub spirv: Vec<u8>,
    pub stage: raegfx::ShaderStage,
    pub bindings: BindingLayout,
    pub io: SignatureMap,
}

/// Options for the translator. Slice 1 has none meaningful yet; kept so the
/// public signature matches the spec and is stable for later slices.
#[derive(Debug, Clone, Copy, Default)]
pub struct TranslateOpts {
    _reserved: u8,
}

// ── DXBC container chunk collector ──────────────────────────────────────────

struct Chunks<'a> {
    shex: &'a [u8],
    isgn: Option<&'a [u8]>,
    osgn: Option<&'a [u8]>,
}

fn rd_u32(b: &[u8], off: usize) -> Result<u32, ShaderError> {
    if off + 4 > b.len() {
        return Err(ShaderError::InvalidBytecode);
    }
    Ok(u32::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
    ]))
}

fn collect_chunks(dxbc: &[u8]) -> Result<Chunks<'_>, ShaderError> {
    if dxbc.len() < 32 {
        return Err(ShaderError::InvalidBytecode);
    }
    if rd_u32(dxbc, 0)? != DXBC_MAGIC {
        return Err(ShaderError::InvalidBytecode);
    }
    let chunk_count = rd_u32(dxbc, 28)? as usize;
    // The offset table must fit. (Guards a huge chunk_count from a hostile blob.)
    let table_end = 32usize
        .checked_add(
            chunk_count
                .checked_mul(4)
                .ok_or(ShaderError::InvalidBytecode)?,
        )
        .ok_or(ShaderError::InvalidBytecode)?;
    if table_end > dxbc.len() {
        return Err(ShaderError::InvalidBytecode);
    }

    let mut shex: Option<&[u8]> = None;
    let mut isgn: Option<&[u8]> = None;
    let mut osgn: Option<&[u8]> = None;

    for i in 0..chunk_count {
        let off = rd_u32(dxbc, 32 + i * 4)? as usize;
        // FourCC + size header.
        if off + 8 > dxbc.len() {
            return Err(ShaderError::InvalidBytecode);
        }
        let fourcc = rd_u32(dxbc, off)?;
        let size = rd_u32(dxbc, off + 4)? as usize;
        let data_start = off + 8;
        let data_end = data_start
            .checked_add(size)
            .ok_or(ShaderError::InvalidBytecode)?;
        if data_end > dxbc.len() {
            return Err(ShaderError::InvalidBytecode);
        }
        let body = &dxbc[data_start..data_end];
        match fourcc {
            FOURCC_SHEX | FOURCC_SHDR => shex = Some(body),
            FOURCC_ISGN | FOURCC_ISG1 => isgn = Some(body),
            FOURCC_OSGN | FOURCC_OSG1 => osgn = Some(body),
            _ => {}
        }
    }

    let shex = shex.ok_or(ShaderError::InvalidBytecode)?;
    Ok(Chunks { shex, isgn, osgn })
}

// ── ISGN/OSGN signature parse ───────────────────────────────────────────────

#[derive(Debug, Clone)]
struct SigElem {
    name: String,
    semantic_index: u32,
    system_value: u32,
    register: u32,
    #[allow(dead_code)]
    mask: u8,
}

fn parse_signature(body: &[u8]) -> Result<Vec<SigElem>, ShaderError> {
    // Header: element_count (u32), unknown/offset (u32). Then `count` elements,
    // each 6 u32: name_offset, semantic_index, system_value_type,
    // component_type, register, mask(byte)+rw_mask(byte). Offsets are relative
    // to the chunk body start.
    if body.len() < 8 {
        return Err(ShaderError::InvalidBytecode);
    }
    let count = rd_u32(body, 0)? as usize;
    let mut out = Vec::with_capacity(count);
    let elem_base = 8usize;
    for i in 0..count {
        let e = elem_base + i * 24;
        if e + 24 > body.len() {
            return Err(ShaderError::InvalidBytecode);
        }
        let name_off = rd_u32(body, e)? as usize;
        let semantic_index = rd_u32(body, e + 4)?;
        let system_value = rd_u32(body, e + 8)?;
        let register = rd_u32(body, e + 16)?;
        let maskword = rd_u32(body, e + 20)?;
        let name = read_cstr(body, name_off)?;
        out.push(SigElem {
            name,
            semantic_index,
            system_value,
            register,
            mask: (maskword & 0xFF) as u8,
        });
    }
    Ok(out)
}

fn read_cstr(b: &[u8], off: usize) -> Result<String, ShaderError> {
    if off >= b.len() {
        return Err(ShaderError::InvalidBytecode);
    }
    let mut end = off;
    while end < b.len() && b[end] != 0 {
        end += 1;
    }
    core::str::from_utf8(&b[off..end])
        .map(String::from)
        .map_err(|_| ShaderError::InvalidBytecode)
}

// ── SM4/SM5 token-stream decode (slice-1 subset) ────────────────────────────

/// Source-operand modifier (from the extended operand token, ext_type == 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SrcMod {
    None,
    Neg,
    Abs,
    AbsNeg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SrcKind {
    Input(u32),          // v#  (register index)
    Temp(u32),           // r#
    Immediate([u32; 4]), // l(...)
    /// `cb<slot>[index]` — a constant-buffer read. `dyn_index = Some((temp, comp))`
    /// adds a runtime element index `r<temp>.<comp>` to the `index` offset
    /// (register-indexed / dynamic, e.g. skinning); `None` = static `index`.
    ConstBuffer {
        slot: u32,
        index: u32,
        dyn_index: Option<(u32, u8)>,
    },
}

/// A fully-decoded source operand: register/immediate + a 4-lane swizzle (each
/// entry selects source component 0..3) + modifier. Slice 1 ignored the swizzle
/// and modifier words; slice 2 honours them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Src {
    kind: SrcKind,
    swizzle: [u8; 4],
    modifier: SrcMod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DstKind {
    Output(u32),
    Temp(u32),
}

/// A fully-decoded destination operand: register + write-mask (bit i = lane i).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dst {
    kind: DstKind,
    write_mask: u8,
}

/// The ALU operations slice 2 lowers. Each is float, lane-wise over the dest
/// write-mask, sources swizzled/modified, result optionally saturated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AluOp {
    Mov,
    Add,
    Mul,
    Mad,
    Div,
    Min,
    Max,
    Sqrt,
    Rsq,
    Rcp,
    Frc,
    RoundNe,
    RoundNi,
    RoundPi,
    RoundZ,
    Exp2,
    Log2,
    Dp2,
    Dp3,
    Dp4,
    // Screen-space derivatives (ddx/ddy) -> OpDPdx/OpDPdy (core, fragment-only).
    DdX,
    DdY,
    // Integer ALU (operate on the register's bits as int32 vec4 via bitcast).
    IAdd,
    INeg,
    And,
    Or,
    Xor,
    Not,
    IShl,
    IShr, // arithmetic (sign-extending) right shift
    UShr, // logical (zero-filling) right shift
    FToI, // float -> signed int (value stored as int bits)
    IToF, // signed int -> float value
    FToU, // float -> unsigned int
    UToF, // unsigned int -> float value
    Ge,   // float >= -> 0xFFFFFFFF / 0 mask
    Lt,   // float <  -> mask
    Eq,   // float == -> mask
    Ne,   // float != -> mask
    Ige,  // int >= -> mask
    Ilt,  // int <  -> mask
    Ieq,  // int == -> mask
    Ine,  // int != -> mask
    Movc, // per-lane: src0 ? src1 : src2
    // Integer min/max + multiply-add (GLSL SMin/SMax/UMin/UMax; IMul+IAdd).
    IMin,
    IMax,
    UMin,
    UMax,
    IMad,
    // SM5 bit-manipulation.
    BfRev,       // reverse bit order (OpBitReverse)
    CountBits,   // population count (OpBitCount)
    FirstBitLo,  // lowest set bit index, 0xFFFFFFFF if none (FindILsb)
    FirstBitHi,  // highest set bit counted from MSB, unsigned (31 - FindUMsb)
    FirstBitShi, // highest sign-differing bit from MSB (31 - FindSMsb)
    // SM5 bitfield extract/insert (per-lane, scalar offset/count in SPIR-V).
    Ubfe, // unsigned bitfield extract (OpBitFieldUExtract)
    Ibfe, // signed bitfield extract (OpBitFieldSExtract)
    Bfi,  // bitfield insert (OpBitFieldInsert)
    // SM5 half-precision conversions (per-lane, via GLSL Pack/UnpackHalf2x16).
    F32ToF16, // float -> fp16 bits in low 16 of a uint
    F16ToF32, // fp16 bits in low 16 -> float
}

#[derive(Debug, Clone)]
enum DecodedOp {
    /// Generic ALU: 1..3 sources, write-masked dest, `saturate` result modifier.
    Alu {
        op: AluOp,
        dst: Dst,
        srcs: Vec<Src>,
        saturate: bool,
    },
    /// `if_nz`/`if_z src`: structured branch on whether the condition lane is
    /// non-zero (`test_nz`) or zero.
    If {
        cond: Src,
        test_nz: bool,
    },
    Else,
    EndIf,
    /// `loop` ... `endloop` (infinite loop exited by break/breakc).
    Loop,
    EndLoop,
    Break,
    BreakC {
        cond: Src,
        test_nz: bool,
    },
    /// `discard_nz`/`discard_z src` — kill the fragment when the condition lane is
    /// non-zero (`test_nz`) / zero. The HLSL `clip()` + alpha-test idiom.
    Discard {
        cond: Src,
        test_nz: bool,
    },
    /// `sincos dest_sin, dest_cos, src` — either destination may be unused (null).
    SinCos {
        dst_sin: Option<Dst>,
        dst_cos: Option<Dst>,
        src: Src,
    },
    /// `sample dst, coord, t<tex_reg>, s<samp_reg>` — texture sample. `kind` (from
    /// the resource's `dcl_resource` dimension) selects 2D / 2DArray / Cube, which
    /// drives the image type and coord width.
    Sample {
        dst: Dst,
        coord: Src,
        tex_reg: u32,
        samp_reg: u32,
        kind: TexKind,
    },
    /// `sample_l dst, coord, t#, s#, lod` — texture sample at an explicit LOD.
    SampleL {
        dst: Dst,
        coord: Src,
        tex_reg: u32,
        samp_reg: u32,
        lod: Src,
        kind: TexKind,
    },
    /// `sample_c`/`sample_c_lz dst, coord, t#, s#, ref` — depth-comparison sample
    /// (shadow maps). `lz` = LOD-zero variant. The image is a 2D depth image; the
    /// result is the scalar comparison value (splatted to the dst lanes).
    SampleC {
        dst: Dst,
        coord: Src,
        tex_reg: u32,
        samp_reg: u32,
        dref: Src,
        lz: bool,
    },
    /// `gather4 dst, coord, t#, s#` — fetch the 4 bilinear texels of channel
    /// `component` (0=R..3=A, from the sampler swizzle). Texture2D.Gather.
    Gather4 {
        dst: Dst,
        coord: Src,
        tex_reg: u32,
        samp_reg: u32,
        component: u8,
    },
    /// `ld dst, coord, t#` — Texture2D.Load: exact texel fetch (no sampler). The
    /// integer coord's `.xy` are the texel, `.z` the mip level.
    Ld {
        dst: Dst,
        coord: Src,
        tex_reg: u32,
    },
    /// `ld_ms dst, coord, t#, sample` — Texture2DMS.Load: a multisampled texel
    /// fetch at integer `.xy` and the given sample index.
    LdMs {
        dst: Dst,
        coord: Src,
        tex_reg: u32,
        sample: Src,
    },
    Ret,
}

struct TokenStream<'a> {
    words: &'a [u32],
    pos: usize,
}

impl<'a> TokenStream<'a> {
    fn next_word(&mut self) -> Result<u32, ShaderError> {
        let w = *self
            .words
            .get(self.pos)
            .ok_or(ShaderError::InvalidBytecode)?;
        self.pos += 1;
        Ok(w)
    }
    fn peek(&self) -> Option<u32> {
        self.words.get(self.pos).copied()
    }
}

/// A decoded operand carrying the full component-selection + modifier info.
struct Operand {
    file: u32,
    reg: u32,
    /// For a 2-index operand (e.g. `cb<slot>[elem]`), the FIRST index (the slot);
    /// `reg` then holds the second index (the element). 0 for 1-index operands.
    index0: u32,
    /// Relative (register-indexed) addressing: `Some((temp_reg, component))` when
    /// the element index is `r<temp_reg>.<component> + reg` (the `reg` immediate is
    /// then the offset). Only a temp-register index is modelled. None = static.
    rel_temp: Option<(u32, u8)>,
    imm: [u32; 4],
    /// 4-lane swizzle (each selects source comp 0..3). For mask mode this is the
    /// identity `[0,1,2,3]`; the mask itself is in `write_mask`.
    swizzle: [u8; 4],
    /// Write-mask bits (dest, mask mode): bit i = lane i.
    write_mask: u8,
    modifier: SrcMod,
    is_immediate: bool,
}

/// Decode one operand starting at the current position. Honours component-
/// selection (mask/swizzle/select1) and the extended operand modifier token.
/// Bounds-checked; rejects relative addressing and >1 extended token.
fn decode_operand(ts: &mut TokenStream) -> Result<Operand, ShaderError> {
    let tok = ts.next_word()?;
    let num_comp = tok & 0x3;
    let sel_mode = (tok >> 2) & 0x3;
    let operand_type = (tok >> 12) & 0xFF;
    let index_dim = (tok >> 20) & 0x3;

    // ── extended operand token chain (slice 2: a single modifier token) ──
    let mut modifier = SrcMod::None;
    if (tok >> 31) & 1 != 0 {
        let ext = ts.next_word()?;
        // Another extension after this one is not modelled.
        if (ext >> 31) & 1 != 0 {
            return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF04));
        }
        let ext_type = ext & 0x3F;
        if ext_type == 1 {
            // D3D10_SB_EXTENDED_OPERAND_MODIFIER: modifier in bits[6:13].
            let m = (ext >> 6) & 0xFF;
            modifier = match m {
                0 => SrcMod::None,
                1 => SrcMod::Neg,
                2 => SrcMod::Abs,
                3 => SrcMod::AbsNeg,
                _ => return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF05)),
            };
        } else if ext_type != 0 {
            // Unknown extension kind (precision, non-uniform, ...).
            return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF06));
        }
    }

    // ── component selection: derive swizzle + write-mask ──
    // Defaults: identity swizzle, full write-mask.
    let mut swizzle = [0u8, 1, 2, 3];
    let mut write_mask = 0xF;
    match num_comp {
        // 0-component (e.g. some declarations / null operands): leave defaults.
        0 => {}
        // 1-component: a single scalar lane; broadcast it across the swizzle.
        1 => {
            swizzle = [0, 0, 0, 0];
            write_mask = 0x1;
        }
        // 4-component: selection mode says how to read bits[4:11].
        2 => match sel_mode {
            0 => {
                // mask mode (destinations)
                write_mask = ((tok >> 4) & 0xF) as u8;
            }
            1 => {
                // swizzle mode (sources): 2 bits per output lane
                let sw = (tok >> 4) & 0xFF;
                swizzle = [
                    (sw & 0x3) as u8,
                    ((sw >> 2) & 0x3) as u8,
                    ((sw >> 4) & 0x3) as u8,
                    ((sw >> 6) & 0x3) as u8,
                ];
            }
            2 => {
                // select1 mode (a single source component, broadcast)
                let c = ((tok >> 4) & 0x3) as u8;
                swizzle = [c, c, c, c];
            }
            _ => return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF07)),
        },
        _ => return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF08)),
    }

    if operand_type == OPERAND_IMMEDIATE32 {
        // num_comp: 1 => 1 immediate, 2 => 4 immediates.
        let n = match num_comp {
            1 => 1,
            2 => 4,
            _ => return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF02)),
        };
        let mut vals = [0u32; 4];
        for slot in vals.iter_mut().take(n) {
            *slot = ts.next_word()?;
        }
        // A scalar immediate splats across the vector (DXBC l(x) semantics).
        if n == 1 {
            vals = [vals[0], vals[0], vals[0], vals[0]];
        }
        return Ok(Operand {
            file: operand_type,
            reg: 0,
            index0: 0,
            rel_temp: None,
            imm: vals,
            swizzle: [0, 1, 2, 3],
            write_mask,
            modifier,
            is_immediate: true,
        });
    }

    // Register-file operand: read `index_dim` index tokens. For a 2-index operand
    // (cb<slot>[elem]) the first index is the slot (`index0`) and the last is the
    // element (`reg`). The element may be IMMEDIATE32 (static) or a relative form
    // `r<temp>.<comp> + imm` (dynamic, e.g. skinning/instancing).
    let mut reg = 0u32;
    let mut index0 = 0u32;
    let mut rel_temp: Option<(u32, u8)> = None;
    for d in 0..index_dim {
        let idx_rep = (tok >> (22 + d * 3)) & 0x7;
        match idx_rep {
            // IMMEDIATE32 — the static index.
            0 => {
                let v = ts.next_word()?;
                if d == 0 {
                    index0 = v;
                }
                reg = v;
            }
            // RELATIVE (2) / IMMEDIATE32_PLUS_RELATIVE (3): an optional immediate
            // offset (only for 3) then a relative operand (the index register).
            2 | 3 => {
                let offset = if idx_rep == 3 { ts.next_word()? } else { 0 };
                let rel = decode_operand(ts)?;
                if rel.file != OPERAND_TEMP {
                    // Only a temp-register relative index is modelled.
                    return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF0C));
                }
                rel_temp = Some((rel.reg, rel.swizzle[0]));
                if d == 0 {
                    index0 = offset;
                }
                reg = offset;
            }
            // 1 = IMMEDIATE64, 4 = IMMEDIATE64_PLUS_RELATIVE: not modelled.
            _ => return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF03)),
        }
    }

    Ok(Operand {
        file: operand_type,
        reg,
        index0,
        rel_temp,
        imm: [0; 4],
        swizzle,
        write_mask,
        modifier,
        is_immediate: false,
    })
}

/// Convert a decoded register/immediate `Operand` to a `Src` (sources never
/// carry a write-mask).
fn operand_to_src(o: &Operand) -> Result<Src, ShaderError> {
    let kind = if o.is_immediate {
        SrcKind::Immediate(o.imm)
    } else {
        match o.file {
            OPERAND_INPUT => SrcKind::Input(o.reg),
            OPERAND_TEMP => SrcKind::Temp(o.reg),
            OPERAND_CONSTANT_BUFFER => SrcKind::ConstBuffer {
                slot: o.index0,
                index: o.reg,
                dyn_index: o.rel_temp,
            },
            _ => return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF09)),
        }
    };
    Ok(Src {
        kind,
        swizzle: o.swizzle,
        modifier: o.modifier,
    })
}

/// Convert a decoded `Operand` to a `Dst` (destinations carry a write-mask).
fn operand_to_dst(o: &Operand) -> Result<Dst, ShaderError> {
    if o.is_immediate {
        return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF0A));
    }
    let kind = match o.file {
        OPERAND_OUTPUT => DstKind::Output(o.reg),
        OPERAND_TEMP => DstKind::Temp(o.reg),
        _ => return Err(ShaderError::UnsupportedInstruction(0xFFFF_FF0B)),
    };
    Ok(Dst {
        kind,
        write_mask: o.write_mask,
    })
}

/// Map a raw SM4/5 opcode to its `AluOp` + source-operand count, or `None` if it
/// is not a slice-2 float-ALU instruction.
fn alu_op_of(opcode: u32) -> Option<(AluOp, usize)> {
    Some(match opcode {
        OP_MOV => (AluOp::Mov, 1),
        OP_FRC => (AluOp::Frc, 1),
        OP_SQRT => (AluOp::Sqrt, 1),
        OP_RSQ => (AluOp::Rsq, 1),
        OP_RCP => (AluOp::Rcp, 1),
        OP_ROUND_NE => (AluOp::RoundNe, 1),
        OP_ROUND_NI => (AluOp::RoundNi, 1),
        OP_ROUND_PI => (AluOp::RoundPi, 1),
        OP_ROUND_Z => (AluOp::RoundZ, 1),
        OP_EXP => (AluOp::Exp2, 1),
        OP_LOG => (AluOp::Log2, 1),
        OP_DERIV_RTX | OP_DERIV_RTX_COARSE | OP_DERIV_RTX_FINE => (AluOp::DdX, 1),
        OP_DERIV_RTY | OP_DERIV_RTY_COARSE | OP_DERIV_RTY_FINE => (AluOp::DdY, 1),
        OP_IADD => (AluOp::IAdd, 2),
        OP_INEG => (AluOp::INeg, 1),
        OP_AND => (AluOp::And, 2),
        OP_OR => (AluOp::Or, 2),
        OP_XOR => (AluOp::Xor, 2),
        OP_NOT => (AluOp::Not, 1),
        OP_ISHL => (AluOp::IShl, 2),
        OP_ISHR => (AluOp::IShr, 2),
        OP_USHR => (AluOp::UShr, 2),
        OP_FTOI => (AluOp::FToI, 1),
        OP_ITOF => (AluOp::IToF, 1),
        OP_FTOU => (AluOp::FToU, 1),
        OP_UTOF => (AluOp::UToF, 1),
        OP_GE => (AluOp::Ge, 2),
        OP_LT => (AluOp::Lt, 2),
        OP_EQ => (AluOp::Eq, 2),
        OP_NE => (AluOp::Ne, 2),
        OP_IGE => (AluOp::Ige, 2),
        OP_ILT => (AluOp::Ilt, 2),
        OP_IEQ => (AluOp::Ieq, 2),
        OP_INE => (AluOp::Ine, 2),
        OP_IMIN => (AluOp::IMin, 2),
        OP_IMAX => (AluOp::IMax, 2),
        OP_UMIN => (AluOp::UMin, 2),
        OP_UMAX => (AluOp::UMax, 2),
        OP_IMAD => (AluOp::IMad, 3),
        OP_BFREV => (AluOp::BfRev, 1),
        OP_COUNTBITS => (AluOp::CountBits, 1),
        OP_FIRSTBIT_LO => (AluOp::FirstBitLo, 1),
        OP_FIRSTBIT_HI => (AluOp::FirstBitHi, 1),
        OP_FIRSTBIT_SHI => (AluOp::FirstBitShi, 1),
        OP_UBFE => (AluOp::Ubfe, 3),
        OP_IBFE => (AluOp::Ibfe, 3),
        OP_BFI => (AluOp::Bfi, 4),
        OP_F32TOF16 => (AluOp::F32ToF16, 1),
        OP_F16TOF32 => (AluOp::F16ToF32, 1),
        OP_MOVC => (AluOp::Movc, 3),
        OP_ADD => (AluOp::Add, 2),
        OP_MUL => (AluOp::Mul, 2),
        OP_DIV => (AluOp::Div, 2),
        OP_MIN => (AluOp::Min, 2),
        OP_MAX => (AluOp::Max, 2),
        OP_DP2 => (AluOp::Dp2, 2),
        OP_DP3 => (AluOp::Dp3, 2),
        OP_DP4 => (AluOp::Dp4, 2),
        OP_MAD => (AluOp::Mad, 3),
        _ => return None,
    })
}

/// Decode the SHEX token stream into the op list plus the declared temp count.
/// Skips `dcl_*` declarations (their interface effect comes from the
/// signatures), and rejects any opcode outside the supported subset.
fn decode_shex(shex: &[u8]) -> Result<(Vec<DecodedOp>, u32, BTreeMap<u32, u32>), ShaderError> {
    if shex.len() < 8 || shex.len() % 4 != 0 {
        return Err(ShaderError::InvalidBytecode);
    }
    let nwords = shex.len() / 4;
    let mut words = Vec::with_capacity(nwords);
    for i in 0..nwords {
        words.push(rd_u32(shex, i * 4)?);
    }

    // words[0] = version token, words[1] = total length in tokens.
    let total_len = words[1] as usize;
    let limit = core::cmp::min(total_len, nwords);

    let mut ts = TokenStream {
        words: &words[..limit],
        pos: 2,
    };

    let mut ops = Vec::new();
    let mut temp_count = 0u32;
    // Resource register (t#) -> D3D resource dimension, from dcl_resource. Lets a
    // SAMPLE pick the Texture2DArray image type + a 3-component coord.
    let mut res_dims: BTreeMap<u32, u32> = BTreeMap::new();
    // Constant-buffer slot (cb#) -> element count, from dcl_constantbuffer. Sizes
    // the emitted uniform-block array.
    let mut cb_sizes: BTreeMap<u32, u32> = BTreeMap::new();

    while ts.pos < limit {
        let op_token = match ts.peek() {
            Some(t) => t,
            None => break,
        };
        let opcode = op_token & 0x7FF;
        let extended = (op_token >> 31) & 1;
        // Result-modifier: D3D10_SB_INSTRUCTION_RETURN_TYPE / SATURATE is bit 13.
        let saturate = (op_token >> 13) & 1 != 0;

        // Instruction length in tokens. For most opcodes this lives in
        // bits[30:24]; for the custom-data block the next word is the length.
        let instr_len = ((op_token >> 24) & 0x7F) as usize;

        // Float-ALU opcodes lower through one generic path. `arity` is the source
        // operand count.
        let alu = alu_op_of(opcode);

        match opcode {
            OP_RET => {
                advance(&mut ts, instr_len)?;
                ops.push(DecodedOp::Ret);
            }
            _ if alu.is_some() => {
                // The opcode token may itself be extended (opcode-extension
                // tokens, e.g. sample-resource-dimension); our ALU set does not
                // use them, so reject to avoid mis-parsing operand positions.
                if extended != 0 {
                    return Err(ShaderError::UnsupportedInstruction(opcode));
                }
                let (op, arity) = alu.unwrap();
                let start = ts.pos;
                ts.pos += 1; // consume opcode token
                let dst = operand_to_dst(&decode_operand(&mut ts)?)?;
                let mut srcs = Vec::with_capacity(arity);
                for _ in 0..arity {
                    srcs.push(operand_to_src(&decode_operand(&mut ts)?)?);
                }
                // Re-sync to the declared instruction length (skip any trailing
                // tokens we did not model).
                resync(&mut ts, start, instr_len)?;
                ops.push(DecodedOp::Alu {
                    op,
                    dst,
                    srcs,
                    saturate,
                });
            }
            OP_IF => {
                // `if_nz`/`if_z`: the test-boolean modifier lives at token bit 18
                // (D3D10_SB_INSTRUCTION_TEST_BOOLEAN; NONZERO=1, ZERO=0). One src.
                let test_nz = (op_token >> 18) & 1 != 0;
                let start = ts.pos;
                ts.pos += 1;
                let cond = operand_to_src(&decode_operand(&mut ts)?)?;
                resync(&mut ts, start, instr_len)?;
                ops.push(DecodedOp::If { cond, test_nz });
            }
            OP_ELSE => {
                advance(&mut ts, instr_len)?;
                ops.push(DecodedOp::Else);
            }
            OP_ENDIF => {
                advance(&mut ts, instr_len)?;
                ops.push(DecodedOp::EndIf);
            }
            OP_LOOP => {
                advance(&mut ts, instr_len)?;
                ops.push(DecodedOp::Loop);
            }
            OP_ENDLOOP => {
                advance(&mut ts, instr_len)?;
                ops.push(DecodedOp::EndLoop);
            }
            OP_BREAK => {
                advance(&mut ts, instr_len)?;
                ops.push(DecodedOp::Break);
            }
            OP_BREAKC => {
                let test_nz = (op_token >> 18) & 1 != 0;
                let start = ts.pos;
                ts.pos += 1;
                let cond = operand_to_src(&decode_operand(&mut ts)?)?;
                resync(&mut ts, start, instr_len)?;
                ops.push(DecodedOp::BreakC { cond, test_nz });
            }
            OP_DISCARD => {
                // discard_nz/_z src — test boolean @ token bit 18, one source.
                let test_nz = (op_token >> 18) & 1 != 0;
                let start = ts.pos;
                ts.pos += 1;
                let cond = operand_to_src(&decode_operand(&mut ts)?)?;
                resync(&mut ts, start, instr_len)?;
                ops.push(DecodedOp::Discard { cond, test_nz });
            }
            OP_SINCOS => {
                // sincos dest_sin, dest_cos, src — either dest may be a NULL operand.
                let start = ts.pos;
                ts.pos += 1;
                let o_sin = decode_operand(&mut ts)?;
                let o_cos = decode_operand(&mut ts)?;
                let src = operand_to_src(&decode_operand(&mut ts)?)?;
                resync(&mut ts, start, instr_len)?;
                let dst_sin = if o_sin.file == OPERAND_NULL {
                    None
                } else {
                    Some(operand_to_dst(&o_sin)?)
                };
                let dst_cos = if o_cos.file == OPERAND_NULL {
                    None
                } else {
                    Some(operand_to_dst(&o_cos)?)
                };
                ops.push(DecodedOp::SinCos {
                    dst_sin,
                    dst_cos,
                    src,
                });
            }
            OP_DCL_TEMPS => {
                let start = ts.pos;
                ts.pos += 1;
                temp_count = ts.next_word()?;
                resync(&mut ts, start, instr_len)?;
            }
            OP_SAMPLE => {
                // sample dst, coord, t<resource>, s<sampler>. We model Texture2D
                // sampling; the resource/sampler bound variables are created lazily.
                let start = ts.pos;
                // Consume the opcode token, then any extended opcode tokens
                // (`sample_indexable` carries a resource-dimension extended token;
                // each ext token chains via bit 31). They precede the operands.
                let mut tok = op_token;
                ts.pos += 1;
                while (tok >> 31) & 1 != 0 {
                    tok = ts.next_word()?;
                }
                let dst = operand_to_dst(&decode_operand(&mut ts)?)?;
                let coord = operand_to_src(&decode_operand(&mut ts)?)?;
                let res_op = decode_operand(&mut ts)?;
                let samp_op = decode_operand(&mut ts)?;
                resync(&mut ts, start, instr_len)?;
                if res_op.file != OPERAND_RESOURCE || samp_op.file != OPERAND_SAMPLER {
                    return Err(ShaderError::UnsupportedInstruction(OP_SAMPLE));
                }
                ops.push(DecodedOp::Sample {
                    dst,
                    coord,
                    tex_reg: res_op.reg,
                    samp_reg: samp_op.reg,
                    kind: TexKind::from_res_dim(res_dims.get(&res_op.reg).copied().unwrap_or(0)),
                });
            }
            OP_SAMPLE_L => {
                // sample_l dst, coord, t#, s#, lod — like sample plus an explicit
                // LOD source. Same extended-opcode-token skipping as sample.
                let start = ts.pos;
                let mut tok = op_token;
                ts.pos += 1;
                while (tok >> 31) & 1 != 0 {
                    tok = ts.next_word()?;
                }
                let dst = operand_to_dst(&decode_operand(&mut ts)?)?;
                let coord = operand_to_src(&decode_operand(&mut ts)?)?;
                let res_op = decode_operand(&mut ts)?;
                let samp_op = decode_operand(&mut ts)?;
                let lod = operand_to_src(&decode_operand(&mut ts)?)?;
                resync(&mut ts, start, instr_len)?;
                if res_op.file != OPERAND_RESOURCE || samp_op.file != OPERAND_SAMPLER {
                    return Err(ShaderError::UnsupportedInstruction(OP_SAMPLE_L));
                }
                ops.push(DecodedOp::SampleL {
                    dst,
                    coord,
                    tex_reg: res_op.reg,
                    samp_reg: samp_op.reg,
                    lod,
                    kind: TexKind::from_res_dim(res_dims.get(&res_op.reg).copied().unwrap_or(0)),
                });
            }
            OP_SAMPLE_C | OP_SAMPLE_C_LZ => {
                // sample_c[_lz] dst, coord, t#, s#, ref — depth comparison (shadow
                // maps). Same operand shape as sample_l, but the 5th source is the
                // comparison reference (Dref), not a LOD. Extended-token skip as
                // sample.
                let lz = opcode == OP_SAMPLE_C_LZ;
                let start = ts.pos;
                let mut tok = op_token;
                ts.pos += 1;
                while (tok >> 31) & 1 != 0 {
                    tok = ts.next_word()?;
                }
                let dst = operand_to_dst(&decode_operand(&mut ts)?)?;
                let coord = operand_to_src(&decode_operand(&mut ts)?)?;
                let res_op = decode_operand(&mut ts)?;
                let samp_op = decode_operand(&mut ts)?;
                let dref = operand_to_src(&decode_operand(&mut ts)?)?;
                resync(&mut ts, start, instr_len)?;
                if res_op.file != OPERAND_RESOURCE || samp_op.file != OPERAND_SAMPLER {
                    return Err(ShaderError::UnsupportedInstruction(opcode));
                }
                ops.push(DecodedOp::SampleC {
                    dst,
                    coord,
                    tex_reg: res_op.reg,
                    samp_reg: samp_op.reg,
                    dref,
                    lz,
                });
            }
            OP_GATHER4 => {
                // gather4 dst, coord, t#, s# — the gathered CHANNEL is the
                // sampler operand's swizzle (s0.x = red). Extended-token skip as
                // sample.
                let start = ts.pos;
                let mut tok = op_token;
                ts.pos += 1;
                while (tok >> 31) & 1 != 0 {
                    tok = ts.next_word()?;
                }
                let dst = operand_to_dst(&decode_operand(&mut ts)?)?;
                let coord = operand_to_src(&decode_operand(&mut ts)?)?;
                let res_op = decode_operand(&mut ts)?;
                let samp_op = decode_operand(&mut ts)?;
                resync(&mut ts, start, instr_len)?;
                if res_op.file != OPERAND_RESOURCE || samp_op.file != OPERAND_SAMPLER {
                    return Err(ShaderError::UnsupportedInstruction(OP_GATHER4));
                }
                ops.push(DecodedOp::Gather4 {
                    dst,
                    coord,
                    tex_reg: res_op.reg,
                    samp_reg: samp_op.reg,
                    component: samp_op.swizzle[0],
                });
            }
            OP_LD => {
                // ld dst, coord, t# — texel fetch (no sampler). Extended-token skip.
                let start = ts.pos;
                let mut tok = op_token;
                ts.pos += 1;
                while (tok >> 31) & 1 != 0 {
                    tok = ts.next_word()?;
                }
                let dst = operand_to_dst(&decode_operand(&mut ts)?)?;
                let coord = operand_to_src(&decode_operand(&mut ts)?)?;
                let res_op = decode_operand(&mut ts)?;
                resync(&mut ts, start, instr_len)?;
                if res_op.file != OPERAND_RESOURCE {
                    return Err(ShaderError::UnsupportedInstruction(OP_LD));
                }
                ops.push(DecodedOp::Ld {
                    dst,
                    coord,
                    tex_reg: res_op.reg,
                });
            }
            OP_LD_MS => {
                // ld_ms dst, coord, t#, sampleIndex — multisampled texel fetch.
                let start = ts.pos;
                let mut tok = op_token;
                ts.pos += 1;
                while (tok >> 31) & 1 != 0 {
                    tok = ts.next_word()?;
                }
                let dst = operand_to_dst(&decode_operand(&mut ts)?)?;
                let coord = operand_to_src(&decode_operand(&mut ts)?)?;
                let res_op = decode_operand(&mut ts)?;
                let sample = operand_to_src(&decode_operand(&mut ts)?)?;
                resync(&mut ts, start, instr_len)?;
                if res_op.file != OPERAND_RESOURCE {
                    return Err(ShaderError::UnsupportedInstruction(OP_LD_MS));
                }
                ops.push(DecodedOp::LdMs {
                    dst,
                    coord,
                    tex_reg: res_op.reg,
                    sample,
                });
            }
            OP_DCL_RESOURCE => {
                // dcl_resource: the resource DIMENSION is in opcode-token bits
                // [15:11]; the operand names the t# register. Record dim per
                // register so a later SAMPLE can pick Texture2D vs Texture2DArray.
                let start = ts.pos;
                let dim = (op_token >> 11) & 0x1F;
                let mut tok = op_token;
                ts.pos += 1;
                while (tok >> 31) & 1 != 0 {
                    tok = ts.next_word()?;
                }
                let res_op = decode_operand(&mut ts)?;
                if res_op.file == OPERAND_RESOURCE {
                    res_dims.insert(res_op.reg, dim);
                }
                resync(&mut ts, start, instr_len)?;
            }
            OP_DCL_CONSTANT_BUFFER => {
                // dcl_constantbuffer cb<slot>[size]: the operand is a 2-index cb
                // (index0 = slot, reg = element COUNT). Record the size so the
                // uniform-block array is dimensioned correctly.
                let start = ts.pos;
                let mut tok = op_token;
                ts.pos += 1;
                while (tok >> 31) & 1 != 0 {
                    tok = ts.next_word()?;
                }
                let cb_op = decode_operand(&mut ts)?;
                if cb_op.file == OPERAND_CONSTANT_BUFFER {
                    cb_sizes.insert(cb_op.index0, cb_op.reg.max(1));
                }
                resync(&mut ts, start, instr_len)?;
            }
            OP_DCL_INPUT | OP_DCL_INPUT_PS | OP_DCL_OUTPUT | OP_DCL_OUTPUT_SIV
            | OP_DCL_INPUT_SIV | OP_DCL_INPUT_PS_SIV | OP_DCL_INPUT_PS_SGV | OP_DCL_INPUT_SGV
            | OP_DCL_OUTPUT_SGV | OP_DCL_GLOBAL_FLAGS | OP_DCL_SAMPLER => {
                // Declarations: their semantic effect is taken from ISGN/OSGN;
                // we only need to skip them by their token length.
                advance(&mut ts, instr_len)?;
            }
            OP_CUSTOMDATA => {
                // Custom-data block: length is the NEXT word, not in the opcode.
                if ts.pos + 1 >= limit {
                    return Err(ShaderError::InvalidBytecode);
                }
                let len = words[ts.pos + 1] as usize;
                if len < 2 {
                    return Err(ShaderError::InvalidBytecode);
                }
                advance(&mut ts, len)?;
            }
            _ => return Err(ShaderError::UnsupportedInstruction(opcode)),
        }
    }

    Ok((ops, temp_count, cb_sizes))
}

/// Advance the stream by a whole instruction of `len` tokens from its current
/// (opcode) position. `len == 0` would not progress — reject it.
fn advance(ts: &mut TokenStream, len: usize) -> Result<(), ShaderError> {
    if len == 0 {
        return Err(ShaderError::InvalidBytecode);
    }
    let next = ts
        .pos
        .checked_add(len)
        .ok_or(ShaderError::InvalidBytecode)?;
    if next > ts.words.len() {
        return Err(ShaderError::InvalidBytecode);
    }
    ts.pos = next;
    Ok(())
}

/// Re-sync `pos` to `start + len` after manually decoding part of an instruction.
fn resync(ts: &mut TokenStream, start: usize, len: usize) -> Result<(), ShaderError> {
    if len == 0 {
        return Err(ShaderError::InvalidBytecode);
    }
    let target = start.checked_add(len).ok_or(ShaderError::InvalidBytecode)?;
    if target > ts.words.len() || target < ts.pos {
        return Err(ShaderError::InvalidBytecode);
    }
    ts.pos = target;
    Ok(())
}

// ── SPIR-V word builder ─────────────────────────────────────────────────────

// SPIR-V opcodes (Khronos unified spec, §3).
const SPV_OP_CAPABILITY: u16 = 17;
const SPV_OP_MEMORY_MODEL: u16 = 14;
const SPV_OP_ENTRY_POINT: u16 = 15;
const SPV_OP_EXECUTION_MODE: u16 = 16;
const SPV_OP_DECORATE: u16 = 71;
const SPV_OP_TYPE_VOID: u16 = 19;
const SPV_OP_TYPE_FLOAT: u16 = 22;
const SPV_OP_TYPE_VECTOR: u16 = 23;
const SPV_OP_TYPE_POINTER: u16 = 32;
const SPV_OP_TYPE_FUNCTION: u16 = 33;
const SPV_OP_CONSTANT: u16 = 43;
const SPV_OP_CONSTANT_COMPOSITE: u16 = 44;
const SPV_OP_VARIABLE: u16 = 59;
const SPV_OP_LOAD: u16 = 61;
const SPV_OP_STORE: u16 = 62;
const SPV_OP_FUNCTION: u16 = 54;
const SPV_OP_DPDX: u16 = 207;
const SPV_OP_DPDY: u16 = 208;
const SPV_OP_LABEL: u16 = 248;
const SPV_OP_KILL: u16 = 252;
const SPV_OP_RETURN: u16 = 253;
const SPV_OP_FUNCTION_END: u16 = 56;
// Structured control flow (if/else/endif).
const SPV_OP_COMPOSITE_EXTRACT: u16 = 81;
const SPV_OP_I_EQUAL: u16 = 170;
const SPV_OP_SELECTION_MERGE: u16 = 247;
const SPV_OP_LOOP_MERGE: u16 = 246;
const SPV_OP_BRANCH: u16 = 249;
const SPV_OP_BRANCH_CONDITIONAL: u16 = 250;
// Textures / samplers (Texture2D sample).
const SPV_OP_TYPE_IMAGE: u16 = 25;
const SPV_OP_TYPE_SAMPLER: u16 = 26;
const SPV_OP_TYPE_SAMPLED_IMAGE: u16 = 27;
const SPV_OP_SAMPLED_IMAGE: u16 = 86;
const SPV_OP_IMAGE_SAMPLE_IMPLICIT_LOD: u16 = 87;
const SPV_OP_IMAGE_SAMPLE_EXPLICIT_LOD: u16 = 88;
const SPV_OP_IMAGE_SAMPLE_DREF_IMPLICIT_LOD: u16 = 89; // sample_c
const SPV_OP_IMAGE_SAMPLE_DREF_EXPLICIT_LOD: u16 = 90; // sample_c_lz
const SPV_OP_IMAGE_GATHER: u16 = 96; // gather4
const SPV_OP_IMAGE_FETCH: u16 = 95; // ld / ld_ms (texel load, no sampler)
const SPV_IMAGE_OPERAND_SAMPLE: u32 = 0x40; // ImageOperands Sample bit (ld_ms)
const SPV_IMAGE_OPERAND_LOD: u32 = 0x2; // ImageOperands Lod bit (for sample_l)
const SPV_STORAGE_UNIFORM_CONSTANT: u32 = 0;
const SPV_DECOR_BINDING: u32 = 33;
const SPV_DECOR_DESCRIPTOR_SET: u32 = 34;
// Constant buffers (uniform blocks).
const SPV_OP_TYPE_ARRAY: u16 = 28;
const SPV_OP_TYPE_STRUCT: u16 = 30;
const SPV_OP_ACCESS_CHAIN: u16 = 65;
const SPV_OP_MEMBER_DECORATE: u16 = 72;
const SPV_STORAGE_UNIFORM: u32 = 2;
const SPV_DECOR_BLOCK: u32 = 2;
const SPV_DECOR_ARRAY_STRIDE: u32 = 6;
const SPV_DECOR_OFFSET: u32 = 35;
const SPV_DIM_2D: u32 = 1;
const SPV_DIM_CUBE: u32 = 3;
const SPV_IMAGE_FORMAT_UNKNOWN: u32 = 0;

// Slice 2 ALU SPIR-V opcodes.
const SPV_OP_EXT_INST_IMPORT: u16 = 11;
const SPV_OP_EXT_INST: u16 = 12;
const SPV_OP_COMPOSITE_CONSTRUCT: u16 = 80;
const SPV_OP_VECTOR_SHUFFLE: u16 = 79;
const SPV_OP_F_NEGATE: u16 = 127;
const SPV_OP_F_ADD: u16 = 129;
const SPV_OP_F_MUL: u16 = 133;
const SPV_OP_F_DIV: u16 = 136;
const SPV_OP_DOT: u16 = 148;
// Integer-ALU SPIR-V ops + the type/bitcast plumbing they need (Khronos §3).
const SPV_OP_TYPE_INT: u16 = 21;
const SPV_OP_BITCAST: u16 = 124;
const SPV_OP_S_NEGATE: u16 = 126;
const SPV_OP_I_ADD: u16 = 128;
const SPV_OP_I_SUB: u16 = 130;
const SPV_OP_I_MUL: u16 = 132;
const SPV_OP_BIT_FIELD_INSERT: u16 = 201;
const SPV_OP_BIT_FIELD_S_EXTRACT: u16 = 202;
const SPV_OP_BIT_FIELD_U_EXTRACT: u16 = 203;
const SPV_OP_BIT_REVERSE: u16 = 204;
const SPV_OP_BIT_COUNT: u16 = 205;
const SPV_OP_SHIFT_RIGHT_LOGICAL: u16 = 194;
const SPV_OP_SHIFT_RIGHT_ARITHMETIC: u16 = 195;
const SPV_OP_SHIFT_LEFT_LOGICAL: u16 = 196;
const SPV_OP_BITWISE_OR: u16 = 197;
const SPV_OP_BITWISE_XOR: u16 = 198;
const SPV_OP_BITWISE_AND: u16 = 199;
const SPV_OP_NOT: u16 = 200;
const SPV_OP_CONVERT_F_TO_U: u16 = 109; // ftou
const SPV_OP_CONVERT_F_TO_S: u16 = 110; // ftoi
const SPV_OP_CONVERT_S_TO_F: u16 = 111; // itof
const SPV_OP_CONVERT_U_TO_F: u16 = 112; // utof
                                        // Comparison + select (ge/lt/eq/ne -> uint mask; movc -> per-lane select).
const SPV_OP_TYPE_BOOL: u16 = 20;
const SPV_OP_SELECT: u16 = 169;
const SPV_OP_I_NOT_EQUAL: u16 = 171;
const SPV_OP_F_ORD_EQUAL: u16 = 180;
const SPV_OP_F_ORD_NOT_EQUAL: u16 = 182;
const SPV_OP_F_ORD_LESS_THAN: u16 = 184;
const SPV_OP_F_ORD_GREATER_THAN_EQUAL: u16 = 190;
const SPV_OP_S_GREATER_THAN_EQUAL: u16 = 175; // ige
const SPV_OP_S_LESS_THAN: u16 = 177; // ilt

// GLSL.std.450 ext-inst numbers (the InverseSqrt etc. set).
const GLSL_FABS: u32 = 4;
const GLSL_FCLAMP: u32 = 43;
const GLSL_FMIN: u32 = 37;
const GLSL_FMAX: u32 = 40;
const GLSL_FMA: u32 = 50;
const GLSL_FRACT: u32 = 10;
const GLSL_SQRT: u32 = 31;
const GLSL_INVERSE_SQRT: u32 = 32;
const GLSL_EXP2: u32 = 29;
const GLSL_LOG2: u32 = 30;
const GLSL_SIN: u32 = 13;
const GLSL_COS: u32 = 14;
const GLSL_ROUND_EVEN: u32 = 2;
const GLSL_FLOOR: u32 = 8;
const GLSL_CEIL: u32 = 9;
const GLSL_TRUNC: u32 = 3;
// Integer min/max + bit-scan (GLSL.std.450 signed/unsigned variants).
const GLSL_UMIN: u32 = 38;
const GLSL_SMIN: u32 = 39;
const GLSL_UMAX: u32 = 41;
const GLSL_SMAX: u32 = 42;
const GLSL_FIND_ILSB: u32 = 73; // lowest set bit index, -1 if none
const GLSL_FIND_SMSB: u32 = 74; // signed most-significant meaningful bit, -1 if none
const GLSL_FIND_UMSB: u32 = 75; // unsigned most-significant set bit, -1 if none
const GLSL_PACK_HALF_2X16: u32 = 58; // vec2 float -> uint (two fp16 halves)
const GLSL_UNPACK_HALF_2X16: u32 = 62; // uint (two fp16 halves) -> vec2 float

// Operand enums we need.
const SPV_CAP_SHADER: u32 = 1;
const SPV_ADDR_LOGICAL: u32 = 0;
const SPV_MEM_GLSL450: u32 = 1;
const SPV_EXECMODEL_VERTEX: u32 = 0;
const SPV_EXECMODEL_FRAGMENT: u32 = 4;
const SPV_EXECMODE_ORIGIN_UPPER_LEFT: u32 = 7;
const SPV_DECOR_BUILTIN: u32 = 11;
const SPV_DECOR_LOCATION: u32 = 30;
const SPV_BUILTIN_POSITION: u32 = 0;
const SPV_BUILTIN_FRAGCOORD: u32 = 15;
const SPV_STORAGE_INPUT: u32 = 1;
const SPV_STORAGE_OUTPUT: u32 = 3;
const SPV_STORAGE_FUNCTION: u32 = 7;
const SPV_FUNCTION_CONTROL_NONE: u32 = 0;

struct SpirvBuilder {
    next_id: u32,
    capabilities: Vec<u32>,
    entry: Vec<u32>,
    exec_modes: Vec<u32>,
    decorations: Vec<u32>,
    types_consts: Vec<u32>,
    variables: Vec<u32>,
    function: Vec<u32>,
    // de-dup caches
    float_ty: Option<u32>,
    vec4_ty: Option<u32>,
    int_ty: Option<u32>,   // signed 32-bit int (for the integer-ALU typed view)
    ivec4_ty: Option<u32>, // signed int32 vec4 (bitcast target for int ops)
    uint_ty: Option<u32>,  // unsigned 32-bit int (ftou/utof signedness)
    uvec4_ty: Option<u32>, // unsigned int32 vec4
    bool_ty: Option<u32>,  // bool scalar (comparison result component)
    bvec4_ty: Option<u32>, // bool vec4 (comparison result / movc condition)
    vecn_ty: BTreeMap<u32, u32>, // float vector of width 2/3 (for dot product)
    void_ty: Option<u32>,
    fn_ty: Option<u32>,
    ptr_cache: BTreeMap<(u32, u32), u32>,
    const_f32: BTreeMap<u32, u32>,
    const_vec4: BTreeMap<[u32; 4], u32>,
    const_i32: BTreeMap<u32, u32>, // int32 scalar consts (mask 0 / -1)
    const_ivec4: BTreeMap<[u32; 4], u32>, // int32 vec4 consts
    const_uvec4: BTreeMap<[u32; 4], u32>, // uint32 vec4 consts
    glsl_ext: Option<u32>,         // GLSL.std.450 ext-inst-set id
    image2d_ty: Option<u32>,       // OpTypeImage (float 2D, sampled)
    image2d_array_ty: Option<u32>, // OpTypeImage (float 2D, Arrayed=1)
    image_cube_ty: Option<u32>,    // OpTypeImage (float Cube, sampled)
    image_depth_ty: Option<u32>,   // OpTypeImage (float 2D, Depth=1, sampled) — sample_c
    image_ms_ty: Option<u32>,      // OpTypeImage (float 2D, MS=1, sampled) — ld_ms
    ivec2_ty: Option<u32>,         // signed int32 vec2 — texel-fetch coordinate
    fvec2_ty: Option<u32>,         // float32 vec2 — Pack/UnpackHalf2x16 operand
    sampler_ty: Option<u32>,       // OpTypeSampler
    sampled_image_ty: Option<u32>, // OpTypeSampledImage of image2d
    sampled_image_array_ty: Option<u32>, // OpTypeSampledImage of image2d_array
    sampled_image_cube_ty: Option<u32>, // OpTypeSampledImage of image_cube
    sampled_image_depth_ty: Option<u32>, // OpTypeSampledImage of image_depth
    tex_vars: BTreeMap<u32, u32>,  // texture register (t#) -> bound OpVariable
    samp_vars: BTreeMap<u32, u32>, // sampler register (s#) -> bound OpVariable
    cb_vars: BTreeMap<u32, u32>,   // constant-buffer slot (cb#) -> uniform-block OpVariable
    const_u32: BTreeMap<u32, u32>, // uint32 scalar consts (cb access indices)
}

impl SpirvBuilder {
    fn new() -> Self {
        Self {
            next_id: 1,
            capabilities: Vec::new(),
            entry: Vec::new(),
            exec_modes: Vec::new(),
            decorations: Vec::new(),
            types_consts: Vec::new(),
            variables: Vec::new(),
            function: Vec::new(),
            float_ty: None,
            vec4_ty: None,
            int_ty: None,
            ivec4_ty: None,
            uint_ty: None,
            uvec4_ty: None,
            bool_ty: None,
            bvec4_ty: None,
            vecn_ty: BTreeMap::new(),
            void_ty: None,
            fn_ty: None,
            ptr_cache: BTreeMap::new(),
            const_f32: BTreeMap::new(),
            const_vec4: BTreeMap::new(),
            const_i32: BTreeMap::new(),
            const_ivec4: BTreeMap::new(),
            const_uvec4: BTreeMap::new(),
            glsl_ext: None,
            image2d_ty: None,
            image2d_array_ty: None,
            image_cube_ty: None,
            image_depth_ty: None,
            image_ms_ty: None,
            ivec2_ty: None,
            fvec2_ty: None,
            sampler_ty: None,
            sampled_image_ty: None,
            sampled_image_array_ty: None,
            sampled_image_cube_ty: None,
            sampled_image_depth_ty: None,
            tex_vars: BTreeMap::new(),
            samp_vars: BTreeMap::new(),
            cb_vars: BTreeMap::new(),
            const_u32: BTreeMap::new(),
        }
    }

    fn alloc(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn float(&mut self) -> u32 {
        if let Some(id) = self.float_ty {
            return id;
        }
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_FLOAT, &[id, 32]);
        self.float_ty = Some(id);
        id
    }

    fn vec4(&mut self) -> u32 {
        if let Some(id) = self.vec4_ty {
            return id;
        }
        let f = self.float();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_VECTOR, &[id, f, 4]);
        self.vec4_ty = Some(id);
        id
    }

    /// Signed 32-bit integer scalar type (`OpTypeInt 32 1`).
    fn int(&mut self) -> u32 {
        if let Some(id) = self.int_ty {
            return id;
        }
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_INT, &[id, 32, 1]);
        self.int_ty = Some(id);
        id
    }

    /// Signed int32 vec4 — the typed view integer ALU ops operate on (the float
    /// vec4 register is `OpBitcast`-ed to/from this around each integer op).
    fn ivec4(&mut self) -> u32 {
        if let Some(id) = self.ivec4_ty {
            return id;
        }
        let i = self.int();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_VECTOR, &[id, i, 4]);
        self.ivec4_ty = Some(id);
        id
    }

    /// Signed int32 vec2 — the integer texel-fetch coordinate for `ld`/`ld_ms`.
    fn ivec2(&mut self) -> u32 {
        if let Some(id) = self.ivec2_ty {
            return id;
        }
        let i = self.int();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_VECTOR, &[id, i, 2]);
        self.ivec2_ty = Some(id);
        id
    }

    /// Float32 vec2 — the operand of `PackHalf2x16` and result of
    /// `UnpackHalf2x16` (f32tof16 / f16tof32 half-precision conversions).
    fn fvec2(&mut self) -> u32 {
        if let Some(id) = self.fvec2_ty {
            return id;
        }
        let f = self.float();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_VECTOR, &[id, f, 2]);
        self.fvec2_ty = Some(id);
        id
    }

    /// Unsigned 32-bit integer scalar (`OpTypeInt 32 0`) — needed so `ftou`/`utof`
    /// have the signedness the SPIR-V validator requires.
    fn uint(&mut self) -> u32 {
        if let Some(id) = self.uint_ty {
            return id;
        }
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_INT, &[id, 32, 0]);
        self.uint_ty = Some(id);
        id
    }

    /// Unsigned int32 vec4 (the ftou result / utof operand typed view).
    fn uvec4(&mut self) -> u32 {
        if let Some(id) = self.uvec4_ty {
            return id;
        }
        let u = self.uint();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_VECTOR, &[id, u, 4]);
        self.uvec4_ty = Some(id);
        id
    }

    /// Bool scalar type (`OpTypeBool`) — the component type of a comparison result.
    fn bool_t(&mut self) -> u32 {
        if let Some(id) = self.bool_ty {
            return id;
        }
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_BOOL, &[id]);
        self.bool_ty = Some(id);
        id
    }

    /// Bool vec4 — a per-lane comparison result / `movc` condition.
    fn bvec4(&mut self) -> u32 {
        if let Some(id) = self.bvec4_ty {
            return id;
        }
        let b = self.bool_t();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_VECTOR, &[id, b, 4]);
        self.bvec4_ty = Some(id);
        id
    }

    /// A signed-int32 scalar constant.
    fn const_int(&mut self, val: u32) -> u32 {
        if let Some(id) = self.const_i32.get(&val) {
            return *id;
        }
        let i = self.int();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_CONSTANT, &[i, id, val]);
        self.const_i32.insert(val, id);
        id
    }

    /// An unsigned int32 scalar constant — the index type for `OpAccessChain`
    /// into a constant-buffer's uniform-block array, and the array length.
    fn const_uint(&mut self, val: u32) -> u32 {
        if let Some(id) = self.const_u32.get(&val) {
            return *id;
        }
        let u = self.uint();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_CONSTANT, &[u, id, val]);
        self.const_u32.insert(val, id);
        id
    }

    /// Lazily create the uniform-block OpVariable for constant buffer `cb<slot>`
    /// with `size` float4 elements: `struct { vec4[size] }` (Block, member offset
    /// 0, array stride 16), bound at DescriptorSet 1 / Binding `slot`. Cached per
    /// slot. The block is the standard SPIR-V representation of an HLSL cbuffer.
    fn cbuffer_var(&mut self, slot: u32, size: u32) -> u32 {
        if let Some(id) = self.cb_vars.get(&slot) {
            return *id;
        }
        let vec4 = self.vec4();
        let len = self.const_uint(size.max(1));
        let arr = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_ARRAY, &[arr, vec4, len]);
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[arr, SPV_DECOR_ARRAY_STRIDE, 16],
        );
        let strukt = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_STRUCT, &[strukt, arr]);
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[strukt, SPV_DECOR_BLOCK],
        );
        emit(
            &mut self.decorations,
            SPV_OP_MEMBER_DECORATE,
            &[strukt, 0, SPV_DECOR_OFFSET, 0],
        );
        let ptr_struct = self.ptr_to(SPV_STORAGE_UNIFORM, strukt);
        let var = self.alloc();
        emit(
            &mut self.variables,
            SPV_OP_VARIABLE,
            &[ptr_struct, var, SPV_STORAGE_UNIFORM],
        );
        // Constant buffers live in descriptor set 1 (textures/samplers use set 0),
        // so cb0 and t0 never collide on a binding.
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[var, SPV_DECOR_DESCRIPTOR_SET, 1],
        );
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[var, SPV_DECOR_BINDING, slot],
        );
        self.cb_vars.insert(slot, var);
        var
    }

    /// A splatted int32 vec4 constant (e.g. all-0 / all-0xFFFFFFFF for masks).
    fn const_ivec4(&mut self, val: u32) -> u32 {
        let key = [val, val, val, val];
        if let Some(id) = self.const_ivec4.get(&key) {
            return *id;
        }
        let c = self.const_int(val);
        let iv = self.ivec4();
        let id = self.alloc();
        emit(
            &mut self.types_consts,
            SPV_OP_CONSTANT_COMPOSITE,
            &[iv, id, c, c, c, c],
        );
        self.const_ivec4.insert(key, id);
        id
    }

    /// A splatted uint32 vec4 constant (for the unsigned bit-scan sentinel math).
    fn const_uvec4(&mut self, val: u32) -> u32 {
        let key = [val, val, val, val];
        if let Some(id) = self.const_uvec4.get(&key) {
            return *id;
        }
        let c = self.const_uint(val);
        let uv = self.uvec4();
        let id = self.alloc();
        emit(
            &mut self.types_consts,
            SPV_OP_CONSTANT_COMPOSITE,
            &[uv, id, c, c, c, c],
        );
        self.const_uvec4.insert(key, id);
        id
    }

    /// Float vector of `width` components (2..=4). Width 4 aliases `vec4()`.
    fn vecn(&mut self, width: u32) -> u32 {
        if width == 4 {
            return self.vec4();
        }
        if let Some(id) = self.vecn_ty.get(&width) {
            return *id;
        }
        let f = self.float();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_VECTOR, &[id, f, width]);
        self.vecn_ty.insert(width, id);
        id
    }

    /// The GLSL.std.450 ext-inst-set id (lazily allocated; emitted in `finish`).
    fn glsl(&mut self) -> u32 {
        if let Some(id) = self.glsl_ext {
            return id;
        }
        let id = self.alloc();
        self.glsl_ext = Some(id);
        id
    }

    fn void(&mut self) -> u32 {
        if let Some(id) = self.void_ty {
            return id;
        }
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_VOID, &[id]);
        self.void_ty = Some(id);
        id
    }

    fn fn_void(&mut self) -> u32 {
        if let Some(id) = self.fn_ty {
            return id;
        }
        let v = self.void();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_FUNCTION, &[id, v]);
        self.fn_ty = Some(id);
        id
    }

    /// `OpTypePointer storage pointee`, de-duplicated.
    fn ptr_to(&mut self, storage: u32, pointee: u32) -> u32 {
        if let Some(id) = self.ptr_cache.get(&(storage, pointee)) {
            return *id;
        }
        let id = self.alloc();
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_POINTER,
            &[id, storage, pointee],
        );
        self.ptr_cache.insert((storage, pointee), id);
        id
    }

    fn ptr_vec4(&mut self, storage: u32) -> u32 {
        let v = self.vec4();
        self.ptr_to(storage, v)
    }

    /// `OpTypeImage %float 2D` (sampled). The texture type for Texture2D sampling.
    fn image2d(&mut self) -> u32 {
        if let Some(id) = self.image2d_ty {
            return id;
        }
        let f = self.float();
        let id = self.alloc();
        // sampled_type, Dim=2D, Depth=0, Arrayed=0, MS=0, Sampled=1, Format=Unknown
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_IMAGE,
            &[id, f, SPV_DIM_2D, 0, 0, 0, 1, SPV_IMAGE_FORMAT_UNKNOWN],
        );
        self.image2d_ty = Some(id);
        id
    }

    /// `OpTypeImage %float 2D Arrayed` (sampled). For Texture2DArray sampling —
    /// same as [`image2d`] but with the Arrayed bit set, so the sample coord
    /// carries an extra array-slice component.
    fn image2d_array(&mut self) -> u32 {
        if let Some(id) = self.image2d_array_ty {
            return id;
        }
        let f = self.float();
        let id = self.alloc();
        // sampled_type, Dim=2D, Depth=0, Arrayed=1, MS=0, Sampled=1, Format=Unknown
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_IMAGE,
            &[id, f, SPV_DIM_2D, 0, 1, 0, 1, SPV_IMAGE_FORMAT_UNKNOWN],
        );
        self.image2d_array_ty = Some(id);
        id
    }

    /// `OpTypeImage %float Cube` (sampled). For TextureCube sampling — the coord
    /// is a 3-component direction vector.
    fn image_cube(&mut self) -> u32 {
        if let Some(id) = self.image_cube_ty {
            return id;
        }
        let f = self.float();
        let id = self.alloc();
        // sampled_type, Dim=Cube, Depth=0, Arrayed=0, MS=0, Sampled=1, Format=Unknown
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_IMAGE,
            &[id, f, SPV_DIM_CUBE, 0, 0, 0, 1, SPV_IMAGE_FORMAT_UNKNOWN],
        );
        self.image_cube_ty = Some(id);
        id
    }

    /// `OpTypeImage %float 2D Depth` (sampled) — the comparison/shadow image for
    /// `sample_c` (Dref sampling).
    fn image2d_depth(&mut self) -> u32 {
        if let Some(id) = self.image_depth_ty {
            return id;
        }
        let f = self.float();
        let id = self.alloc();
        // sampled_type, Dim=2D, Depth=1, Arrayed=0, MS=0, Sampled=1, Format=Unknown
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_IMAGE,
            &[id, f, SPV_DIM_2D, 1, 0, 0, 1, SPV_IMAGE_FORMAT_UNKNOWN],
        );
        self.image_depth_ty = Some(id);
        id
    }

    /// `OpTypeImage %float 2D MS` (sampled) — the multisampled image for `ld_ms`
    /// (MSAA texel fetch).
    fn image2d_ms(&mut self) -> u32 {
        if let Some(id) = self.image_ms_ty {
            return id;
        }
        let f = self.float();
        let id = self.alloc();
        // sampled_type, Dim=2D, Depth=0, Arrayed=0, MS=1, Sampled=1, Format=Unknown
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_IMAGE,
            &[id, f, SPV_DIM_2D, 0, 0, 1, 1, SPV_IMAGE_FORMAT_UNKNOWN],
        );
        self.image_ms_ty = Some(id);
        id
    }

    /// The sampled-image OpTypeImage for a [`TexKind`].
    fn image_for(&mut self, kind: TexKind) -> u32 {
        match kind {
            TexKind::Tex2D => self.image2d(),
            TexKind::Tex2DArray => self.image2d_array(),
            TexKind::Cube => self.image_cube(),
            TexKind::Depth2D => self.image2d_depth(),
            TexKind::Tex2DMS => self.image2d_ms(),
        }
    }

    fn sampler_t(&mut self) -> u32 {
        if let Some(id) = self.sampler_ty {
            return id;
        }
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_TYPE_SAMPLER, &[id]);
        self.sampler_ty = Some(id);
        id
    }

    fn sampled_image(&mut self) -> u32 {
        if let Some(id) = self.sampled_image_ty {
            return id;
        }
        let img = self.image2d();
        let id = self.alloc();
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_SAMPLED_IMAGE,
            &[id, img],
        );
        self.sampled_image_ty = Some(id);
        id
    }

    fn sampled_image_array(&mut self) -> u32 {
        if let Some(id) = self.sampled_image_array_ty {
            return id;
        }
        let img = self.image2d_array();
        let id = self.alloc();
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_SAMPLED_IMAGE,
            &[id, img],
        );
        self.sampled_image_array_ty = Some(id);
        id
    }

    fn sampled_image_cube(&mut self) -> u32 {
        if let Some(id) = self.sampled_image_cube_ty {
            return id;
        }
        let img = self.image_cube();
        let id = self.alloc();
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_SAMPLED_IMAGE,
            &[id, img],
        );
        self.sampled_image_cube_ty = Some(id);
        id
    }

    fn sampled_image_depth(&mut self) -> u32 {
        if let Some(id) = self.sampled_image_depth_ty {
            return id;
        }
        let img = self.image2d_depth();
        let id = self.alloc();
        emit(
            &mut self.types_consts,
            SPV_OP_TYPE_SAMPLED_IMAGE,
            &[id, img],
        );
        self.sampled_image_depth_ty = Some(id);
        id
    }

    /// The OpTypeSampledImage for a [`TexKind`]. `Tex2DMS` is fetch-only (no
    /// sampler), so it has no sampled-image; the arm exists only for totality.
    fn sampled_image_for(&mut self, kind: TexKind) -> u32 {
        match kind {
            TexKind::Tex2D | TexKind::Tex2DMS => self.sampled_image(),
            TexKind::Tex2DArray => self.sampled_image_array(),
            TexKind::Cube => self.sampled_image_cube(),
            TexKind::Depth2D => self.sampled_image_depth(),
        }
    }

    /// Lazily create a descriptor-bound texture variable for register `t<reg>`
    /// (UniformConstant storage, DescriptorSet 0, Binding `reg*2`). `kind` selects
    /// the image shape; a register is one dimension for the whole shader, so the
    /// first call's `kind` defines the cached variable.
    fn texture_var(&mut self, reg: u32, kind: TexKind) -> u32 {
        if let Some(id) = self.tex_vars.get(&reg) {
            return *id;
        }
        let img = self.image_for(kind);
        let ptr = self.ptr_to(SPV_STORAGE_UNIFORM_CONSTANT, img);
        let var = self.alloc();
        emit(
            &mut self.variables,
            SPV_OP_VARIABLE,
            &[ptr, var, SPV_STORAGE_UNIFORM_CONSTANT],
        );
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[var, SPV_DECOR_DESCRIPTOR_SET, 0],
        );
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[var, SPV_DECOR_BINDING, reg * 2],
        );
        self.tex_vars.insert(reg, var);
        var
    }

    /// Lazily create a descriptor-bound sampler variable for register `s<reg>`
    /// (DescriptorSet 0, Binding `reg*2 + 1`).
    fn sampler_var(&mut self, reg: u32) -> u32 {
        if let Some(id) = self.samp_vars.get(&reg) {
            return *id;
        }
        let s = self.sampler_t();
        let ptr = self.ptr_to(SPV_STORAGE_UNIFORM_CONSTANT, s);
        let var = self.alloc();
        emit(
            &mut self.variables,
            SPV_OP_VARIABLE,
            &[ptr, var, SPV_STORAGE_UNIFORM_CONSTANT],
        );
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[var, SPV_DECOR_DESCRIPTOR_SET, 0],
        );
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[var, SPV_DECOR_BINDING, reg * 2 + 1],
        );
        self.samp_vars.insert(reg, var);
        var
    }

    fn const_float(&mut self, bits: u32) -> u32 {
        if let Some(id) = self.const_f32.get(&bits) {
            return *id;
        }
        let f = self.float();
        let id = self.alloc();
        emit(&mut self.types_consts, SPV_OP_CONSTANT, &[f, id, bits]);
        self.const_f32.insert(bits, id);
        id
    }

    fn const_vec4(&mut self, bits: [u32; 4]) -> u32 {
        if let Some(id) = self.const_vec4.get(&bits) {
            return *id;
        }
        let c0 = self.const_float(bits[0]);
        let c1 = self.const_float(bits[1]);
        let c2 = self.const_float(bits[2]);
        let c3 = self.const_float(bits[3]);
        let v = self.vec4();
        let id = self.alloc();
        emit(
            &mut self.types_consts,
            SPV_OP_CONSTANT_COMPOSITE,
            &[v, id, c0, c1, c2, c3],
        );
        self.const_vec4.insert(bits, id);
        id
    }

    fn variable(&mut self, storage: u32) -> u32 {
        let ptr = self.ptr_vec4(storage);
        let id = self.alloc();
        emit(&mut self.variables, SPV_OP_VARIABLE, &[ptr, id, storage]);
        id
    }

    fn decorate_builtin(&mut self, target: u32, builtin: u32) {
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[target, SPV_DECOR_BUILTIN, builtin],
        );
    }

    fn decorate_location(&mut self, target: u32, location: u32) {
        emit(
            &mut self.decorations,
            SPV_OP_DECORATE,
            &[target, SPV_DECOR_LOCATION, location],
        );
    }

    /// Assemble the module words in SPIR-V logical layout order.
    fn finish(self, entry_main_id: u32) -> Vec<u32> {
        let _ = entry_main_id; // ids already embedded in `entry`
        let mut words = Vec::new();
        words.push(SPIRV_MAGIC);
        words.push(0x0001_0000); // version 1.0
        words.push(0x000E_0000); // generator magic (AthBridge)
        words.push(self.next_id); // bound
        words.push(0); // schema
        words.extend_from_slice(&self.capabilities);
        // ext-inst imports (must precede the memory model). Only emitted if some
        // ALU op pulled in GLSL.std.450.
        if let Some(ext_id) = self.glsl_ext {
            let mut operands = Vec::with_capacity(4);
            operands.push(ext_id);
            push_spirv_string(&mut operands, "GLSL.std.450");
            emit(&mut words, SPV_OP_EXT_INST_IMPORT, &operands);
        }
        // memory model
        emit(
            &mut words,
            SPV_OP_MEMORY_MODEL,
            &[SPV_ADDR_LOGICAL, SPV_MEM_GLSL450],
        );
        words.extend_from_slice(&self.entry);
        words.extend_from_slice(&self.exec_modes);
        words.extend_from_slice(&self.decorations);
        words.extend_from_slice(&self.types_consts);
        words.extend_from_slice(&self.variables);
        words.extend_from_slice(&self.function);
        words
    }
}

/// Append one SPIR-V instruction: (word_count << 16 | opcode) then operands.
fn emit(out: &mut Vec<u32>, opcode: u16, operands: &[u32]) {
    let word_count = (operands.len() + 1) as u32;
    out.push((word_count << 16) | opcode as u32);
    out.extend_from_slice(operands);
}

/// Append a SPIR-V string literal (UTF-8, nul-terminated, padded to a word
/// boundary) to an operand list.
fn push_spirv_string(out: &mut Vec<u32>, s: &str) {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0); // nul terminator
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
    for chunk in bytes.chunks_exact(4) {
        out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
}

// ── Lowering: decoded ops + signatures -> SPIR-V module ─────────────────────

fn deterministic_location(elems: &[SigElem], reg: u32) -> u32 {
    // Assign Locations by signature order, skipping builtins, so VS-out and PS-in
    // would agree for the same semantic ordering. Slice 1 has at most one
    // user-location output (SV_Target).
    let mut loc = 0u32;
    for e in elems {
        let is_builtin =
            e.system_value == SV_POSITION || e.name.eq_ignore_ascii_case("SV_Position");
        if e.register == reg {
            return if is_builtin { u32::MAX } else { loc };
        }
        if !is_builtin {
            loc += 1;
        }
    }
    0
}

fn translate_inner(
    stage: raegfx::ShaderStage,
    ops: &[DecodedOp],
    inputs: &[SigElem],
    outputs: &[SigElem],
    cb_sizes: &BTreeMap<u32, u32>,
) -> Result<(Vec<u32>, SignatureMap), ShaderError> {
    let mut b = SpirvBuilder::new();

    // Capability + (deferred) entry/memory model are emitted in finish().
    emit(&mut b.capabilities, SPV_OP_CAPABILITY, &[SPV_CAP_SHADER]);

    let is_fragment = matches!(stage, raegfx::ShaderStage::Fragment);

    // ── input variables (v#) ──
    let mut input_var: BTreeMap<u32, u32> = BTreeMap::new();
    let mut io = SignatureMap::default();
    for e in inputs {
        let var = b.variable(SPV_STORAGE_INPUT);
        let builtin = if e.system_value == SV_POSITION {
            // SV_Position as a PS input is FragCoord; as a VS input it is a
            // builtin only on fragment stages. Slice-1 VS input is POSITION
            // (system_value NONE) so this branch is fragment-only.
            let bi = if is_fragment {
                b.decorate_builtin(var, SPV_BUILTIN_FRAGCOORD);
                SpirvBuiltIn::FragCoord
            } else {
                b.decorate_builtin(var, SPV_BUILTIN_POSITION);
                SpirvBuiltIn::Position
            };
            Some(bi)
        } else {
            let loc = deterministic_location(inputs, e.register);
            b.decorate_location(var, loc);
            None
        };
        input_var.insert(e.register, var);
        io.inputs.push(SemanticBinding {
            register: e.register,
            semantic: e.name.clone(),
            semantic_index: e.semantic_index,
            location: if builtin.is_some() {
                u32::MAX
            } else {
                deterministic_location(inputs, e.register)
            },
            builtin,
        });
    }

    // ── output variables (o#) ──
    let mut output_var: BTreeMap<u32, u32> = BTreeMap::new();
    for e in outputs {
        let var = b.variable(SPV_STORAGE_OUTPUT);
        let is_position =
            e.system_value == SV_POSITION || e.name.eq_ignore_ascii_case("SV_Position");
        let builtin = if is_position {
            b.decorate_builtin(var, SPV_BUILTIN_POSITION);
            Some(SpirvBuiltIn::Position)
        } else {
            // SV_Target / user output -> Location.
            let loc = deterministic_location(outputs, e.register);
            b.decorate_location(var, loc);
            None
        };
        output_var.insert(e.register, var);
        io.outputs.push(SemanticBinding {
            register: e.register,
            semantic: e.name.clone(),
            semantic_index: e.semantic_index,
            location: if builtin.is_some() {
                u32::MAX
            } else {
                deterministic_location(outputs, e.register)
            },
            builtin,
        });
    }

    // ── interface id list for OpEntryPoint (all global I/O vars) ──
    let mut interface: Vec<u32> = Vec::new();
    for (_, v) in input_var.iter() {
        interface.push(*v);
    }
    for (_, v) in output_var.iter() {
        interface.push(*v);
    }

    // ── temp variables (r#) — Function storage, declared in the function ──
    // Created lazily during lowering; collected so they appear right after OpLabel.
    let temp_var: BTreeMap<u32, u32> = BTreeMap::new();

    // ── function skeleton ──
    let fn_ty = b.fn_void();
    let void_ty = b.void();
    let main_id = b.alloc();
    let label_id = b.alloc();

    // Build the entry point now that we have main_id (interface ids known).
    let model = if is_fragment {
        SPV_EXECMODEL_FRAGMENT
    } else {
        SPV_EXECMODEL_VERTEX
    };
    {
        let mut ep = Vec::new();
        ep.push(model);
        ep.push(main_id);
        // name "main" packed as a UTF-8 nul-terminated literal: 'm','a','i','n',0
        ep.push(u32::from_le_bytes([b'm', b'a', b'i', b'n']));
        ep.push(0); // nul terminator word
        ep.extend_from_slice(&interface);
        emit(&mut b.entry, SPV_OP_ENTRY_POINT, &ep);
    }
    if is_fragment {
        emit(
            &mut b.exec_modes,
            SPV_OP_EXECUTION_MODE,
            &[main_id, SPV_EXECMODE_ORIGIN_UPPER_LEFT],
        );
    }

    // OpFunction %void None %fn_ty
    emit(
        &mut b.function,
        SPV_OP_FUNCTION,
        &[void_ty, main_id, SPV_FUNCTION_CONTROL_NONE, fn_ty],
    );
    emit(&mut b.function, SPV_OP_LABEL, &[label_id]);

    // Body lowering. Temp OpVariables (Function storage) must be the first
    // instructions in the entry block; collected separately and spliced in.
    let mut lower = Lowering {
        b: &mut b,
        input_var,
        output_var,
        temp_var,
        temp_decls: Vec::new(),
        body: Vec::new(),
        cf_stack: Vec::new(),
        cb_sizes: cb_sizes.clone(),
    };

    for op in ops {
        match op {
            DecodedOp::Alu {
                op,
                dst,
                srcs,
                saturate,
            } => {
                lower.lower_alu(*op, *dst, srcs, *saturate)?;
            }
            DecodedOp::If { cond, test_nz } => {
                lower.emit_if(cond, *test_nz)?;
            }
            DecodedOp::Else => {
                lower.emit_else()?;
            }
            DecodedOp::EndIf => {
                lower.emit_endif()?;
            }
            DecodedOp::Loop => {
                lower.emit_loop();
            }
            DecodedOp::EndLoop => {
                lower.emit_endloop()?;
            }
            DecodedOp::Break => {
                lower.emit_break()?;
            }
            DecodedOp::BreakC { cond, test_nz } => {
                lower.emit_breakc(cond, *test_nz)?;
            }
            DecodedOp::Discard { cond, test_nz } => {
                lower.emit_discard(cond, *test_nz)?;
            }
            DecodedOp::SinCos {
                dst_sin,
                dst_cos,
                src,
            } => {
                lower.emit_sincos(dst_sin, dst_cos, src)?;
            }
            DecodedOp::Sample {
                dst,
                coord,
                tex_reg,
                samp_reg,
                kind,
            } => {
                lower.emit_sample(*dst, coord, *tex_reg, *samp_reg, *kind)?;
            }
            DecodedOp::SampleL {
                dst,
                coord,
                tex_reg,
                samp_reg,
                lod,
                kind,
            } => {
                lower.emit_sample_l(*dst, coord, *tex_reg, *samp_reg, lod, *kind)?;
            }
            DecodedOp::SampleC {
                dst,
                coord,
                tex_reg,
                samp_reg,
                dref,
                lz,
            } => {
                lower.emit_sample_c(*dst, coord, *tex_reg, *samp_reg, dref, *lz)?;
            }
            DecodedOp::Gather4 {
                dst,
                coord,
                tex_reg,
                samp_reg,
                component,
            } => {
                lower.emit_gather4(*dst, coord, *tex_reg, *samp_reg, *component)?;
            }
            DecodedOp::Ld {
                dst,
                coord,
                tex_reg,
            } => {
                lower.emit_ld(*dst, coord, *tex_reg)?;
            }
            DecodedOp::LdMs {
                dst,
                coord,
                tex_reg,
                sample,
            } => {
                lower.emit_ld_ms(*dst, coord, *tex_reg, sample)?;
            }
            DecodedOp::Ret => { /* ret always closes the block below */ }
        }
    }
    // Unbalanced if/endif would leave open blocks the final splice can't terminate.
    if !lower.cf_stack.is_empty() {
        return Err(ShaderError::InvalidBytecode);
    }

    let temp_decls = lower.temp_decls;
    let body = lower.body;

    // Splice: temp decls first (entry-block requirement), then the body, then ret.
    b.function.extend_from_slice(&temp_decls);
    b.function.extend_from_slice(&body);
    emit(&mut b.function, SPV_OP_RETURN, &[]);
    emit(&mut b.function, SPV_OP_FUNCTION_END, &[]);

    let words = b.finish(main_id);
    Ok((words, io))
}

/// Per-function lowering state. Holds the SPIR-V builder plus the register->var
/// maps so the ALU lowering can load/store with swizzle/mask.
/// An open structured control-flow region (if or loop), tracked on a stack so
/// `else`/`endif`/`break`/`endloop` resolve their target labels by nesting.
enum CfFrame {
    If {
        else_label: u32,
        merge_label: u32,
        saw_else: bool,
    },
    Loop {
        header: u32,
        continue_label: u32,
        merge: u32,
    },
}

struct Lowering<'a> {
    b: &'a mut SpirvBuilder,
    input_var: BTreeMap<u32, u32>,
    output_var: BTreeMap<u32, u32>,
    temp_var: BTreeMap<u32, u32>,
    temp_decls: Vec<u32>,
    body: Vec<u32>,
    /// Open if/loop regions (innermost last).
    cf_stack: Vec<CfFrame>,
    /// Constant-buffer slot -> element count (uniform-block array size).
    cb_sizes: BTreeMap<u32, u32>,
}

impl Lowering<'_> {
    /// Get-or-create the Function-storage vec4 variable for temp register `reg`.
    fn ensure_temp(&mut self, reg: u32) -> u32 {
        if let Some(id) = self.temp_var.get(&reg) {
            return *id;
        }
        let ptr = self.b.ptr_vec4(SPV_STORAGE_FUNCTION);
        let id = self.b.alloc();
        emit(
            &mut self.temp_decls,
            SPV_OP_VARIABLE,
            &[ptr, id, SPV_STORAGE_FUNCTION],
        );
        self.temp_var.insert(reg, id);
        id
    }

    /// Load a source register/immediate as a vec4 (no swizzle/modifier yet).
    fn load_src_raw(&mut self, kind: SrcKind) -> Result<u32, ShaderError> {
        let vec4_ty = self.b.vec4();
        Ok(match kind {
            SrcKind::Immediate(bits) => self.b.const_vec4(bits),
            SrcKind::Input(reg) => {
                let var = *self
                    .input_var
                    .get(&reg)
                    .ok_or(ShaderError::TranslationFailed(
                        "alu: undeclared input register",
                    ))?;
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_LOAD, &[vec4_ty, id, var]);
                id
            }
            SrcKind::Temp(reg) => {
                let var = self.ensure_temp(reg);
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_LOAD, &[vec4_ty, id, var]);
                id
            }
            SrcKind::ConstBuffer {
                slot,
                index,
                dyn_index,
            } => {
                // cb<slot>[...] -> OpAccessChain into the uniform block's array
                // (member 0), then OpLoad the vec4.
                let size = self.cb_sizes.get(&slot).copied().unwrap_or(index + 1);
                let var = self.b.cbuffer_var(slot, size);
                let ptr_vec4 = self.b.ptr_to(SPV_STORAGE_UNIFORM, vec4_ty);
                let member0 = self.b.const_uint(0);
                // Element index: a uint constant (static) or a runtime int from a
                // temp lane (+ optional offset) for register-indexed access.
                let elem = match dyn_index {
                    None => self.b.const_uint(index),
                    Some((temp, comp)) => {
                        let temp_var = self.ensure_temp(temp);
                        let temp_vec = self.b.alloc();
                        emit(&mut self.body, SPV_OP_LOAD, &[vec4_ty, temp_vec, temp_var]);
                        let float_ty = self.b.float();
                        let lane = self.b.alloc();
                        emit(
                            &mut self.body,
                            SPV_OP_COMPOSITE_EXTRACT,
                            &[float_ty, lane, temp_vec, comp as u32],
                        );
                        // The index register holds an int (via ftoi) in its float
                        // bits; reinterpret to int for the access chain.
                        let int_ty = self.b.int();
                        let idx_i = self.b.alloc();
                        emit(&mut self.body, SPV_OP_BITCAST, &[int_ty, idx_i, lane]);
                        if index != 0 {
                            let off = self.b.const_int(index);
                            let sum = self.b.alloc();
                            emit(&mut self.body, SPV_OP_I_ADD, &[int_ty, sum, idx_i, off]);
                            sum
                        } else {
                            idx_i
                        }
                    }
                };
                let chain = self.b.alloc();
                emit(
                    &mut self.body,
                    SPV_OP_ACCESS_CHAIN,
                    &[ptr_vec4, chain, var, member0, elem],
                );
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_LOAD, &[vec4_ty, id, chain]);
                id
            }
        })
    }

    /// Apply a 4-lane swizzle to a vec4 value -> a new vec4 value (identity
    /// swizzle is returned unchanged to keep the module tidy).
    fn apply_swizzle(&mut self, value: u32, swizzle: [u8; 4]) -> u32 {
        if swizzle == [0, 1, 2, 3] {
            return value;
        }
        let vec4_ty = self.b.vec4();
        let id = self.b.alloc();
        // OpVectorShuffle %vec4 %res %value %value c0 c1 c2 c3
        emit(
            &mut self.body,
            SPV_OP_VECTOR_SHUFFLE,
            &[
                vec4_ty,
                id,
                value,
                value,
                swizzle[0] as u32,
                swizzle[1] as u32,
                swizzle[2] as u32,
                swizzle[3] as u32,
            ],
        );
        id
    }

    /// Apply a source modifier (neg/abs) to a vec4 value.
    fn apply_modifier(&mut self, value: u32, modifier: SrcMod) -> u32 {
        let vec4_ty = self.b.vec4();
        match modifier {
            SrcMod::None => value,
            SrcMod::Neg => {
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_F_NEGATE, &[vec4_ty, id, value]);
                id
            }
            SrcMod::Abs => self.ext1(GLSL_FABS, value),
            SrcMod::AbsNeg => {
                let a = self.ext1(GLSL_FABS, value);
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_F_NEGATE, &[vec4_ty, id, a]);
                id
            }
        }
    }

    /// Fully evaluate a source operand to a swizzled, modified vec4 value.
    fn eval_src(&mut self, src: &Src) -> Result<u32, ShaderError> {
        let raw = self.load_src_raw(src.kind)?;
        let sw = self.apply_swizzle(raw, src.swizzle);
        Ok(self.apply_modifier(sw, src.modifier))
    }

    /// Emit `OpExtInst %vec4 %id %glsl <inst> <a>` (1-arg GLSL.std.450).
    fn ext1(&mut self, inst: u32, a: u32) -> u32 {
        let vec4_ty = self.b.vec4();
        let set = self.b.glsl();
        let id = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_EXT_INST,
            &[vec4_ty, id, set, inst, a],
        );
        id
    }

    /// Emit `OpExtInst %vec4 %id %glsl <inst> <a> <b>` (2-arg GLSL.std.450).
    fn ext2(&mut self, inst: u32, a: u32, bb: u32) -> u32 {
        let vec4_ty = self.b.vec4();
        let set = self.b.glsl();
        let id = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_EXT_INST,
            &[vec4_ty, id, set, inst, a, bb],
        );
        id
    }

    /// Emit `OpExtInst %vec4 %id %glsl <inst> <a> <b> <c>` (3-arg).
    fn ext3(&mut self, inst: u32, a: u32, bb: u32, c: u32) -> u32 {
        let vec4_ty = self.b.vec4();
        let set = self.b.glsl();
        let id = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_EXT_INST,
            &[vec4_ty, id, set, inst, a, bb, c],
        );
        id
    }

    /// Bitcast a float-vec4 register value to the signed int32-vec4 typed view.
    fn to_ivec4(&mut self, fv: u32) -> u32 {
        let iv = self.b.ivec4();
        let id = self.b.alloc();
        emit(&mut self.body, SPV_OP_BITCAST, &[iv, id, fv]);
        id
    }

    /// Bitcast an int32-vec4 value back to the float-vec4 register view.
    fn to_fvec4(&mut self, iv: u32) -> u32 {
        let fv = self.b.vec4();
        let id = self.b.alloc();
        emit(&mut self.body, SPV_OP_BITCAST, &[fv, id, iv]);
        id
    }

    /// Integer binary op over the float-vec4 registers: bitcast both sources to
    /// int-vec4, apply the SPIR-V int `op`, bitcast the result back. DXBC stores
    /// every register as a 4x32-bit vector; integers are the same bits viewed as
    /// `int`, so the bitcasts are free reinterpretations (no value change).
    fn int_binop(&mut self, op: u16, a: u32, b: u32) -> u32 {
        let iv = self.b.ivec4();
        let ia = self.to_ivec4(a);
        let ib = self.to_ivec4(b);
        let r = self.b.alloc();
        emit(&mut self.body, op, &[iv, r, ia, ib]);
        self.to_fvec4(r)
    }

    /// Integer unary op (SNegate / Not) over the float-vec4 registers.
    fn int_unop(&mut self, op: u16, a: u32) -> u32 {
        let iv = self.b.ivec4();
        let ia = self.to_ivec4(a);
        let r = self.b.alloc();
        emit(&mut self.body, op, &[iv, r, ia]);
        self.to_fvec4(r)
    }

    /// Signed-integer GLSL binary (`imin`/`imax` -> SMin/SMax): bitcast both
    /// sources to signed int-vec4, apply the ext-inst with a signed result type,
    /// bitcast back into the float register.
    fn sext2(&mut self, inst: u32, a: u32, b: u32) -> u32 {
        let iv = self.b.ivec4();
        let set = self.b.glsl();
        let ia = self.to_ivec4(a);
        let ib = self.to_ivec4(b);
        let id = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_EXT_INST,
            &[iv, id, set, inst, ia, ib],
        );
        self.to_fvec4(id)
    }

    /// Unsigned-integer GLSL binary (`umin`/`umax` -> UMin/UMax): unsigned typed
    /// view so the validator sees the correct signedness.
    fn uext2(&mut self, inst: u32, a: u32, b: u32) -> u32 {
        let uv = self.b.uvec4();
        let set = self.b.glsl();
        let ua = self.to_uvec4(a);
        let ub = self.to_uvec4(b);
        let id = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_EXT_INST,
            &[uv, id, set, inst, ua, ub],
        );
        self.to_fvec4(id)
    }

    /// `imad dst, a, b, c` = `a * b + c` on int lanes (bit-reinterpreted regs).
    fn imad(&mut self, a: u32, b: u32, c: u32) -> u32 {
        let iv = self.b.ivec4();
        let ia = self.to_ivec4(a);
        let ib = self.to_ivec4(b);
        let ic = self.to_ivec4(c);
        let m = self.b.alloc();
        emit(&mut self.body, SPV_OP_I_MUL, &[iv, m, ia, ib]);
        let r = self.b.alloc();
        emit(&mut self.body, SPV_OP_I_ADD, &[iv, r, m, ic]);
        self.to_fvec4(r)
    }

    /// `bfrev`: reverse the bit order of each lane (`OpBitReverse`, uint view).
    fn bit_reverse(&mut self, a: u32) -> u32 {
        let uv = self.b.uvec4();
        let ua = self.to_uvec4(a);
        let r = self.b.alloc();
        emit(&mut self.body, SPV_OP_BIT_REVERSE, &[uv, r, ua]);
        self.to_fvec4(r)
    }

    /// `countbits`: population count per lane (`OpBitCount`, uint view; result is
    /// the same component width as the operand, per the SPIR-V spec).
    fn bit_count(&mut self, a: u32) -> u32 {
        let uv = self.b.uvec4();
        let ua = self.to_uvec4(a);
        let r = self.b.alloc();
        emit(&mut self.body, SPV_OP_BIT_COUNT, &[uv, r, ua]);
        self.to_fvec4(r)
    }

    /// `firstbit_lo`: index of the lowest set bit, `0xFFFFFFFF` if none. GLSL
    /// `FindILsb` matches D3D exactly (LSB-indexed, returns -1 for a zero input).
    fn firstbit_lo(&mut self, a: u32) -> u32 {
        let uv = self.b.uvec4();
        let set = self.b.glsl();
        let ua = self.to_uvec4(a);
        let r = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_EXT_INST,
            &[uv, r, set, GLSL_FIND_ILSB, ua],
        );
        self.to_fvec4(r)
    }

    /// `firstbit_hi` (`signed=false`) / `firstbit_shi` (`signed=true`): D3D counts
    /// the first meaningful bit DOWN from the MSB (bit31 -> 0), returning
    /// `0xFFFFFFFF` when there is none. GLSL `FindUMsb`/`FindSMsb` return an
    /// LSB-indexed position (or -1), so map `pos -> 31 - pos`, preserving the -1.
    fn firstbit_from_msb(&mut self, a: u32, signed: bool) -> u32 {
        let set = self.b.glsl();
        let (ty, operand, inst) = if signed {
            (self.b.ivec4(), self.to_ivec4(a), GLSL_FIND_SMSB)
        } else {
            (self.b.uvec4(), self.to_uvec4(a), GLSL_FIND_UMSB)
        };
        let msb = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_EXT_INST,
            &[ty, msb, set, inst, operand],
        );
        let c31 = if signed {
            self.b.const_ivec4(31)
        } else {
            self.b.const_uvec4(31)
        };
        let diff = self.b.alloc();
        emit(&mut self.body, SPV_OP_I_SUB, &[ty, diff, c31, msb]);
        // Preserve the "no meaningful bit" sentinel (-1 / 0xFFFFFFFF) unchanged.
        let neg1 = if signed {
            self.b.const_ivec4(0xFFFF_FFFF)
        } else {
            self.b.const_uvec4(0xFFFF_FFFF)
        };
        let bvec4 = self.b.bvec4();
        let is_none = self.b.alloc();
        emit(&mut self.body, SPV_OP_I_EQUAL, &[bvec4, is_none, msb, neg1]);
        let sel = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_SELECT,
            &[ty, sel, is_none, neg1, diff],
        );
        self.to_fvec4(sel)
    }

    /// D3D `ubfe`/`ibfe`: per-lane bitfield extract of `width` bits starting at
    /// `offset` from `src`. SPIR-V's `OpBitField*Extract` require SCALAR
    /// `Offset`/`Count` operands, so the vec4 is decomposed into four scalar
    /// extracts and recomposed. D3D uses only bits `[4:0]` of `width`/`offset`
    /// (a shift-count semantics), so each is masked with `& 31` — this both
    /// matches the hardware and keeps the SPIR-V result well-defined.
    fn bitfield_extract(&mut self, signed: bool, width: u32, offset: u32, src: u32) -> u32 {
        let uint_s = self.b.uint();
        let scalar_ty = if signed { self.b.int() } else { self.b.uint() };
        let vec_ty = if signed {
            self.b.ivec4()
        } else {
            self.b.uvec4()
        };
        let wu = self.to_uvec4(width);
        let ou = self.to_uvec4(offset);
        let base = if signed {
            self.to_ivec4(src)
        } else {
            self.to_uvec4(src)
        };
        let mask = self.b.const_uint(31);
        let extract_op = if signed {
            SPV_OP_BIT_FIELD_S_EXTRACT
        } else {
            SPV_OP_BIT_FIELD_U_EXTRACT
        };
        let mut comps = [0u32; 4];
        for (i, comp) in comps.iter_mut().enumerate() {
            let idx = i as u32;
            let ci_raw = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[uint_s, ci_raw, wu, idx],
            );
            let ci = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_BITWISE_AND,
                &[uint_s, ci, ci_raw, mask],
            );
            let oi_raw = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[uint_s, oi_raw, ou, idx],
            );
            let oi = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_BITWISE_AND,
                &[uint_s, oi, oi_raw, mask],
            );
            let bi = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[scalar_ty, bi, base, idx],
            );
            let ri = self.b.alloc();
            emit(&mut self.body, extract_op, &[scalar_ty, ri, bi, oi, ci]);
            *comp = ri;
        }
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_CONSTRUCT,
            &[vec_ty, res, comps[0], comps[1], comps[2], comps[3]],
        );
        self.to_fvec4(res)
    }

    /// D3D `bfi dst, width, offset, insert, base`: insert the low `width` bits of
    /// `insert` into `base` at bit `offset`, per lane. `OpBitFieldInsert` also
    /// needs scalar `Offset`/`Count`, so this decomposes the same way as
    /// [`bitfield_extract`]. `width`/`offset` masked to `[4:0]` per D3D.
    fn bitfield_insert(&mut self, width: u32, offset: u32, insert: u32, base: u32) -> u32 {
        let uint_s = self.b.uint();
        let uvec = self.b.uvec4();
        let wu = self.to_uvec4(width);
        let ou = self.to_uvec4(offset);
        let ins = self.to_uvec4(insert);
        let bas = self.to_uvec4(base);
        let mask = self.b.const_uint(31);
        let mut comps = [0u32; 4];
        for (i, comp) in comps.iter_mut().enumerate() {
            let idx = i as u32;
            let ci_raw = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[uint_s, ci_raw, wu, idx],
            );
            let ci = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_BITWISE_AND,
                &[uint_s, ci, ci_raw, mask],
            );
            let oi_raw = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[uint_s, oi_raw, ou, idx],
            );
            let oi = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_BITWISE_AND,
                &[uint_s, oi, oi_raw, mask],
            );
            let insi = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[uint_s, insi, ins, idx],
            );
            let basi = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[uint_s, basi, bas, idx],
            );
            let ri = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_BIT_FIELD_INSERT,
                &[uint_s, ri, basi, insi, oi, ci],
            );
            *comp = ri;
        }
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_CONSTRUCT,
            &[uvec, res, comps[0], comps[1], comps[2], comps[3]],
        );
        self.to_fvec4(res)
    }

    /// D3D `f32tof16`: per-lane convert a float to its fp16 bit pattern in the low
    /// 16 bits of a uint (high 16 zero). GLSL `PackHalf2x16` packs a vec2 float
    /// into a uint (x -> low 16, y -> high 16), so each lane is packed as
    /// `PackHalf2x16(vec2(lane, 0.0))` and the four uints recomposited. The result
    /// is an int-domain value, so it is bitcast into the float-vec4 register.
    fn pack_half(&mut self, src: u32) -> u32 {
        let float_s = self.b.float();
        let uint_s = self.b.uint();
        let fvec2 = self.b.fvec2();
        let uvec = self.b.uvec4();
        let set = self.b.glsl();
        let zero = self.b.const_float(0x0000_0000); // 0.0f scalar
        let mut comps = [0u32; 4];
        for (i, comp) in comps.iter_mut().enumerate() {
            let idx = i as u32;
            let lane = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[float_s, lane, src, idx],
            );
            let pair = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_CONSTRUCT,
                &[fvec2, pair, lane, zero],
            );
            let packed = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_EXT_INST,
                &[uint_s, packed, set, GLSL_PACK_HALF_2X16, pair],
            );
            *comp = packed;
        }
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_CONSTRUCT,
            &[uvec, res, comps[0], comps[1], comps[2], comps[3]],
        );
        self.to_fvec4(res)
    }

    /// D3D `f16tof32`: per-lane read the fp16 bit pattern in the low 16 bits of a
    /// uint back to a float. GLSL `UnpackHalf2x16` yields a vec2 float (low 16 ->
    /// .x, high 16 -> .y); this takes `.x` per lane. The register bits are the
    /// uint source, so they are reinterpreted (bitcast) first; the result is a
    /// genuine float value stored directly.
    fn unpack_half(&mut self, src: u32) -> u32 {
        let float_s = self.b.float();
        let uint_s = self.b.uint();
        let fvec2 = self.b.fvec2();
        let vec4 = self.b.vec4();
        let set = self.b.glsl();
        let us = self.to_uvec4(src);
        let mut comps = [0u32; 4];
        for (i, comp) in comps.iter_mut().enumerate() {
            let idx = i as u32;
            let ui = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[uint_s, ui, us, idx],
            );
            let pair = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_EXT_INST,
                &[fvec2, pair, set, GLSL_UNPACK_HALF_2X16, ui],
            );
            let lo = self.b.alloc();
            emit(
                &mut self.body,
                SPV_OP_COMPOSITE_EXTRACT,
                &[float_s, lo, pair, 0],
            );
            *comp = lo;
        }
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_CONSTRUCT,
            &[vec4, res, comps[0], comps[1], comps[2], comps[3]],
        );
        res
    }

    /// `ftoi`: convert the float VALUE in `a` to a signed int, then store the int
    /// bits in the float-vec4 register (bitcast). `a` is a genuine float here.
    fn cvt_f_to_int(&mut self, a: u32) -> u32 {
        let iv = self.b.ivec4();
        let id = self.b.alloc();
        emit(&mut self.body, SPV_OP_CONVERT_F_TO_S, &[iv, id, a]);
        self.to_fvec4(id)
    }

    /// `itof`: reinterpret the register's bits as a signed int, then convert to a
    /// genuine float value.
    fn cvt_int_to_f(&mut self, a: u32) -> u32 {
        let vec4 = self.b.vec4();
        let ia = self.to_ivec4(a);
        let id = self.b.alloc();
        emit(&mut self.body, SPV_OP_CONVERT_S_TO_F, &[vec4, id, ia]);
        id
    }

    /// Bitcast a float-vec4 register value to the unsigned int32-vec4 typed view.
    fn to_uvec4(&mut self, fv: u32) -> u32 {
        let uv = self.b.uvec4();
        let id = self.b.alloc();
        emit(&mut self.body, SPV_OP_BITCAST, &[uv, id, fv]);
        id
    }

    /// `ftou`: convert the float VALUE to an unsigned int, store the bits in the
    /// register (bitcast). Unsigned typed view satisfies the validator.
    fn cvt_f_to_uint(&mut self, a: u32) -> u32 {
        let uv = self.b.uvec4();
        let id = self.b.alloc();
        emit(&mut self.body, SPV_OP_CONVERT_F_TO_U, &[uv, id, a]);
        self.to_fvec4(id)
    }

    /// `utof`: reinterpret the register's bits as unsigned int, then convert to a
    /// genuine float value.
    fn cvt_uint_to_f(&mut self, a: u32) -> u32 {
        let vec4 = self.b.vec4();
        let ua = self.to_uvec4(a);
        let id = self.b.alloc();
        emit(&mut self.body, SPV_OP_CONVERT_U_TO_F, &[vec4, id, ua]);
        id
    }

    /// Float comparison (ge/lt/eq/ne) -> the SM4 result: a per-lane uint mask
    /// (0xFFFFFFFF on true, 0 on false) stored in the float-vec4 register. Emits
    /// `OpFOrd*` -> bvec4, then `OpSelect` of the all-ones / all-zeros int masks,
    /// then bitcasts the int mask into the float register.
    fn fcmp(&mut self, fop: u16, a: u32, b: u32) -> u32 {
        let bvec4 = self.b.bvec4();
        let cmp = self.b.alloc();
        emit(&mut self.body, fop, &[bvec4, cmp, a, b]);
        let ones = self.b.const_ivec4(0xFFFF_FFFF);
        let zeros = self.b.const_ivec4(0);
        let iv = self.b.ivec4();
        let sel = self.b.alloc();
        emit(&mut self.body, SPV_OP_SELECT, &[iv, sel, cmp, ones, zeros]);
        self.to_fvec4(sel)
    }

    /// Integer comparison (ige/ilt/ieq/ine) -> the SM4 uint mask. Reinterpret both
    /// operands as int, `OpS*`/`OpIEqual`/`OpINotEqual` -> bvec4, `OpSelect` the
    /// all-ones/all-zeros masks, bitcast into the float register.
    fn icmp(&mut self, iop: u16, a: u32, b: u32) -> u32 {
        let bvec4 = self.b.bvec4();
        let ia = self.to_ivec4(a);
        let ib = self.to_ivec4(b);
        let cmp = self.b.alloc();
        emit(&mut self.body, iop, &[bvec4, cmp, ia, ib]);
        let ones = self.b.const_ivec4(0xFFFF_FFFF);
        let zeros = self.b.const_ivec4(0);
        let iv = self.b.ivec4();
        let sel = self.b.alloc();
        emit(&mut self.body, SPV_OP_SELECT, &[iv, sel, cmp, ones, zeros]);
        self.to_fvec4(sel)
    }

    /// `movc dst, c, t, f`: per-lane `dst = (c != 0) ? t : f`. `c` is a uint mask
    /// register; reinterpret it as int, test `!= 0` to a bvec4, then `OpSelect`
    /// the (float) `t`/`f` registers.
    fn movc(&mut self, c: u32, t: u32, f: u32) -> u32 {
        let bvec4 = self.b.bvec4();
        let ci = self.to_ivec4(c);
        let zeros = self.b.const_ivec4(0);
        let cond = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_I_NOT_EQUAL,
            &[bvec4, cond, ci, zeros],
        );
        let vec4 = self.b.vec4();
        let id = self.b.alloc();
        emit(&mut self.body, SPV_OP_SELECT, &[vec4, id, cond, t, f]);
        id
    }

    /// A vec4 broadcast of a single scalar id (used by dot products / rcp).
    fn broadcast(&mut self, scalar: u32) -> u32 {
        let vec4_ty = self.b.vec4();
        let id = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_CONSTRUCT,
            &[vec4_ty, id, scalar, scalar, scalar, scalar],
        );
        id
    }

    /// A vec4 const splat of `bits` (f32 bit pattern) for clamp/reciprocal.
    fn const_splat(&mut self, bits: u32) -> u32 {
        self.b.const_vec4([bits, bits, bits, bits])
    }

    /// Dot product of two vec4s over `width` lanes -> scalar -> broadcast vec4.
    fn dot(&mut self, a: u32, bb: u32, width: u32) -> Result<u32, ShaderError> {
        // OpDot needs equal-width float vectors. Shuffle each operand down to the
        // first `width` lanes when width < 4.
        let (la, lb) = if width == 4 {
            (a, bb)
        } else {
            let vty = self.b.vecn(width);
            let mk = |this: &mut Self, v: u32| -> u32 {
                let id = this.b.alloc();
                let mut ops = Vec::with_capacity(4 + width as usize);
                ops.push(vty);
                ops.push(id);
                ops.push(v);
                ops.push(v);
                for c in 0..width {
                    ops.push(c);
                }
                emit(&mut this.body, SPV_OP_VECTOR_SHUFFLE, &ops);
                id
            };
            (mk(self, a), mk(self, bb))
        };
        let float_ty = self.b.float();
        let scalar = self.b.alloc();
        emit(&mut self.body, SPV_OP_DOT, &[float_ty, scalar, la, lb]);
        Ok(self.broadcast(scalar))
    }

    /// Lower one ALU instruction: evaluate sources, compute a vec4 result,
    /// optionally saturate, then write only the masked lanes of the dest.
    fn lower_alu(
        &mut self,
        op: AluOp,
        dst: Dst,
        srcs: &[Src],
        saturate: bool,
    ) -> Result<(), ShaderError> {
        let vec4_ty = self.b.vec4();

        // Evaluate sources.
        let mut sv = Vec::with_capacity(srcs.len());
        for s in srcs {
            sv.push(self.eval_src(s)?);
        }
        let need = match op {
            AluOp::Bfi => 4,
            AluOp::Mad | AluOp::Movc | AluOp::IMad | AluOp::Ubfe | AluOp::Ibfe => 3,
            AluOp::Add
            | AluOp::Mul
            | AluOp::Div
            | AluOp::Min
            | AluOp::Max
            | AluOp::Dp2
            | AluOp::Dp3
            | AluOp::Dp4
            | AluOp::IAdd
            | AluOp::And
            | AluOp::Or
            | AluOp::Xor
            | AluOp::IShl
            | AluOp::IShr
            | AluOp::UShr
            | AluOp::Ge
            | AluOp::Lt
            | AluOp::Eq
            | AluOp::Ne
            | AluOp::Ige
            | AluOp::Ilt
            | AluOp::Ieq
            | AluOp::Ine
            | AluOp::IMin
            | AluOp::IMax
            | AluOp::UMin
            | AluOp::UMax => 2,
            _ => 1,
        };
        if sv.len() != need {
            return Err(ShaderError::TranslationFailed("alu: wrong source arity"));
        }

        // Compute the full-vec4 result.
        let mut result = match op {
            AluOp::Mov => sv[0],
            AluOp::Add => {
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_F_ADD, &[vec4_ty, id, sv[0], sv[1]]);
                id
            }
            AluOp::Mul => {
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_F_MUL, &[vec4_ty, id, sv[0], sv[1]]);
                id
            }
            AluOp::Div => {
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_F_DIV, &[vec4_ty, id, sv[0], sv[1]]);
                id
            }
            AluOp::Mad => self.ext3(GLSL_FMA, sv[0], sv[1], sv[2]),
            AluOp::Min => self.ext2(GLSL_FMIN, sv[0], sv[1]),
            AluOp::Max => self.ext2(GLSL_FMAX, sv[0], sv[1]),
            AluOp::Sqrt => self.ext1(GLSL_SQRT, sv[0]),
            AluOp::Rsq => self.ext1(GLSL_INVERSE_SQRT, sv[0]),
            AluOp::Frc => self.ext1(GLSL_FRACT, sv[0]),
            AluOp::RoundNe => self.ext1(GLSL_ROUND_EVEN, sv[0]),
            AluOp::RoundNi => self.ext1(GLSL_FLOOR, sv[0]),
            AluOp::RoundPi => self.ext1(GLSL_CEIL, sv[0]),
            AluOp::RoundZ => self.ext1(GLSL_TRUNC, sv[0]),
            AluOp::Exp2 => self.ext1(GLSL_EXP2, sv[0]),
            AluOp::Log2 => self.ext1(GLSL_LOG2, sv[0]),
            AluOp::DdX => {
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_DPDX, &[vec4_ty, id, sv[0]]);
                id
            }
            AluOp::DdY => {
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_DPDY, &[vec4_ty, id, sv[0]]);
                id
            }
            AluOp::IAdd => self.int_binop(SPV_OP_I_ADD, sv[0], sv[1]),
            AluOp::INeg => self.int_unop(SPV_OP_S_NEGATE, sv[0]),
            AluOp::And => self.int_binop(SPV_OP_BITWISE_AND, sv[0], sv[1]),
            AluOp::Or => self.int_binop(SPV_OP_BITWISE_OR, sv[0], sv[1]),
            AluOp::Xor => self.int_binop(SPV_OP_BITWISE_XOR, sv[0], sv[1]),
            AluOp::Not => self.int_unop(SPV_OP_NOT, sv[0]),
            AluOp::IShl => self.int_binop(SPV_OP_SHIFT_LEFT_LOGICAL, sv[0], sv[1]),
            AluOp::IShr => self.int_binop(SPV_OP_SHIFT_RIGHT_ARITHMETIC, sv[0], sv[1]),
            AluOp::UShr => self.int_binop(SPV_OP_SHIFT_RIGHT_LOGICAL, sv[0], sv[1]),
            AluOp::FToI => self.cvt_f_to_int(sv[0]),
            AluOp::IToF => self.cvt_int_to_f(sv[0]),
            AluOp::FToU => self.cvt_f_to_uint(sv[0]),
            AluOp::UToF => self.cvt_uint_to_f(sv[0]),
            AluOp::Ge => self.fcmp(SPV_OP_F_ORD_GREATER_THAN_EQUAL, sv[0], sv[1]),
            AluOp::Lt => self.fcmp(SPV_OP_F_ORD_LESS_THAN, sv[0], sv[1]),
            AluOp::Eq => self.fcmp(SPV_OP_F_ORD_EQUAL, sv[0], sv[1]),
            AluOp::Ne => self.fcmp(SPV_OP_F_ORD_NOT_EQUAL, sv[0], sv[1]),
            AluOp::Ige => self.icmp(SPV_OP_S_GREATER_THAN_EQUAL, sv[0], sv[1]),
            AluOp::Ilt => self.icmp(SPV_OP_S_LESS_THAN, sv[0], sv[1]),
            AluOp::Ieq => self.icmp(SPV_OP_I_EQUAL, sv[0], sv[1]),
            AluOp::Ine => self.icmp(SPV_OP_I_NOT_EQUAL, sv[0], sv[1]),
            AluOp::IMin => self.sext2(GLSL_SMIN, sv[0], sv[1]),
            AluOp::IMax => self.sext2(GLSL_SMAX, sv[0], sv[1]),
            AluOp::UMin => self.uext2(GLSL_UMIN, sv[0], sv[1]),
            AluOp::UMax => self.uext2(GLSL_UMAX, sv[0], sv[1]),
            AluOp::IMad => self.imad(sv[0], sv[1], sv[2]),
            AluOp::BfRev => self.bit_reverse(sv[0]),
            AluOp::CountBits => self.bit_count(sv[0]),
            AluOp::FirstBitLo => self.firstbit_lo(sv[0]),
            AluOp::FirstBitHi => self.firstbit_from_msb(sv[0], false),
            AluOp::FirstBitShi => self.firstbit_from_msb(sv[0], true),
            AluOp::Ubfe => self.bitfield_extract(false, sv[0], sv[1], sv[2]),
            AluOp::Ibfe => self.bitfield_extract(true, sv[0], sv[1], sv[2]),
            AluOp::Bfi => self.bitfield_insert(sv[0], sv[1], sv[2], sv[3]),
            AluOp::F32ToF16 => self.pack_half(sv[0]),
            AluOp::F16ToF32 => self.unpack_half(sv[0]),
            AluOp::Movc => self.movc(sv[0], sv[1], sv[2]),
            AluOp::Rcp => {
                // reciprocal: 1.0 / x, lane-wise.
                let one = self.const_splat(0x3F80_0000); // 1.0f
                let id = self.b.alloc();
                emit(&mut self.body, SPV_OP_F_DIV, &[vec4_ty, id, one, sv[0]]);
                id
            }
            AluOp::Dp2 => self.dot(sv[0], sv[1], 2)?,
            AluOp::Dp3 => self.dot(sv[0], sv[1], 3)?,
            AluOp::Dp4 => self.dot(sv[0], sv[1], 4)?,
        };

        // Saturate (_sat result modifier): clamp 0..1 lane-wise.
        if saturate {
            let zero = self.const_splat(0x0000_0000); // 0.0f
            let one = self.const_splat(0x3F80_0000); // 1.0f
            result = self.ext3(GLSL_FCLAMP, result, zero, one);
        }

        // Write the masked lanes back to the destination.
        self.store_masked(dst, result)
    }

    /// Test lane 0 of a condition register as a bool: reinterpret the bits as int
    /// and compare `!= 0` (`test_nz`) or `== 0`. Shared by `if`/`breakc`.
    fn cond_bool(&mut self, cond: &Src, test_nz: bool) -> Result<u32, ShaderError> {
        let cv = self.eval_src(cond)?;
        let float_ty = self.b.float();
        let lane = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_EXTRACT,
            &[float_ty, lane, cv, 0],
        );
        let int_ty = self.b.int();
        let lane_i = self.b.alloc();
        emit(&mut self.body, SPV_OP_BITCAST, &[int_ty, lane_i, lane]);
        let zero = self.b.const_int(0);
        let bool_ty = self.b.bool_t();
        let cond_b = self.b.alloc();
        let cmp = if test_nz {
            SPV_OP_I_NOT_EQUAL
        } else {
            SPV_OP_I_EQUAL
        };
        emit(&mut self.body, cmp, &[bool_ty, cond_b, lane_i, zero]);
        Ok(cond_b)
    }

    /// `if_nz`/`if_z`: open a structured-if; the then-block follows.
    fn emit_if(&mut self, cond: &Src, test_nz: bool) -> Result<(), ShaderError> {
        let cond_b = self.cond_bool(cond, test_nz)?;
        let then_label = self.b.alloc();
        let else_label = self.b.alloc();
        let merge_label = self.b.alloc();
        emit(&mut self.body, SPV_OP_SELECTION_MERGE, &[merge_label, 0]);
        emit(
            &mut self.body,
            SPV_OP_BRANCH_CONDITIONAL,
            &[cond_b, then_label, else_label],
        );
        emit(&mut self.body, SPV_OP_LABEL, &[then_label]);
        self.cf_stack.push(CfFrame::If {
            else_label,
            merge_label,
            saw_else: false,
        });
        Ok(())
    }

    /// `discard_nz`/`discard_z`: kill the fragment when the condition holds. A
    /// structured selection whose taken branch is a single `OpKill` block (a
    /// block terminator), the merge block continuing the shader. This is the
    /// HLSL `clip()` / alpha-test idiom — common in real pixel shaders.
    fn emit_discard(&mut self, cond: &Src, test_nz: bool) -> Result<(), ShaderError> {
        let cond_b = self.cond_bool(cond, test_nz)?;
        let kill_label = self.b.alloc();
        let merge_label = self.b.alloc();
        emit(&mut self.body, SPV_OP_SELECTION_MERGE, &[merge_label, 0]);
        emit(
            &mut self.body,
            SPV_OP_BRANCH_CONDITIONAL,
            &[cond_b, kill_label, merge_label],
        );
        emit(&mut self.body, SPV_OP_LABEL, &[kill_label]);
        emit(&mut self.body, SPV_OP_KILL, &[]);
        emit(&mut self.body, SPV_OP_LABEL, &[merge_label]);
        Ok(())
    }

    /// `else`: close the then-block, open the else-block.
    fn emit_else(&mut self) -> Result<(), ShaderError> {
        match self.cf_stack.last_mut() {
            Some(CfFrame::If {
                else_label,
                merge_label,
                saw_else,
            }) => {
                if *saw_else {
                    return Err(ShaderError::InvalidBytecode); // double else
                }
                let (else_label, merge_label) = (*else_label, *merge_label);
                *saw_else = true;
                emit(&mut self.body, SPV_OP_BRANCH, &[merge_label]);
                emit(&mut self.body, SPV_OP_LABEL, &[else_label]);
                Ok(())
            }
            _ => Err(ShaderError::InvalidBytecode), // else without matching if
        }
    }

    /// `endif`: terminate the current block, open the merge block (synthesizing an
    /// empty else-block if there was no `else`).
    fn emit_endif(&mut self) -> Result<(), ShaderError> {
        match self.cf_stack.pop() {
            Some(CfFrame::If {
                else_label,
                merge_label,
                saw_else,
            }) => {
                emit(&mut self.body, SPV_OP_BRANCH, &[merge_label]);
                if !saw_else {
                    emit(&mut self.body, SPV_OP_LABEL, &[else_label]);
                    emit(&mut self.body, SPV_OP_BRANCH, &[merge_label]);
                }
                emit(&mut self.body, SPV_OP_LABEL, &[merge_label]);
                Ok(())
            }
            _ => Err(ShaderError::InvalidBytecode), // endif without matching if
        }
    }

    /// `loop`: open a structured infinite loop (exited by `break`/`breakc`). Emits
    /// the header (with `OpLoopMerge`) and falls into the loop body.
    fn emit_loop(&mut self) {
        let header = self.b.alloc();
        let body_label = self.b.alloc();
        let continue_label = self.b.alloc();
        let merge = self.b.alloc();
        emit(&mut self.body, SPV_OP_BRANCH, &[header]);
        emit(&mut self.body, SPV_OP_LABEL, &[header]);
        emit(
            &mut self.body,
            SPV_OP_LOOP_MERGE,
            &[merge, continue_label, 0],
        );
        emit(&mut self.body, SPV_OP_BRANCH, &[body_label]);
        emit(&mut self.body, SPV_OP_LABEL, &[body_label]);
        self.cf_stack.push(CfFrame::Loop {
            header,
            continue_label,
            merge,
        });
    }

    /// The merge label of the innermost enclosing loop (the `break` target).
    fn innermost_loop_merge(&self) -> Option<u32> {
        self.cf_stack.iter().rev().find_map(|f| match f {
            CfFrame::Loop { merge, .. } => Some(*merge),
            _ => None,
        })
    }

    /// `break`: branch to the innermost loop's merge. Opens a fresh (unreachable)
    /// block so any trailing ops before `endloop` still have a block to live in.
    fn emit_break(&mut self) -> Result<(), ShaderError> {
        let merge = self
            .innermost_loop_merge()
            .ok_or(ShaderError::InvalidBytecode)?;
        emit(&mut self.body, SPV_OP_BRANCH, &[merge]);
        let dead = self.b.alloc();
        emit(&mut self.body, SPV_OP_LABEL, &[dead]);
        Ok(())
    }

    /// `breakc_nz`/`breakc_z`: conditional break — branch to the loop merge when the
    /// condition holds, else continue in the loop body.
    fn emit_breakc(&mut self, cond: &Src, test_nz: bool) -> Result<(), ShaderError> {
        let merge = self
            .innermost_loop_merge()
            .ok_or(ShaderError::InvalidBytecode)?;
        let cond_b = self.cond_bool(cond, test_nz)?;
        let cont = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_BRANCH_CONDITIONAL,
            &[cond_b, merge, cont],
        );
        emit(&mut self.body, SPV_OP_LABEL, &[cont]);
        Ok(())
    }

    /// `endloop`: terminate the body, emit the continue block (back-edge to the
    /// header), and open the merge block.
    fn emit_endloop(&mut self) -> Result<(), ShaderError> {
        match self.cf_stack.pop() {
            Some(CfFrame::Loop {
                header,
                continue_label,
                merge,
            }) => {
                emit(&mut self.body, SPV_OP_BRANCH, &[continue_label]);
                emit(&mut self.body, SPV_OP_LABEL, &[continue_label]);
                emit(&mut self.body, SPV_OP_BRANCH, &[header]); // back-edge
                emit(&mut self.body, SPV_OP_LABEL, &[merge]);
                Ok(())
            }
            _ => Err(ShaderError::InvalidBytecode), // endloop without matching loop
        }
    }

    /// `sincos dest_sin, dest_cos, src`: write sin(src) and/or cos(src) (per the
    /// non-null destinations) via GLSL.std.450 Sin/Cos.
    fn emit_sincos(
        &mut self,
        dst_sin: &Option<Dst>,
        dst_cos: &Option<Dst>,
        src: &Src,
    ) -> Result<(), ShaderError> {
        let s = self.eval_src(src)?;
        if let Some(d) = dst_sin {
            let v = self.ext1(GLSL_SIN, s);
            self.store_masked(*d, v)?;
        }
        if let Some(d) = dst_cos {
            let v = self.ext1(GLSL_COS, s);
            self.store_masked(*d, v)?;
        }
        Ok(())
    }

    /// `sample dst, coord, t#, s#`: Texture2D sample. Loads the bound image +
    /// sampler, combines them, samples at `coord.xy` (implicit LOD), stores the
    /// RGBA result to `dst`.
    fn emit_sample(
        &mut self,
        dst: Dst,
        coord: &Src,
        tex_reg: u32,
        samp_reg: u32,
        kind: TexKind,
    ) -> Result<(), ShaderError> {
        let img_var = self.b.texture_var(tex_reg, kind);
        let smp_var = self.b.sampler_var(samp_reg);
        let img_ty = self.b.image_for(kind);
        let smp_ty = self.b.sampler_t();
        let si_ty = self.b.sampled_image_for(kind);
        let vec4_ty = self.b.vec4();

        let img = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[img_ty, img, img_var]);
        let smp = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[smp_ty, smp, smp_var]);
        let si = self.b.alloc();
        emit(&mut self.body, SPV_OP_SAMPLED_IMAGE, &[si_ty, si, img, smp]);

        let coordv = self.sample_coord(coord, kind)?;
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_IMAGE_SAMPLE_IMPLICIT_LOD,
            &[vec4_ty, res, si, coordv],
        );
        self.store_masked(dst, res)
    }

    /// Build the sample coordinate vector from `coord`: `.xy` (vec2) for a plain
    /// Texture2D, `.xyz` (vec3) for a Texture2DArray (uv + slice) or a TextureCube
    /// (xyz direction).
    fn sample_coord(&mut self, coord: &Src, kind: TexKind) -> Result<u32, ShaderError> {
        let cv = self.eval_src(coord)?;
        let out = self.b.alloc();
        match kind.coord_components() {
            3 => {
                let vec3_ty = self.b.vecn(3);
                emit(
                    &mut self.body,
                    SPV_OP_VECTOR_SHUFFLE,
                    &[vec3_ty, out, cv, cv, 0, 1, 2],
                );
            }
            _ => {
                let vec2_ty = self.b.vecn(2);
                emit(
                    &mut self.body,
                    SPV_OP_VECTOR_SHUFFLE,
                    &[vec2_ty, out, cv, cv, 0, 1],
                );
            }
        }
        Ok(out)
    }

    /// `sample_l dst, coord, t#, s#, lod`: like `emit_sample` but with an explicit
    /// LOD (OpImageSampleExplicitLod + the Lod image operand). Valid in any stage
    /// (explicit LOD, unlike implicit, is not fragment-only).
    fn emit_sample_l(
        &mut self,
        dst: Dst,
        coord: &Src,
        tex_reg: u32,
        samp_reg: u32,
        lod: &Src,
        kind: TexKind,
    ) -> Result<(), ShaderError> {
        let img_var = self.b.texture_var(tex_reg, kind);
        let smp_var = self.b.sampler_var(samp_reg);
        let img_ty = self.b.image_for(kind);
        let smp_ty = self.b.sampler_t();
        let si_ty = self.b.sampled_image_for(kind);
        let vec4_ty = self.b.vec4();
        let float_ty = self.b.float();

        let img = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[img_ty, img, img_var]);
        let smp = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[smp_ty, smp, smp_var]);
        let si = self.b.alloc();
        emit(&mut self.body, SPV_OP_SAMPLED_IMAGE, &[si_ty, si, img, smp]);

        let coordv = self.sample_coord(coord, kind)?;
        // LOD scalar = lane 0 of the lod source.
        let lv = self.eval_src(lod)?;
        let lod_scalar = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_EXTRACT,
            &[float_ty, lod_scalar, lv, 0],
        );
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_IMAGE_SAMPLE_EXPLICIT_LOD,
            &[vec4_ty, res, si, coordv, SPV_IMAGE_OPERAND_LOD, lod_scalar],
        );
        self.store_masked(dst, res)
    }

    /// `sample_c`/`sample_c_lz`: depth-comparison (shadow) sample. Loads a 2D
    /// depth image, samples with the Dref (comparison) value via
    /// OpImageSampleDref{Implicit,Explicit}Lod, and splats the SCALAR result
    /// across the dst lanes (the comparison value is a scalar; DXBC writes one
    /// lane then mov-splats). `lz` uses explicit LOD 0.
    fn emit_sample_c(
        &mut self,
        dst: Dst,
        coord: &Src,
        tex_reg: u32,
        samp_reg: u32,
        dref: &Src,
        lz: bool,
    ) -> Result<(), ShaderError> {
        let kind = TexKind::Depth2D;
        let img_var = self.b.texture_var(tex_reg, kind);
        let smp_var = self.b.sampler_var(samp_reg);
        let img_ty = self.b.image_for(kind);
        let smp_ty = self.b.sampler_t();
        let si_ty = self.b.sampled_image_for(kind);
        let vec4_ty = self.b.vec4();
        let float_ty = self.b.float();

        let img = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[img_ty, img, img_var]);
        let smp = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[smp_ty, smp, smp_var]);
        let si = self.b.alloc();
        emit(&mut self.body, SPV_OP_SAMPLED_IMAGE, &[si_ty, si, img, smp]);

        let coordv = self.sample_coord(coord, kind)?;
        // Dref = lane 0 of the reference source (a scalar comparison value).
        let dv = self.eval_src(dref)?;
        let dref_scalar = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_EXTRACT,
            &[float_ty, dref_scalar, dv, 0],
        );
        // The Dref sample returns a SCALAR float.
        let scalar = self.b.alloc();
        if lz {
            let zero_lod = self.b.const_float(0);
            emit(
                &mut self.body,
                SPV_OP_IMAGE_SAMPLE_DREF_EXPLICIT_LOD,
                &[
                    float_ty,
                    scalar,
                    si,
                    coordv,
                    dref_scalar,
                    SPV_IMAGE_OPERAND_LOD,
                    zero_lod,
                ],
            );
        } else {
            emit(
                &mut self.body,
                SPV_OP_IMAGE_SAMPLE_DREF_IMPLICIT_LOD,
                &[float_ty, scalar, si, coordv, dref_scalar],
            );
        }
        // Splat the scalar across a vec4 so store_masked can write the dst lanes.
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_CONSTRUCT,
            &[vec4_ty, res, scalar, scalar, scalar, scalar],
        );
        self.store_masked(dst, res)
    }

    /// `gather4`: fetch the 4 bilinear-footprint texels of channel `component`
    /// (Texture2D.Gather) -> OpImageGather, a vec4 result.
    fn emit_gather4(
        &mut self,
        dst: Dst,
        coord: &Src,
        tex_reg: u32,
        samp_reg: u32,
        component: u8,
    ) -> Result<(), ShaderError> {
        let kind = TexKind::Tex2D;
        let img_var = self.b.texture_var(tex_reg, kind);
        let smp_var = self.b.sampler_var(samp_reg);
        let img_ty = self.b.image_for(kind);
        let smp_ty = self.b.sampler_t();
        let si_ty = self.b.sampled_image_for(kind);
        let vec4_ty = self.b.vec4();

        let img = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[img_ty, img, img_var]);
        let smp = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[smp_ty, smp, smp_var]);
        let si = self.b.alloc();
        emit(&mut self.body, SPV_OP_SAMPLED_IMAGE, &[si_ty, si, img, smp]);

        let coordv = self.sample_coord(coord, kind)?;
        let comp = self.b.const_int(component as u32);
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_IMAGE_GATHER,
            &[vec4_ty, res, si, coordv, comp],
        );
        self.store_masked(dst, res)
    }

    /// Reinterpret a source's bits as an integer vec4 (the texel-fetch coord holds
    /// ints in its float bits, e.g. from `ftoi`); returns the ivec4 id.
    fn coord_as_ivec4(&mut self, coord: &Src) -> Result<u32, ShaderError> {
        let cv = self.eval_src(coord)?;
        let ivec4_ty = self.b.ivec4();
        let ci = self.b.alloc();
        emit(&mut self.body, SPV_OP_BITCAST, &[ivec4_ty, ci, cv]);
        Ok(ci)
    }

    /// `ld`: Texture2D.Load — exact texel fetch via OpImageFetch. The integer
    /// coord's `.xy` are the texel and `.z` the mip (the Lod image operand).
    fn emit_ld(&mut self, dst: Dst, coord: &Src, tex_reg: u32) -> Result<(), ShaderError> {
        let kind = TexKind::Tex2D;
        let img_var = self.b.texture_var(tex_reg, kind);
        let img_ty = self.b.image_for(kind);
        let vec4_ty = self.b.vec4();
        let img = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[img_ty, img, img_var]);

        let ci = self.coord_as_ivec4(coord)?;
        let ivec2_ty = self.b.ivec2();
        let coord2 = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_VECTOR_SHUFFLE,
            &[ivec2_ty, coord2, ci, ci, 0, 1],
        );
        let int_ty = self.b.int();
        let mip = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_EXTRACT,
            &[int_ty, mip, ci, 2],
        );
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_IMAGE_FETCH,
            &[vec4_ty, res, img, coord2, SPV_IMAGE_OPERAND_LOD, mip],
        );
        self.store_masked(dst, res)
    }

    /// `ld_ms`: Texture2DMS.Load — multisampled texel fetch (OpImageFetch + the
    /// Sample image operand). Integer `.xy` coord + a sample index.
    fn emit_ld_ms(
        &mut self,
        dst: Dst,
        coord: &Src,
        tex_reg: u32,
        sample: &Src,
    ) -> Result<(), ShaderError> {
        let kind = TexKind::Tex2DMS;
        let img_var = self.b.texture_var(tex_reg, kind);
        let img_ty = self.b.image_for(kind);
        let vec4_ty = self.b.vec4();
        let img = self.b.alloc();
        emit(&mut self.body, SPV_OP_LOAD, &[img_ty, img, img_var]);

        let ci = self.coord_as_ivec4(coord)?;
        let ivec2_ty = self.b.ivec2();
        let coord2 = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_VECTOR_SHUFFLE,
            &[ivec2_ty, coord2, ci, ci, 0, 1],
        );
        // sample index = lane 0 of the sample source, reinterpreted as int.
        let si = self.coord_as_ivec4(sample)?;
        let int_ty = self.b.int();
        let sidx = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_COMPOSITE_EXTRACT,
            &[int_ty, sidx, si, 0],
        );
        let res = self.b.alloc();
        emit(
            &mut self.body,
            SPV_OP_IMAGE_FETCH,
            &[vec4_ty, res, img, coord2, SPV_IMAGE_OPERAND_SAMPLE, sidx],
        );
        self.store_masked(dst, res)
    }

    /// Store a vec4 `value` into the destination honouring its write-mask: lanes
    /// outside the mask keep the dest's previous contents (load + shuffle-merge).
    fn store_masked(&mut self, dst: Dst, value: u32) -> Result<(), ShaderError> {
        let vec4_ty = self.b.vec4();
        let dst_var = match dst.kind {
            DstKind::Output(reg) => {
                *self
                    .output_var
                    .get(&reg)
                    .ok_or(ShaderError::TranslationFailed(
                        "alu: undeclared output register",
                    ))?
            }
            DstKind::Temp(reg) => self.ensure_temp(reg),
        };
        let mask = dst.write_mask & 0xF;
        let to_store = if mask == 0xF {
            value
        } else {
            // Load the current dest, then build a merged vec4: for each lane,
            // select from `value` (lanes 0..3) if masked, else from the old dest
            // (lanes 4..7 in OpVectorShuffle's combined index space).
            let old = self.b.alloc();
            emit(&mut self.body, SPV_OP_LOAD, &[vec4_ty, old, dst_var]);
            let merged = self.b.alloc();
            let mut ops = Vec::with_capacity(8);
            ops.push(vec4_ty);
            ops.push(merged);
            ops.push(value); // vector 1 -> indices 0..3
            ops.push(old); //   vector 2 -> indices 4..7
            for lane in 0..4u32 {
                if mask & (1 << lane) != 0 {
                    ops.push(lane); // take from `value`
                } else {
                    ops.push(4 + lane); // keep old dest lane
                }
            }
            emit(&mut self.body, SPV_OP_VECTOR_SHUFFLE, &ops);
            merged
        };
        emit(&mut self.body, SPV_OP_STORE, &[dst_var, to_store]);
        Ok(())
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Translate a DXBC (SM4/SM5) shader to SPIR-V. Slice-1 supported subset:
/// `mov`, `ret`, `dcl_*`. Returns `Err` (never panics) on malformed/hostile
/// bytes or any unsupported opcode.
pub fn translate(dxbc: &[u8], _opts: TranslateOpts) -> Result<Translated, ShaderError> {
    let chunks = collect_chunks(dxbc)?;

    // Determine stage from the SHEX version token (program_type).
    if chunks.shex.len() < 4 {
        return Err(ShaderError::InvalidBytecode);
    }
    let version_token = rd_u32(chunks.shex, 0)?;
    let program_type = (version_token >> 16) & 0xFFFF;
    let stage = match program_type {
        0 => raegfx::ShaderStage::Fragment,
        1 => raegfx::ShaderStage::Vertex,
        2 => raegfx::ShaderStage::Geometry,
        3 => raegfx::ShaderStage::TessControl,
        4 => raegfx::ShaderStage::TessEvaluation,
        5 => raegfx::ShaderStage::Compute,
        other => return Err(ShaderError::UnsupportedShaderModel(other)),
    };
    // Slice 1 supports only vertex + fragment.
    if !matches!(
        stage,
        raegfx::ShaderStage::Vertex | raegfx::ShaderStage::Fragment
    ) {
        return Err(ShaderError::UnsupportedShaderModel(program_type));
    }

    let inputs = match chunks.isgn {
        Some(b) => parse_signature(b)?,
        None => Vec::new(),
    };
    let outputs = match chunks.osgn {
        Some(b) => parse_signature(b)?,
        None => Vec::new(),
    };

    let (ops, _temps, cb_sizes) = decode_shex(chunks.shex)?;

    let (words, io) = translate_inner(stage, &ops, &inputs, &outputs, &cb_sizes)?;

    let mut spirv = Vec::with_capacity(words.len() * 4);
    for w in &words {
        spirv.extend_from_slice(&w.to_le_bytes());
    }

    Ok(Translated {
        spirv,
        stage,
        bindings: BindingLayout::default(),
        io,
    })
}

// ── R10 boot self-test (FAIL-able) ──────────────────────────────────────────

/// Embedded passthrough-VS fixture (fxc vs_5_0), so the boot smoketest needs no
/// filesystem. The kernel-side `[raebridge]` smoketest calls `run_self_test()`.
pub const EMBED_PASSTHROUGH_VS: &[u8] = include_bytes!("../tests/fixtures/passthrough_vs.dxbc");

/// Embedded slice-2 ALU pixel shader (fxc ps_5_0): `dp3_sat` + negate + `mad`
/// with swizzles, a write-mask and a temp. Lets the boot self-test prove the
/// slice-2 ALU/swizzle/modifier path off-target, not just the slice-1 passthrough.
pub const EMBED_ALU_PS: &[u8] = include_bytes!("../tests/fixtures/alu_ps.dxbc");

/// Translate the embedded passthrough-VS and the slice-2 ALU PS and verify each
/// emitted SPIR-V module is structurally valid. Returns false on any error — a
/// real FAIL signal, not a tautology.
pub fn run_self_test() -> bool {
    // Slice-1 passthrough VS.
    let t = match translate(EMBED_PASSTHROUGH_VS, TranslateOpts::default()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    if t.stage != raegfx::ShaderStage::Vertex {
        return false;
    }
    if t.spirv.len() < 20 {
        return false;
    }
    let magic = u32::from_le_bytes([t.spirv[0], t.spirv[1], t.spirv[2], t.spirv[3]]);
    if magic != SPIRV_MAGIC {
        return false;
    }
    let bound = u32::from_le_bytes([t.spirv[12], t.spirv[13], t.spirv[14], t.spirv[15]]);
    if bound <= 1 {
        return false;
    }
    if !has_entrypoint(&t.spirv, SPV_EXECMODEL_VERTEX) {
        return false;
    }

    // Slice-2 ALU PS: must translate and must actually contain the lowered
    // OpDot (dp3), OpExtInst (mad/_sat) and OpVectorShuffle (swizzle) the ALU
    // path emits — proves slice 2 lowered, not just slice 1.
    let p = match translate(EMBED_ALU_PS, TranslateOpts::default()) {
        Ok(p) => p,
        Err(_) => return false,
    };
    if p.stage != raegfx::ShaderStage::Fragment {
        return false;
    }
    if !has_entrypoint(&p.spirv, SPV_EXECMODEL_FRAGMENT) {
        return false;
    }
    count_spirv_op(&p.spirv, SPV_OP_DOT) >= 1
        && count_spirv_op(&p.spirv, SPV_OP_EXT_INST) >= 1
        && count_spirv_op(&p.spirv, SPV_OP_VECTOR_SHUFFLE) >= 1
}

/// Count SPIR-V instructions of one opcode in a byte stream (header-skipping,
/// bounds-checked; bails on a malformed word-count rather than looping).
fn count_spirv_op(spirv: &[u8], opcode: u16) -> usize {
    let mut i = 20usize;
    let mut n = 0;
    while i + 4 <= spirv.len() {
        let w = u32::from_le_bytes([spirv[i], spirv[i + 1], spirv[i + 2], spirv[i + 3]]);
        let wc = (w >> 16) as usize;
        if wc == 0 {
            break;
        }
        if (w & 0xFFFF) as u16 == opcode {
            n += 1;
        }
        i += wc * 4;
    }
    n
}

/// The translated `bound` for the embedded passthrough-VS, for procfs/log lines.
pub fn self_test_bound() -> u32 {
    match translate(EMBED_PASSTHROUGH_VS, TranslateOpts::default()) {
        Ok(t) if t.spirv.len() >= 16 => {
            u32::from_le_bytes([t.spirv[12], t.spirv[13], t.spirv[14], t.spirv[15]])
        }
        _ => 0,
    }
}

/// Scan a SPIR-V byte stream for an `OpEntryPoint` with the given execution model.
fn has_entrypoint(spirv: &[u8], model: u32) -> bool {
    // Skip the 5-word header.
    let mut i = 20usize;
    while i + 4 <= spirv.len() {
        let w = u32::from_le_bytes([spirv[i], spirv[i + 1], spirv[i + 2], spirv[i + 3]]);
        let wc = (w >> 16) as usize;
        let op = (w & 0xFFFF) as u16;
        if wc == 0 {
            return false; // malformed; bail rather than loop forever
        }
        if op == SPV_OP_ENTRY_POINT && i + 8 <= spirv.len() {
            let m = u32::from_le_bytes([spirv[i + 4], spirv[i + 5], spirv[i + 6], spirv[i + 7]]);
            if m == model {
                return true;
            }
        }
        i += wc * 4;
    }
    false
}
