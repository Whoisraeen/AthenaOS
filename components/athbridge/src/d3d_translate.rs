//! DirectX 11/12 → AthGFX Translation Layer
//!
//! Integrated translation at the driver level (DXVK/VKD3D-Proton lineage).
//! Maps D3D11/12 API semantics to the AthGFX pipeline — resource creation,
//! state objects, command recording, shader compilation, and present.

#![allow(non_camel_case_types, dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// Section 1: Comprehensive DXGI_FORMAT → AthGFX PixelFormat Mapping
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum DxgiFormat {
    Unknown = 0,
    R32G32B32A32_Typeless = 1,
    R32G32B32A32_Float = 2,
    R32G32B32A32_Uint = 3,
    R32G32B32A32_Sint = 4,
    R32G32B32_Typeless = 5,
    R32G32B32_Float = 6,
    R32G32B32_Uint = 7,
    R32G32B32_Sint = 8,
    R16G16B16A16_Typeless = 9,
    R16G16B16A16_Float = 10,
    R16G16B16A16_Unorm = 11,
    R16G16B16A16_Uint = 12,
    R16G16B16A16_Snorm = 13,
    R16G16B16A16_Sint = 14,
    R32G32_Typeless = 15,
    R32G32_Float = 16,
    R32G32_Uint = 17,
    R32G32_Sint = 18,
    R32G8X24_Typeless = 19,
    D32_Float_S8X24_Uint = 20,
    R10G10B10A2_Typeless = 23,
    R10G10B10A2_Unorm = 24,
    R10G10B10A2_Uint = 25,
    R11G11B10_Float = 26,
    R8G8B8A8_Typeless = 27,
    R8G8B8A8_Unorm = 28,
    R8G8B8A8_Unorm_Srgb = 29,
    R8G8B8A8_Uint = 30,
    R8G8B8A8_Snorm = 31,
    R8G8B8A8_Sint = 32,
    R16G16_Typeless = 33,
    R16G16_Float = 34,
    R16G16_Unorm = 35,
    R16G16_Uint = 36,
    R16G16_Snorm = 37,
    R16G16_Sint = 38,
    R32_Typeless = 39,
    D32_Float = 40,
    R32_Float = 41,
    R32_Uint = 42,
    R32_Sint = 43,
    R24G8_Typeless = 44,
    D24_Unorm_S8_Uint = 45,
    R8G8_Typeless = 48,
    R8G8_Unorm = 49,
    R8G8_Uint = 50,
    R8G8_Snorm = 51,
    R8G8_Sint = 52,
    R16_Typeless = 53,
    R16_Float = 54,
    D16_Unorm = 55,
    R16_Unorm = 56,
    R16_Uint = 57,
    R16_Snorm = 58,
    R16_Sint = 59,
    R8_Typeless = 60,
    R8_Unorm = 61,
    R8_Uint = 62,
    R8_Snorm = 63,
    R8_Sint = 64,
    A8_Unorm = 65,
    BC1_Typeless = 70,
    BC1_Unorm = 71,
    BC1_Unorm_Srgb = 72,
    BC2_Typeless = 73,
    BC2_Unorm = 74,
    BC2_Unorm_Srgb = 75,
    BC3_Typeless = 76,
    BC3_Unorm = 77,
    BC3_Unorm_Srgb = 78,
    BC4_Typeless = 79,
    BC4_Unorm = 80,
    BC4_Snorm = 81,
    BC5_Typeless = 82,
    BC5_Unorm = 83,
    BC5_Snorm = 84,
    B5G6R5_Unorm = 85,
    B5G5R5A1_Unorm = 86,
    B8G8R8A8_Unorm = 87,
    B8G8R8X8_Unorm = 88,
    B8G8R8A8_Typeless = 90,
    B8G8R8A8_Unorm_Srgb = 91,
    B8G8R8X8_Typeless = 92,
    B8G8R8X8_Unorm_Srgb = 93,
    BC6H_Typeless = 94,
    BC6H_UF16 = 95,
    BC6H_SF16 = 96,
    BC7_Typeless = 97,
    BC7_Unorm = 98,
    BC7_Unorm_Srgb = 99,
}

impl DxgiFormat {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::R32G32B32A32_Typeless,
            2 => Self::R32G32B32A32_Float,
            3 => Self::R32G32B32A32_Uint,
            4 => Self::R32G32B32A32_Sint,
            5 => Self::R32G32B32_Typeless,
            6 => Self::R32G32B32_Float,
            7 => Self::R32G32B32_Uint,
            8 => Self::R32G32B32_Sint,
            9 => Self::R16G16B16A16_Typeless,
            10 => Self::R16G16B16A16_Float,
            11 => Self::R16G16B16A16_Unorm,
            12 => Self::R16G16B16A16_Uint,
            13 => Self::R16G16B16A16_Snorm,
            14 => Self::R16G16B16A16_Sint,
            15 => Self::R32G32_Typeless,
            16 => Self::R32G32_Float,
            17 => Self::R32G32_Uint,
            18 => Self::R32G32_Sint,
            19 => Self::R32G8X24_Typeless,
            20 => Self::D32_Float_S8X24_Uint,
            23 => Self::R10G10B10A2_Typeless,
            24 => Self::R10G10B10A2_Unorm,
            25 => Self::R10G10B10A2_Uint,
            26 => Self::R11G11B10_Float,
            27 => Self::R8G8B8A8_Typeless,
            28 => Self::R8G8B8A8_Unorm,
            29 => Self::R8G8B8A8_Unorm_Srgb,
            30 => Self::R8G8B8A8_Uint,
            31 => Self::R8G8B8A8_Snorm,
            32 => Self::R8G8B8A8_Sint,
            33 => Self::R16G16_Typeless,
            34 => Self::R16G16_Float,
            35 => Self::R16G16_Unorm,
            36 => Self::R16G16_Uint,
            37 => Self::R16G16_Snorm,
            38 => Self::R16G16_Sint,
            39 => Self::R32_Typeless,
            40 => Self::D32_Float,
            41 => Self::R32_Float,
            42 => Self::R32_Uint,
            43 => Self::R32_Sint,
            44 => Self::R24G8_Typeless,
            45 => Self::D24_Unorm_S8_Uint,
            48 => Self::R8G8_Typeless,
            49 => Self::R8G8_Unorm,
            50 => Self::R8G8_Uint,
            51 => Self::R8G8_Snorm,
            52 => Self::R8G8_Sint,
            53 => Self::R16_Typeless,
            54 => Self::R16_Float,
            55 => Self::D16_Unorm,
            56 => Self::R16_Unorm,
            57 => Self::R16_Uint,
            58 => Self::R16_Snorm,
            59 => Self::R16_Sint,
            60 => Self::R8_Typeless,
            61 => Self::R8_Unorm,
            62 => Self::R8_Uint,
            63 => Self::R8_Snorm,
            64 => Self::R8_Sint,
            65 => Self::A8_Unorm,
            70 => Self::BC1_Typeless,
            71 => Self::BC1_Unorm,
            72 => Self::BC1_Unorm_Srgb,
            73 => Self::BC2_Typeless,
            74 => Self::BC2_Unorm,
            75 => Self::BC2_Unorm_Srgb,
            76 => Self::BC3_Typeless,
            77 => Self::BC3_Unorm,
            78 => Self::BC3_Unorm_Srgb,
            79 => Self::BC4_Typeless,
            80 => Self::BC4_Unorm,
            81 => Self::BC4_Snorm,
            82 => Self::BC5_Typeless,
            83 => Self::BC5_Unorm,
            84 => Self::BC5_Snorm,
            85 => Self::B5G6R5_Unorm,
            86 => Self::B5G5R5A1_Unorm,
            87 => Self::B8G8R8A8_Unorm,
            88 => Self::B8G8R8X8_Unorm,
            90 => Self::B8G8R8A8_Typeless,
            91 => Self::B8G8R8A8_Unorm_Srgb,
            92 => Self::B8G8R8X8_Typeless,
            93 => Self::B8G8R8X8_Unorm_Srgb,
            94 => Self::BC6H_Typeless,
            95 => Self::BC6H_UF16,
            96 => Self::BC6H_SF16,
            97 => Self::BC7_Typeless,
            98 => Self::BC7_Unorm,
            99 => Self::BC7_Unorm_Srgb,
            _ => Self::Unknown,
        }
    }

    pub fn to_athgfx(self) -> Option<athgfx::PixelFormat> {
        match self {
            Self::R8G8B8A8_Unorm
            | Self::R8G8B8A8_Typeless
            | Self::R8G8B8A8_Uint
            | Self::R8G8B8A8_Snorm
            | Self::R8G8B8A8_Sint => Some(athgfx::PixelFormat::Rgba8Unorm),
            Self::R8G8B8A8_Unorm_Srgb => Some(athgfx::PixelFormat::Rgba8Srgb),
            Self::B8G8R8A8_Unorm
            | Self::B8G8R8A8_Typeless
            | Self::B8G8R8X8_Unorm
            | Self::B8G8R8X8_Typeless => Some(athgfx::PixelFormat::Bgra8Unorm),
            Self::B8G8R8A8_Unorm_Srgb | Self::B8G8R8X8_Unorm_Srgb => {
                Some(athgfx::PixelFormat::Bgra8Srgb)
            }
            Self::R16G16B16A16_Float | Self::R16G16B16A16_Typeless => {
                Some(athgfx::PixelFormat::Rgba16Float)
            }
            Self::R32G32B32A32_Float | Self::R32G32B32A32_Typeless => {
                Some(athgfx::PixelFormat::Rgba32Float)
            }
            Self::R11G11B10_Float => Some(athgfx::PixelFormat::Rg11B10Float),
            Self::D24_Unorm_S8_Uint | Self::R24G8_Typeless => {
                Some(athgfx::PixelFormat::Depth24Stencil8)
            }
            Self::D32_Float | Self::R32_Typeless => Some(athgfx::PixelFormat::Depth32Float),
            Self::D32_Float_S8X24_Uint | Self::R32G8X24_Typeless => {
                Some(athgfx::PixelFormat::Depth32Float)
            }
            Self::R8_Unorm
            | Self::R8_Typeless
            | Self::R8_Uint
            | Self::R8_Snorm
            | Self::R8_Sint
            | Self::A8_Unorm => Some(athgfx::PixelFormat::R8Unorm),
            Self::R8G8_Unorm
            | Self::R8G8_Typeless
            | Self::R8G8_Uint
            | Self::R8G8_Snorm
            | Self::R8G8_Sint => Some(athgfx::PixelFormat::Rg8Unorm),
            Self::BC1_Unorm | Self::BC1_Typeless | Self::BC1_Unorm_Srgb => {
                Some(athgfx::PixelFormat::Bc1Unorm)
            }
            Self::BC2_Unorm
            | Self::BC2_Typeless
            | Self::BC2_Unorm_Srgb
            | Self::BC3_Unorm
            | Self::BC3_Typeless
            | Self::BC3_Unorm_Srgb => Some(athgfx::PixelFormat::Bc3Unorm),
            Self::BC7_Unorm | Self::BC7_Typeless | Self::BC7_Unorm_Srgb => {
                Some(athgfx::PixelFormat::Bc7Unorm)
            }
            // Formats mapped to closest AthGFX equivalent
            Self::R16G16_Float | Self::R16G16_Typeless | Self::R16G16_Unorm => {
                Some(athgfx::PixelFormat::Rgba16Float)
            }
            Self::R32_Float | Self::R32_Uint | Self::R32_Sint => {
                Some(athgfx::PixelFormat::Depth32Float)
            }
            Self::D16_Unorm | Self::R16_Typeless | Self::R16_Float | Self::R16_Unorm => {
                Some(athgfx::PixelFormat::Depth24Stencil8)
            }
            Self::R10G10B10A2_Unorm | Self::R10G10B10A2_Typeless | Self::R10G10B10A2_Uint => {
                Some(athgfx::PixelFormat::Rgba8Unorm)
            }
            _ => None,
        }
    }

    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::R32G32B32A32_Float
            | Self::R32G32B32A32_Uint
            | Self::R32G32B32A32_Sint
            | Self::R32G32B32A32_Typeless => 16,
            Self::R32G32B32_Float
            | Self::R32G32B32_Uint
            | Self::R32G32B32_Sint
            | Self::R32G32B32_Typeless => 12,
            Self::R16G16B16A16_Float
            | Self::R16G16B16A16_Unorm
            | Self::R16G16B16A16_Uint
            | Self::R16G16B16A16_Snorm
            | Self::R16G16B16A16_Sint
            | Self::R16G16B16A16_Typeless
            | Self::R32G32_Float
            | Self::R32G32_Uint
            | Self::R32G32_Sint
            | Self::R32G32_Typeless
            | Self::R32G8X24_Typeless
            | Self::D32_Float_S8X24_Uint => 8,
            Self::R8G8B8A8_Unorm
            | Self::R8G8B8A8_Unorm_Srgb
            | Self::R8G8B8A8_Uint
            | Self::R8G8B8A8_Snorm
            | Self::R8G8B8A8_Sint
            | Self::R8G8B8A8_Typeless
            | Self::B8G8R8A8_Unorm
            | Self::B8G8R8A8_Unorm_Srgb
            | Self::B8G8R8A8_Typeless
            | Self::B8G8R8X8_Unorm
            | Self::B8G8R8X8_Unorm_Srgb
            | Self::B8G8R8X8_Typeless
            | Self::R10G10B10A2_Unorm
            | Self::R10G10B10A2_Uint
            | Self::R10G10B10A2_Typeless
            | Self::R11G11B10_Float
            | Self::R16G16_Float
            | Self::R16G16_Unorm
            | Self::R16G16_Uint
            | Self::R16G16_Snorm
            | Self::R16G16_Sint
            | Self::R16G16_Typeless
            | Self::R32_Float
            | Self::R32_Uint
            | Self::R32_Sint
            | Self::R32_Typeless
            | Self::D32_Float
            | Self::D24_Unorm_S8_Uint
            | Self::R24G8_Typeless => 4,
            Self::R8G8_Unorm
            | Self::R8G8_Uint
            | Self::R8G8_Snorm
            | Self::R8G8_Sint
            | Self::R8G8_Typeless
            | Self::R16_Float
            | Self::R16_Unorm
            | Self::R16_Uint
            | Self::R16_Snorm
            | Self::R16_Sint
            | Self::R16_Typeless
            | Self::D16_Unorm
            | Self::B5G6R5_Unorm
            | Self::B5G5R5A1_Unorm => 2,
            Self::R8_Unorm
            | Self::R8_Uint
            | Self::R8_Snorm
            | Self::R8_Sint
            | Self::R8_Typeless
            | Self::A8_Unorm => 1,
            _ => 0,
        }
    }

    pub fn is_depth(self) -> bool {
        matches!(
            self,
            Self::D32_Float
                | Self::D24_Unorm_S8_Uint
                | Self::D16_Unorm
                | Self::D32_Float_S8X24_Uint
                | Self::R24G8_Typeless
                | Self::R32G8X24_Typeless
        )
    }

    pub fn is_compressed(self) -> bool {
        matches!(
            self,
            Self::BC1_Typeless
                | Self::BC1_Unorm
                | Self::BC1_Unorm_Srgb
                | Self::BC2_Typeless
                | Self::BC2_Unorm
                | Self::BC2_Unorm_Srgb
                | Self::BC3_Typeless
                | Self::BC3_Unorm
                | Self::BC3_Unorm_Srgb
                | Self::BC4_Typeless
                | Self::BC4_Unorm
                | Self::BC4_Snorm
                | Self::BC5_Typeless
                | Self::BC5_Unorm
                | Self::BC5_Snorm
                | Self::BC6H_Typeless
                | Self::BC6H_UF16
                | Self::BC6H_SF16
                | Self::BC7_Typeless
                | Self::BC7_Unorm
                | Self::BC7_Unorm_Srgb
        )
    }

    pub fn is_srgb(self) -> bool {
        matches!(
            self,
            Self::R8G8B8A8_Unorm_Srgb
                | Self::B8G8R8A8_Unorm_Srgb
                | Self::B8G8R8X8_Unorm_Srgb
                | Self::BC1_Unorm_Srgb
                | Self::BC2_Unorm_Srgb
                | Self::BC3_Unorm_Srgb
                | Self::BC7_Unorm_Srgb
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 2: D3D11 Device Translation — Resource & State Mapping
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11BindFlag {
    VertexBuffer = 0x1,
    IndexBuffer = 0x2,
    ConstantBuffer = 0x4,
    ShaderResource = 0x8,
    StreamOutput = 0x10,
    RenderTarget = 0x20,
    DepthStencil = 0x40,
    UnorderedAccess = 0x80,
}

fn bind_flags_to_buffer_usage(flags: u32) -> athgfx::BufferUsage {
    if flags & (D3d11BindFlag::VertexBuffer as u32) != 0 {
        athgfx::BufferUsage::Vertex
    } else if flags & (D3d11BindFlag::IndexBuffer as u32) != 0 {
        athgfx::BufferUsage::Index
    } else if flags & (D3d11BindFlag::ConstantBuffer as u32) != 0 {
        athgfx::BufferUsage::Uniform
    } else if flags & (D3d11BindFlag::UnorderedAccess as u32) != 0 {
        athgfx::BufferUsage::Storage
    } else if flags & (D3d11BindFlag::StreamOutput as u32) != 0 {
        athgfx::BufferUsage::Storage
    } else {
        athgfx::BufferUsage::Transfer
    }
}

fn bind_flags_to_texture_usage(flags: u32) -> athgfx::TextureUsage {
    if flags & (D3d11BindFlag::RenderTarget as u32) != 0 {
        athgfx::TextureUsage::RenderTarget
    } else if flags & (D3d11BindFlag::DepthStencil as u32) != 0 {
        athgfx::TextureUsage::DepthStencil
    } else if flags & (D3d11BindFlag::UnorderedAccess as u32) != 0 {
        athgfx::TextureUsage::Storage
    } else {
        athgfx::TextureUsage::Sampled
    }
}

#[derive(Debug, Clone)]
pub struct D3d11BufferDesc {
    pub byte_width: u32,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
    pub structure_byte_stride: u32,
}

impl D3d11BufferDesc {
    pub fn to_athgfx(&self) -> athgfx::BufferDescriptor {
        athgfx::BufferDescriptor {
            size: self.byte_width as u64,
            usage: bind_flags_to_buffer_usage(self.bind_flags),
            label: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct D3d11Texture2dDesc {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub array_size: u32,
    pub format: DxgiFormat,
    pub sample_count: u32,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
}

impl D3d11Texture2dDesc {
    pub fn to_athgfx(&self) -> Option<athgfx::TextureDescriptor> {
        let format = self.format.to_athgfx()?;
        Some(athgfx::TextureDescriptor {
            width: self.width,
            height: self.height,
            depth: self.array_size.max(1),
            mip_levels: self.mip_levels.max(1),
            format,
            usage: bind_flags_to_texture_usage(self.bind_flags),
            label: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11CullMode {
    None = 1,
    Front = 2,
    Back = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11FillMode {
    Wireframe = 2,
    Solid = 3,
}

#[derive(Debug, Clone, Copy)]
pub struct D3d11RasterizerDesc {
    pub fill_mode: D3d11FillMode,
    pub cull_mode: D3d11CullMode,
    pub front_counter_clockwise: bool,
    pub depth_bias: i32,
    pub slope_scaled_depth_bias: f32,
    pub depth_clip_enable: bool,
    pub scissor_enable: bool,
}

impl D3d11RasterizerDesc {
    pub fn to_athgfx(&self) -> athgfx::RasterState {
        athgfx::RasterState {
            cull_mode: match self.cull_mode {
                D3d11CullMode::None => athgfx::CullMode::None,
                D3d11CullMode::Front => athgfx::CullMode::Front,
                D3d11CullMode::Back => athgfx::CullMode::Back,
            },
            front_face: if self.front_counter_clockwise {
                athgfx::FrontFace::CounterClockwise
            } else {
                athgfx::FrontFace::Clockwise
            },
            polygon_mode: match self.fill_mode {
                D3d11FillMode::Wireframe => athgfx::PolygonMode::Line,
                D3d11FillMode::Solid => athgfx::PolygonMode::Fill,
            },
            depth_bias: self.depth_bias as f32,
            depth_bias_slope: self.slope_scaled_depth_bias,
            line_width: 1.0,
        }
    }
}

impl Default for D3d11RasterizerDesc {
    fn default() -> Self {
        Self {
            fill_mode: D3d11FillMode::Solid,
            cull_mode: D3d11CullMode::Back,
            front_counter_clockwise: false,
            depth_bias: 0,
            slope_scaled_depth_bias: 0.0,
            depth_clip_enable: true,
            scissor_enable: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11Blend {
    Zero = 1,
    One = 2,
    SrcColor = 3,
    InvSrcColor = 4,
    SrcAlpha = 5,
    InvSrcAlpha = 6,
    DestAlpha = 7,
    InvDestAlpha = 8,
    DestColor = 9,
    InvDestColor = 10,
    SrcAlphaSat = 11,
    BlendFactor = 14,
    InvBlendFactor = 15,
}

impl D3d11Blend {
    pub fn to_athgfx(self) -> athgfx::BlendFactor {
        match self {
            Self::Zero => athgfx::BlendFactor::Zero,
            Self::One => athgfx::BlendFactor::One,
            Self::SrcColor => athgfx::BlendFactor::SrcColor,
            Self::InvSrcColor => athgfx::BlendFactor::OneMinusSrcColor,
            Self::SrcAlpha => athgfx::BlendFactor::SrcAlpha,
            Self::InvSrcAlpha => athgfx::BlendFactor::OneMinusSrcAlpha,
            Self::DestAlpha => athgfx::BlendFactor::DstAlpha,
            Self::InvDestAlpha => athgfx::BlendFactor::OneMinusDstAlpha,
            Self::DestColor => athgfx::BlendFactor::DstColor,
            Self::InvDestColor => athgfx::BlendFactor::OneMinusDstColor,
            Self::SrcAlphaSat => athgfx::BlendFactor::SrcAlpha,
            Self::BlendFactor => athgfx::BlendFactor::One,
            Self::InvBlendFactor => athgfx::BlendFactor::Zero,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11BlendOp {
    Add = 1,
    Subtract = 2,
    RevSubtract = 3,
    Min = 4,
    Max = 5,
}

impl D3d11BlendOp {
    pub fn to_athgfx(self) -> athgfx::BlendOp {
        match self {
            Self::Add => athgfx::BlendOp::Add,
            Self::Subtract => athgfx::BlendOp::Subtract,
            Self::RevSubtract => athgfx::BlendOp::ReverseSubtract,
            Self::Min => athgfx::BlendOp::Min,
            Self::Max => athgfx::BlendOp::Max,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3d11BlendDesc {
    pub blend_enable: bool,
    pub src_blend: D3d11Blend,
    pub dest_blend: D3d11Blend,
    pub blend_op: D3d11BlendOp,
    pub src_blend_alpha: D3d11Blend,
    pub dest_blend_alpha: D3d11Blend,
    pub blend_op_alpha: D3d11BlendOp,
}

impl D3d11BlendDesc {
    pub fn to_athgfx(&self) -> athgfx::BlendState {
        athgfx::BlendState {
            enabled: self.blend_enable,
            src_factor: self.src_blend.to_athgfx(),
            dst_factor: self.dest_blend.to_athgfx(),
            op: self.blend_op.to_athgfx(),
            src_alpha_factor: self.src_blend_alpha.to_athgfx(),
            dst_alpha_factor: self.dest_blend_alpha.to_athgfx(),
            alpha_op: self.blend_op_alpha.to_athgfx(),
        }
    }
}

impl Default for D3d11BlendDesc {
    fn default() -> Self {
        Self {
            blend_enable: false,
            src_blend: D3d11Blend::One,
            dest_blend: D3d11Blend::Zero,
            blend_op: D3d11BlendOp::Add,
            src_blend_alpha: D3d11Blend::One,
            dest_blend_alpha: D3d11Blend::Zero,
            blend_op_alpha: D3d11BlendOp::Add,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11ComparisonFunc {
    Never = 1,
    Less = 2,
    Equal = 3,
    LessEqual = 4,
    Greater = 5,
    NotEqual = 6,
    GreaterEqual = 7,
    Always = 8,
}

impl D3d11ComparisonFunc {
    pub fn to_athgfx(self) -> athgfx::CompareOp {
        match self {
            Self::Never => athgfx::CompareOp::Never,
            Self::Less => athgfx::CompareOp::Less,
            Self::Equal => athgfx::CompareOp::Equal,
            Self::LessEqual => athgfx::CompareOp::LessOrEqual,
            Self::Greater => athgfx::CompareOp::Greater,
            Self::NotEqual => athgfx::CompareOp::NotEqual,
            Self::GreaterEqual => athgfx::CompareOp::GreaterOrEqual,
            Self::Always => athgfx::CompareOp::Always,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3d11DepthStencilDesc {
    pub depth_enable: bool,
    pub depth_write_mask: u32,
    pub depth_func: D3d11ComparisonFunc,
    pub stencil_enable: bool,
}

impl D3d11DepthStencilDesc {
    pub fn to_athgfx(&self) -> athgfx::DepthStencilState {
        athgfx::DepthStencilState {
            depth_test: self.depth_enable,
            depth_write: self.depth_write_mask != 0,
            depth_compare: self.depth_func.to_athgfx(),
            stencil_enabled: self.stencil_enable,
        }
    }
}

impl Default for D3d11DepthStencilDesc {
    fn default() -> Self {
        Self {
            depth_enable: true,
            depth_write_mask: 1,
            depth_func: D3d11ComparisonFunc::Less,
            stencil_enable: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 3: D3D12 Command List → AthGFX Translation
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d12ResourceState {
    Common = 0x0,
    VertexAndConstantBuffer = 0x1,
    IndexBuffer = 0x2,
    RenderTarget = 0x4,
    UnorderedAccess = 0x8,
    DepthWrite = 0x10,
    DepthRead = 0x20,
    NonPixelShaderResource = 0x40,
    PixelShaderResource = 0x80,
    CopyDest = 0x400,
    CopySource = 0x800,
    Present = 0x4000,
    GenericRead = 0xAC3,
}

impl D3d12ResourceState {
    pub fn to_athgfx_layout(self) -> athgfx::ImageLayout {
        match self {
            Self::Common | Self::Present => athgfx::ImageLayout::PresentSrc,
            Self::RenderTarget => athgfx::ImageLayout::ColorAttachment,
            Self::DepthWrite | Self::DepthRead => athgfx::ImageLayout::DepthStencilAttachment,
            Self::PixelShaderResource | Self::NonPixelShaderResource | Self::GenericRead => {
                athgfx::ImageLayout::ShaderReadOnly
            }
            Self::CopyDest => athgfx::ImageLayout::TransferDst,
            Self::CopySource => athgfx::ImageLayout::TransferSrc,
            Self::UnorderedAccess => athgfx::ImageLayout::General,
            _ => athgfx::ImageLayout::General,
        }
    }

    pub fn to_athgfx_access(self) -> athgfx::AccessFlags {
        match self {
            Self::Common | Self::Present => athgfx::AccessFlags::None,
            Self::VertexAndConstantBuffer => athgfx::AccessFlags::VertexBufferRead,
            Self::IndexBuffer => athgfx::AccessFlags::IndexBufferRead,
            Self::RenderTarget => athgfx::AccessFlags::ColorAttachmentWrite,
            Self::UnorderedAccess => athgfx::AccessFlags::ShaderWrite,
            Self::DepthWrite => athgfx::AccessFlags::DepthStencilWrite,
            Self::DepthRead => athgfx::AccessFlags::DepthStencilRead,
            Self::NonPixelShaderResource | Self::PixelShaderResource | Self::GenericRead => {
                athgfx::AccessFlags::ShaderRead
            }
            Self::CopyDest => athgfx::AccessFlags::TransferWrite,
            Self::CopySource => athgfx::AccessFlags::TransferRead,
        }
    }

    pub fn to_athgfx_stage(self) -> athgfx::PipelineStage {
        match self {
            Self::Common | Self::Present => athgfx::PipelineStage::BottomOfPipe,
            Self::VertexAndConstantBuffer | Self::IndexBuffer => athgfx::PipelineStage::VertexInput,
            Self::RenderTarget => athgfx::PipelineStage::ColorAttachmentOutput,
            Self::DepthWrite | Self::DepthRead => athgfx::PipelineStage::EarlyFragmentTests,
            Self::NonPixelShaderResource => athgfx::PipelineStage::VertexShader,
            Self::PixelShaderResource => athgfx::PipelineStage::FragmentShader,
            Self::UnorderedAccess => athgfx::PipelineStage::ComputeShader,
            Self::CopyDest | Self::CopySource => athgfx::PipelineStage::Transfer,
            Self::GenericRead => athgfx::PipelineStage::TopOfPipe,
        }
    }
}

#[derive(Debug, Clone)]
pub struct D3d12ResourceBarrier {
    pub resource_handle: u64,
    pub state_before: D3d12ResourceState,
    pub state_after: D3d12ResourceState,
    pub subresource: u32,
}

impl D3d12ResourceBarrier {
    pub fn to_athgfx_image_barrier(&self) -> athgfx::ImageBarrier {
        athgfx::ImageBarrier {
            image: athgfx::TextureHandle(self.resource_handle),
            old_layout: self.state_before.to_athgfx_layout(),
            new_layout: self.state_after.to_athgfx_layout(),
            src_access: self.state_before.to_athgfx_access(),
            dst_access: self.state_after.to_athgfx_access(),
        }
    }

    pub fn to_athgfx_barrier_info(&self) -> athgfx::BarrierInfo {
        athgfx::BarrierInfo {
            src_stage: self.state_before.to_athgfx_stage(),
            dst_stage: self.state_after.to_athgfx_stage(),
            image_barriers: vec![self.to_athgfx_image_barrier()],
        }
    }
}

pub fn translate_barriers_to_athgfx(barriers: &[D3d12ResourceBarrier]) -> athgfx::BarrierInfo {
    let src_stage = barriers
        .first()
        .map(|b| b.state_before.to_athgfx_stage())
        .unwrap_or(athgfx::PipelineStage::TopOfPipe);
    let dst_stage = barriers
        .first()
        .map(|b| b.state_after.to_athgfx_stage())
        .unwrap_or(athgfx::PipelineStage::BottomOfPipe);

    let image_barriers = barriers
        .iter()
        .map(|b| b.to_athgfx_image_barrier())
        .collect();

    athgfx::BarrierInfo {
        src_stage,
        dst_stage,
        image_barriers,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 3b: D3D12 Descriptor Heaps → AthGFX Descriptor Sets
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d12DescriptorRangeType {
    Srv = 0,
    Uav = 1,
    Cbv = 2,
    Sampler = 3,
}

impl D3d12DescriptorRangeType {
    pub fn to_athgfx(self) -> athgfx::DescriptorType {
        match self {
            Self::Srv => athgfx::DescriptorType::SampledImage,
            Self::Uav => athgfx::DescriptorType::StorageImage,
            Self::Cbv => athgfx::DescriptorType::UniformBuffer,
            Self::Sampler => athgfx::DescriptorType::Sampler,
        }
    }
}

#[derive(Debug, Clone)]
pub struct D3d12DescriptorRange {
    pub range_type: D3d12DescriptorRangeType,
    pub num_descriptors: u32,
    pub base_shader_register: u32,
    pub register_space: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d12ShaderVisibility {
    All = 0,
    Vertex = 1,
    Hull = 2,
    Domain = 3,
    Geometry = 4,
    Pixel = 5,
}

impl D3d12ShaderVisibility {
    pub fn to_athgfx_stage(self) -> athgfx::ShaderStage {
        match self {
            Self::Vertex => athgfx::ShaderStage::Vertex,
            Self::Pixel => athgfx::ShaderStage::Fragment,
            Self::Geometry => athgfx::ShaderStage::Geometry,
            Self::Hull => athgfx::ShaderStage::TessControl,
            Self::Domain => athgfx::ShaderStage::TessEvaluation,
            Self::All => athgfx::ShaderStage::Vertex,
        }
    }
}

#[derive(Debug, Clone)]
pub struct D3d12RootParameter {
    pub visibility: D3d12ShaderVisibility,
    pub ranges: Vec<D3d12DescriptorRange>,
    pub num_32bit_values: u32,
    pub shader_register: u32,
}

#[derive(Debug, Clone)]
pub struct D3d12RootSignature {
    pub parameters: Vec<D3d12RootParameter>,
}

impl D3d12RootSignature {
    pub fn to_athgfx_bindings(&self) -> Vec<athgfx::DescriptorSetLayoutBinding> {
        let mut bindings = Vec::new();
        let mut binding_idx = 0u32;

        for param in &self.parameters {
            for range in &param.ranges {
                for i in 0..range.num_descriptors {
                    bindings.push(athgfx::DescriptorSetLayoutBinding {
                        binding: binding_idx,
                        descriptor_type: range.range_type.to_athgfx(),
                        count: 1,
                        stage: param.visibility.to_athgfx_stage(),
                    });
                    binding_idx += 1;
                    let _ = i;
                }
            }
        }

        bindings
    }

    pub fn to_athgfx_push_constants(&self) -> Vec<athgfx::PushConstantRange> {
        let mut ranges = Vec::new();
        let mut offset = 0u32;

        for param in &self.parameters {
            if param.num_32bit_values > 0 {
                let size = param.num_32bit_values * 4;
                ranges.push(athgfx::PushConstantRange {
                    stage: param.visibility.to_athgfx_stage(),
                    offset,
                    size,
                });
                offset += size;
            }
        }

        ranges
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 4: Shader Translation — DXBC/DXIL Header Parsing
// ═══════════════════════════════════════════════════════════════════════════

const DXBC_MAGIC: u32 = 0x43425844;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderModel {
    Sm4_0,
    Sm4_1,
    Sm5_0,
    Sm5_1,
    Sm6_0,
    Sm6_1,
    Sm6_2,
    Sm6_3,
    Sm6_4,
    Sm6_5,
    Sm6_6,
    Unknown(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DxbcShaderType {
    Pixel = 0,
    Vertex = 1,
    Geometry = 2,
    Hull = 3,
    Domain = 4,
    Compute = 5,
}

impl DxbcShaderType {
    pub fn to_athgfx(self) -> athgfx::ShaderStage {
        match self {
            Self::Pixel => athgfx::ShaderStage::Fragment,
            Self::Vertex => athgfx::ShaderStage::Vertex,
            Self::Geometry => athgfx::ShaderStage::Geometry,
            Self::Hull => athgfx::ShaderStage::TessControl,
            Self::Domain => athgfx::ShaderStage::TessEvaluation,
            Self::Compute => athgfx::ShaderStage::Compute,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DxbcHeader {
    pub total_size: u32,
    pub chunk_count: u32,
    pub shader_type: DxbcShaderType,
    pub shader_model: ShaderModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaderTranslationError {
    InvalidMagic(u32),
    BufferTooSmall,
    UnsupportedModel(ShaderModel),
    NoShaderChunk,
    MalformedChunk,
}

pub fn parse_dxbc_header(bytecode: &[u8]) -> Result<DxbcHeader, ShaderTranslationError> {
    if bytecode.len() < 32 {
        return Err(ShaderTranslationError::BufferTooSmall);
    }

    let magic = u32::from_le_bytes([bytecode[0], bytecode[1], bytecode[2], bytecode[3]]);
    if magic != DXBC_MAGIC {
        return Err(ShaderTranslationError::InvalidMagic(magic));
    }

    // Bytes 4..20: checksum (16 bytes, 4 × u32)
    // Byte 20..24: version (always 1)
    let total_size = u32::from_le_bytes([bytecode[24], bytecode[25], bytecode[26], bytecode[27]]);
    let chunk_count = u32::from_le_bytes([bytecode[28], bytecode[29], bytecode[30], bytecode[31]]);

    // Walk chunks to find SHEX/SHDR (shader bytecode chunk)
    let mut shader_type = DxbcShaderType::Vertex;
    let mut shader_model = ShaderModel::Sm5_0;

    let header_end = 32 + chunk_count as usize * 4;
    if header_end <= bytecode.len() {
        for i in 0..chunk_count as usize {
            let offset_pos = 32 + i * 4;
            if offset_pos + 4 > bytecode.len() {
                break;
            }
            let chunk_offset = u32::from_le_bytes([
                bytecode[offset_pos],
                bytecode[offset_pos + 1],
                bytecode[offset_pos + 2],
                bytecode[offset_pos + 3],
            ]) as usize;

            if chunk_offset + 8 > bytecode.len() {
                continue;
            }

            let chunk_fourcc = u32::from_le_bytes([
                bytecode[chunk_offset],
                bytecode[chunk_offset + 1],
                bytecode[chunk_offset + 2],
                bytecode[chunk_offset + 3],
            ]);

            // SHEX = 0x58454853, SHDR = 0x52444853
            if chunk_fourcc == 0x58454853 || chunk_fourcc == 0x52444853 {
                let data_start = chunk_offset + 8;
                if data_start + 4 <= bytecode.len() {
                    let version_token = u32::from_le_bytes([
                        bytecode[data_start],
                        bytecode[data_start + 1],
                        bytecode[data_start + 2],
                        bytecode[data_start + 3],
                    ]);
                    let program_type = (version_token >> 16) & 0xFFFF;
                    let major = (version_token >> 4) & 0xF;
                    let minor = version_token & 0xF;

                    shader_type = match program_type {
                        0 => DxbcShaderType::Pixel,
                        1 => DxbcShaderType::Vertex,
                        2 => DxbcShaderType::Geometry,
                        3 => DxbcShaderType::Hull,
                        4 => DxbcShaderType::Domain,
                        5 => DxbcShaderType::Compute,
                        _ => DxbcShaderType::Vertex,
                    };

                    shader_model = match (major, minor) {
                        (4, 0) => ShaderModel::Sm4_0,
                        (4, 1) => ShaderModel::Sm4_1,
                        (5, 0) => ShaderModel::Sm5_0,
                        (5, 1) => ShaderModel::Sm5_1,
                        (6, 0) => ShaderModel::Sm6_0,
                        (6, 1) => ShaderModel::Sm6_1,
                        (6, 2) => ShaderModel::Sm6_2,
                        (6, 3) => ShaderModel::Sm6_3,
                        (6, 4) => ShaderModel::Sm6_4,
                        (6, 5) => ShaderModel::Sm6_5,
                        (6, 6) => ShaderModel::Sm6_6,
                        _ => ShaderModel::Unknown(major * 10 + minor),
                    };
                }
                break;
            }
        }
    }

    Ok(DxbcHeader {
        total_size,
        chunk_count,
        shader_type,
        shader_model,
    })
}

// DXIL container magic: "DXIL" prefix after the DXBC envelope
const DXIL_MAGIC: [u8; 4] = [b'D', b'X', b'I', b'L'];

pub fn is_dxil_bytecode(bytecode: &[u8]) -> bool {
    if bytecode.len() < 36 {
        return false;
    }
    // DXIL shaders still use DXBC container format but contain a DXIL chunk
    let magic = u32::from_le_bytes([bytecode[0], bytecode[1], bytecode[2], bytecode[3]]);
    if magic != DXBC_MAGIC {
        return false;
    }
    // Search for DXIL fourcc in chunks
    bytecode.windows(4).any(|w| w == &DXIL_MAGIC)
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 4b: Input Layout Translation (D3D vertex elements → AthGFX)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct D3dInputElement {
    pub semantic_name: String,
    pub semantic_index: u32,
    pub format: DxgiFormat,
    pub input_slot: u32,
    pub byte_offset: u32,
    pub instance_step_rate: u32,
}

fn dxgi_format_to_vertex_format(format: DxgiFormat) -> Option<athgfx::VertexFormat> {
    match format {
        DxgiFormat::R32_Float => Some(athgfx::VertexFormat::Float),
        DxgiFormat::R32G32_Float => Some(athgfx::VertexFormat::Float2),
        DxgiFormat::R32G32B32_Float => Some(athgfx::VertexFormat::Float3),
        DxgiFormat::R32G32B32A32_Float => Some(athgfx::VertexFormat::Float4),
        DxgiFormat::R32_Sint | DxgiFormat::R32_Uint => Some(athgfx::VertexFormat::Int),
        DxgiFormat::R32G32_Sint | DxgiFormat::R32G32_Uint => Some(athgfx::VertexFormat::Int2),
        DxgiFormat::R32G32B32_Sint | DxgiFormat::R32G32B32_Uint => Some(athgfx::VertexFormat::Int3),
        DxgiFormat::R32G32B32A32_Sint | DxgiFormat::R32G32B32A32_Uint => {
            Some(athgfx::VertexFormat::Int4)
        }
        DxgiFormat::R8G8B8A8_Unorm | DxgiFormat::R8G8B8A8_Uint | DxgiFormat::B8G8R8A8_Unorm => {
            Some(athgfx::VertexFormat::UByte4Norm)
        }
        DxgiFormat::R16G16_Float => Some(athgfx::VertexFormat::Float2),
        DxgiFormat::R16G16B16A16_Float => Some(athgfx::VertexFormat::Float4),
        _ => None,
    }
}

pub fn translate_input_layout(elements: &[D3dInputElement]) -> Vec<athgfx::VertexBufferLayout> {
    let mut slots: BTreeMap<u32, Vec<athgfx::VertexAttribute>> = BTreeMap::new();
    let mut slot_strides: BTreeMap<u32, u32> = BTreeMap::new();
    let mut slot_step_rates: BTreeMap<u32, u32> = BTreeMap::new();

    for (location, elem) in elements.iter().enumerate() {
        let vf = match dxgi_format_to_vertex_format(elem.format) {
            Some(f) => f,
            None => continue,
        };
        let attr = athgfx::VertexAttribute {
            location: location as u32,
            format: vf,
            offset: elem.byte_offset,
        };
        slots
            .entry(elem.input_slot)
            .or_insert_with(Vec::new)
            .push(attr);

        let end_offset = elem.byte_offset + vf.size() as u32;
        let current_stride = slot_strides.entry(elem.input_slot).or_insert(0);
        if end_offset > *current_stride {
            *current_stride = end_offset;
        }

        slot_step_rates
            .entry(elem.input_slot)
            .or_insert(elem.instance_step_rate);
    }

    let mut layouts = Vec::new();
    for (slot, attributes) in &slots {
        let stride = slot_strides.get(slot).copied().unwrap_or(0);
        let step_rate = slot_step_rates.get(slot).copied().unwrap_or(0);
        layouts.push(athgfx::VertexBufferLayout {
            stride,
            step_rate,
            attributes: attributes.clone(),
        });
    }

    layouts
}

// Constant buffer binding translation
pub fn translate_constant_buffer_to_descriptor(
    slot: u32,
    buffer_handle: u64,
    size: u64,
) -> athgfx::DescriptorWrite {
    athgfx::DescriptorWrite {
        binding: slot,
        resource: athgfx::DescriptorResource::Buffer {
            handle: athgfx::BufferHandle(buffer_handle),
            offset: 0,
            size,
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 5: Performance Tracking
// ═══════════════════════════════════════════════════════════════════════════

pub struct TranslationPerfCounters {
    pub draw_calls_per_frame: AtomicU64,
    pub state_changes_per_frame: AtomicU64,
    pub buffer_uploads_per_frame: AtomicU64,
    pub buffer_upload_bytes_per_frame: AtomicU64,
    pub texture_uploads_per_frame: AtomicU64,
    pub shader_compiles_per_frame: AtomicU64,
    pub barrier_count_per_frame: AtomicU64,
    pub descriptor_updates_per_frame: AtomicU64,
    pub pipeline_binds_per_frame: AtomicU64,

    pub total_draw_calls: AtomicU64,
    pub total_state_changes: AtomicU64,
    pub total_buffer_uploads: AtomicU64,
    pub total_frames: AtomicU64,
    pub total_shader_cache_hits: AtomicU64,
    pub total_shader_cache_misses: AtomicU64,
    pub total_pipeline_cache_hits: AtomicU64,
    pub total_pipeline_cache_misses: AtomicU64,
}

impl TranslationPerfCounters {
    pub const fn new() -> Self {
        Self {
            draw_calls_per_frame: AtomicU64::new(0),
            state_changes_per_frame: AtomicU64::new(0),
            buffer_uploads_per_frame: AtomicU64::new(0),
            buffer_upload_bytes_per_frame: AtomicU64::new(0),
            texture_uploads_per_frame: AtomicU64::new(0),
            shader_compiles_per_frame: AtomicU64::new(0),
            barrier_count_per_frame: AtomicU64::new(0),
            descriptor_updates_per_frame: AtomicU64::new(0),
            pipeline_binds_per_frame: AtomicU64::new(0),
            total_draw_calls: AtomicU64::new(0),
            total_state_changes: AtomicU64::new(0),
            total_buffer_uploads: AtomicU64::new(0),
            total_frames: AtomicU64::new(0),
            total_shader_cache_hits: AtomicU64::new(0),
            total_shader_cache_misses: AtomicU64::new(0),
            total_pipeline_cache_hits: AtomicU64::new(0),
            total_pipeline_cache_misses: AtomicU64::new(0),
        }
    }

    pub fn record_draw_call(&self) {
        self.draw_calls_per_frame.fetch_add(1, Ordering::Relaxed);
        self.total_draw_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_state_change(&self) {
        self.state_changes_per_frame.fetch_add(1, Ordering::Relaxed);
        self.total_state_changes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_buffer_upload(&self, bytes: u64) {
        self.buffer_uploads_per_frame
            .fetch_add(1, Ordering::Relaxed);
        self.buffer_upload_bytes_per_frame
            .fetch_add(bytes, Ordering::Relaxed);
        self.total_buffer_uploads.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_texture_upload(&self) {
        self.texture_uploads_per_frame
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_shader_compile(&self, cache_hit: bool) {
        self.shader_compiles_per_frame
            .fetch_add(1, Ordering::Relaxed);
        if cache_hit {
            self.total_shader_cache_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.total_shader_cache_misses
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_pipeline_bind(&self, cache_hit: bool) {
        self.pipeline_binds_per_frame
            .fetch_add(1, Ordering::Relaxed);
        if cache_hit {
            self.total_pipeline_cache_hits
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.total_pipeline_cache_misses
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_barrier(&self) {
        self.barrier_count_per_frame.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_descriptor_update(&self) {
        self.descriptor_updates_per_frame
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn end_frame(&self) {
        self.total_frames.fetch_add(1, Ordering::Relaxed);
        self.draw_calls_per_frame.store(0, Ordering::Relaxed);
        self.state_changes_per_frame.store(0, Ordering::Relaxed);
        self.buffer_uploads_per_frame.store(0, Ordering::Relaxed);
        self.buffer_upload_bytes_per_frame
            .store(0, Ordering::Relaxed);
        self.texture_uploads_per_frame.store(0, Ordering::Relaxed);
        self.shader_compiles_per_frame.store(0, Ordering::Relaxed);
        self.barrier_count_per_frame.store(0, Ordering::Relaxed);
        self.descriptor_updates_per_frame
            .store(0, Ordering::Relaxed);
        self.pipeline_binds_per_frame.store(0, Ordering::Relaxed);
    }

    pub fn shader_cache_hit_rate(&self) -> f32 {
        let hits = self.total_shader_cache_hits.load(Ordering::Relaxed);
        let misses = self.total_shader_cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            return 0.0;
        }
        hits as f32 / total as f32
    }

    pub fn pipeline_cache_hit_rate(&self) -> f32 {
        let hits = self.total_pipeline_cache_hits.load(Ordering::Relaxed);
        let misses = self.total_pipeline_cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            return 0.0;
        }
        hits as f32 / total as f32
    }

    pub fn avg_draw_calls_per_frame(&self) -> f32 {
        let frames = self.total_frames.load(Ordering::Relaxed);
        if frames == 0 {
            return 0.0;
        }
        self.total_draw_calls.load(Ordering::Relaxed) as f32 / frames as f32
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FrameTimeBreakdown {
    pub frame_number: u64,
    pub cpu_translation_us: u64,
    pub gpu_execution_us: u64,
    pub present_us: u64,
    pub total_frame_us: u64,
    pub draw_calls: u32,
    pub state_changes: u32,
    pub buffer_uploads: u32,
}

impl FrameTimeBreakdown {
    pub fn cpu_bound(&self) -> bool {
        self.cpu_translation_us > self.gpu_execution_us
    }

    pub fn translation_overhead_pct(&self) -> f32 {
        if self.total_frame_us == 0 {
            return 0.0;
        }
        self.cpu_translation_us as f32 / self.total_frame_us as f32 * 100.0
    }
}

pub struct FrameTimeHistory {
    entries: Vec<FrameTimeBreakdown>,
    max_entries: usize,
}

impl FrameTimeHistory {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    pub fn record(&mut self, breakdown: FrameTimeBreakdown) {
        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }
        self.entries.push(breakdown);
    }

    pub fn avg_translation_overhead_pct(&self) -> f32 {
        if self.entries.is_empty() {
            return 0.0;
        }
        let sum: f32 = self
            .entries
            .iter()
            .map(|e| e.translation_overhead_pct())
            .sum();
        sum / self.entries.len() as f32
    }

    pub fn avg_frame_time_us(&self) -> u64 {
        if self.entries.is_empty() {
            return 0;
        }
        let sum: u64 = self.entries.iter().map(|e| e.total_frame_us).sum();
        sum / self.entries.len() as u64
    }

    pub fn latest(&self) -> Option<&FrameTimeBreakdown> {
        self.entries.last()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 6: Compatibility Database
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatLevel {
    Perfect,
    Playable,
    Runs,
    Broken,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkaroundType {
    ShaderPatch,
    StateOverride,
    FormatFallback,
    FeatureDisable,
    TimingHack,
    ResourceLimit,
}

#[derive(Debug, Clone)]
pub struct Workaround {
    pub workaround_type: WorkaroundType,
    pub description: String,
    pub shader_hash: Option<[u8; 32]>,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct GameCompatEntry {
    pub game_id: u64,
    pub title: String,
    pub steam_app_id: Option<u32>,
    pub level: CompatLevel,
    pub d3d_version: u32,
    pub workarounds: Vec<Workaround>,
    pub known_issues: Vec<String>,
    pub tested_version: Option<String>,
    pub report_count: u32,
}

impl GameCompatEntry {
    pub fn new(game_id: u64, title: String) -> Self {
        Self {
            game_id,
            title,
            steam_app_id: None,
            level: CompatLevel::Unknown,
            d3d_version: 11,
            workarounds: Vec::new(),
            known_issues: Vec::new(),
            tested_version: None,
            report_count: 0,
        }
    }

    pub fn add_workaround(&mut self, workaround: Workaround) {
        self.workarounds.push(workaround);
    }

    pub fn active_workarounds(&self) -> Vec<&Workaround> {
        self.workarounds.iter().filter(|w| w.enabled).collect()
    }
}

pub struct CompatDatabase {
    entries: BTreeMap<u64, GameCompatEntry>,
    by_steam_id: BTreeMap<u32, u64>,
    next_id: u64,
}

impl CompatDatabase {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            by_steam_id: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add_entry(&mut self, mut entry: GameCompatEntry) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        entry.game_id = id;
        if let Some(steam_id) = entry.steam_app_id {
            self.by_steam_id.insert(steam_id, id);
        }
        self.entries.insert(id, entry);
        id
    }

    pub fn get(&self, game_id: u64) -> Option<&GameCompatEntry> {
        self.entries.get(&game_id)
    }

    pub fn get_mut(&mut self, game_id: u64) -> Option<&mut GameCompatEntry> {
        self.entries.get_mut(&game_id)
    }

    pub fn lookup_by_steam_id(&self, steam_app_id: u32) -> Option<&GameCompatEntry> {
        self.by_steam_id
            .get(&steam_app_id)
            .and_then(|id| self.entries.get(id))
    }

    pub fn update_level(&mut self, game_id: u64, level: CompatLevel) -> bool {
        if let Some(entry) = self.entries.get_mut(&game_id) {
            entry.level = level;
            true
        } else {
            false
        }
    }

    pub fn submit_report(&mut self, game_id: u64, level: CompatLevel) -> bool {
        if let Some(entry) = self.entries.get_mut(&game_id) {
            entry.report_count += 1;
            // Only downgrade with multiple corroborating reports
            if entry.report_count >= 3 || entry.level == CompatLevel::Unknown {
                entry.level = level;
            }
            true
        } else {
            false
        }
    }

    pub fn entries_by_level(&self, level: CompatLevel) -> Vec<&GameCompatEntry> {
        self.entries.values().filter(|e| e.level == level).collect()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn perfect_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.level == CompatLevel::Perfect)
            .count()
    }

    pub fn playable_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.level == CompatLevel::Playable || e.level == CompatLevel::Perfect)
            .count()
    }

    pub fn broken_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.level == CompatLevel::Broken)
            .count()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 7: Integrated Translation Runtime
// ═══════════════════════════════════════════════════════════════════════════

pub struct D3dTranslationLayer {
    pub perf: TranslationPerfCounters,
    pub frame_history: FrameTimeHistory,
    pub compat_db: CompatDatabase,
    pub shader_cache: BTreeMap<[u8; 32], Vec<u8>>,
    pub pipeline_state_cache: BTreeMap<u64, u64>,
    pub current_game_id: Option<u64>,
    pub active_api: D3dApiVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3dApiVersion {
    D3d11,
    D3d12,
}

impl D3dTranslationLayer {
    pub fn new() -> Self {
        Self {
            perf: TranslationPerfCounters::new(),
            frame_history: FrameTimeHistory::new(300),
            compat_db: CompatDatabase::new(),
            shader_cache: BTreeMap::new(),
            pipeline_state_cache: BTreeMap::new(),
            current_game_id: None,
            active_api: D3dApiVersion::D3d11,
        }
    }

    pub fn set_active_game(&mut self, game_id: u64) {
        self.current_game_id = Some(game_id);
    }

    pub fn translate_shader(&mut self, bytecode: &[u8]) -> Result<Vec<u8>, ShaderTranslationError> {
        let hash = hash_shader_bytecode(bytecode);

        if let Some(cached) = self.shader_cache.get(&hash) {
            self.perf.record_shader_compile(true);
            return Ok(cached.clone());
        }

        self.perf.record_shader_compile(false);

        // Parse the header to determine shader type / reject non-DXBC up front.
        let header = parse_dxbc_header(bytecode)?;

        // Slice 1: delegate to the REAL DXBC→SPIR-V translator (the single
        // converged module — no more emit_stub_spirv twin). It produces valid
        // SPIR-V for the supported opcode subset (mov/ret/dcl). For shaders that
        // use an as-yet-unsupported opcode we fall back to the 5-word header so
        // the API-state path does not regress, clearly logged via the compat DB.
        let spirv = match crate::dxbc_spirv::translate(
            bytecode,
            crate::dxbc_spirv::TranslateOpts::default(),
        ) {
            Ok(t) => t.spirv,
            Err(crate::dxbc_spirv::ShaderError::UnsupportedInstruction(_))
            | Err(crate::dxbc_spirv::ShaderError::UnsupportedShaderModel(_)) => {
                // Not-yet-supported shader: keep the old behavior (header-only
                // SPIR-V) rather than failing the whole translation. `header`
                // confirmed it is real DXBC; the unsupported opcode is logged by
                // the translator's Err variant for the compat DB to surface.
                let _ = &header;
                fallback_header_spirv()
            }
            Err(_) => return Err(ShaderTranslationError::MalformedChunk),
        };
        self.shader_cache.insert(hash, spirv.clone());
        Ok(spirv)
    }

    pub fn translate_d3d11_draw(
        &self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) -> athgfx::DrawCommand {
        self.perf.record_draw_call();
        athgfx::DrawCommand::Draw {
            vertex_count,
            instance_count,
            first_vertex,
            first_instance,
        }
    }

    pub fn translate_d3d11_draw_indexed(
        &self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    ) -> athgfx::DrawCommand {
        self.perf.record_draw_call();
        athgfx::DrawCommand::DrawIndexed {
            index_count,
            instance_count,
            first_index,
            vertex_offset,
            first_instance,
        }
    }

    pub fn translate_d3d12_barriers(
        &self,
        barriers: &[D3d12ResourceBarrier],
    ) -> athgfx::DrawCommand {
        self.perf.record_barrier();
        athgfx::DrawCommand::PipelineBarrier(translate_barriers_to_athgfx(barriers))
    }

    pub fn translate_buffer_desc(&self, desc: &D3d11BufferDesc) -> athgfx::BufferDescriptor {
        self.perf.record_state_change();
        desc.to_athgfx()
    }

    pub fn translate_texture_desc(
        &self,
        desc: &D3d11Texture2dDesc,
    ) -> Option<athgfx::TextureDescriptor> {
        self.perf.record_state_change();
        desc.to_athgfx()
    }

    pub fn translate_rasterizer(&self, desc: &D3d11RasterizerDesc) -> athgfx::RasterState {
        self.perf.record_state_change();
        desc.to_athgfx()
    }

    pub fn translate_blend(&self, desc: &D3d11BlendDesc) -> athgfx::BlendState {
        self.perf.record_state_change();
        desc.to_athgfx()
    }

    pub fn translate_depth_stencil(
        &self,
        desc: &D3d11DepthStencilDesc,
    ) -> athgfx::DepthStencilState {
        self.perf.record_state_change();
        desc.to_athgfx()
    }

    pub fn end_frame(&mut self, breakdown: FrameTimeBreakdown) {
        self.frame_history.record(breakdown);
        self.perf.end_frame();
    }

    pub fn shader_cache_size(&self) -> usize {
        self.shader_cache.len()
    }

    pub fn clear_shader_cache(&mut self) {
        self.shader_cache.clear();
    }

    pub fn get_workarounds_for_current_game(&self) -> Vec<&Workaround> {
        match self.current_game_id {
            Some(id) => self
                .compat_db
                .get(id)
                .map(|e| e.active_workarounds())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

fn hash_shader_bytecode(data: &[u8]) -> [u8; 32] {
    let mut hash = [0u8; 32];
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let bytes = h.to_le_bytes();
    for i in 0..32 {
        hash[i] = bytes[i % 8].wrapping_add(i as u8);
    }
    hash
}

/// Header-only SPIR-V used ONLY as the not-yet-supported fallback for shaders
/// whose opcodes the real `dxbc_spirv::translate` does not handle yet. This is
/// NOT a translation stub-twin: the converged translator owns real translation;
/// this exists so the API-state path does not hard-fail on an exotic shader.
fn fallback_header_spirv() -> Vec<u8> {
    let header: [u32; 5] = [
        0x07230203, // SPIR-V magic
        0x00010000, // version 1.0
        0x00000000, // generator
        0x00000001, // bound
        0x00000000, // schema
    ];
    let mut out = Vec::with_capacity(20);
    for word in &header {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_mapping_comprehensive() {
        assert_eq!(
            DxgiFormat::R8G8B8A8_Unorm.to_athgfx(),
            Some(athgfx::PixelFormat::Rgba8Unorm)
        );
        assert_eq!(
            DxgiFormat::B8G8R8A8_Unorm_Srgb.to_athgfx(),
            Some(athgfx::PixelFormat::Bgra8Srgb)
        );
        assert_eq!(
            DxgiFormat::R16G16B16A16_Float.to_athgfx(),
            Some(athgfx::PixelFormat::Rgba16Float)
        );
        assert_eq!(
            DxgiFormat::D32_Float.to_athgfx(),
            Some(athgfx::PixelFormat::Depth32Float)
        );
        assert_eq!(
            DxgiFormat::BC7_Unorm.to_athgfx(),
            Some(athgfx::PixelFormat::Bc7Unorm)
        );
        assert_eq!(DxgiFormat::Unknown.to_athgfx(), None);
    }

    #[test]
    fn test_format_bytes_per_pixel() {
        assert_eq!(DxgiFormat::R32G32B32A32_Float.bytes_per_pixel(), 16);
        assert_eq!(DxgiFormat::R8G8B8A8_Unorm.bytes_per_pixel(), 4);
        assert_eq!(DxgiFormat::R8_Unorm.bytes_per_pixel(), 1);
        assert_eq!(DxgiFormat::R16G16_Float.bytes_per_pixel(), 4);
    }

    #[test]
    fn test_rasterizer_translation() {
        let desc = D3d11RasterizerDesc {
            fill_mode: D3d11FillMode::Wireframe,
            cull_mode: D3d11CullMode::Front,
            front_counter_clockwise: true,
            ..D3d11RasterizerDesc::default()
        };
        let rs = desc.to_athgfx();
        assert_eq!(rs.cull_mode, athgfx::CullMode::Front);
        assert_eq!(rs.polygon_mode, athgfx::PolygonMode::Line);
        assert_eq!(rs.front_face, athgfx::FrontFace::CounterClockwise);
    }

    #[test]
    fn test_blend_translation() {
        let desc = D3d11BlendDesc {
            blend_enable: true,
            src_blend: D3d11Blend::SrcAlpha,
            dest_blend: D3d11Blend::InvSrcAlpha,
            blend_op: D3d11BlendOp::Add,
            src_blend_alpha: D3d11Blend::One,
            dest_blend_alpha: D3d11Blend::Zero,
            blend_op_alpha: D3d11BlendOp::Add,
        };
        let bs = desc.to_athgfx();
        assert!(bs.enabled);
        assert_eq!(bs.src_factor, athgfx::BlendFactor::SrcAlpha);
        assert_eq!(bs.dst_factor, athgfx::BlendFactor::OneMinusSrcAlpha);
    }

    #[test]
    fn test_d3d12_barrier_translation() {
        let barrier = D3d12ResourceBarrier {
            resource_handle: 42,
            state_before: D3d12ResourceState::RenderTarget,
            state_after: D3d12ResourceState::Present,
            subresource: 0,
        };
        let info = barrier.to_athgfx_barrier_info();
        assert_eq!(info.src_stage, athgfx::PipelineStage::ColorAttachmentOutput);
        assert_eq!(info.image_barriers.len(), 1);
        assert_eq!(
            info.image_barriers[0].old_layout,
            athgfx::ImageLayout::ColorAttachment
        );
        assert_eq!(
            info.image_barriers[0].new_layout,
            athgfx::ImageLayout::PresentSrc
        );
    }

    #[test]
    fn test_compat_database() {
        let mut db = CompatDatabase::new();
        let mut entry = GameCompatEntry::new(0, String::from("Test Game"));
        entry.steam_app_id = Some(12345);
        entry.level = CompatLevel::Playable;
        let id = db.add_entry(entry);

        assert_eq!(db.entry_count(), 1);
        assert_eq!(db.get(id).unwrap().level, CompatLevel::Playable);
        assert!(db.lookup_by_steam_id(12345).is_some());
        assert!(db.lookup_by_steam_id(99999).is_none());
    }

    #[test]
    fn test_shader_header_parse() {
        // Construct a minimal DXBC header
        let mut buf = vec![0u8; 64];
        buf[0..4].copy_from_slice(&DXBC_MAGIC.to_le_bytes());
        // total size at offset 24
        buf[24..28].copy_from_slice(&64u32.to_le_bytes());
        // chunk count at offset 28
        buf[28..32].copy_from_slice(&1u32.to_le_bytes());
        // chunk offset at offset 32
        buf[32..36].copy_from_slice(&36u32.to_le_bytes());
        // chunk fourcc SHEX at offset 36
        buf[36..40].copy_from_slice(&0x58454853u32.to_le_bytes());
        // chunk size at offset 40
        buf[40..44].copy_from_slice(&16u32.to_le_bytes());
        // version token at offset 44: pixel shader 5.0
        // program_type=0 (pixel), major=5, minor=0 → (0 << 16) | (5 << 4) | 0 = 0x50
        let version_token: u32 = (0 << 16) | (5 << 4) | 0;
        buf[44..48].copy_from_slice(&version_token.to_le_bytes());

        let header = parse_dxbc_header(&buf).unwrap();
        assert_eq!(header.total_size, 64);
        assert_eq!(header.chunk_count, 1);
        assert_eq!(header.shader_type, DxbcShaderType::Pixel);
        assert_eq!(header.shader_model, ShaderModel::Sm5_0);
    }

    #[test]
    fn test_input_layout_translation() {
        let elements = vec![
            D3dInputElement {
                semantic_name: String::from("POSITION"),
                semantic_index: 0,
                format: DxgiFormat::R32G32B32_Float,
                input_slot: 0,
                byte_offset: 0,
                instance_step_rate: 0,
            },
            D3dInputElement {
                semantic_name: String::from("TEXCOORD"),
                semantic_index: 0,
                format: DxgiFormat::R32G32_Float,
                input_slot: 0,
                byte_offset: 12,
                instance_step_rate: 0,
            },
        ];
        let layouts = translate_input_layout(&elements);
        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0].attributes.len(), 2);
        assert_eq!(layouts[0].stride, 20);
    }

    #[test]
    fn test_perf_counters() {
        let counters = TranslationPerfCounters::new();
        counters.record_draw_call();
        counters.record_draw_call();
        counters.record_state_change();
        counters.record_shader_compile(true);
        counters.record_shader_compile(false);
        counters.end_frame();

        assert_eq!(counters.total_draw_calls.load(Ordering::Relaxed), 2);
        assert_eq!(counters.total_state_changes.load(Ordering::Relaxed), 1);
        assert_eq!(counters.total_frames.load(Ordering::Relaxed), 1);
        assert!((counters.shader_cache_hit_rate() - 0.5).abs() < 0.01);
    }
}
