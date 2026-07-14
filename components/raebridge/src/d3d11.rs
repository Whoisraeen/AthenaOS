//! Direct3D 11 API emulation layer for RaeBridge.
//!
//! Translates D3D11 calls to RaeGFX Vulkan backend.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// HRESULT constants
// ---------------------------------------------------------------------------

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_FAIL: i32 = -2147467259; // 0x80004005
pub const E_INVALIDARG: i32 = -2147024809; // 0x80070057
pub const E_OUTOFMEMORY: i32 = -2147024882; // 0x8007000E
pub const E_NOTIMPL: i32 = -2147467263; // 0x80004001
pub const DXGI_ERROR_DEVICE_REMOVED: i32 = -2005270523; // 0x887A0005
pub const DXGI_ERROR_DEVICE_RESET: i32 = -2005270521; // 0x887A0007
pub const DXGI_ERROR_INVALID_CALL: i32 = -2005270527; // 0x887A0001
pub const D3D11_ERROR_FILE_NOT_FOUND: i32 = -2005139454; // 0x887C0002
pub const D3D11_ERROR_TOO_MANY_UNIQUE_STATE_OBJECTS: i32 = -2005139455; // 0x887C0001

// ---------------------------------------------------------------------------
// Feature levels
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum D3D_FEATURE_LEVEL {
    D3D_FEATURE_LEVEL_9_1 = 0x9100,
    D3D_FEATURE_LEVEL_9_2 = 0x9200,
    D3D_FEATURE_LEVEL_9_3 = 0x9300,
    D3D_FEATURE_LEVEL_10_0 = 0xa000,
    D3D_FEATURE_LEVEL_10_1 = 0xa100,
    D3D_FEATURE_LEVEL_11_0 = 0xb000,
    D3D_FEATURE_LEVEL_11_1 = 0xb100,
}

impl D3D_FEATURE_LEVEL {
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0x9100 => Some(Self::D3D_FEATURE_LEVEL_9_1),
            0x9200 => Some(Self::D3D_FEATURE_LEVEL_9_2),
            0x9300 => Some(Self::D3D_FEATURE_LEVEL_9_3),
            0xa000 => Some(Self::D3D_FEATURE_LEVEL_10_0),
            0xa100 => Some(Self::D3D_FEATURE_LEVEL_10_1),
            0xb000 => Some(Self::D3D_FEATURE_LEVEL_11_0),
            0xb100 => Some(Self::D3D_FEATURE_LEVEL_11_1),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// DXGI formats (100+ variants)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum DXGI_FORMAT {
    UNKNOWN = 0,
    R32G32B32A32_TYPELESS = 1,
    R32G32B32A32_FLOAT = 2,
    R32G32B32A32_UINT = 3,
    R32G32B32A32_SINT = 4,
    R32G32B32_TYPELESS = 5,
    R32G32B32_FLOAT = 6,
    R32G32B32_UINT = 7,
    R32G32B32_SINT = 8,
    R16G16B16A16_TYPELESS = 9,
    R16G16B16A16_FLOAT = 10,
    R16G16B16A16_UNORM = 11,
    R16G16B16A16_UINT = 12,
    R16G16B16A16_SNORM = 13,
    R16G16B16A16_SINT = 14,
    R32G32_TYPELESS = 15,
    R32G32_FLOAT = 16,
    R32G32_UINT = 17,
    R32G32_SINT = 18,
    R32G8X24_TYPELESS = 19,
    D32_FLOAT_S8X24_UINT = 20,
    R32_FLOAT_X8X24_TYPELESS = 21,
    X32_TYPELESS_G8X24_UINT = 22,
    R10G10B10A2_TYPELESS = 23,
    R10G10B10A2_UNORM = 24,
    R10G10B10A2_UINT = 25,
    R11G11B10_FLOAT = 26,
    R8G8B8A8_TYPELESS = 27,
    R8G8B8A8_UNORM = 28,
    R8G8B8A8_UNORM_SRGB = 29,
    R8G8B8A8_UINT = 30,
    R8G8B8A8_SNORM = 31,
    R8G8B8A8_SINT = 32,
    R16G16_TYPELESS = 33,
    R16G16_FLOAT = 34,
    R16G16_UNORM = 35,
    R16G16_UINT = 36,
    R16G16_SNORM = 37,
    R16G16_SINT = 38,
    R32_TYPELESS = 39,
    D32_FLOAT = 40,
    R32_FLOAT = 41,
    R32_UINT = 42,
    R32_SINT = 43,
    R24G8_TYPELESS = 44,
    D24_UNORM_S8_UINT = 45,
    R24_UNORM_X8_TYPELESS = 46,
    X24_TYPELESS_G8_UINT = 47,
    R8G8_TYPELESS = 48,
    R8G8_UNORM = 49,
    R8G8_UINT = 50,
    R8G8_SNORM = 51,
    R8G8_SINT = 52,
    R16_TYPELESS = 53,
    R16_FLOAT = 54,
    D16_UNORM = 55,
    R16_UNORM = 56,
    R16_UINT = 57,
    R16_SNORM = 58,
    R16_SINT = 59,
    R8_TYPELESS = 60,
    R8_UNORM = 61,
    R8_UINT = 62,
    R8_SNORM = 63,
    R8_SINT = 64,
    A8_UNORM = 65,
    R1_UNORM = 66,
    R9G9B9E5_SHAREDEXP = 67,
    R8G8_B8G8_UNORM = 68,
    G8R8_G8B8_UNORM = 69,
    BC1_TYPELESS = 70,
    BC1_UNORM = 71,
    BC1_UNORM_SRGB = 72,
    BC2_TYPELESS = 73,
    BC2_UNORM = 74,
    BC2_UNORM_SRGB = 75,
    BC3_TYPELESS = 76,
    BC3_UNORM = 77,
    BC3_UNORM_SRGB = 78,
    BC4_TYPELESS = 79,
    BC4_UNORM = 80,
    BC4_SNORM = 81,
    BC5_TYPELESS = 82,
    BC5_UNORM = 83,
    BC5_SNORM = 84,
    B5G6R5_UNORM = 85,
    B5G5R5A1_UNORM = 86,
    B8G8R8A8_UNORM = 87,
    B8G8R8X8_UNORM = 88,
    R10G10B10_XR_BIAS_A2_UNORM = 89,
    B8G8R8A8_TYPELESS = 90,
    B8G8R8A8_UNORM_SRGB = 91,
    B8G8R8X8_TYPELESS = 92,
    B8G8R8X8_UNORM_SRGB = 93,
    BC6H_TYPELESS = 94,
    BC6H_UF16 = 95,
    BC6H_SF16 = 96,
    BC7_TYPELESS = 97,
    BC7_UNORM = 98,
    BC7_UNORM_SRGB = 99,
    AYUV = 100,
    Y410 = 101,
    Y416 = 102,
    NV12 = 103,
    P010 = 104,
    P016 = 105,
    YUV_420_OPAQUE = 106,
    YUY2 = 107,
    Y210 = 108,
    Y216 = 109,
    NV11 = 110,
    AI44 = 111,
    IA44 = 112,
    P8 = 113,
    A8P8 = 114,
    B4G4R4A4_UNORM = 115,
}

impl DXGI_FORMAT {
    pub fn bytes_per_pixel(&self) -> u32 {
        match self {
            Self::R32G32B32A32_FLOAT
            | Self::R32G32B32A32_UINT
            | Self::R32G32B32A32_SINT
            | Self::R32G32B32A32_TYPELESS => 16,
            Self::R32G32B32_FLOAT
            | Self::R32G32B32_UINT
            | Self::R32G32B32_SINT
            | Self::R32G32B32_TYPELESS => 12,
            Self::R16G16B16A16_FLOAT
            | Self::R16G16B16A16_UNORM
            | Self::R16G16B16A16_UINT
            | Self::R16G16B16A16_SNORM
            | Self::R16G16B16A16_SINT
            | Self::R16G16B16A16_TYPELESS
            | Self::R32G32_FLOAT
            | Self::R32G32_UINT
            | Self::R32G32_SINT
            | Self::R32G32_TYPELESS => 8,
            Self::R8G8B8A8_UNORM
            | Self::R8G8B8A8_UNORM_SRGB
            | Self::R8G8B8A8_UINT
            | Self::R8G8B8A8_SNORM
            | Self::R8G8B8A8_SINT
            | Self::R8G8B8A8_TYPELESS
            | Self::B8G8R8A8_UNORM
            | Self::B8G8R8A8_UNORM_SRGB
            | Self::B8G8R8A8_TYPELESS
            | Self::B8G8R8X8_UNORM
            | Self::B8G8R8X8_UNORM_SRGB
            | Self::B8G8R8X8_TYPELESS
            | Self::R10G10B10A2_UNORM
            | Self::R10G10B10A2_UINT
            | Self::R10G10B10A2_TYPELESS
            | Self::R11G11B10_FLOAT
            | Self::R16G16_FLOAT
            | Self::R16G16_UNORM
            | Self::R16G16_UINT
            | Self::R16G16_SNORM
            | Self::R16G16_SINT
            | Self::R16G16_TYPELESS
            | Self::R32_FLOAT
            | Self::R32_UINT
            | Self::R32_SINT
            | Self::R32_TYPELESS
            | Self::D32_FLOAT
            | Self::D24_UNORM_S8_UINT
            | Self::R9G9B9E5_SHAREDEXP => 4,
            Self::R8G8_UNORM
            | Self::R8G8_UINT
            | Self::R8G8_SNORM
            | Self::R8G8_SINT
            | Self::R8G8_TYPELESS
            | Self::R16_FLOAT
            | Self::R16_UNORM
            | Self::R16_UINT
            | Self::R16_SNORM
            | Self::R16_SINT
            | Self::R16_TYPELESS
            | Self::D16_UNORM
            | Self::B5G6R5_UNORM
            | Self::B5G5R5A1_UNORM
            | Self::B4G4R4A4_UNORM => 2,
            Self::R8_UNORM
            | Self::R8_UINT
            | Self::R8_SNORM
            | Self::R8_SINT
            | Self::R8_TYPELESS
            | Self::A8_UNORM => 1,
            _ => 0,
        }
    }

    pub fn is_compressed(&self) -> bool {
        matches!(
            self,
            Self::BC1_TYPELESS
                | Self::BC1_UNORM
                | Self::BC1_UNORM_SRGB
                | Self::BC2_TYPELESS
                | Self::BC2_UNORM
                | Self::BC2_UNORM_SRGB
                | Self::BC3_TYPELESS
                | Self::BC3_UNORM
                | Self::BC3_UNORM_SRGB
                | Self::BC4_TYPELESS
                | Self::BC4_UNORM
                | Self::BC4_SNORM
                | Self::BC5_TYPELESS
                | Self::BC5_UNORM
                | Self::BC5_SNORM
                | Self::BC6H_TYPELESS
                | Self::BC6H_UF16
                | Self::BC6H_SF16
                | Self::BC7_TYPELESS
                | Self::BC7_UNORM
                | Self::BC7_UNORM_SRGB
        )
    }

    pub fn is_depth_stencil(&self) -> bool {
        matches!(
            self,
            Self::D32_FLOAT
                | Self::D24_UNORM_S8_UINT
                | Self::D16_UNORM
                | Self::D32_FLOAT_S8X24_UINT
                | Self::R32G8X24_TYPELESS
                | Self::R24G8_TYPELESS
        )
    }

    pub fn is_srgb(&self) -> bool {
        matches!(
            self,
            Self::R8G8B8A8_UNORM_SRGB
                | Self::B8G8R8A8_UNORM_SRGB
                | Self::B8G8R8X8_UNORM_SRGB
                | Self::BC1_UNORM_SRGB
                | Self::BC2_UNORM_SRGB
                | Self::BC3_UNORM_SRGB
                | Self::BC7_UNORM_SRGB
        )
    }

    pub fn to_vulkan_format(&self) -> u32 {
        match self {
            Self::R8G8B8A8_UNORM => 37,      // VK_FORMAT_R8G8B8A8_UNORM
            Self::R8G8B8A8_UNORM_SRGB => 43, // VK_FORMAT_R8G8B8A8_SRGB
            Self::B8G8R8A8_UNORM => 44,      // VK_FORMAT_B8G8R8A8_UNORM
            Self::B8G8R8A8_UNORM_SRGB => 50, // VK_FORMAT_B8G8R8A8_SRGB
            Self::R32G32B32A32_FLOAT => 109, // VK_FORMAT_R32G32B32A32_SFLOAT
            Self::R16G16B16A16_FLOAT => 97,  // VK_FORMAT_R16G16B16A16_SFLOAT
            Self::R32_FLOAT => 100,          // VK_FORMAT_R32_SFLOAT
            Self::D32_FLOAT => 126,          // VK_FORMAT_D32_SFLOAT
            Self::D24_UNORM_S8_UINT => 129,  // VK_FORMAT_D24_UNORM_S8_UINT
            Self::D16_UNORM => 124,          // VK_FORMAT_D16_UNORM
            Self::R16G16_FLOAT => 83,        // VK_FORMAT_R16G16_SFLOAT
            Self::R11G11B10_FLOAT => 122,    // VK_FORMAT_B10G11R11_UFLOAT_PACK32
            Self::R10G10B10A2_UNORM => 64,   // VK_FORMAT_A2B10G10R10_UNORM_PACK32
            _ => 0,                          // VK_FORMAT_UNDEFINED
        }
    }
}

// ---------------------------------------------------------------------------
// Primitive topology
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_PRIMITIVE_TOPOLOGY {
    UNDEFINED = 0,
    POINTLIST = 1,
    LINELIST = 2,
    LINESTRIP = 3,
    TRIANGLELIST = 4,
    TRIANGLESTRIP = 5,
    LINELIST_ADJ = 10,
    LINESTRIP_ADJ = 11,
    TRIANGLELIST_ADJ = 12,
    TRIANGLESTRIP_ADJ = 13,
    CONTROL_POINT_PATCHLIST_1 = 33,
    CONTROL_POINT_PATCHLIST_2 = 34,
    CONTROL_POINT_PATCHLIST_3 = 35,
    CONTROL_POINT_PATCHLIST_4 = 36,
    CONTROL_POINT_PATCHLIST_5 = 37,
    CONTROL_POINT_PATCHLIST_6 = 38,
    CONTROL_POINT_PATCHLIST_7 = 39,
    CONTROL_POINT_PATCHLIST_8 = 40,
    CONTROL_POINT_PATCHLIST_9 = 41,
    CONTROL_POINT_PATCHLIST_10 = 42,
    CONTROL_POINT_PATCHLIST_11 = 43,
    CONTROL_POINT_PATCHLIST_12 = 44,
    CONTROL_POINT_PATCHLIST_13 = 45,
    CONTROL_POINT_PATCHLIST_14 = 46,
    CONTROL_POINT_PATCHLIST_15 = 47,
    CONTROL_POINT_PATCHLIST_16 = 48,
    CONTROL_POINT_PATCHLIST_17 = 49,
    CONTROL_POINT_PATCHLIST_18 = 50,
    CONTROL_POINT_PATCHLIST_19 = 51,
    CONTROL_POINT_PATCHLIST_20 = 52,
    CONTROL_POINT_PATCHLIST_21 = 53,
    CONTROL_POINT_PATCHLIST_22 = 54,
    CONTROL_POINT_PATCHLIST_23 = 55,
    CONTROL_POINT_PATCHLIST_24 = 56,
    CONTROL_POINT_PATCHLIST_25 = 57,
    CONTROL_POINT_PATCHLIST_26 = 58,
    CONTROL_POINT_PATCHLIST_27 = 59,
    CONTROL_POINT_PATCHLIST_28 = 60,
    CONTROL_POINT_PATCHLIST_29 = 61,
    CONTROL_POINT_PATCHLIST_30 = 62,
    CONTROL_POINT_PATCHLIST_31 = 63,
    CONTROL_POINT_PATCHLIST_32 = 64,
}

impl D3D11_PRIMITIVE_TOPOLOGY {
    pub fn to_vulkan_topology(&self) -> u32 {
        match self {
            Self::POINTLIST => 0,         // VK_PRIMITIVE_TOPOLOGY_POINT_LIST
            Self::LINELIST => 1,          // VK_PRIMITIVE_TOPOLOGY_LINE_LIST
            Self::LINESTRIP => 2,         // VK_PRIMITIVE_TOPOLOGY_LINE_STRIP
            Self::TRIANGLELIST => 3,      // VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST
            Self::TRIANGLESTRIP => 4,     // VK_PRIMITIVE_TOPOLOGY_TRIANGLE_STRIP
            Self::LINELIST_ADJ => 6,      // VK_PRIMITIVE_TOPOLOGY_LINE_LIST_WITH_ADJACENCY
            Self::LINESTRIP_ADJ => 7,     // VK_PRIMITIVE_TOPOLOGY_LINE_STRIP_WITH_ADJACENCY
            Self::TRIANGLELIST_ADJ => 8,  // VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST_WITH_ADJACENCY
            Self::TRIANGLESTRIP_ADJ => 9, // VK_PRIMITIVE_TOPOLOGY_TRIANGLE_STRIP_WITH_ADJACENCY
            _ => 3,                       // default to triangle list
        }
    }
}

// ---------------------------------------------------------------------------
// Blend state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_BLEND {
    ZERO = 1,
    ONE = 2,
    SRC_COLOR = 3,
    INV_SRC_COLOR = 4,
    SRC_ALPHA = 5,
    INV_SRC_ALPHA = 6,
    DEST_ALPHA = 7,
    INV_DEST_ALPHA = 8,
    DEST_COLOR = 9,
    INV_DEST_COLOR = 10,
    SRC_ALPHA_SAT = 11,
    BLEND_FACTOR = 14,
    INV_BLEND_FACTOR = 15,
    SRC1_COLOR = 16,
    INV_SRC1_COLOR = 17,
    SRC1_ALPHA = 18,
    INV_SRC1_ALPHA = 19,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_BLEND_OP {
    ADD = 1,
    SUBTRACT = 2,
    REV_SUBTRACT = 3,
    MIN = 4,
    MAX = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum D3D11_COLOR_WRITE_ENABLE {
    RED = 1,
    GREEN = 2,
    BLUE = 4,
    ALPHA = 8,
    ALL = 15,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_RENDER_TARGET_BLEND_DESC {
    pub blend_enable: bool,
    pub src_blend: D3D11_BLEND,
    pub dest_blend: D3D11_BLEND,
    pub blend_op: D3D11_BLEND_OP,
    pub src_blend_alpha: D3D11_BLEND,
    pub dest_blend_alpha: D3D11_BLEND,
    pub blend_op_alpha: D3D11_BLEND_OP,
    pub render_target_write_mask: u8,
}

impl Default for D3D11_RENDER_TARGET_BLEND_DESC {
    fn default() -> Self {
        Self {
            blend_enable: false,
            src_blend: D3D11_BLEND::ONE,
            dest_blend: D3D11_BLEND::ZERO,
            blend_op: D3D11_BLEND_OP::ADD,
            src_blend_alpha: D3D11_BLEND::ONE,
            dest_blend_alpha: D3D11_BLEND::ZERO,
            blend_op_alpha: D3D11_BLEND_OP::ADD,
            render_target_write_mask: D3D11_COLOR_WRITE_ENABLE::ALL as u8,
        }
    }
}

#[derive(Debug, Clone)]
pub struct D3D11_BLEND_DESC {
    pub alpha_to_coverage_enable: bool,
    pub independent_blend_enable: bool,
    pub render_target: [D3D11_RENDER_TARGET_BLEND_DESC; 8],
}

impl Default for D3D11_BLEND_DESC {
    fn default() -> Self {
        Self {
            alpha_to_coverage_enable: false,
            independent_blend_enable: false,
            render_target: [D3D11_RENDER_TARGET_BLEND_DESC::default(); 8],
        }
    }
}

// ---------------------------------------------------------------------------
// Depth stencil state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_COMPARISON_FUNC {
    NEVER = 1,
    LESS = 2,
    EQUAL = 3,
    LESS_EQUAL = 4,
    GREATER = 5,
    NOT_EQUAL = 6,
    GREATER_EQUAL = 7,
    ALWAYS = 8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_STENCIL_OP {
    KEEP = 1,
    ZERO = 2,
    REPLACE = 3,
    INCR_SAT = 4,
    DECR_SAT = 5,
    INVERT = 6,
    INCR = 7,
    DECR = 8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_DEPTH_WRITE_MASK {
    ZERO = 0,
    ALL = 1,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_DEPTH_STENCILOP_DESC {
    pub stencil_fail_op: D3D11_STENCIL_OP,
    pub stencil_depth_fail_op: D3D11_STENCIL_OP,
    pub stencil_pass_op: D3D11_STENCIL_OP,
    pub stencil_func: D3D11_COMPARISON_FUNC,
}

impl Default for D3D11_DEPTH_STENCILOP_DESC {
    fn default() -> Self {
        Self {
            stencil_fail_op: D3D11_STENCIL_OP::KEEP,
            stencil_depth_fail_op: D3D11_STENCIL_OP::KEEP,
            stencil_pass_op: D3D11_STENCIL_OP::KEEP,
            stencil_func: D3D11_COMPARISON_FUNC::ALWAYS,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_DEPTH_STENCIL_DESC {
    pub depth_enable: bool,
    pub depth_write_mask: D3D11_DEPTH_WRITE_MASK,
    pub depth_func: D3D11_COMPARISON_FUNC,
    pub stencil_enable: bool,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
    pub front_face: D3D11_DEPTH_STENCILOP_DESC,
    pub back_face: D3D11_DEPTH_STENCILOP_DESC,
}

impl Default for D3D11_DEPTH_STENCIL_DESC {
    fn default() -> Self {
        Self {
            depth_enable: true,
            depth_write_mask: D3D11_DEPTH_WRITE_MASK::ALL,
            depth_func: D3D11_COMPARISON_FUNC::LESS,
            stencil_enable: false,
            stencil_read_mask: 0xFF,
            stencil_write_mask: 0xFF,
            front_face: D3D11_DEPTH_STENCILOP_DESC::default(),
            back_face: D3D11_DEPTH_STENCILOP_DESC::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Rasterizer state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_FILL_MODE {
    WIREFRAME = 2,
    SOLID = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_CULL_MODE {
    NONE = 1,
    FRONT = 2,
    BACK = 3,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_RASTERIZER_DESC {
    pub fill_mode: D3D11_FILL_MODE,
    pub cull_mode: D3D11_CULL_MODE,
    pub front_counter_clockwise: bool,
    pub depth_bias: i32,
    pub depth_bias_clamp: f32,
    pub slope_scaled_depth_bias: f32,
    pub depth_clip_enable: bool,
    pub scissor_enable: bool,
    pub multisample_enable: bool,
    pub antialiased_line_enable: bool,
}

impl Default for D3D11_RASTERIZER_DESC {
    fn default() -> Self {
        Self {
            fill_mode: D3D11_FILL_MODE::SOLID,
            cull_mode: D3D11_CULL_MODE::BACK,
            front_counter_clockwise: false,
            depth_bias: 0,
            depth_bias_clamp: 0.0,
            slope_scaled_depth_bias: 0.0,
            depth_clip_enable: true,
            scissor_enable: false,
            multisample_enable: false,
            antialiased_line_enable: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Sampler state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_FILTER {
    MIN_MAG_MIP_POINT = 0,
    MIN_MAG_POINT_MIP_LINEAR = 0x1,
    MIN_POINT_MAG_LINEAR_MIP_POINT = 0x4,
    MIN_POINT_MAG_MIP_LINEAR = 0x5,
    MIN_LINEAR_MAG_MIP_POINT = 0x10,
    MIN_LINEAR_MAG_POINT_MIP_LINEAR = 0x11,
    MIN_MAG_LINEAR_MIP_POINT = 0x14,
    MIN_MAG_MIP_LINEAR = 0x15,
    ANISOTROPIC = 0x55,
    COMPARISON_MIN_MAG_MIP_POINT = 0x80,
    COMPARISON_MIN_MAG_MIP_LINEAR = 0x95,
    COMPARISON_ANISOTROPIC = 0xD5,
    MINIMUM_MIN_MAG_MIP_POINT = 0x100,
    MINIMUM_MIN_MAG_MIP_LINEAR = 0x115,
    MINIMUM_ANISOTROPIC = 0x155,
    MAXIMUM_MIN_MAG_MIP_POINT = 0x180,
    MAXIMUM_MIN_MAG_MIP_LINEAR = 0x195,
    MAXIMUM_ANISOTROPIC = 0x1D5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_TEXTURE_ADDRESS_MODE {
    WRAP = 1,
    MIRROR = 2,
    CLAMP = 3,
    BORDER = 4,
    MIRROR_ONCE = 5,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_SAMPLER_DESC {
    pub filter: D3D11_FILTER,
    pub address_u: D3D11_TEXTURE_ADDRESS_MODE,
    pub address_v: D3D11_TEXTURE_ADDRESS_MODE,
    pub address_w: D3D11_TEXTURE_ADDRESS_MODE,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison_func: D3D11_COMPARISON_FUNC,
    pub border_color: [f32; 4],
    pub min_lod: f32,
    pub max_lod: f32,
}

impl Default for D3D11_SAMPLER_DESC {
    fn default() -> Self {
        Self {
            filter: D3D11_FILTER::MIN_MAG_MIP_LINEAR,
            address_u: D3D11_TEXTURE_ADDRESS_MODE::CLAMP,
            address_v: D3D11_TEXTURE_ADDRESS_MODE::CLAMP,
            address_w: D3D11_TEXTURE_ADDRESS_MODE::CLAMP,
            mip_lod_bias: 0.0,
            max_anisotropy: 1,
            comparison_func: D3D11_COMPARISON_FUNC::NEVER,
            border_color: [0.0; 4],
            min_lod: 0.0,
            max_lod: f32::MAX,
        }
    }
}

// ---------------------------------------------------------------------------
// Resource descriptions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_USAGE {
    DEFAULT = 0,
    IMMUTABLE = 1,
    DYNAMIC = 2,
    STAGING = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_BIND_FLAG {
    VERTEX_BUFFER = 0x1,
    INDEX_BUFFER = 0x2,
    CONSTANT_BUFFER = 0x4,
    SHADER_RESOURCE = 0x8,
    STREAM_OUTPUT = 0x10,
    RENDER_TARGET = 0x20,
    DEPTH_STENCIL = 0x40,
    UNORDERED_ACCESS = 0x80,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_CPU_ACCESS_FLAG {
    NONE = 0,
    WRITE = 0x10000,
    READ = 0x20000,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_RESOURCE_MISC_FLAG {
    NONE = 0,
    GENERATE_MIPS = 0x1,
    SHARED = 0x2,
    TEXTURECUBE = 0x4,
    DRAWINDIRECT_ARGS = 0x10,
    BUFFER_ALLOW_RAW_VIEWS = 0x20,
    BUFFER_STRUCTURED = 0x40,
    RESOURCE_CLAMP = 0x80,
    SHARED_KEYEDMUTEX = 0x100,
    GDI_COMPATIBLE = 0x200,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_BUFFER_DESC {
    pub byte_width: u32,
    pub usage: D3D11_USAGE,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
    pub structure_byte_stride: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_TEXTURE1D_DESC {
    pub width: u32,
    pub mip_levels: u32,
    pub array_size: u32,
    pub format: DXGI_FORMAT,
    pub usage: D3D11_USAGE,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_TEXTURE2D_DESC {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub array_size: u32,
    pub format: DXGI_FORMAT,
    pub sample_count: u32,
    pub sample_quality: u32,
    pub usage: D3D11_USAGE,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_TEXTURE3D_DESC {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mip_levels: u32,
    pub format: DXGI_FORMAT,
    pub usage: D3D11_USAGE,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_SUBRESOURCE_DATA {
    pub sys_mem: u64,
    pub sys_mem_pitch: u32,
    pub sys_mem_slice_pitch: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_MAPPED_SUBRESOURCE {
    pub data: u64,
    pub row_pitch: u32,
    pub depth_pitch: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_MAP {
    READ = 1,
    WRITE = 2,
    READ_WRITE = 3,
    WRITE_DISCARD = 4,
    WRITE_NO_OVERWRITE = 5,
}

// ---------------------------------------------------------------------------
// Shader model 5.0 types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_SHADER_TYPE {
    VERTEX = 0,
    HULL = 1,
    DOMAIN = 2,
    GEOMETRY = 3,
    PIXEL = 4,
    COMPUTE = 5,
}

#[derive(Debug, Clone)]
pub struct D3D11_INPUT_ELEMENT_DESC {
    pub semantic_name: String,
    pub semantic_index: u32,
    pub format: DXGI_FORMAT,
    pub input_slot: u32,
    pub aligned_byte_offset: u32,
    pub input_slot_class: u32,
    pub instance_data_step_rate: u32,
}

#[derive(Debug, Clone)]
pub struct ShaderBytecode {
    pub shader_type: D3D11_SHADER_TYPE,
    pub bytecode: Vec<u8>,
    pub entry_point: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct InputSignatureElement {
    pub semantic_name: String,
    pub semantic_index: u32,
    pub register: u32,
    pub component_type: u32,
    pub mask: u8,
}

#[derive(Debug, Clone)]
pub struct OutputSignatureElement {
    pub semantic_name: String,
    pub semantic_index: u32,
    pub register: u32,
    pub component_type: u32,
    pub mask: u8,
}

// ---------------------------------------------------------------------------
// Viewport and scissor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct D3D11_VIEWPORT {
    pub top_left_x: f32,
    pub top_left_y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

// ---------------------------------------------------------------------------
// Query / predicate / counter types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D11_QUERY {
    EVENT = 0,
    OCCLUSION = 1,
    TIMESTAMP = 2,
    TIMESTAMP_DISJOINT = 3,
    PIPELINE_STATISTICS = 4,
    OCCLUSION_PREDICATE = 5,
    SO_STATISTICS = 6,
    SO_OVERFLOW_PREDICATE = 7,
    SO_STATISTICS_STREAM0 = 8,
    SO_STATISTICS_STREAM1 = 9,
    SO_STATISTICS_STREAM2 = 10,
    SO_STATISTICS_STREAM3 = 11,
    SO_OVERFLOW_PREDICATE_STREAM0 = 12,
    SO_OVERFLOW_PREDICATE_STREAM1 = 13,
    SO_OVERFLOW_PREDICATE_STREAM2 = 14,
    SO_OVERFLOW_PREDICATE_STREAM3 = 15,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D11_QUERY_DESC {
    pub query: D3D11_QUERY,
    pub misc_flags: u32,
}

// ---------------------------------------------------------------------------
// Internal resource handles
// ---------------------------------------------------------------------------

type ResourceId = u64;

#[derive(Debug, Clone)]
pub enum D3D11Resource {
    Buffer {
        id: ResourceId,
        desc: D3D11_BUFFER_DESC,
        data: Vec<u8>,
    },
    Texture1D {
        id: ResourceId,
        desc: D3D11_TEXTURE1D_DESC,
        data: Vec<u8>,
    },
    Texture2D {
        id: ResourceId,
        desc: D3D11_TEXTURE2D_DESC,
        data: Vec<u8>,
    },
    Texture3D {
        id: ResourceId,
        desc: D3D11_TEXTURE3D_DESC,
        data: Vec<u8>,
    },
}

impl D3D11Resource {
    pub fn id(&self) -> ResourceId {
        match self {
            Self::Buffer { id, .. } => *id,
            Self::Texture1D { id, .. } => *id,
            Self::Texture2D { id, .. } => *id,
            Self::Texture3D { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct D3D11View {
    pub id: ResourceId,
    pub resource_id: ResourceId,
    pub format: DXGI_FORMAT,
}

#[derive(Debug, Clone)]
pub struct D3D11BlendState {
    pub id: ResourceId,
    pub desc: D3D11_BLEND_DESC,
}

#[derive(Debug, Clone)]
pub struct D3D11DepthStencilState {
    pub id: ResourceId,
    pub desc: D3D11_DEPTH_STENCIL_DESC,
}

#[derive(Debug, Clone)]
pub struct D3D11RasterizerState {
    pub id: ResourceId,
    pub desc: D3D11_RASTERIZER_DESC,
}

#[derive(Debug, Clone)]
pub struct D3D11SamplerState {
    pub id: ResourceId,
    pub desc: D3D11_SAMPLER_DESC,
}

#[derive(Debug, Clone)]
pub struct D3D11InputLayout {
    pub id: ResourceId,
    pub elements: Vec<D3D11_INPUT_ELEMENT_DESC>,
}

#[derive(Debug, Clone)]
pub struct D3D11Shader {
    pub id: ResourceId,
    pub shader_type: D3D11_SHADER_TYPE,
    pub bytecode: Vec<u8>,
    pub input_signature: Vec<InputSignatureElement>,
    pub output_signature: Vec<OutputSignatureElement>,
}

#[derive(Debug, Clone)]
pub struct D3D11QueryObject {
    pub id: ResourceId,
    pub desc: D3D11_QUERY_DESC,
    pub active: bool,
    pub result: u64,
}

// ---------------------------------------------------------------------------
// ID3D11DeviceContext state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PipelineState {
    pub input_layout: Option<ResourceId>,
    pub primitive_topology: D3D11_PRIMITIVE_TOPOLOGY,
    pub vertex_shader: Option<ResourceId>,
    pub hull_shader: Option<ResourceId>,
    pub domain_shader: Option<ResourceId>,
    pub geometry_shader: Option<ResourceId>,
    pub pixel_shader: Option<ResourceId>,
    pub compute_shader: Option<ResourceId>,
    pub vertex_buffers: [Option<ResourceId>; 16],
    pub vertex_strides: [u32; 16],
    pub vertex_offsets: [u32; 16],
    pub index_buffer: Option<ResourceId>,
    pub index_format: DXGI_FORMAT,
    pub index_offset: u32,
    pub vs_constant_buffers: [Option<ResourceId>; 14],
    pub ps_constant_buffers: [Option<ResourceId>; 14],
    pub ps_shader_resources: [Option<ResourceId>; 128],
    pub ps_samplers: [Option<ResourceId>; 16],
    pub render_targets: [Option<ResourceId>; 8],
    pub depth_stencil_view: Option<ResourceId>,
    pub blend_state: Option<ResourceId>,
    pub blend_factor: [f32; 4],
    pub sample_mask: u32,
    pub depth_stencil_state: Option<ResourceId>,
    pub stencil_ref: u32,
    pub rasterizer_state: Option<ResourceId>,
    pub viewports: Vec<D3D11_VIEWPORT>,
    pub scissor_rects: Vec<D3D11_RECT>,
    pub so_targets: [Option<ResourceId>; 4],
}

impl Default for PipelineState {
    fn default() -> Self {
        Self {
            input_layout: None,
            primitive_topology: D3D11_PRIMITIVE_TOPOLOGY::UNDEFINED,
            vertex_shader: None,
            hull_shader: None,
            domain_shader: None,
            geometry_shader: None,
            pixel_shader: None,
            compute_shader: None,
            vertex_buffers: [None; 16],
            vertex_strides: [0; 16],
            vertex_offsets: [0; 16],
            index_buffer: None,
            index_format: DXGI_FORMAT::UNKNOWN,
            index_offset: 0,
            vs_constant_buffers: [None; 14],
            ps_constant_buffers: [None; 14],
            ps_shader_resources: [None; 128],
            ps_samplers: [None; 16],
            render_targets: [None; 8],
            depth_stencil_view: None,
            blend_state: None,
            blend_factor: [1.0; 4],
            sample_mask: 0xFFFFFFFF,
            depth_stencil_state: None,
            stencil_ref: 0,
            rasterizer_state: None,
            viewports: Vec::new(),
            scissor_rects: Vec::new(),
            so_targets: [None; 4],
        }
    }
}

// ---------------------------------------------------------------------------
// ID3D11DeviceContext
// ---------------------------------------------------------------------------

pub struct D3D11DeviceContext {
    pub is_deferred: bool,
    pub state: PipelineState,
    pub command_list: Vec<D3D11Command>,
    draw_call_count: u64,
    dispatch_count: u64,
}

#[derive(Debug, Clone)]
pub enum D3D11Command {
    Draw {
        vertex_count: u32,
        start_vertex: u32,
    },
    DrawIndexed {
        index_count: u32,
        start_index: u32,
        base_vertex: i32,
    },
    DrawInstanced {
        vertex_count: u32,
        instance_count: u32,
        start_vertex: u32,
        start_instance: u32,
    },
    DrawIndexedInstanced {
        index_count: u32,
        instance_count: u32,
        start_index: u32,
        base_vertex: i32,
        start_instance: u32,
    },
    Dispatch {
        x: u32,
        y: u32,
        z: u32,
    },
    ClearRenderTarget {
        view_id: ResourceId,
        color: [f32; 4],
    },
    ClearDepthStencil {
        view_id: ResourceId,
        clear_flags: u32,
        depth: f32,
        stencil: u8,
    },
    CopyResource {
        dst: ResourceId,
        src: ResourceId,
    },
    CopySubresource {
        dst: ResourceId,
        dst_subresource: u32,
        dst_x: u32,
        dst_y: u32,
        dst_z: u32,
        src: ResourceId,
        src_subresource: u32,
    },
    UpdateSubresource {
        dst: ResourceId,
        subresource: u32,
        data_ptr: u64,
        row_pitch: u32,
        depth_pitch: u32,
    },
    GenerateMips {
        srv_id: ResourceId,
    },
    ResolveSubresource {
        dst: ResourceId,
        dst_sub: u32,
        src: ResourceId,
        src_sub: u32,
        format: DXGI_FORMAT,
    },
    SetState(Box<PipelineState>),
}

impl D3D11DeviceContext {
    pub fn new(deferred: bool) -> Self {
        Self {
            is_deferred: deferred,
            state: PipelineState::default(),
            command_list: Vec::new(),
            draw_call_count: 0,
            dispatch_count: 0,
        }
    }

    pub fn ia_set_input_layout(&mut self, layout_id: ResourceId) {
        self.state.input_layout = Some(layout_id);
    }

    pub fn ia_set_vertex_buffers(&mut self, start_slot: u32, buffers: &[(ResourceId, u32, u32)]) {
        for (i, &(buf, stride, offset)) in buffers.iter().enumerate() {
            let slot = (start_slot as usize) + i;
            if slot < 16 {
                self.state.vertex_buffers[slot] = Some(buf);
                self.state.vertex_strides[slot] = stride;
                self.state.vertex_offsets[slot] = offset;
            }
        }
    }

    pub fn ia_set_index_buffer(&mut self, buffer: ResourceId, format: DXGI_FORMAT, offset: u32) {
        self.state.index_buffer = Some(buffer);
        self.state.index_format = format;
        self.state.index_offset = offset;
    }

    pub fn ia_set_primitive_topology(&mut self, topology: D3D11_PRIMITIVE_TOPOLOGY) {
        self.state.primitive_topology = topology;
    }

    pub fn vs_set_shader(&mut self, shader_id: Option<ResourceId>) {
        self.state.vertex_shader = shader_id;
    }

    pub fn hs_set_shader(&mut self, shader_id: Option<ResourceId>) {
        self.state.hull_shader = shader_id;
    }

    pub fn ds_set_shader(&mut self, shader_id: Option<ResourceId>) {
        self.state.domain_shader = shader_id;
    }

    pub fn gs_set_shader(&mut self, shader_id: Option<ResourceId>) {
        self.state.geometry_shader = shader_id;
    }

    pub fn ps_set_shader(&mut self, shader_id: Option<ResourceId>) {
        self.state.pixel_shader = shader_id;
    }

    pub fn cs_set_shader(&mut self, shader_id: Option<ResourceId>) {
        self.state.compute_shader = shader_id;
    }

    pub fn vs_set_constant_buffers(&mut self, start_slot: u32, buffers: &[ResourceId]) {
        for (i, &buf) in buffers.iter().enumerate() {
            let slot = (start_slot as usize) + i;
            if slot < 14 {
                self.state.vs_constant_buffers[slot] = Some(buf);
            }
        }
    }

    pub fn ps_set_constant_buffers(&mut self, start_slot: u32, buffers: &[ResourceId]) {
        for (i, &buf) in buffers.iter().enumerate() {
            let slot = (start_slot as usize) + i;
            if slot < 14 {
                self.state.ps_constant_buffers[slot] = Some(buf);
            }
        }
    }

    pub fn ps_set_shader_resources(&mut self, start_slot: u32, views: &[ResourceId]) {
        for (i, &v) in views.iter().enumerate() {
            let slot = (start_slot as usize) + i;
            if slot < 128 {
                self.state.ps_shader_resources[slot] = Some(v);
            }
        }
    }

    pub fn ps_set_samplers(&mut self, start_slot: u32, samplers: &[ResourceId]) {
        for (i, &s) in samplers.iter().enumerate() {
            let slot = (start_slot as usize) + i;
            if slot < 16 {
                self.state.ps_samplers[slot] = Some(s);
            }
        }
    }

    pub fn om_set_render_targets(&mut self, views: &[Option<ResourceId>], dsv: Option<ResourceId>) {
        for i in 0..8 {
            self.state.render_targets[i] = if i < views.len() { views[i] } else { None };
        }
        self.state.depth_stencil_view = dsv;
    }

    pub fn om_set_depth_stencil_state(&mut self, state_id: Option<ResourceId>, stencil_ref: u32) {
        self.state.depth_stencil_state = state_id;
        self.state.stencil_ref = stencil_ref;
    }

    pub fn om_set_blend_state(
        &mut self,
        state_id: Option<ResourceId>,
        blend_factor: [f32; 4],
        sample_mask: u32,
    ) {
        self.state.blend_state = state_id;
        self.state.blend_factor = blend_factor;
        self.state.sample_mask = sample_mask;
    }

    pub fn rs_set_viewports(&mut self, viewports: &[D3D11_VIEWPORT]) {
        self.state.viewports = viewports.to_vec();
    }

    pub fn rs_set_scissor_rects(&mut self, rects: &[D3D11_RECT]) {
        self.state.scissor_rects = rects.to_vec();
    }

    pub fn rs_set_state(&mut self, state_id: Option<ResourceId>) {
        self.state.rasterizer_state = state_id;
    }

    pub fn so_set_targets(&mut self, targets: &[Option<ResourceId>]) {
        for i in 0..4 {
            self.state.so_targets[i] = if i < targets.len() { targets[i] } else { None };
        }
    }

    pub fn draw(&mut self, vertex_count: u32, start_vertex: u32) {
        self.draw_call_count += 1;
        let cmd = D3D11Command::Draw {
            vertex_count,
            start_vertex,
        };
        self.command_list.push(cmd);
    }

    pub fn draw_indexed(&mut self, index_count: u32, start_index: u32, base_vertex: i32) {
        self.draw_call_count += 1;
        let cmd = D3D11Command::DrawIndexed {
            index_count,
            start_index,
            base_vertex,
        };
        self.command_list.push(cmd);
    }

    pub fn draw_instanced(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        start_vertex: u32,
        start_instance: u32,
    ) {
        self.draw_call_count += 1;
        let cmd = D3D11Command::DrawInstanced {
            vertex_count,
            instance_count,
            start_vertex,
            start_instance,
        };
        self.command_list.push(cmd);
    }

    pub fn draw_indexed_instanced(
        &mut self,
        index_count: u32,
        instance_count: u32,
        start_index: u32,
        base_vertex: i32,
        start_instance: u32,
    ) {
        self.draw_call_count += 1;
        let cmd = D3D11Command::DrawIndexedInstanced {
            index_count,
            instance_count,
            start_index,
            base_vertex,
            start_instance,
        };
        self.command_list.push(cmd);
    }

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        self.dispatch_count += 1;
        let cmd = D3D11Command::Dispatch { x, y, z };
        self.command_list.push(cmd);
    }

    pub fn clear_render_target_view(&mut self, view_id: ResourceId, color: [f32; 4]) {
        let cmd = D3D11Command::ClearRenderTarget { view_id, color };
        self.command_list.push(cmd);
    }

    pub fn clear_depth_stencil_view(
        &mut self,
        view_id: ResourceId,
        clear_flags: u32,
        depth: f32,
        stencil: u8,
    ) {
        let cmd = D3D11Command::ClearDepthStencil {
            view_id,
            clear_flags,
            depth,
            stencil,
        };
        self.command_list.push(cmd);
    }

    pub fn copy_resource(&mut self, dst: ResourceId, src: ResourceId) {
        let cmd = D3D11Command::CopyResource { dst, src };
        self.command_list.push(cmd);
    }

    pub fn copy_subresource_region(
        &mut self,
        dst: ResourceId,
        dst_subresource: u32,
        dst_x: u32,
        dst_y: u32,
        dst_z: u32,
        src: ResourceId,
        src_subresource: u32,
    ) {
        let cmd = D3D11Command::CopySubresource {
            dst,
            dst_subresource,
            dst_x,
            dst_y,
            dst_z,
            src,
            src_subresource,
        };
        self.command_list.push(cmd);
    }

    pub fn update_subresource(
        &mut self,
        dst: ResourceId,
        subresource: u32,
        data_ptr: u64,
        row_pitch: u32,
        depth_pitch: u32,
    ) {
        let cmd = D3D11Command::UpdateSubresource {
            dst,
            subresource,
            data_ptr,
            row_pitch,
            depth_pitch,
        };
        self.command_list.push(cmd);
    }

    pub fn map(
        &self,
        _resource_id: ResourceId,
        _subresource: u32,
        _map_type: D3D11_MAP,
    ) -> Result<D3D11_MAPPED_SUBRESOURCE, i32> {
        Ok(D3D11_MAPPED_SUBRESOURCE {
            data: 0,
            row_pitch: 0,
            depth_pitch: 0,
        })
    }

    pub fn unmap(&self, _resource_id: ResourceId, _subresource: u32) {}

    pub fn flush(&mut self) {
        // Submit recorded commands to the Vulkan backend (stub)
    }

    pub fn begin(&mut self, _query_id: ResourceId) {}

    pub fn end(&mut self, _query_id: ResourceId) {}

    pub fn get_data(&self, _query_id: ResourceId) -> Result<u64, i32> {
        Err(S_FALSE)
    }

    pub fn generate_mips(&mut self, srv_id: ResourceId) {
        let cmd = D3D11Command::GenerateMips { srv_id };
        self.command_list.push(cmd);
    }

    pub fn resolve_subresource(
        &mut self,
        dst: ResourceId,
        dst_sub: u32,
        src: ResourceId,
        src_sub: u32,
        format: DXGI_FORMAT,
    ) {
        let cmd = D3D11Command::ResolveSubresource {
            dst,
            dst_sub,
            src,
            src_sub,
            format,
        };
        self.command_list.push(cmd);
    }

    pub fn execute_command_list(&mut self, command_list: &[D3D11Command], restore_state: bool) {
        let saved = if restore_state {
            Some(self.state.clone())
        } else {
            None
        };
        for cmd in command_list {
            self.command_list.push(cmd.clone());
        }
        if let Some(s) = saved {
            self.state = s;
        }
    }

    pub fn finish_command_list(&mut self, restore_state: bool) -> Vec<D3D11Command> {
        let commands = core::mem::take(&mut self.command_list);
        if !restore_state {
            self.state = PipelineState::default();
        }
        commands
    }

    pub fn draw_call_count(&self) -> u64 {
        self.draw_call_count
    }
    pub fn dispatch_count(&self) -> u64 {
        self.dispatch_count
    }
}

// ---------------------------------------------------------------------------
// ID3D11Device
// ---------------------------------------------------------------------------

pub struct D3D11Device {
    feature_level: D3D_FEATURE_LEVEL,
    creation_flags: u32,
    next_id: AtomicU64,
    resources: Vec<D3D11Resource>,
    views: Vec<D3D11View>,
    blend_states: Vec<D3D11BlendState>,
    depth_stencil_states: Vec<D3D11DepthStencilState>,
    rasterizer_states: Vec<D3D11RasterizerState>,
    sampler_states: Vec<D3D11SamplerState>,
    input_layouts: Vec<D3D11InputLayout>,
    shaders: Vec<D3D11Shader>,
    queries: Vec<D3D11QueryObject>,
    immediate_context: D3D11DeviceContext,
    device_removed_reason: i32,
}

impl D3D11Device {
    pub fn new(feature_level: D3D_FEATURE_LEVEL, creation_flags: u32) -> Self {
        Self {
            feature_level,
            creation_flags,
            next_id: AtomicU64::new(1),
            resources: Vec::new(),
            views: Vec::new(),
            blend_states: Vec::new(),
            depth_stencil_states: Vec::new(),
            rasterizer_states: Vec::new(),
            sampler_states: Vec::new(),
            input_layouts: Vec::new(),
            shaders: Vec::new(),
            queries: Vec::new(),
            immediate_context: D3D11DeviceContext::new(false),
            device_removed_reason: S_OK,
        }
    }

    fn alloc_id(&self) -> ResourceId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn create_buffer(
        &mut self,
        desc: &D3D11_BUFFER_DESC,
        initial_data: Option<&[u8]>,
    ) -> Result<ResourceId, i32> {
        if desc.byte_width == 0 {
            return Err(E_INVALIDARG);
        }
        let id = self.alloc_id();
        let data = match initial_data {
            Some(d) => d.to_vec(),
            None => alloc::vec![0u8; desc.byte_width as usize],
        };
        self.resources.push(D3D11Resource::Buffer {
            id,
            desc: *desc,
            data,
        });
        Ok(id)
    }

    pub fn create_texture1d(
        &mut self,
        desc: &D3D11_TEXTURE1D_DESC,
        initial_data: Option<&[u8]>,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        let size = (desc.width
            * desc.format.bytes_per_pixel()
            * desc.mip_levels.max(1)
            * desc.array_size.max(1)) as usize;
        let data = match initial_data {
            Some(d) => d.to_vec(),
            None => alloc::vec![0u8; size],
        };
        self.resources.push(D3D11Resource::Texture1D {
            id,
            desc: *desc,
            data,
        });
        Ok(id)
    }

    pub fn create_texture2d(
        &mut self,
        desc: &D3D11_TEXTURE2D_DESC,
        initial_data: Option<&[u8]>,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        let bpp = desc.format.bytes_per_pixel().max(1);
        let size =
            (desc.width * desc.height * bpp * desc.mip_levels.max(1) * desc.array_size.max(1))
                as usize;
        let data = match initial_data {
            Some(d) => d.to_vec(),
            None => alloc::vec![0u8; size],
        };
        self.resources.push(D3D11Resource::Texture2D {
            id,
            desc: *desc,
            data,
        });
        Ok(id)
    }

    pub fn create_texture3d(
        &mut self,
        desc: &D3D11_TEXTURE3D_DESC,
        initial_data: Option<&[u8]>,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        let bpp = desc.format.bytes_per_pixel().max(1);
        let size = (desc.width * desc.height * desc.depth * bpp * desc.mip_levels.max(1)) as usize;
        let data = match initial_data {
            Some(d) => d.to_vec(),
            None => alloc::vec![0u8; size],
        };
        self.resources.push(D3D11Resource::Texture3D {
            id,
            desc: *desc,
            data,
        });
        Ok(id)
    }

    pub fn create_shader_resource_view(
        &mut self,
        resource_id: ResourceId,
        format: DXGI_FORMAT,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.views.push(D3D11View {
            id,
            resource_id,
            format,
        });
        Ok(id)
    }

    pub fn create_unordered_access_view(
        &mut self,
        resource_id: ResourceId,
        format: DXGI_FORMAT,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.views.push(D3D11View {
            id,
            resource_id,
            format,
        });
        Ok(id)
    }

    pub fn create_render_target_view(
        &mut self,
        resource_id: ResourceId,
        format: DXGI_FORMAT,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.views.push(D3D11View {
            id,
            resource_id,
            format,
        });
        Ok(id)
    }

    pub fn create_depth_stencil_view(
        &mut self,
        resource_id: ResourceId,
        format: DXGI_FORMAT,
    ) -> Result<ResourceId, i32> {
        if !format.is_depth_stencil() && format != DXGI_FORMAT::UNKNOWN {
            return Err(E_INVALIDARG);
        }
        let id = self.alloc_id();
        self.views.push(D3D11View {
            id,
            resource_id,
            format,
        });
        Ok(id)
    }

    pub fn create_input_layout(
        &mut self,
        elements: Vec<D3D11_INPUT_ELEMENT_DESC>,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.input_layouts.push(D3D11InputLayout { id, elements });
        Ok(id)
    }

    pub fn create_vertex_shader(&mut self, bytecode: &[u8]) -> Result<ResourceId, i32> {
        self.create_shader_internal(D3D11_SHADER_TYPE::VERTEX, bytecode)
    }

    pub fn create_hull_shader(&mut self, bytecode: &[u8]) -> Result<ResourceId, i32> {
        self.create_shader_internal(D3D11_SHADER_TYPE::HULL, bytecode)
    }

    pub fn create_domain_shader(&mut self, bytecode: &[u8]) -> Result<ResourceId, i32> {
        self.create_shader_internal(D3D11_SHADER_TYPE::DOMAIN, bytecode)
    }

    pub fn create_geometry_shader(&mut self, bytecode: &[u8]) -> Result<ResourceId, i32> {
        self.create_shader_internal(D3D11_SHADER_TYPE::GEOMETRY, bytecode)
    }

    pub fn create_pixel_shader(&mut self, bytecode: &[u8]) -> Result<ResourceId, i32> {
        self.create_shader_internal(D3D11_SHADER_TYPE::PIXEL, bytecode)
    }

    pub fn create_compute_shader(&mut self, bytecode: &[u8]) -> Result<ResourceId, i32> {
        self.create_shader_internal(D3D11_SHADER_TYPE::COMPUTE, bytecode)
    }

    fn create_shader_internal(
        &mut self,
        shader_type: D3D11_SHADER_TYPE,
        bytecode: &[u8],
    ) -> Result<ResourceId, i32> {
        if bytecode.is_empty() {
            return Err(E_INVALIDARG);
        }
        let id = self.alloc_id();
        self.shaders.push(D3D11Shader {
            id,
            shader_type,
            bytecode: bytecode.to_vec(),
            input_signature: Vec::new(),
            output_signature: Vec::new(),
        });
        Ok(id)
    }

    pub fn create_blend_state(&mut self, desc: &D3D11_BLEND_DESC) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.blend_states.push(D3D11BlendState {
            id,
            desc: desc.clone(),
        });
        Ok(id)
    }

    pub fn create_depth_stencil_state(
        &mut self,
        desc: &D3D11_DEPTH_STENCIL_DESC,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.depth_stencil_states
            .push(D3D11DepthStencilState { id, desc: *desc });
        Ok(id)
    }

    pub fn create_rasterizer_state(
        &mut self,
        desc: &D3D11_RASTERIZER_DESC,
    ) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.rasterizer_states
            .push(D3D11RasterizerState { id, desc: *desc });
        Ok(id)
    }

    pub fn create_sampler_state(&mut self, desc: &D3D11_SAMPLER_DESC) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.sampler_states
            .push(D3D11SamplerState { id, desc: *desc });
        Ok(id)
    }

    pub fn create_query(&mut self, desc: &D3D11_QUERY_DESC) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        self.queries.push(D3D11QueryObject {
            id,
            desc: *desc,
            active: false,
            result: 0,
        });
        Ok(id)
    }

    pub fn create_predicate(&mut self, desc: &D3D11_QUERY_DESC) -> Result<ResourceId, i32> {
        self.create_query(desc)
    }

    pub fn create_counter(&mut self) -> Result<ResourceId, i32> {
        let id = self.alloc_id();
        Ok(id)
    }

    pub fn create_deferred_context(&self) -> D3D11DeviceContext {
        D3D11DeviceContext::new(true)
    }

    pub fn open_shared_resource(&self, _handle: u64) -> Result<ResourceId, i32> {
        Err(E_NOTIMPL)
    }

    pub fn check_feature_support(&self, feature: u32) -> Result<u32, i32> {
        match feature {
            0 => Ok(1), // D3D11_FEATURE_THREADING: driver supports concurrent creates
            1 => Ok(1), // D3D11_FEATURE_DOUBLES: supports double precision
            _ => Err(E_INVALIDARG),
        }
    }

    pub fn get_feature_level(&self) -> D3D_FEATURE_LEVEL {
        self.feature_level
    }

    pub fn get_creation_flags(&self) -> u32 {
        self.creation_flags
    }

    pub fn get_device_removed_reason(&self) -> i32 {
        self.device_removed_reason
    }

    pub fn get_immediate_context(&mut self) -> &mut D3D11DeviceContext {
        &mut self.immediate_context
    }

    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }
    pub fn view_count(&self) -> usize {
        self.views.len()
    }
    pub fn shader_count(&self) -> usize {
        self.shaders.len()
    }
}

// ---------------------------------------------------------------------------
// D3D11 → Vulkan translation layer stubs
// ---------------------------------------------------------------------------

pub struct D3D11ToVulkanTranslator {
    pub vk_instance: u64,
    pub vk_device: u64,
    pub vk_queue: u64,
    pub vk_command_pool: u64,
    pub active_render_pass: u64,
    pub active_pipeline: u64,
    pub active_framebuffer: u64,
    pub pipeline_cache: Vec<(u64, u64)>,
    pub descriptor_pool: u64,
}

impl D3D11ToVulkanTranslator {
    pub fn new() -> Self {
        Self {
            vk_instance: 0,
            vk_device: 0,
            vk_queue: 0,
            vk_command_pool: 0,
            active_render_pass: 0,
            active_pipeline: 0,
            active_framebuffer: 0,
            pipeline_cache: Vec::new(),
            descriptor_pool: 0,
        }
    }

    pub fn translate_draw(&mut self, cmd: &D3D11Command) -> i32 {
        match cmd {
            D3D11Command::Draw { .. } => {
                /* vkCmdDraw */
                S_OK
            }
            D3D11Command::DrawIndexed { .. } => {
                /* vkCmdDrawIndexed */
                S_OK
            }
            D3D11Command::DrawInstanced { .. } => {
                /* vkCmdDraw with instance */
                S_OK
            }
            D3D11Command::DrawIndexedInstanced { .. } => {
                /* vkCmdDrawIndexed with instance */
                S_OK
            }
            D3D11Command::Dispatch { .. } => {
                /* vkCmdDispatch */
                S_OK
            }
            D3D11Command::ClearRenderTarget { .. } => {
                /* vkCmdClearColorImage */
                S_OK
            }
            D3D11Command::ClearDepthStencil { .. } => {
                /* vkCmdClearDepthStencilImage */
                S_OK
            }
            D3D11Command::CopyResource { .. } => {
                /* vkCmdCopyBuffer / vkCmdCopyImage */
                S_OK
            }
            D3D11Command::CopySubresource { .. } => {
                /* vkCmdCopyBufferToImage */
                S_OK
            }
            D3D11Command::UpdateSubresource { .. } => {
                /* vkCmdUpdateBuffer */
                S_OK
            }
            D3D11Command::GenerateMips { .. } => {
                /* vkCmdBlitImage chain */
                S_OK
            }
            D3D11Command::ResolveSubresource { .. } => {
                /* vkCmdResolveImage */
                S_OK
            }
            D3D11Command::SetState(_) => S_OK,
        }
    }

    pub fn translate_blend_to_vk(blend: D3D11_BLEND) -> u32 {
        match blend {
            D3D11_BLEND::ZERO => 0,
            D3D11_BLEND::ONE => 1,
            D3D11_BLEND::SRC_COLOR => 2,
            D3D11_BLEND::INV_SRC_COLOR => 3,
            D3D11_BLEND::SRC_ALPHA => 6,
            D3D11_BLEND::INV_SRC_ALPHA => 7,
            D3D11_BLEND::DEST_ALPHA => 8,
            D3D11_BLEND::INV_DEST_ALPHA => 9,
            D3D11_BLEND::DEST_COLOR => 4,
            D3D11_BLEND::INV_DEST_COLOR => 5,
            D3D11_BLEND::SRC_ALPHA_SAT => 10,
            D3D11_BLEND::BLEND_FACTOR => 14,
            D3D11_BLEND::INV_BLEND_FACTOR => 15,
            D3D11_BLEND::SRC1_COLOR => 16,
            D3D11_BLEND::INV_SRC1_COLOR => 17,
            D3D11_BLEND::SRC1_ALPHA => 18,
            D3D11_BLEND::INV_SRC1_ALPHA => 19,
        }
    }

    pub fn translate_comparison_to_vk(cmp: D3D11_COMPARISON_FUNC) -> u32 {
        match cmp {
            D3D11_COMPARISON_FUNC::NEVER => 0,
            D3D11_COMPARISON_FUNC::LESS => 1,
            D3D11_COMPARISON_FUNC::EQUAL => 2,
            D3D11_COMPARISON_FUNC::LESS_EQUAL => 3,
            D3D11_COMPARISON_FUNC::GREATER => 4,
            D3D11_COMPARISON_FUNC::NOT_EQUAL => 5,
            D3D11_COMPARISON_FUNC::GREATER_EQUAL => 6,
            D3D11_COMPARISON_FUNC::ALWAYS => 7,
        }
    }

    pub fn translate_stencil_op_to_vk(op: D3D11_STENCIL_OP) -> u32 {
        match op {
            D3D11_STENCIL_OP::KEEP => 0,
            D3D11_STENCIL_OP::ZERO => 1,
            D3D11_STENCIL_OP::REPLACE => 2,
            D3D11_STENCIL_OP::INCR_SAT => 3,
            D3D11_STENCIL_OP::DECR_SAT => 4,
            D3D11_STENCIL_OP::INVERT => 5,
            D3D11_STENCIL_OP::INCR => 6,
            D3D11_STENCIL_OP::DECR => 7,
        }
    }

    pub fn translate_filter_to_vk(filter: D3D11_FILTER) -> (u32, u32, u32) {
        match filter {
            D3D11_FILTER::MIN_MAG_MIP_POINT => (0, 0, 0),
            D3D11_FILTER::MIN_MAG_MIP_LINEAR => (1, 1, 1),
            D3D11_FILTER::ANISOTROPIC => (1, 1, 1),
            D3D11_FILTER::MIN_MAG_POINT_MIP_LINEAR => (0, 0, 1),
            D3D11_FILTER::MIN_POINT_MAG_LINEAR_MIP_POINT => (0, 1, 0),
            D3D11_FILTER::MIN_POINT_MAG_MIP_LINEAR => (0, 1, 1),
            D3D11_FILTER::MIN_LINEAR_MAG_MIP_POINT => (1, 0, 0),
            D3D11_FILTER::MIN_LINEAR_MAG_POINT_MIP_LINEAR => (1, 0, 1),
            D3D11_FILTER::MIN_MAG_LINEAR_MIP_POINT => (1, 1, 0),
            _ => (1, 1, 1),
        }
    }

    pub fn translate_address_mode_to_vk(mode: D3D11_TEXTURE_ADDRESS_MODE) -> u32 {
        match mode {
            D3D11_TEXTURE_ADDRESS_MODE::WRAP => 0,
            D3D11_TEXTURE_ADDRESS_MODE::MIRROR => 1,
            D3D11_TEXTURE_ADDRESS_MODE::CLAMP => 2,
            D3D11_TEXTURE_ADDRESS_MODE::BORDER => 3,
            D3D11_TEXTURE_ADDRESS_MODE::MIRROR_ONCE => 4,
        }
    }

    pub fn translate_fill_mode_to_vk(mode: D3D11_FILL_MODE) -> u32 {
        match mode {
            D3D11_FILL_MODE::WIREFRAME => 1,
            D3D11_FILL_MODE::SOLID => 0,
        }
    }

    pub fn translate_cull_mode_to_vk(mode: D3D11_CULL_MODE) -> u32 {
        match mode {
            D3D11_CULL_MODE::NONE => 0,
            D3D11_CULL_MODE::FRONT => 1,
            D3D11_CULL_MODE::BACK => 2,
        }
    }

    pub fn submit_commands(&mut self, commands: &[D3D11Command]) -> i32 {
        for cmd in commands {
            let result = self.translate_draw(cmd);
            if result != S_OK {
                return result;
            }
        }
        S_OK
    }
}

// ---------------------------------------------------------------------------
// Global D3D11 runtime
// ---------------------------------------------------------------------------

pub struct D3D11Runtime {
    pub initialized: AtomicBool,
    pub device: Option<D3D11Device>,
    pub translator: Option<D3D11ToVulkanTranslator>,
    pub adapter_description: String,
    pub adapter_vendor_id: u32,
    pub adapter_device_id: u32,
    pub dedicated_video_memory: u64,
    pub dedicated_system_memory: u64,
    pub shared_system_memory: u64,
}

impl D3D11Runtime {
    pub const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            device: None,
            translator: None,
            adapter_description: String::new(),
            adapter_vendor_id: 0,
            adapter_device_id: 0,
            dedicated_video_memory: 0,
            dedicated_system_memory: 0,
            shared_system_memory: 0,
        }
    }

    pub fn init(&mut self) -> i32 {
        if self.initialized.load(Ordering::Acquire) {
            return S_OK;
        }

        self.adapter_description = String::from("RaeGFX Virtual Adapter (Vulkan)");
        self.adapter_vendor_id = 0x1002;
        self.adapter_device_id = 0x7340;
        self.dedicated_video_memory = 8 * 1024 * 1024 * 1024;
        self.dedicated_system_memory = 256 * 1024 * 1024;
        self.shared_system_memory = 16 * 1024 * 1024 * 1024;

        let device = D3D11Device::new(D3D_FEATURE_LEVEL::D3D_FEATURE_LEVEL_11_1, 0);
        self.device = Some(device);
        self.translator = Some(D3D11ToVulkanTranslator::new());

        self.initialized.store(true, Ordering::Release);
        S_OK
    }

    pub fn shutdown(&mut self) {
        self.device = None;
        self.translator = None;
        self.initialized.store(false, Ordering::Release);
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    pub fn device(&mut self) -> Option<&mut D3D11Device> {
        self.device.as_mut()
    }

    pub fn translator(&mut self) -> Option<&mut D3D11ToVulkanTranslator> {
        self.translator.as_mut()
    }
}

pub static mut D3D11_RUNTIME: D3D11Runtime = D3D11Runtime::new();

pub fn init() -> i32 {
    unsafe { D3D11_RUNTIME.init() }
}
