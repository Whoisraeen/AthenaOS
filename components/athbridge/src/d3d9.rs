//! d3d9.dll — Direct3D 9 API emulation for AthBridge.
//!
//! Provides full IDirect3D9 and IDirect3DDevice9 interface stubs,
//! device capabilities, vertex formats, texture formats, render states,
//! texture stage states, sampler states, transform types, and the
//! fixed-function pipeline emulation layer.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::{HResult, WinHandle};

// =========================================================================
// HRESULT codes
// =========================================================================

pub const D3D_OK: i32 = 0;
pub const D3DERR_INVALIDCALL: i32 = 0x8876086C_u32 as i32;
pub const D3DERR_NOTAVAILABLE: i32 = 0x8876086A_u32 as i32;
pub const D3DERR_OUTOFVIDEOMEMORY: i32 = 0x8876017C_u32 as i32;
pub const D3DERR_DEVICELOST: i32 = 0x88760868_u32 as i32;
pub const D3DERR_DEVICENOTRESET: i32 = 0x88760869_u32 as i32;
pub const D3DERR_NOTFOUND: i32 = 0x88760866_u32 as i32;
pub const D3DERR_WASSTILLDRAWING: i32 = 0x8876021C_u32 as i32;
pub const D3DERR_DRIVERINTERNALERROR: i32 = 0x88760827_u32 as i32;
pub const D3DERR_MOREDATA: i32 = 0x88760863_u32 as i32;
pub const D3DERR_WRONGTEXTUREFORMAT: i32 = 0x88760818_u32 as i32;
pub const D3DERR_UNSUPPORTEDCOLOROPERATION: i32 = 0x88760819_u32 as i32;
pub const D3DERR_UNSUPPORTEDCOLORARG: i32 = 0x8876081A_u32 as i32;
pub const D3DERR_UNSUPPORTEDALPHAOPERATION: i32 = 0x8876081B_u32 as i32;
pub const D3DERR_TOOMANYOPERATIONS: i32 = 0x8876081D_u32 as i32;
pub const D3DERR_CONFLICTINGRENDERSTATE: i32 = 0x88760822_u32 as i32;

// =========================================================================
// Texture format constants (D3DFORMAT)
// =========================================================================

pub const D3DFMT_UNKNOWN: u32 = 0;
pub const D3DFMT_R8G8B8: u32 = 20;
pub const D3DFMT_A8R8G8B8: u32 = 21;
pub const D3DFMT_X8R8G8B8: u32 = 22;
pub const D3DFMT_R5G6B5: u32 = 23;
pub const D3DFMT_X1R5G5B5: u32 = 24;
pub const D3DFMT_A1R5G5B5: u32 = 25;
pub const D3DFMT_A4R4G4B4: u32 = 26;
pub const D3DFMT_R3G3B2: u32 = 27;
pub const D3DFMT_A8: u32 = 28;
pub const D3DFMT_A8R3G3B2: u32 = 29;
pub const D3DFMT_X4R4G4B4: u32 = 30;
pub const D3DFMT_A2B10G10R10: u32 = 31;
pub const D3DFMT_A8B8G8R8: u32 = 32;
pub const D3DFMT_X8B8G8R8: u32 = 33;
pub const D3DFMT_G16R16: u32 = 34;
pub const D3DFMT_A2R10G10B10: u32 = 35;
pub const D3DFMT_A16B16G16R16: u32 = 36;
pub const D3DFMT_A8P8: u32 = 40;
pub const D3DFMT_P8: u32 = 41;
pub const D3DFMT_L8: u32 = 50;
pub const D3DFMT_A8L8: u32 = 51;
pub const D3DFMT_A4L4: u32 = 52;
pub const D3DFMT_V8U8: u32 = 60;
pub const D3DFMT_L6V5U5: u32 = 61;
pub const D3DFMT_X8L8V8U8: u32 = 62;
pub const D3DFMT_Q8W8V8U8: u32 = 63;
pub const D3DFMT_V16U16: u32 = 64;
pub const D3DFMT_A2W10V10U10: u32 = 67;
pub const D3DFMT_D16_LOCKABLE: u32 = 70;
pub const D3DFMT_D32: u32 = 71;
pub const D3DFMT_D15S1: u32 = 73;
pub const D3DFMT_D24S8: u32 = 75;
pub const D3DFMT_D24X8: u32 = 77;
pub const D3DFMT_D24X4S4: u32 = 79;
pub const D3DFMT_D16: u32 = 80;
pub const D3DFMT_D32F_LOCKABLE: u32 = 82;
pub const D3DFMT_D24FS8: u32 = 83;
pub const D3DFMT_L16: u32 = 81;
pub const D3DFMT_Q16W16V16U16: u32 = 110;
pub const D3DFMT_R16F: u32 = 111;
pub const D3DFMT_G16R16F: u32 = 112;
pub const D3DFMT_A16B16G16R16F: u32 = 113;
pub const D3DFMT_R32F: u32 = 114;
pub const D3DFMT_G32R32F: u32 = 115;
pub const D3DFMT_A32B32G32R32F: u32 = 116;
pub const D3DFMT_CxV8U8: u32 = 117;

pub const fn d3dfmt_fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

pub const D3DFMT_DXT1: u32 = d3dfmt_fourcc(b'D', b'X', b'T', b'1');
pub const D3DFMT_DXT2: u32 = d3dfmt_fourcc(b'D', b'X', b'T', b'2');
pub const D3DFMT_DXT3: u32 = d3dfmt_fourcc(b'D', b'X', b'T', b'3');
pub const D3DFMT_DXT4: u32 = d3dfmt_fourcc(b'D', b'X', b'T', b'4');
pub const D3DFMT_DXT5: u32 = d3dfmt_fourcc(b'D', b'X', b'T', b'5');
pub const D3DFMT_INTZ: u32 = d3dfmt_fourcc(b'I', b'N', b'T', b'Z');
pub const D3DFMT_NULL: u32 = d3dfmt_fourcc(b'N', b'U', b'L', b'L');
pub const D3DFMT_DF16: u32 = d3dfmt_fourcc(b'D', b'F', b'1', b'6');
pub const D3DFMT_DF24: u32 = d3dfmt_fourcc(b'D', b'F', b'2', b'4');
pub const D3DFMT_RAWZ: u32 = d3dfmt_fourcc(b'R', b'A', b'W', b'Z');

// =========================================================================
// Flexible Vertex Format (FVF) codes
// =========================================================================

pub const D3DFVF_XYZ: u32 = 0x002;
pub const D3DFVF_XYZRHW: u32 = 0x004;
pub const D3DFVF_XYZB1: u32 = 0x006;
pub const D3DFVF_XYZB2: u32 = 0x008;
pub const D3DFVF_XYZB3: u32 = 0x00A;
pub const D3DFVF_XYZB4: u32 = 0x00C;
pub const D3DFVF_XYZB5: u32 = 0x00E;
pub const D3DFVF_XYZW: u32 = 0x4002;
pub const D3DFVF_NORMAL: u32 = 0x010;
pub const D3DFVF_PSIZE: u32 = 0x020;
pub const D3DFVF_DIFFUSE: u32 = 0x040;
pub const D3DFVF_SPECULAR: u32 = 0x080;
pub const D3DFVF_TEX0: u32 = 0x000;
pub const D3DFVF_TEX1: u32 = 0x100;
pub const D3DFVF_TEX2: u32 = 0x200;
pub const D3DFVF_TEX3: u32 = 0x300;
pub const D3DFVF_TEX4: u32 = 0x400;
pub const D3DFVF_TEX5: u32 = 0x500;
pub const D3DFVF_TEX6: u32 = 0x600;
pub const D3DFVF_TEX7: u32 = 0x700;
pub const D3DFVF_TEX8: u32 = 0x800;
pub const D3DFVF_LASTBETA_UBYTE4: u32 = 0x1000;
pub const D3DFVF_LASTBETA_D3DCOLOR: u32 = 0x8000;

// =========================================================================
// Vertex declaration element types
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum D3dDeclType {
    Float1 = 0,
    Float2 = 1,
    Float3 = 2,
    Float4 = 3,
    D3dColor = 4,
    Ubyte4 = 5,
    Short2 = 6,
    Short4 = 7,
    Ubyte4N = 8,
    Short2N = 9,
    Short4N = 10,
    UShort2N = 11,
    UShort4N = 12,
    UDec3 = 13,
    Dec3N = 14,
    Float16_2 = 15,
    Float16_4 = 16,
    Unused = 17,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum D3dDeclUsage {
    Position = 0,
    BlendWeight = 1,
    BlendIndices = 2,
    Normal = 3,
    PSize = 4,
    TexCoord = 5,
    Tangent = 6,
    Binormal = 7,
    TessFactor = 8,
    PositionT = 9,
    Color = 10,
    Fog = 11,
    Depth = 12,
    Sample = 13,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct D3dVertexElement {
    pub stream: u16,
    pub offset: u16,
    pub decl_type: D3dDeclType,
    pub method: u8,
    pub usage: D3dDeclUsage,
    pub usage_index: u8,
}

// =========================================================================
// Primitive types
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3dPrimitiveType {
    PointList = 1,
    LineList = 2,
    LineStrip = 3,
    TriangleList = 4,
    TriangleStrip = 5,
    TriangleFan = 6,
}

// =========================================================================
// Transform types
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum D3dTransformType {
    View = 2,
    Projection = 3,
    World = 256,
    World1 = 257,
    World2 = 258,
    World3 = 259,
    Texture0 = 16,
    Texture1 = 17,
    Texture2 = 18,
    Texture3 = 19,
    Texture4 = 20,
    Texture5 = 21,
    Texture6 = 22,
    Texture7 = 23,
}

// =========================================================================
// Render state identifiers (D3DRS_*)
// =========================================================================

pub const D3DRS_ZENABLE: u32 = 7;
pub const D3DRS_FILLMODE: u32 = 8;
pub const D3DRS_SHADEMODE: u32 = 9;
pub const D3DRS_ZWRITEENABLE: u32 = 14;
pub const D3DRS_ALPHATESTENABLE: u32 = 15;
pub const D3DRS_LASTPIXEL: u32 = 16;
pub const D3DRS_SRCBLEND: u32 = 19;
pub const D3DRS_DESTBLEND: u32 = 20;
pub const D3DRS_CULLMODE: u32 = 22;
pub const D3DRS_ZFUNC: u32 = 23;
pub const D3DRS_ALPHAREF: u32 = 24;
pub const D3DRS_ALPHAFUNC: u32 = 25;
pub const D3DRS_DITHERENABLE: u32 = 26;
pub const D3DRS_ALPHABLENDENABLE: u32 = 27;
pub const D3DRS_FOGENABLE: u32 = 28;
pub const D3DRS_SPECULARENABLE: u32 = 29;
pub const D3DRS_FOGCOLOR: u32 = 34;
pub const D3DRS_FOGTABLEMODE: u32 = 35;
pub const D3DRS_FOGSTART: u32 = 36;
pub const D3DRS_FOGEND: u32 = 37;
pub const D3DRS_FOGDENSITY: u32 = 38;
pub const D3DRS_RANGEFOGENABLE: u32 = 48;
pub const D3DRS_STENCILENABLE: u32 = 52;
pub const D3DRS_STENCILFAIL: u32 = 53;
pub const D3DRS_STENCILZFAIL: u32 = 54;
pub const D3DRS_STENCILPASS: u32 = 55;
pub const D3DRS_STENCILFUNC: u32 = 56;
pub const D3DRS_STENCILREF: u32 = 57;
pub const D3DRS_STENCILMASK: u32 = 58;
pub const D3DRS_STENCILWRITEMASK: u32 = 59;
pub const D3DRS_TEXTUREFACTOR: u32 = 60;
pub const D3DRS_WRAP0: u32 = 128;
pub const D3DRS_WRAP1: u32 = 129;
pub const D3DRS_WRAP2: u32 = 130;
pub const D3DRS_WRAP3: u32 = 131;
pub const D3DRS_WRAP4: u32 = 132;
pub const D3DRS_WRAP5: u32 = 133;
pub const D3DRS_WRAP6: u32 = 134;
pub const D3DRS_WRAP7: u32 = 135;
pub const D3DRS_CLIPPING: u32 = 136;
pub const D3DRS_LIGHTING: u32 = 137;
pub const D3DRS_AMBIENT: u32 = 139;
pub const D3DRS_FOGVERTEXMODE: u32 = 140;
pub const D3DRS_COLORVERTEX: u32 = 141;
pub const D3DRS_LOCALVIEWER: u32 = 142;
pub const D3DRS_NORMALIZENORMALS: u32 = 143;
pub const D3DRS_DIFFUSEMATERIALSOURCE: u32 = 145;
pub const D3DRS_SPECULARMATERIALSOURCE: u32 = 146;
pub const D3DRS_AMBIENTMATERIALSOURCE: u32 = 147;
pub const D3DRS_EMISSIVEMATERIALSOURCE: u32 = 148;
pub const D3DRS_VERTEXBLEND: u32 = 151;
pub const D3DRS_CLIPPLANEENABLE: u32 = 152;
pub const D3DRS_POINTSIZE: u32 = 154;
pub const D3DRS_POINTSIZE_MIN: u32 = 155;
pub const D3DRS_POINTSPRITEENABLE: u32 = 156;
pub const D3DRS_POINTSCALEENABLE: u32 = 157;
pub const D3DRS_POINTSCALE_A: u32 = 158;
pub const D3DRS_POINTSCALE_B: u32 = 159;
pub const D3DRS_POINTSCALE_C: u32 = 160;
pub const D3DRS_MULTISAMPLEANTIALIAS: u32 = 161;
pub const D3DRS_MULTISAMPLEMASK: u32 = 162;
pub const D3DRS_PATCHEDGESTYLE: u32 = 163;
pub const D3DRS_DEBUGMONITORTOKEN: u32 = 165;
pub const D3DRS_POINTSIZE_MAX: u32 = 166;
pub const D3DRS_INDEXEDVERTEXBLENDENABLE: u32 = 167;
pub const D3DRS_COLORWRITEENABLE: u32 = 168;
pub const D3DRS_TWEENFACTOR: u32 = 170;
pub const D3DRS_BLENDOP: u32 = 171;
pub const D3DRS_POSITIONDEGREE: u32 = 172;
pub const D3DRS_NORMALDEGREE: u32 = 173;
pub const D3DRS_SCISSORTESTENABLE: u32 = 174;
pub const D3DRS_SLOPESCALEDEPTHBIAS: u32 = 175;
pub const D3DRS_ANTIALIASEDLINEENABLE: u32 = 176;
pub const D3DRS_MINTESSELLATIONLEVEL: u32 = 178;
pub const D3DRS_MAXTESSELLATIONLEVEL: u32 = 179;
pub const D3DRS_ADAPTIVETESS_X: u32 = 180;
pub const D3DRS_ADAPTIVETESS_Y: u32 = 181;
pub const D3DRS_ADAPTIVETESS_Z: u32 = 182;
pub const D3DRS_ADAPTIVETESS_W: u32 = 183;
pub const D3DRS_ENABLEADAPTIVETESSELLATION: u32 = 184;
pub const D3DRS_TWOSIDEDSTENCILMODE: u32 = 185;
pub const D3DRS_CCW_STENCILFAIL: u32 = 186;
pub const D3DRS_CCW_STENCILZFAIL: u32 = 187;
pub const D3DRS_CCW_STENCILPASS: u32 = 188;
pub const D3DRS_CCW_STENCILFUNC: u32 = 189;
pub const D3DRS_COLORWRITEENABLE1: u32 = 190;
pub const D3DRS_COLORWRITEENABLE2: u32 = 191;
pub const D3DRS_COLORWRITEENABLE3: u32 = 192;
pub const D3DRS_BLENDFACTOR: u32 = 193;
pub const D3DRS_SRGBWRITEENABLE: u32 = 194;
pub const D3DRS_DEPTHBIAS: u32 = 195;
pub const D3DRS_SEPARATEALPHABLENDENABLE: u32 = 206;
pub const D3DRS_SRCBLENDALPHA: u32 = 207;
pub const D3DRS_DESTBLENDALPHA: u32 = 208;
pub const D3DRS_BLENDOPALPHA: u32 = 209;

// =========================================================================
// Texture stage state identifiers (D3DTSS_*)
// =========================================================================

pub const D3DTSS_COLOROP: u32 = 1;
pub const D3DTSS_COLORARG1: u32 = 2;
pub const D3DTSS_COLORARG2: u32 = 3;
pub const D3DTSS_ALPHAOP: u32 = 4;
pub const D3DTSS_ALPHAARG1: u32 = 5;
pub const D3DTSS_ALPHAARG2: u32 = 6;
pub const D3DTSS_BUMPENVMAT00: u32 = 7;
pub const D3DTSS_BUMPENVMAT01: u32 = 8;
pub const D3DTSS_BUMPENVMAT10: u32 = 9;
pub const D3DTSS_BUMPENVMAT11: u32 = 10;
pub const D3DTSS_TEXCOORDINDEX: u32 = 11;
pub const D3DTSS_BUMPENVLSCALE: u32 = 22;
pub const D3DTSS_BUMPENVLOFFSET: u32 = 23;
pub const D3DTSS_TEXTURETRANSFORMFLAGS: u32 = 24;
pub const D3DTSS_COLORARG0: u32 = 26;
pub const D3DTSS_ALPHAARG0: u32 = 27;
pub const D3DTSS_RESULTARG: u32 = 28;
pub const D3DTSS_CONSTANT: u32 = 32;

// Texture operations for D3DTSS_COLOROP / D3DTSS_ALPHAOP
pub const D3DTOP_DISABLE: u32 = 1;
pub const D3DTOP_SELECTARG1: u32 = 2;
pub const D3DTOP_SELECTARG2: u32 = 3;
pub const D3DTOP_MODULATE: u32 = 4;
pub const D3DTOP_MODULATE2X: u32 = 5;
pub const D3DTOP_MODULATE4X: u32 = 6;
pub const D3DTOP_ADD: u32 = 7;
pub const D3DTOP_ADDSIGNED: u32 = 8;
pub const D3DTOP_ADDSIGNED2X: u32 = 9;
pub const D3DTOP_SUBTRACT: u32 = 10;
pub const D3DTOP_ADDSMOOTH: u32 = 11;
pub const D3DTOP_BLENDDIFFUSEALPHA: u32 = 12;
pub const D3DTOP_BLENDTEXTUREALPHA: u32 = 13;
pub const D3DTOP_BLENDFACTORALPHA: u32 = 14;
pub const D3DTOP_BLENDTEXTUREALPHAPM: u32 = 15;
pub const D3DTOP_BLENDCURRENTALPHA: u32 = 16;
pub const D3DTOP_PREMODULATE: u32 = 17;
pub const D3DTOP_MODULATEALPHA_ADDCOLOR: u32 = 18;
pub const D3DTOP_MODULATECOLOR_ADDALPHA: u32 = 19;
pub const D3DTOP_MODULATEINVALPHA_ADDCOLOR: u32 = 20;
pub const D3DTOP_MODULATEINVCOLOR_ADDALPHA: u32 = 21;
pub const D3DTOP_BUMPENVMAP: u32 = 22;
pub const D3DTOP_BUMPENVMAPLUMINANCE: u32 = 23;
pub const D3DTOP_DOTPRODUCT3: u32 = 24;
pub const D3DTOP_MULTIPLYADD: u32 = 25;
pub const D3DTOP_LERP: u32 = 26;

// Texture arguments
pub const D3DTA_SELECTMASK: u32 = 0x0000000F;
pub const D3DTA_DIFFUSE: u32 = 0x00000000;
pub const D3DTA_CURRENT: u32 = 0x00000001;
pub const D3DTA_TEXTURE: u32 = 0x00000002;
pub const D3DTA_TFACTOR: u32 = 0x00000003;
pub const D3DTA_SPECULAR: u32 = 0x00000004;
pub const D3DTA_TEMP: u32 = 0x00000005;
pub const D3DTA_CONSTANT: u32 = 0x00000006;
pub const D3DTA_COMPLEMENT: u32 = 0x00000010;
pub const D3DTA_ALPHAREPLICATE: u32 = 0x00000020;

// =========================================================================
// Sampler state identifiers (D3DSAMP_*)
// =========================================================================

pub const D3DSAMP_ADDRESSU: u32 = 1;
pub const D3DSAMP_ADDRESSV: u32 = 2;
pub const D3DSAMP_ADDRESSW: u32 = 3;
pub const D3DSAMP_BORDERCOLOR: u32 = 4;
pub const D3DSAMP_MAGFILTER: u32 = 5;
pub const D3DSAMP_MINFILTER: u32 = 6;
pub const D3DSAMP_MIPFILTER: u32 = 7;
pub const D3DSAMP_MIPMAPLODBIAS: u32 = 8;
pub const D3DSAMP_MAXMIPLEVEL: u32 = 9;
pub const D3DSAMP_MAXANISOTROPY: u32 = 10;
pub const D3DSAMP_SRGBTEXTURE: u32 = 11;
pub const D3DSAMP_ELEMENTINDEX: u32 = 12;
pub const D3DSAMP_DMAPOFFSET: u32 = 13;

// =========================================================================
// Light type
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3dLightType {
    Point = 1,
    Spot = 2,
    Directional = 3,
}

// =========================================================================
// Core structures
// =========================================================================

#[derive(Debug, Clone, Copy)]
pub struct D3dVector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Default for D3dVector {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3dColorValue {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for D3dColorValue {
    fn default() -> Self {
        Self {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3dMatrix {
    pub m: [[f32; 4]; 4],
}

impl Default for D3dMatrix {
    fn default() -> Self {
        Self {
            m: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
}

impl D3dMatrix {
    pub fn multiply(&self, other: &D3dMatrix) -> D3dMatrix {
        let mut result = D3dMatrix { m: [[0.0; 4]; 4] };
        for i in 0..4 {
            for j in 0..4 {
                let mut sum = 0.0f32;
                for k in 0..4 {
                    sum += self.m[i][k] * other.m[k][j];
                }
                result.m[i][j] = sum;
            }
        }
        result
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3dMaterial9 {
    pub diffuse: D3dColorValue,
    pub ambient: D3dColorValue,
    pub specular: D3dColorValue,
    pub emissive: D3dColorValue,
    pub power: f32,
}

impl Default for D3dMaterial9 {
    fn default() -> Self {
        Self {
            diffuse: D3dColorValue::default(),
            ambient: D3dColorValue::default(),
            specular: D3dColorValue::default(),
            emissive: D3dColorValue::default(),
            power: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3dLight9 {
    pub light_type: D3dLightType,
    pub diffuse: D3dColorValue,
    pub specular: D3dColorValue,
    pub ambient: D3dColorValue,
    pub position: D3dVector,
    pub direction: D3dVector,
    pub range: f32,
    pub falloff: f32,
    pub attenuation0: f32,
    pub attenuation1: f32,
    pub attenuation2: f32,
    pub theta: f32,
    pub phi: f32,
}

impl Default for D3dLight9 {
    fn default() -> Self {
        Self {
            light_type: D3dLightType::Directional,
            diffuse: D3dColorValue::default(),
            specular: D3dColorValue::default(),
            ambient: D3dColorValue::default(),
            position: D3dVector::default(),
            direction: D3dVector {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            range: 0.0,
            falloff: 0.0,
            attenuation0: 0.0,
            attenuation1: 0.0,
            attenuation2: 0.0,
            theta: 0.0,
            phi: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct D3dViewport9 {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub min_z: f32,
    pub max_z: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct D3dRect {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

// =========================================================================
// Presentation parameters
// =========================================================================

#[derive(Debug, Clone)]
pub struct D3dPresentParameters {
    pub back_buffer_width: u32,
    pub back_buffer_height: u32,
    pub back_buffer_format: u32,
    pub back_buffer_count: u32,
    pub multi_sample_type: u32,
    pub multi_sample_quality: u32,
    pub swap_effect: u32,
    pub device_window: WinHandle,
    pub windowed: bool,
    pub enable_auto_depth_stencil: bool,
    pub auto_depth_stencil_format: u32,
    pub flags: u32,
    pub fullscreen_refresh_rate: u32,
    pub presentation_interval: u32,
}

impl Default for D3dPresentParameters {
    fn default() -> Self {
        Self {
            back_buffer_width: 800,
            back_buffer_height: 600,
            back_buffer_format: D3DFMT_X8R8G8B8,
            back_buffer_count: 1,
            multi_sample_type: 0,
            multi_sample_quality: 0,
            swap_effect: 1,
            device_window: WinHandle(0),
            windowed: true,
            enable_auto_depth_stencil: false,
            auto_depth_stencil_format: D3DFMT_UNKNOWN,
            flags: 0,
            fullscreen_refresh_rate: 0,
            presentation_interval: 0x00000001,
        }
    }
}

// =========================================================================
// Device capabilities
// =========================================================================

#[derive(Debug, Clone)]
pub struct D3dCaps9 {
    pub device_type: u32,
    pub adapter_ordinal: u32,
    pub caps: u32,
    pub caps2: u32,
    pub caps3: u32,
    pub presentation_intervals: u32,
    pub cursor_caps: u32,
    pub dev_caps: u32,
    pub primitive_misc_caps: u32,
    pub raster_caps: u32,
    pub z_cmp_caps: u32,
    pub src_blend_caps: u32,
    pub dest_blend_caps: u32,
    pub alpha_cmp_caps: u32,
    pub shade_caps: u32,
    pub texture_caps: u32,
    pub texture_filter_caps: u32,
    pub cube_texture_filter_caps: u32,
    pub volume_texture_filter_caps: u32,
    pub texture_address_caps: u32,
    pub volume_texture_address_caps: u32,
    pub line_caps: u32,
    pub max_texture_width: u32,
    pub max_texture_height: u32,
    pub max_volume_extent: u32,
    pub max_texture_repeat: u32,
    pub max_texture_aspect_ratio: u32,
    pub max_anisotropy: u32,
    pub max_vertex_w: f32,
    pub guard_band_left: f32,
    pub guard_band_top: f32,
    pub guard_band_right: f32,
    pub guard_band_bottom: f32,
    pub extents_adjust: f32,
    pub stencil_caps: u32,
    pub fvf_caps: u32,
    pub texture_op_caps: u32,
    pub max_texture_blend_stages: u32,
    pub max_simultaneous_textures: u32,
    pub vertex_processing_caps: u32,
    pub max_active_lights: u32,
    pub max_user_clip_planes: u32,
    pub max_vertex_blend_matrices: u32,
    pub max_vertex_blend_matrix_index: u32,
    pub max_point_size: f32,
    pub max_primitive_count: u32,
    pub max_vertex_index: u32,
    pub max_streams: u32,
    pub max_stream_stride: u32,
    pub vertex_shader_version: u32,
    pub max_vertex_shader_const: u32,
    pub pixel_shader_version: u32,
    pub pixel_shader_1x_max_value: f32,
    pub dev_caps2: u32,
    pub max_npatch_tesselation_level: f32,
    pub master_adapter_ordinal: u32,
    pub adapter_ordinal_in_group: u32,
    pub number_of_adapters_in_group: u32,
    pub decl_types: u32,
    pub num_simultaneous_rts: u32,
    pub stretch_rect_filter_caps: u32,
    pub vs20_caps_static_flow_ctrl_depth: u32,
    pub vs20_caps_dynamic_flow_ctrl_depth: u32,
    pub vs20_caps_num_temps: u32,
    pub ps20_caps_static_flow_ctrl_depth: u32,
    pub ps20_caps_dynamic_flow_ctrl_depth: u32,
    pub ps20_caps_num_temps: u32,
    pub ps20_caps_num_instruction_slots: u32,
    pub vertex_texture_filter_caps: u32,
    pub max_v_shader_instructions_executed: u32,
    pub max_p_shader_instructions_executed: u32,
    pub max_vertex_shader30_instruction_slots: u32,
    pub max_pixel_shader30_instruction_slots: u32,
}

impl Default for D3dCaps9 {
    fn default() -> Self {
        Self {
            device_type: 1,
            adapter_ordinal: 0,
            caps: 0x00020000,
            caps2: 0x80000000,
            caps3: 0x00000200,
            presentation_intervals: 0x8000000F,
            cursor_caps: 1,
            dev_caps: 0x001B2FF0,
            primitive_misc_caps: 0x000FCEF2,
            raster_caps: 0x07732191,
            z_cmp_caps: 0xFF,
            src_blend_caps: 0x1FFF,
            dest_blend_caps: 0x1FFF,
            alpha_cmp_caps: 0xFF,
            shade_caps: 0x000A0003,
            texture_caps: 0x0001FEF5,
            texture_filter_caps: 0x07030F00,
            cube_texture_filter_caps: 0x07030F00,
            volume_texture_filter_caps: 0x07030F00,
            texture_address_caps: 0x3F,
            volume_texture_address_caps: 0x3F,
            line_caps: 0x1F,
            max_texture_width: 8192,
            max_texture_height: 8192,
            max_volume_extent: 2048,
            max_texture_repeat: 8192,
            max_texture_aspect_ratio: 8192,
            max_anisotropy: 16,
            max_vertex_w: 10000000000.0,
            guard_band_left: -32768.0,
            guard_band_top: -32768.0,
            guard_band_right: 32768.0,
            guard_band_bottom: 32768.0,
            extents_adjust: 0.0,
            stencil_caps: 0x1FF,
            fvf_caps: 0x00100008,
            texture_op_caps: 0x03FFFFFF,
            max_texture_blend_stages: 8,
            max_simultaneous_textures: 8,
            vertex_processing_caps: 0x0000017B,
            max_active_lights: 8,
            max_user_clip_planes: 6,
            max_vertex_blend_matrices: 4,
            max_vertex_blend_matrix_index: 0,
            max_point_size: 256.0,
            max_primitive_count: 0x000FFFFF,
            max_vertex_index: 0x00FFFFFF,
            max_streams: 16,
            max_stream_stride: 508,
            vertex_shader_version: 0xFFFE0300,
            max_vertex_shader_const: 256,
            pixel_shader_version: 0xFFFF0300,
            pixel_shader_1x_max_value: 65504.0,
            dev_caps2: 0x00000091,
            max_npatch_tesselation_level: 1.0,
            master_adapter_ordinal: 0,
            adapter_ordinal_in_group: 0,
            number_of_adapters_in_group: 1,
            decl_types: 0x30F,
            num_simultaneous_rts: 4,
            stretch_rect_filter_caps: 0x03030300,
            vs20_caps_static_flow_ctrl_depth: 4,
            vs20_caps_dynamic_flow_ctrl_depth: 24,
            vs20_caps_num_temps: 32,
            ps20_caps_static_flow_ctrl_depth: 4,
            ps20_caps_dynamic_flow_ctrl_depth: 24,
            ps20_caps_num_temps: 32,
            ps20_caps_num_instruction_slots: 512,
            vertex_texture_filter_caps: 0x07030F00,
            max_v_shader_instructions_executed: 65535,
            max_p_shader_instructions_executed: 65535,
            max_vertex_shader30_instruction_slots: 32768,
            max_pixel_shader30_instruction_slots: 32768,
        }
    }
}

// =========================================================================
// Adapter identifier
// =========================================================================

#[derive(Debug, Clone)]
pub struct D3dAdapterIdentifier9 {
    pub driver: String,
    pub description: String,
    pub device_name: String,
    pub driver_version_low: u32,
    pub driver_version_high: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub sub_sys_id: u32,
    pub revision: u32,
    pub device_identifier: [u8; 16],
    pub whql_level: u32,
}

impl Default for D3dAdapterIdentifier9 {
    fn default() -> Self {
        Self {
            driver: String::from("athbridge_d3d9"),
            description: String::from("AthBridge D3D9 Compatibility Adapter"),
            device_name: String::from("\\\\.\\DISPLAY1"),
            driver_version_low: 0x00010001,
            driver_version_high: 0x00090003,
            vendor_id: 0x1002,
            device_id: 0x67B1,
            sub_sys_id: 0,
            revision: 0,
            device_identifier: [
                0x46, 0xD1, 0x4E, 0x90, 0xA5, 0xC3, 0x42, 0xB8, 0x92, 0x7E, 0x2E, 0xB1, 0x7C, 0xAA,
                0x45, 0x22,
            ],
            whql_level: 1,
        }
    }
}

// =========================================================================
// Display mode
// =========================================================================

#[derive(Debug, Clone, Copy)]
pub struct D3dDisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
    pub format: u32,
}

// =========================================================================
// Resource handles
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct D3dResourceHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3dResourceType {
    VertexBuffer,
    IndexBuffer,
    Texture2D,
    CubeTexture,
    VolumeTexture,
    RenderTarget,
    DepthStencilSurface,
    VertexDeclaration,
    VertexShader,
    PixelShader,
    StateBlock,
    OffscreenPlainSurface,
    SwapChain,
    Query,
}

#[derive(Debug, Clone)]
pub struct D3dResource {
    pub handle: D3dResourceHandle,
    pub resource_type: D3dResourceType,
    pub size_bytes: u64,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub levels: u32,
    pub usage: u32,
    pub pool: u32,
    pub fvf: u32,
    pub data: Vec<u8>,
}

// =========================================================================
// State block types
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3dStateBlockType {
    All = 1,
    PixelState = 2,
    VertexState = 3,
}

#[derive(Debug, Clone)]
pub struct D3dStateBlock {
    pub block_type: D3dStateBlockType,
    pub render_states: BTreeMap<u32, u32>,
    pub texture_stage_states: BTreeMap<(u32, u32), u32>,
    pub sampler_states: BTreeMap<(u32, u32), u32>,
    pub transforms: BTreeMap<u32, D3dMatrix>,
    pub material: D3dMaterial9,
    pub viewport: D3dViewport9,
    pub fvf: u32,
    pub vertex_shader: Option<D3dResourceHandle>,
    pub pixel_shader: Option<D3dResourceHandle>,
    pub lights: BTreeMap<u32, D3dLight9>,
    pub lights_enabled: BTreeMap<u32, bool>,
    pub clip_planes: [[f32; 4]; 6],
    pub scissor_rect: D3dRect,
}

// =========================================================================
// Query types
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3dQueryType {
    VCache = 4,
    ResourceManager = 5,
    VertexStats = 6,
    Event = 8,
    Occlusion = 9,
    Timestamp = 10,
    TimestampDisjoint = 11,
    TimestampFreq = 12,
    PipelineTimings = 13,
    InterfaceTimings = 14,
    VertexTimings = 15,
    PixelTimings = 16,
    BandwidthTimings = 17,
    CacheUtilization = 18,
}

// =========================================================================
// Device creation parameters
// =========================================================================

#[derive(Debug, Clone)]
pub struct D3dDeviceCreationParameters {
    pub adapter_ordinal: u32,
    pub device_type: u32,
    pub focus_window: WinHandle,
    pub behavior_flags: u32,
}

// =========================================================================
// D3D9 Device state
// =========================================================================

pub struct D3d9Device {
    pub creation_params: D3dDeviceCreationParameters,
    pub present_params: D3dPresentParameters,
    pub caps: D3dCaps9,
    pub render_states: BTreeMap<u32, u32>,
    pub texture_stage_states: BTreeMap<(u32, u32), u32>,
    pub sampler_states: BTreeMap<(u32, u32), u32>,
    pub transforms: BTreeMap<u32, D3dMatrix>,
    pub material: D3dMaterial9,
    pub lights: BTreeMap<u32, D3dLight9>,
    pub lights_enabled: BTreeMap<u32, bool>,
    pub viewport: D3dViewport9,
    pub scissor_rect: D3dRect,
    pub clip_planes: [[f32; 4]; 6],
    pub fvf: u32,
    pub vertex_shader: Option<D3dResourceHandle>,
    pub pixel_shader: Option<D3dResourceHandle>,
    pub vertex_declaration: Option<D3dResourceHandle>,
    pub stream_sources: BTreeMap<u32, (D3dResourceHandle, u32, u32)>,
    pub indices: Option<D3dResourceHandle>,
    pub textures: [Option<D3dResourceHandle>; 16],
    pub render_targets: [Option<D3dResourceHandle>; 4],
    pub depth_stencil: Option<D3dResourceHandle>,
    pub resources: BTreeMap<u64, D3dResource>,
    pub next_resource_id: u64,
    pub in_scene: bool,
    pub device_lost: bool,
    pub draw_call_count: u64,
    pub frame_count: u64,
    pub available_texture_mem: u64,
    pub cursor_visible: bool,
    pub dialog_box_mode: bool,
    pub palette_entries: BTreeMap<u32, [u32; 256]>,
    pub vs_constants_f: [[f32; 4]; 256],
    pub ps_constants_f: [[f32; 4]; 224],
}

impl D3d9Device {
    pub fn new(
        creation_params: D3dDeviceCreationParameters,
        present_params: D3dPresentParameters,
    ) -> Self {
        let mut render_states = BTreeMap::new();
        Self::init_default_render_states(&mut render_states);
        let mut sampler_states = BTreeMap::new();
        for i in 0..16u32 {
            sampler_states.insert((i, D3DSAMP_ADDRESSU), 1);
            sampler_states.insert((i, D3DSAMP_ADDRESSV), 1);
            sampler_states.insert((i, D3DSAMP_ADDRESSW), 1);
            sampler_states.insert((i, D3DSAMP_MAGFILTER), 2);
            sampler_states.insert((i, D3DSAMP_MINFILTER), 2);
            sampler_states.insert((i, D3DSAMP_MIPFILTER), 1);
            sampler_states.insert((i, D3DSAMP_MAXANISOTROPY), 1);
            sampler_states.insert((i, D3DSAMP_MAXMIPLEVEL), 0);
            sampler_states.insert((i, D3DSAMP_SRGBTEXTURE), 0);
        }
        let mut tss = BTreeMap::new();
        for stage in 0..8u32 {
            let op = if stage == 0 {
                D3DTOP_MODULATE
            } else {
                D3DTOP_DISABLE
            };
            tss.insert((stage, D3DTSS_COLOROP), op);
            tss.insert((stage, D3DTSS_COLORARG1), D3DTA_TEXTURE);
            tss.insert((stage, D3DTSS_COLORARG2), D3DTA_CURRENT);
            let a_op = if stage == 0 {
                D3DTOP_SELECTARG1
            } else {
                D3DTOP_DISABLE
            };
            tss.insert((stage, D3DTSS_ALPHAOP), a_op);
            tss.insert((stage, D3DTSS_ALPHAARG1), D3DTA_TEXTURE);
            tss.insert((stage, D3DTSS_ALPHAARG2), D3DTA_CURRENT);
            tss.insert((stage, D3DTSS_TEXCOORDINDEX), stage);
            tss.insert((stage, D3DTSS_TEXTURETRANSFORMFLAGS), 0);
        }
        Self {
            creation_params,
            present_params,
            caps: D3dCaps9::default(),
            render_states,
            texture_stage_states: tss,
            sampler_states,
            transforms: BTreeMap::new(),
            material: D3dMaterial9::default(),
            lights: BTreeMap::new(),
            lights_enabled: BTreeMap::new(),
            viewport: D3dViewport9::default(),
            scissor_rect: D3dRect::default(),
            clip_planes: [[0.0; 4]; 6],
            fvf: 0,
            vertex_shader: None,
            pixel_shader: None,
            vertex_declaration: None,
            stream_sources: BTreeMap::new(),
            indices: None,
            textures: [None; 16],
            render_targets: [None; 4],
            depth_stencil: None,
            resources: BTreeMap::new(),
            next_resource_id: 1,
            in_scene: false,
            device_lost: false,
            draw_call_count: 0,
            frame_count: 0,
            available_texture_mem: 512 * 1024 * 1024,
            cursor_visible: true,
            dialog_box_mode: false,
            palette_entries: BTreeMap::new(),
            vs_constants_f: [[0.0; 4]; 256],
            ps_constants_f: [[0.0; 4]; 224],
        }
    }

    fn init_default_render_states(rs: &mut BTreeMap<u32, u32>) {
        rs.insert(D3DRS_ZENABLE, 1);
        rs.insert(D3DRS_FILLMODE, 3);
        rs.insert(D3DRS_SHADEMODE, 2);
        rs.insert(D3DRS_ZWRITEENABLE, 1);
        rs.insert(D3DRS_ALPHATESTENABLE, 0);
        rs.insert(D3DRS_LASTPIXEL, 1);
        rs.insert(D3DRS_SRCBLEND, 2);
        rs.insert(D3DRS_DESTBLEND, 1);
        rs.insert(D3DRS_CULLMODE, 3);
        rs.insert(D3DRS_ZFUNC, 4);
        rs.insert(D3DRS_ALPHAREF, 0);
        rs.insert(D3DRS_ALPHAFUNC, 8);
        rs.insert(D3DRS_DITHERENABLE, 0);
        rs.insert(D3DRS_ALPHABLENDENABLE, 0);
        rs.insert(D3DRS_FOGENABLE, 0);
        rs.insert(D3DRS_SPECULARENABLE, 0);
        rs.insert(D3DRS_FOGCOLOR, 0);
        rs.insert(D3DRS_FOGTABLEMODE, 0);
        rs.insert(D3DRS_FOGSTART, 0);
        rs.insert(D3DRS_FOGEND, 0x3F800000);
        rs.insert(D3DRS_FOGDENSITY, 0x3F800000);
        rs.insert(D3DRS_RANGEFOGENABLE, 0);
        rs.insert(D3DRS_STENCILENABLE, 0);
        rs.insert(D3DRS_STENCILFAIL, 1);
        rs.insert(D3DRS_STENCILZFAIL, 1);
        rs.insert(D3DRS_STENCILPASS, 1);
        rs.insert(D3DRS_STENCILFUNC, 8);
        rs.insert(D3DRS_STENCILREF, 0);
        rs.insert(D3DRS_STENCILMASK, 0xFFFFFFFF);
        rs.insert(D3DRS_STENCILWRITEMASK, 0xFFFFFFFF);
        rs.insert(D3DRS_TEXTUREFACTOR, 0xFFFFFFFF);
        rs.insert(D3DRS_CLIPPING, 1);
        rs.insert(D3DRS_LIGHTING, 1);
        rs.insert(D3DRS_AMBIENT, 0);
        rs.insert(D3DRS_FOGVERTEXMODE, 0);
        rs.insert(D3DRS_COLORVERTEX, 1);
        rs.insert(D3DRS_LOCALVIEWER, 1);
        rs.insert(D3DRS_NORMALIZENORMALS, 0);
        rs.insert(D3DRS_DIFFUSEMATERIALSOURCE, 0);
        rs.insert(D3DRS_SPECULARMATERIALSOURCE, 0);
        rs.insert(D3DRS_AMBIENTMATERIALSOURCE, 0);
        rs.insert(D3DRS_EMISSIVEMATERIALSOURCE, 0);
        rs.insert(D3DRS_VERTEXBLEND, 0);
        rs.insert(D3DRS_CLIPPLANEENABLE, 0);
        rs.insert(D3DRS_POINTSIZE, 0x3F800000);
        rs.insert(D3DRS_POINTSIZE_MIN, 0x3F800000);
        rs.insert(D3DRS_POINTSPRITEENABLE, 0);
        rs.insert(D3DRS_POINTSCALEENABLE, 0);
        rs.insert(D3DRS_POINTSCALE_A, 0x3F800000);
        rs.insert(D3DRS_POINTSCALE_B, 0);
        rs.insert(D3DRS_POINTSCALE_C, 0);
        rs.insert(D3DRS_MULTISAMPLEANTIALIAS, 1);
        rs.insert(D3DRS_MULTISAMPLEMASK, 0xFFFFFFFF);
        rs.insert(D3DRS_PATCHEDGESTYLE, 0);
        rs.insert(D3DRS_POINTSIZE_MAX, 0x43480000);
        rs.insert(D3DRS_INDEXEDVERTEXBLENDENABLE, 0);
        rs.insert(D3DRS_COLORWRITEENABLE, 0x0000000F);
        rs.insert(D3DRS_TWEENFACTOR, 0);
        rs.insert(D3DRS_BLENDOP, 1);
        rs.insert(D3DRS_SCISSORTESTENABLE, 0);
        rs.insert(D3DRS_SLOPESCALEDEPTHBIAS, 0);
        rs.insert(D3DRS_ANTIALIASEDLINEENABLE, 0);
        rs.insert(D3DRS_TWOSIDEDSTENCILMODE, 0);
        rs.insert(D3DRS_CCW_STENCILFAIL, 1);
        rs.insert(D3DRS_CCW_STENCILZFAIL, 1);
        rs.insert(D3DRS_CCW_STENCILPASS, 1);
        rs.insert(D3DRS_CCW_STENCILFUNC, 8);
        rs.insert(D3DRS_COLORWRITEENABLE1, 0x0000000F);
        rs.insert(D3DRS_COLORWRITEENABLE2, 0x0000000F);
        rs.insert(D3DRS_COLORWRITEENABLE3, 0x0000000F);
        rs.insert(D3DRS_BLENDFACTOR, 0xFFFFFFFF);
        rs.insert(D3DRS_SRGBWRITEENABLE, 0);
        rs.insert(D3DRS_DEPTHBIAS, 0);
        rs.insert(D3DRS_SEPARATEALPHABLENDENABLE, 0);
        rs.insert(D3DRS_SRCBLENDALPHA, 2);
        rs.insert(D3DRS_DESTBLENDALPHA, 1);
        rs.insert(D3DRS_BLENDOPALPHA, 1);
        for i in 0..8u32 {
            rs.insert(D3DRS_WRAP0 + i, 0);
        }
    }

    fn alloc_resource(
        &mut self,
        res_type: D3dResourceType,
        format: u32,
        w: u32,
        h: u32,
        d: u32,
        levels: u32,
        usage: u32,
        pool: u32,
        fvf: u32,
        size: u64,
    ) -> D3dResourceHandle {
        let id = self.next_resource_id;
        self.next_resource_id += 1;
        let handle = D3dResourceHandle(id);
        self.resources.insert(
            id,
            D3dResource {
                handle,
                resource_type: res_type,
                size_bytes: size,
                format,
                width: w,
                height: h,
                depth: d,
                levels,
                usage,
                pool,
                fvf,
                data: Vec::new(),
            },
        );
        handle
    }

    // -- IDirect3DDevice9 methods --

    pub fn reset(&mut self, params: D3dPresentParameters) -> HResult {
        self.present_params = params;
        self.in_scene = false;
        self.device_lost = false;
        self.frame_count = 0;
        self.draw_call_count = 0;
        HResult(D3D_OK)
    }

    pub fn present(&mut self) -> HResult {
        if self.device_lost {
            return HResult(D3DERR_DEVICELOST);
        }
        self.frame_count += 1;
        HResult(D3D_OK)
    }

    pub fn begin_scene(&mut self) -> HResult {
        if self.in_scene {
            return HResult(D3DERR_INVALIDCALL);
        }
        self.in_scene = true;
        HResult(D3D_OK)
    }

    pub fn end_scene(&mut self) -> HResult {
        if !self.in_scene {
            return HResult(D3DERR_INVALIDCALL);
        }
        self.in_scene = false;
        HResult(D3D_OK)
    }

    pub fn clear(
        &mut self,
        _count: u32,
        _rects: Option<&[D3dRect]>,
        flags: u32,
        _color: u32,
        _z: f32,
        _stencil: u32,
    ) -> HResult {
        let _ = flags;
        HResult(D3D_OK)
    }

    pub fn set_viewport(&mut self, vp: D3dViewport9) -> HResult {
        self.viewport = vp;
        HResult(D3D_OK)
    }

    pub fn set_render_state(&mut self, state: u32, value: u32) -> HResult {
        self.render_states.insert(state, value);
        HResult(D3D_OK)
    }

    pub fn get_render_state(&self, state: u32) -> (HResult, u32) {
        let v = self.render_states.get(&state).copied().unwrap_or(0);
        (HResult(D3D_OK), v)
    }

    pub fn set_texture_stage_state(&mut self, stage: u32, state_type: u32, value: u32) -> HResult {
        self.texture_stage_states.insert((stage, state_type), value);
        HResult(D3D_OK)
    }

    pub fn set_sampler_state(&mut self, sampler: u32, state_type: u32, value: u32) -> HResult {
        self.sampler_states.insert((sampler, state_type), value);
        HResult(D3D_OK)
    }

    pub fn set_transform(
        &mut self,
        transform_type: D3dTransformType,
        matrix: D3dMatrix,
    ) -> HResult {
        self.transforms.insert(transform_type as u32, matrix);
        HResult(D3D_OK)
    }

    pub fn get_transform(&self, transform_type: D3dTransformType) -> (HResult, D3dMatrix) {
        let m = self
            .transforms
            .get(&(transform_type as u32))
            .cloned()
            .unwrap_or_default();
        (HResult(D3D_OK), m)
    }

    pub fn set_material(&mut self, mat: D3dMaterial9) -> HResult {
        self.material = mat;
        HResult(D3D_OK)
    }

    pub fn set_light(&mut self, index: u32, light: D3dLight9) -> HResult {
        self.lights.insert(index, light);
        HResult(D3D_OK)
    }

    pub fn light_enable(&mut self, index: u32, enable: bool) -> HResult {
        self.lights_enabled.insert(index, enable);
        HResult(D3D_OK)
    }

    pub fn create_vertex_buffer(
        &mut self,
        length: u32,
        usage: u32,
        fvf: u32,
        pool: u32,
    ) -> (HResult, D3dResourceHandle) {
        let h = self.alloc_resource(
            D3dResourceType::VertexBuffer,
            0,
            0,
            0,
            0,
            1,
            usage,
            pool,
            fvf,
            length as u64,
        );
        (HResult(D3D_OK), h)
    }

    pub fn create_index_buffer(
        &mut self,
        length: u32,
        usage: u32,
        format: u32,
        pool: u32,
    ) -> (HResult, D3dResourceHandle) {
        let h = self.alloc_resource(
            D3dResourceType::IndexBuffer,
            format,
            0,
            0,
            0,
            1,
            usage,
            pool,
            0,
            length as u64,
        );
        (HResult(D3D_OK), h)
    }

    pub fn create_texture(
        &mut self,
        width: u32,
        height: u32,
        levels: u32,
        usage: u32,
        format: u32,
        pool: u32,
    ) -> (HResult, D3dResourceHandle) {
        let size = (width as u64) * (height as u64) * 4;
        let h = self.alloc_resource(
            D3dResourceType::Texture2D,
            format,
            width,
            height,
            1,
            levels,
            usage,
            pool,
            0,
            size,
        );
        (HResult(D3D_OK), h)
    }

    pub fn create_cube_texture(
        &mut self,
        edge: u32,
        levels: u32,
        usage: u32,
        format: u32,
        pool: u32,
    ) -> (HResult, D3dResourceHandle) {
        let size = (edge as u64) * (edge as u64) * 4 * 6;
        let h = self.alloc_resource(
            D3dResourceType::CubeTexture,
            format,
            edge,
            edge,
            6,
            levels,
            usage,
            pool,
            0,
            size,
        );
        (HResult(D3D_OK), h)
    }

    pub fn create_volume_texture(
        &mut self,
        w: u32,
        h: u32,
        d: u32,
        levels: u32,
        usage: u32,
        format: u32,
        pool: u32,
    ) -> (HResult, D3dResourceHandle) {
        let size = (w as u64) * (h as u64) * (d as u64) * 4;
        let handle = self.alloc_resource(
            D3dResourceType::VolumeTexture,
            format,
            w,
            h,
            d,
            levels,
            usage,
            pool,
            0,
            size,
        );
        (HResult(D3D_OK), handle)
    }

    pub fn create_render_target(
        &mut self,
        w: u32,
        h: u32,
        format: u32,
        multi_sample: u32,
        quality: u32,
        lockable: bool,
    ) -> (HResult, D3dResourceHandle) {
        let _ = (multi_sample, quality, lockable);
        let size = (w as u64) * (h as u64) * 4;
        let handle = self.alloc_resource(
            D3dResourceType::RenderTarget,
            format,
            w,
            h,
            1,
            1,
            1,
            0,
            0,
            size,
        );
        (HResult(D3D_OK), handle)
    }

    pub fn create_depth_stencil_surface(
        &mut self,
        w: u32,
        h: u32,
        format: u32,
        multi_sample: u32,
        quality: u32,
        discard: bool,
    ) -> (HResult, D3dResourceHandle) {
        let _ = (multi_sample, quality, discard);
        let size = (w as u64) * (h as u64) * 4;
        let handle = self.alloc_resource(
            D3dResourceType::DepthStencilSurface,
            format,
            w,
            h,
            1,
            1,
            0,
            0,
            0,
            size,
        );
        (HResult(D3D_OK), handle)
    }

    pub fn create_vertex_declaration(
        &mut self,
        _elements: &[D3dVertexElement],
    ) -> (HResult, D3dResourceHandle) {
        let handle = self.alloc_resource(
            D3dResourceType::VertexDeclaration,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        );
        (HResult(D3D_OK), handle)
    }

    pub fn create_vertex_shader(&mut self, _bytecode: &[u32]) -> (HResult, D3dResourceHandle) {
        let handle = self.alloc_resource(D3dResourceType::VertexShader, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        (HResult(D3D_OK), handle)
    }

    pub fn create_pixel_shader(&mut self, _bytecode: &[u32]) -> (HResult, D3dResourceHandle) {
        let handle = self.alloc_resource(D3dResourceType::PixelShader, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        (HResult(D3D_OK), handle)
    }

    pub fn set_vertex_shader(&mut self, shader: Option<D3dResourceHandle>) -> HResult {
        self.vertex_shader = shader;
        HResult(D3D_OK)
    }

    pub fn set_pixel_shader(&mut self, shader: Option<D3dResourceHandle>) -> HResult {
        self.pixel_shader = shader;
        HResult(D3D_OK)
    }

    pub fn set_vertex_declaration(&mut self, decl: Option<D3dResourceHandle>) -> HResult {
        self.vertex_declaration = decl;
        HResult(D3D_OK)
    }

    pub fn set_stream_source(
        &mut self,
        stream: u32,
        vb: D3dResourceHandle,
        offset: u32,
        stride: u32,
    ) -> HResult {
        self.stream_sources.insert(stream, (vb, offset, stride));
        HResult(D3D_OK)
    }

    pub fn set_indices(&mut self, ib: D3dResourceHandle) -> HResult {
        self.indices = Some(ib);
        HResult(D3D_OK)
    }

    pub fn draw_primitive(
        &mut self,
        prim_type: D3dPrimitiveType,
        start_vertex: u32,
        prim_count: u32,
    ) -> HResult {
        let _ = (prim_type, start_vertex, prim_count);
        self.draw_call_count += 1;
        HResult(D3D_OK)
    }

    pub fn draw_indexed_primitive(
        &mut self,
        prim_type: D3dPrimitiveType,
        base_vertex_index: i32,
        min_vertex_index: u32,
        num_vertices: u32,
        start_index: u32,
        prim_count: u32,
    ) -> HResult {
        let _ = (
            prim_type,
            base_vertex_index,
            min_vertex_index,
            num_vertices,
            start_index,
            prim_count,
        );
        self.draw_call_count += 1;
        HResult(D3D_OK)
    }

    pub fn draw_primitive_up(
        &mut self,
        prim_type: D3dPrimitiveType,
        prim_count: u32,
        _data: &[u8],
        _stride: u32,
    ) -> HResult {
        let _ = (prim_type, prim_count);
        self.draw_call_count += 1;
        HResult(D3D_OK)
    }

    pub fn draw_indexed_primitive_up(
        &mut self,
        prim_type: D3dPrimitiveType,
        min_vertex: u32,
        num_vertices: u32,
        prim_count: u32,
        _index_data: &[u8],
        _index_format: u32,
        _vertex_data: &[u8],
        _stride: u32,
    ) -> HResult {
        let _ = (prim_type, min_vertex, num_vertices, prim_count);
        self.draw_call_count += 1;
        HResult(D3D_OK)
    }

    pub fn set_texture(&mut self, stage: u32, texture: Option<D3dResourceHandle>) -> HResult {
        if (stage as usize) < self.textures.len() {
            self.textures[stage as usize] = texture;
            HResult(D3D_OK)
        } else {
            HResult(D3DERR_INVALIDCALL)
        }
    }

    pub fn set_render_target(&mut self, index: u32, target: Option<D3dResourceHandle>) -> HResult {
        if (index as usize) < self.render_targets.len() {
            self.render_targets[index as usize] = target;
            HResult(D3D_OK)
        } else {
            HResult(D3DERR_INVALIDCALL)
        }
    }

    pub fn set_depth_stencil_surface(&mut self, surface: Option<D3dResourceHandle>) -> HResult {
        self.depth_stencil = surface;
        HResult(D3D_OK)
    }

    pub fn get_render_target(&self, index: u32) -> (HResult, Option<D3dResourceHandle>) {
        if (index as usize) < self.render_targets.len() {
            (HResult(D3D_OK), self.render_targets[index as usize])
        } else {
            (HResult(D3DERR_INVALIDCALL), None)
        }
    }

    pub fn get_depth_stencil_surface(&self) -> (HResult, Option<D3dResourceHandle>) {
        (HResult(D3D_OK), self.depth_stencil)
    }

    pub fn stretch_rect(
        &mut self,
        _src: D3dResourceHandle,
        _src_rect: Option<D3dRect>,
        _dst: D3dResourceHandle,
        _dst_rect: Option<D3dRect>,
        _filter: u32,
    ) -> HResult {
        HResult(D3D_OK)
    }

    pub fn color_fill(
        &mut self,
        _surface: D3dResourceHandle,
        _rect: Option<D3dRect>,
        _color: u32,
    ) -> HResult {
        HResult(D3D_OK)
    }

    pub fn create_offscreen_plain_surface(
        &mut self,
        w: u32,
        h: u32,
        format: u32,
        pool: u32,
    ) -> (HResult, D3dResourceHandle) {
        let size = (w as u64) * (h as u64) * 4;
        let handle = self.alloc_resource(
            D3dResourceType::OffscreenPlainSurface,
            format,
            w,
            h,
            1,
            1,
            0,
            pool,
            0,
            size,
        );
        (HResult(D3D_OK), handle)
    }

    pub fn get_front_buffer_data(&self, _swap_chain: u32, _surface: D3dResourceHandle) -> HResult {
        HResult(D3D_OK)
    }

    pub fn set_clip_plane(&mut self, index: u32, plane: [f32; 4]) -> HResult {
        if (index as usize) < self.clip_planes.len() {
            self.clip_planes[index as usize] = plane;
            HResult(D3D_OK)
        } else {
            HResult(D3DERR_INVALIDCALL)
        }
    }

    pub fn set_scissor_rect(&mut self, rect: D3dRect) -> HResult {
        self.scissor_rect = rect;
        HResult(D3D_OK)
    }

    pub fn get_swap_chain(&self, _index: u32) -> (HResult, Option<D3dResourceHandle>) {
        (HResult(D3D_OK), None)
    }

    pub fn create_query(&mut self, query_type: D3dQueryType) -> (HResult, D3dResourceHandle) {
        let _ = query_type;
        let handle = self.alloc_resource(D3dResourceType::Query, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        (HResult(D3D_OK), handle)
    }

    pub fn set_vertex_shader_constant_f(
        &mut self,
        start_register: u32,
        data: &[[f32; 4]],
    ) -> HResult {
        for (i, val) in data.iter().enumerate() {
            let reg = (start_register as usize) + i;
            if reg < self.vs_constants_f.len() {
                self.vs_constants_f[reg] = *val;
            }
        }
        HResult(D3D_OK)
    }

    pub fn set_pixel_shader_constant_f(
        &mut self,
        start_register: u32,
        data: &[[f32; 4]],
    ) -> HResult {
        for (i, val) in data.iter().enumerate() {
            let reg = (start_register as usize) + i;
            if reg < self.ps_constants_f.len() {
                self.ps_constants_f[reg] = *val;
            }
        }
        HResult(D3D_OK)
    }

    pub fn process_vertices(
        &mut self,
        _src_start: u32,
        _dest_index: u32,
        _vertex_count: u32,
        _dest_buffer: D3dResourceHandle,
        _flags: u32,
    ) -> HResult {
        HResult(D3D_OK)
    }

    pub fn create_state_block(
        &mut self,
        block_type: D3dStateBlockType,
    ) -> (HResult, D3dStateBlock) {
        let sb = D3dStateBlock {
            block_type,
            render_states: self.render_states.clone(),
            texture_stage_states: self.texture_stage_states.clone(),
            sampler_states: self.sampler_states.clone(),
            transforms: self.transforms.clone(),
            material: self.material,
            viewport: self.viewport,
            fvf: self.fvf,
            vertex_shader: self.vertex_shader,
            pixel_shader: self.pixel_shader,
            lights: self.lights.clone(),
            lights_enabled: self.lights_enabled.clone(),
            clip_planes: self.clip_planes,
            scissor_rect: self.scissor_rect,
        };
        (HResult(D3D_OK), sb)
    }

    pub fn apply_state_block(&mut self, sb: &D3dStateBlock) {
        match sb.block_type {
            D3dStateBlockType::All => {
                self.render_states = sb.render_states.clone();
                self.texture_stage_states = sb.texture_stage_states.clone();
                self.sampler_states = sb.sampler_states.clone();
                self.transforms = sb.transforms.clone();
                self.material = sb.material;
                self.viewport = sb.viewport;
                self.fvf = sb.fvf;
                self.vertex_shader = sb.vertex_shader;
                self.pixel_shader = sb.pixel_shader;
                self.lights = sb.lights.clone();
                self.lights_enabled = sb.lights_enabled.clone();
                self.clip_planes = sb.clip_planes;
                self.scissor_rect = sb.scissor_rect;
            }
            D3dStateBlockType::PixelState => {
                self.render_states = sb.render_states.clone();
                self.texture_stage_states = sb.texture_stage_states.clone();
                self.sampler_states = sb.sampler_states.clone();
                self.pixel_shader = sb.pixel_shader;
            }
            D3dStateBlockType::VertexState => {
                self.transforms = sb.transforms.clone();
                self.material = sb.material;
                self.lights = sb.lights.clone();
                self.lights_enabled = sb.lights_enabled.clone();
                self.vertex_shader = sb.vertex_shader;
                self.fvf = sb.fvf;
            }
        }
    }

    pub fn set_fvf(&mut self, fvf: u32) -> HResult {
        self.fvf = fvf;
        HResult(D3D_OK)
    }

    pub fn validate_device(&self) -> (HResult, u32) {
        (HResult(D3D_OK), 1)
    }

    pub fn set_palette_entries(&mut self, palette_num: u32, entries: [u32; 256]) -> HResult {
        self.palette_entries.insert(palette_num, entries);
        HResult(D3D_OK)
    }

    pub fn set_dialog_box_mode(&mut self, enable: bool) -> HResult {
        self.dialog_box_mode = enable;
        HResult(D3D_OK)
    }

    pub fn set_cursor_properties(
        &mut self,
        _x_hot: u32,
        _y_hot: u32,
        _surface: D3dResourceHandle,
    ) -> HResult {
        HResult(D3D_OK)
    }

    pub fn set_cursor_position(&mut self, _x: i32, _y: i32, _flags: u32) {}

    pub fn show_cursor(&mut self, show: bool) -> bool {
        let prev = self.cursor_visible;
        self.cursor_visible = show;
        prev
    }

    pub fn get_display_mode(&self, _swap_chain: u32) -> (HResult, D3dDisplayMode) {
        let mode = D3dDisplayMode {
            width: self.present_params.back_buffer_width,
            height: self.present_params.back_buffer_height,
            refresh_rate: 60,
            format: self.present_params.back_buffer_format,
        };
        (HResult(D3D_OK), mode)
    }

    pub fn get_creation_parameters(&self) -> (HResult, D3dDeviceCreationParameters) {
        (HResult(D3D_OK), self.creation_params.clone())
    }

    pub fn test_cooperative_level(&self) -> HResult {
        if self.device_lost {
            HResult(D3DERR_DEVICENOTRESET)
        } else {
            HResult(D3D_OK)
        }
    }

    pub fn get_available_texture_mem(&self) -> u32 {
        (self.available_texture_mem / (1024 * 1024)) as u32
    }

    pub fn evict_managed_resources(&mut self) -> HResult {
        HResult(D3D_OK)
    }
}

// =========================================================================
// IDirect3D9 runtime
// =========================================================================

pub struct Direct3D9 {
    pub adapters: Vec<D3dAdapterIdentifier9>,
    pub display_modes: Vec<D3dDisplayMode>,
    pub caps: D3dCaps9,
}

impl Direct3D9 {
    pub fn new() -> Self {
        let default_modes = alloc::vec![
            D3dDisplayMode {
                width: 640,
                height: 480,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 800,
                height: 600,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 1024,
                height: 768,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 1280,
                height: 720,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 1280,
                height: 1024,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 1366,
                height: 768,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 1600,
                height: 900,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 1920,
                height: 1080,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 2560,
                height: 1440,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
            D3dDisplayMode {
                width: 3840,
                height: 2160,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8
            },
        ];
        Self {
            adapters: alloc::vec![D3dAdapterIdentifier9::default()],
            display_modes: default_modes,
            caps: D3dCaps9::default(),
        }
    }

    pub fn get_adapter_count(&self) -> u32 {
        self.adapters.len() as u32
    }

    pub fn get_adapter_identifier(&self, adapter: u32) -> Option<&D3dAdapterIdentifier9> {
        self.adapters.get(adapter as usize)
    }

    pub fn get_adapter_mode_count(&self, _adapter: u32, _format: u32) -> u32 {
        self.display_modes.len() as u32
    }

    pub fn enum_adapter_modes(
        &self,
        _adapter: u32,
        _format: u32,
        mode: u32,
    ) -> Option<D3dDisplayMode> {
        self.display_modes.get(mode as usize).copied()
    }

    pub fn get_adapter_display_mode(&self, _adapter: u32) -> (HResult, D3dDisplayMode) {
        let mode = self
            .display_modes
            .last()
            .copied()
            .unwrap_or(D3dDisplayMode {
                width: 1920,
                height: 1080,
                refresh_rate: 60,
                format: D3DFMT_X8R8G8B8,
            });
        (HResult(D3D_OK), mode)
    }

    pub fn check_device_type(
        &self,
        _adapter: u32,
        _dev_type: u32,
        _adapter_fmt: u32,
        _bb_fmt: u32,
        _windowed: bool,
    ) -> HResult {
        HResult(D3D_OK)
    }

    pub fn check_device_format(
        &self,
        _adapter: u32,
        _dev_type: u32,
        _adapter_fmt: u32,
        _usage: u32,
        _res_type: u32,
        _check_fmt: u32,
    ) -> HResult {
        HResult(D3D_OK)
    }

    pub fn check_device_multi_sample_type(
        &self,
        _adapter: u32,
        _dev_type: u32,
        _surface_fmt: u32,
        _windowed: bool,
        _multi_sample: u32,
    ) -> (HResult, u32) {
        (HResult(D3D_OK), 0)
    }

    pub fn check_depth_stencil_match(
        &self,
        _adapter: u32,
        _dev_type: u32,
        _adapter_fmt: u32,
        _rt_fmt: u32,
        _ds_fmt: u32,
    ) -> HResult {
        HResult(D3D_OK)
    }

    pub fn get_device_caps(&self, _adapter: u32, _dev_type: u32) -> (HResult, D3dCaps9) {
        (HResult(D3D_OK), self.caps.clone())
    }

    pub fn create_device(
        &self,
        _adapter: u32,
        _dev_type: u32,
        focus_window: WinHandle,
        behavior_flags: u32,
        present_params: D3dPresentParameters,
    ) -> (HResult, D3d9Device) {
        let creation = D3dDeviceCreationParameters {
            adapter_ordinal: 0,
            device_type: 1,
            focus_window,
            behavior_flags,
        };
        let device = D3d9Device::new(creation, present_params);
        (HResult(D3D_OK), device)
    }
}

// =========================================================================
// Fixed-function pipeline helpers
// =========================================================================

pub fn compute_world_view_projection(device: &D3d9Device) -> D3dMatrix {
    let world = device
        .transforms
        .get(&(D3dTransformType::World as u32))
        .cloned()
        .unwrap_or_default();
    let view = device
        .transforms
        .get(&(D3dTransformType::View as u32))
        .cloned()
        .unwrap_or_default();
    let proj = device
        .transforms
        .get(&(D3dTransformType::Projection as u32))
        .cloned()
        .unwrap_or_default();
    let wv = world.multiply(&view);
    wv.multiply(&proj)
}

fn f32_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    let mut i = 0;
    while i < 15 {
        guess = 0.5 * (guess + x / guess);
        i += 1;
    }
    guess
}

fn f32_floor(x: f32) -> f32 {
    let i = x as i32;
    if (i as f32) > x {
        (i - 1) as f32
    } else {
        i as f32
    }
}

fn f32_cos(x: f32) -> f32 {
    let pi: f32 = 3.14159265;
    let two_pi = 2.0 * pi;
    let mut a = x;
    a = a - f32_floor(a / two_pi) * two_pi;
    if a > pi {
        a -= two_pi;
    }
    if a < -pi {
        a += two_pi;
    }
    let x2 = a * a;
    let mut term = 1.0_f32;
    let mut sum = 1.0_f32;
    let mut i = 1;
    while i <= 10 {
        term *= -x2 / ((2 * i * (2 * i - 1)) as f32);
        sum += term;
        i += 1;
    }
    sum
}

fn f32_ln(x: f32) -> f32 {
    if x <= 0.0 {
        return -1e10;
    }
    let y = (x - 1.0) / (x + 1.0);
    let y2 = y * y;
    let mut term = y;
    let mut sum = y;
    let mut i = 1;
    while i < 20 {
        term *= y2;
        sum += term / (2 * i + 1) as f32;
        i += 1;
    }
    2.0 * sum
}

fn f32_exp(x: f32) -> f32 {
    let mut term = 1.0_f32;
    let mut sum = 1.0_f32;
    let mut i = 1;
    while i < 30 {
        term *= x / i as f32;
        sum += term;
        i += 1;
    }
    sum
}

fn f32_powf(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    if exp == 0.0 {
        return 1.0;
    }
    if exp == 1.0 {
        return base;
    }
    f32_exp(exp * f32_ln(base))
}

pub fn compute_light_contribution(
    light: &D3dLight9,
    _position: &D3dVector,
    normal: &D3dVector,
    material: &D3dMaterial9,
) -> D3dColorValue {
    match light.light_type {
        D3dLightType::Directional => {
            let dot = -(light.direction.x * normal.x
                + light.direction.y * normal.y
                + light.direction.z * normal.z);
            let intensity = if dot > 0.0 { dot } else { 0.0 };
            D3dColorValue {
                r: material.diffuse.r * light.diffuse.r * intensity
                    + material.ambient.r * light.ambient.r,
                g: material.diffuse.g * light.diffuse.g * intensity
                    + material.ambient.g * light.ambient.g,
                b: material.diffuse.b * light.diffuse.b * intensity
                    + material.ambient.b * light.ambient.b,
                a: material.diffuse.a,
            }
        }
        D3dLightType::Point => {
            let dx = _position.x - light.position.x;
            let dy = _position.y - light.position.y;
            let dz = _position.z - light.position.z;
            let dist = f32_sqrt(dx * dx + dy * dy + dz * dz);
            if dist > light.range && light.range > 0.0 {
                return D3dColorValue {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: material.diffuse.a,
                };
            }
            let atten = 1.0
                / (light.attenuation0
                    + light.attenuation1 * dist
                    + light.attenuation2 * dist * dist)
                    .max(0.001);
            let inv_dist = 1.0 / dist.max(0.0001);
            let lx = -dx * inv_dist;
            let ly = -dy * inv_dist;
            let lz = -dz * inv_dist;
            let dot = (lx * normal.x + ly * normal.y + lz * normal.z).max(0.0);
            D3dColorValue {
                r: material.diffuse.r * light.diffuse.r * dot * atten,
                g: material.diffuse.g * light.diffuse.g * dot * atten,
                b: material.diffuse.b * light.diffuse.b * dot * atten,
                a: material.diffuse.a,
            }
        }
        D3dLightType::Spot => {
            let dx = _position.x - light.position.x;
            let dy = _position.y - light.position.y;
            let dz = _position.z - light.position.z;
            let dist = f32_sqrt(dx * dx + dy * dy + dz * dz);
            if dist > light.range && light.range > 0.0 {
                return D3dColorValue {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: material.diffuse.a,
                };
            }
            let inv_dist = 1.0 / dist.max(0.0001);
            let lx = dx * inv_dist;
            let ly = dy * inv_dist;
            let lz = dz * inv_dist;
            let cos_angle =
                lx * light.direction.x + ly * light.direction.y + lz * light.direction.z;
            let cos_half_phi = f32_cos(light.phi * 0.5);
            if cos_angle < cos_half_phi {
                return D3dColorValue {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: material.diffuse.a,
                };
            }
            let cos_half_theta = f32_cos(light.theta * 0.5);
            let spot = if cos_angle >= cos_half_theta {
                1.0
            } else {
                let range = (cos_half_theta - cos_half_phi).max(0.0001);
                f32_powf((cos_angle - cos_half_phi) / range, light.falloff)
            };
            let atten = spot
                / (light.attenuation0
                    + light.attenuation1 * dist
                    + light.attenuation2 * dist * dist)
                    .max(0.001);
            let dot = (-(lx * normal.x + ly * normal.y + lz * normal.z)).max(0.0);
            D3dColorValue {
                r: material.diffuse.r * light.diffuse.r * dot * atten,
                g: material.diffuse.g * light.diffuse.g * dot * atten,
                b: material.diffuse.b * light.diffuse.b * dot * atten,
                a: material.diffuse.a,
            }
        }
    }
}

pub fn apply_fog(color: D3dColorValue, fog_color: u32, fog_factor: f32) -> D3dColorValue {
    let fr = ((fog_color >> 16) & 0xFF) as f32 / 255.0;
    let fg = ((fog_color >> 8) & 0xFF) as f32 / 255.0;
    let fb = (fog_color & 0xFF) as f32 / 255.0;
    let f = fog_factor.clamp(0.0, 1.0);
    D3dColorValue {
        r: color.r * f + fr * (1.0 - f),
        g: color.g * f + fg * (1.0 - f),
        b: color.b * f + fb * (1.0 - f),
        a: color.a,
    }
}

pub fn alpha_test(alpha: u8, ref_val: u8, func: u32) -> bool {
    match func {
        1 => false,
        2 => alpha < ref_val,
        3 => alpha == ref_val,
        4 => alpha <= ref_val,
        5 => alpha > ref_val,
        6 => alpha != ref_val,
        7 => alpha >= ref_val,
        8 => true,
        _ => true,
    }
}

// =========================================================================
// FVF vertex size calculator
// =========================================================================

pub fn fvf_vertex_size(fvf: u32) -> u32 {
    let mut size: u32 = 0;
    let pos_bits = fvf & 0x00E;
    match pos_bits {
        D3DFVF_XYZ => size += 12,
        D3DFVF_XYZRHW => size += 16,
        D3DFVF_XYZB1 => size += 16,
        D3DFVF_XYZB2 => size += 20,
        D3DFVF_XYZB3 => size += 24,
        D3DFVF_XYZB4 => size += 28,
        D3DFVF_XYZB5 => size += 32,
        _ => {}
    }
    if fvf & D3DFVF_NORMAL != 0 {
        size += 12;
    }
    if fvf & D3DFVF_PSIZE != 0 {
        size += 4;
    }
    if fvf & D3DFVF_DIFFUSE != 0 {
        size += 4;
    }
    if fvf & D3DFVF_SPECULAR != 0 {
        size += 4;
    }
    let tex_count = ((fvf >> 8) & 0xF) as u32;
    size += tex_count * 8;
    size
}

// =========================================================================
// Global D3D9 runtime
// =========================================================================

static D3D9_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct D3d9Runtime {
    pub direct3d: Direct3D9,
    pub devices: Vec<D3d9Device>,
}

static mut D3D9_RUNTIME_INNER: Option<D3d9Runtime> = None;

pub fn init() {
    if D3D9_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            D3D9_RUNTIME_INNER = Some(D3d9Runtime {
                direct3d: Direct3D9::new(),
                devices: Vec::new(),
            });
        }
    }
}

pub fn runtime() -> Option<&'static D3d9Runtime> {
    if D3D9_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { D3D9_RUNTIME_INNER.as_ref() }
    } else {
        None
    }
}

pub fn runtime_mut() -> Option<&'static mut D3d9Runtime> {
    if D3D9_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { D3D9_RUNTIME_INNER.as_mut() }
    } else {
        None
    }
}
