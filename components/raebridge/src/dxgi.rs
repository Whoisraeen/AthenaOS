//! DirectX → RaeGFX translation layer.
//!
//! Translates DXGI, Direct3D 11, and Direct3D 12 API calls into RaeGFX
//! pipeline commands — analogous to DXVK/VKD3D-Proton but native to RaeenOS.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

// ═══════════════════════════════════════════════════════════════════════════
// Error types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DxgiError {
    InvalidFormat(u32),
    SwapChainCreationFailed,
    DeviceCreationFailed,
    OutOfMemory,
    InvalidParameter(&'static str),
    Unimplemented(&'static str),
    ShaderTranslationFailed(ShaderError),
    ResourceNotFound(u64),
    CommandListClosed,
}

impl fmt::Display for DxgiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(v) => write!(f, "invalid DXGI format: {v}"),
            Self::SwapChainCreationFailed => write!(f, "swap chain creation failed"),
            Self::DeviceCreationFailed => write!(f, "device creation failed"),
            Self::OutOfMemory => write!(f, "out of memory"),
            Self::InvalidParameter(s) => write!(f, "invalid parameter: {s}"),
            Self::Unimplemented(s) => write!(f, "not implemented: {s}"),
            Self::ShaderTranslationFailed(e) => write!(f, "shader translation failed: {e}"),
            Self::ResourceNotFound(h) => write!(f, "resource not found: handle {h:#x}"),
            Self::CommandListClosed => write!(f, "command list is closed"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DXGI Formats
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DxgiFormat {
    Unknown = 0,
    R32G32B32A32Typeless = 1,
    R32G32B32A32Float = 2,
    R32G32B32A32Uint = 3,
    R32G32B32A32Sint = 4,
    R32G32B32Typeless = 5,
    R32G32B32Float = 6,
    R16G16B16A16Typeless = 9,
    R16G16B16A16Float = 10,
    R16G16B16A16Unorm = 11,
    R16G16B16A16Uint = 12,
    R16G16B16A16Snorm = 13,
    R16G16B16A16Sint = 14,
    R32G32Typeless = 15,
    R32G32Float = 16,
    R11G11B10Float = 26,
    R8G8B8A8Typeless = 27,
    R8G8B8A8Unorm = 28,
    R8G8B8A8UnormSrgb = 29,
    R8G8B8A8Uint = 30,
    R8G8B8A8Snorm = 31,
    R8G8B8A8Sint = 32,
    D32Float = 40,
    R32Float = 41,
    D24UnormS8Uint = 45,
    R16Float = 54,
    D16Unorm = 55,
    R16Unorm = 56,
    R8Unorm = 61,
    Bc1Typeless = 70,
    Bc1Unorm = 71,
    Bc1UnormSrgb = 72,
    Bc2Typeless = 73,
    Bc2Unorm = 74,
    Bc3Typeless = 76,
    Bc3Unorm = 77,
    Bc3UnormSrgb = 78,
    Bc7Typeless = 97,
    Bc7Unorm = 98,
    Bc7UnormSrgb = 99,
    B8G8R8A8Unorm = 87,
    B8G8R8A8UnormSrgb = 91,
    R16G16Float = 34,
    R16G16Unorm = 35,
    R32Uint = 42,
    R32Sint = 43,
    R16Uint = 57,
}

impl DxgiFormat {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::R32G32B32A32Typeless,
            2 => Self::R32G32B32A32Float,
            3 => Self::R32G32B32A32Uint,
            4 => Self::R32G32B32A32Sint,
            5 => Self::R32G32B32Typeless,
            6 => Self::R32G32B32Float,
            9 => Self::R16G16B16A16Typeless,
            10 => Self::R16G16B16A16Float,
            11 => Self::R16G16B16A16Unorm,
            12 => Self::R16G16B16A16Uint,
            26 => Self::R11G11B10Float,
            28 => Self::R8G8B8A8Unorm,
            29 => Self::R8G8B8A8UnormSrgb,
            34 => Self::R16G16Float,
            40 => Self::D32Float,
            41 => Self::R32Float,
            42 => Self::R32Uint,
            45 => Self::D24UnormS8Uint,
            54 => Self::R16Float,
            55 => Self::D16Unorm,
            61 => Self::R8Unorm,
            71 => Self::Bc1Unorm,
            77 => Self::Bc3Unorm,
            87 => Self::B8G8R8A8Unorm,
            91 => Self::B8G8R8A8UnormSrgb,
            57 => Self::R16Uint,
            98 => Self::Bc7Unorm,
            _ => Self::Unknown,
        }
    }

    pub fn to_raegfx(&self) -> Option<raegfx::PixelFormat> {
        match self {
            Self::R8G8B8A8Unorm => Some(raegfx::PixelFormat::Rgba8Unorm),
            Self::R8G8B8A8UnormSrgb => Some(raegfx::PixelFormat::Rgba8Srgb),
            Self::B8G8R8A8Unorm => Some(raegfx::PixelFormat::Bgra8Unorm),
            Self::B8G8R8A8UnormSrgb => Some(raegfx::PixelFormat::Bgra8Srgb),
            Self::R16G16B16A16Float => Some(raegfx::PixelFormat::Rgba16Float),
            Self::R32G32B32A32Float => Some(raegfx::PixelFormat::Rgba32Float),
            Self::R11G11B10Float => Some(raegfx::PixelFormat::Rg11B10Float),
            Self::D24UnormS8Uint => Some(raegfx::PixelFormat::Depth24Stencil8),
            Self::D32Float => Some(raegfx::PixelFormat::Depth32Float),
            Self::R8Unorm => Some(raegfx::PixelFormat::R8Unorm),
            Self::Bc1Unorm => Some(raegfx::PixelFormat::Bc1Unorm),
            Self::Bc3Unorm => Some(raegfx::PixelFormat::Bc3Unorm),
            Self::Bc7Unorm => Some(raegfx::PixelFormat::Bc7Unorm),
            _ => None,
        }
    }

    pub fn is_depth(&self) -> bool {
        matches!(self, Self::D32Float | Self::D24UnormS8Uint | Self::D16Unorm)
    }

    pub fn is_typeless(&self) -> bool {
        matches!(
            self,
            Self::R32G32B32A32Typeless
                | Self::R32G32B32Typeless
                | Self::R16G16B16A16Typeless
                | Self::R32G32Typeless
                | Self::R8G8B8A8Typeless
                | Self::Bc1Typeless
                | Self::Bc2Typeless
                | Self::Bc3Typeless
                | Self::Bc7Typeless
        )
    }

    pub fn bytes_per_pixel(&self) -> u32 {
        match self {
            Self::R8Unorm => 1,
            Self::R16Float | Self::R16Unorm | Self::D16Unorm => 2,
            Self::R11G11B10Float
            | Self::R8G8B8A8Unorm
            | Self::R8G8B8A8UnormSrgb
            | Self::R8G8B8A8Uint
            | Self::R8G8B8A8Snorm
            | Self::R8G8B8A8Sint
            | Self::B8G8R8A8Unorm
            | Self::B8G8R8A8UnormSrgb
            | Self::R8G8B8A8Typeless
            | Self::D32Float
            | Self::R32Float
            | Self::R32Uint
            | Self::R32Sint
            | Self::D24UnormS8Uint
            | Self::R16G16Float
            | Self::R16G16Unorm => 4,
            Self::R16G16B16A16Float
            | Self::R16G16B16A16Unorm
            | Self::R16G16B16A16Uint
            | Self::R16G16B16A16Snorm
            | Self::R16G16B16A16Sint
            | Self::R16G16B16A16Typeless
            | Self::R32G32Float
            | Self::R32G32Typeless => 8,
            Self::R32G32B32Float | Self::R32G32B32Typeless => 12,
            Self::R32G32B32A32Float
            | Self::R32G32B32A32Uint
            | Self::R32G32B32A32Sint
            | Self::R32G32B32A32Typeless => 16,
            _ => 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DXGI Infrastructure
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DxgiPresentMode {
    Discard,
    Sequential,
    FlipSequential,
    FlipDiscard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DxgiRotation {
    Identity,
    Rotate90,
    Rotate180,
    Rotate270,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DxgiScanlineOrder {
    Unspecified,
    Progressive,
    UpperFieldFirst,
    LowerFieldFirst,
}

#[derive(Debug, Clone)]
pub struct DxgiModeDesc {
    pub width: u32,
    pub height: u32,
    pub refresh_num: u32,
    pub refresh_den: u32,
    pub format: DxgiFormat,
    pub scanline: DxgiScanlineOrder,
}

#[derive(Debug, Clone)]
pub struct DxgiOutput {
    pub name: String,
    pub attached: bool,
    pub rotation: DxgiRotation,
    pub modes: Vec<DxgiModeDesc>,
}

#[derive(Debug, Clone)]
pub struct DxgiAdapter {
    pub description: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub subsys_id: u32,
    pub revision: u32,
    pub dedicated_video_memory: u64,
    pub dedicated_system_memory: u64,
    pub shared_system_memory: u64,
    pub luid: u64,
}

impl DxgiAdapter {
    pub fn raeen_default() -> Self {
        Self {
            description: String::from("RaeGFX Virtual GPU"),
            vendor_id: 0x1AEE,
            device_id: 0x0001,
            subsys_id: 0,
            revision: 1,
            dedicated_video_memory: 4 * 1024 * 1024 * 1024,
            dedicated_system_memory: 0,
            shared_system_memory: 8 * 1024 * 1024 * 1024,
            luid: 0x0000_0001_0000_0000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DxgiBuffer {
    pub handle: u64,
    pub width: u32,
    pub height: u32,
    pub format: DxgiFormat,
}

pub struct DxgiSwapChain {
    pub width: u32,
    pub height: u32,
    pub format: DxgiFormat,
    pub buffer_count: u32,
    pub present_mode: DxgiPresentMode,
    pub fullscreen: bool,
    pub buffers: Vec<DxgiBuffer>,
    pub current_back_buffer: u32,
    pub vsync: bool,
    pub tearing_support: bool,
}

impl DxgiSwapChain {
    pub fn new(
        width: u32,
        height: u32,
        format: DxgiFormat,
        buffer_count: u32,
        present_mode: DxgiPresentMode,
    ) -> Self {
        let buffers = (0..buffer_count)
            .map(|i| DxgiBuffer {
                handle: i as u64,
                width,
                height,
                format,
            })
            .collect();
        Self {
            width,
            height,
            format,
            buffer_count,
            present_mode,
            fullscreen: false,
            buffers,
            current_back_buffer: 0,
            vsync: true,
            tearing_support: false,
        }
    }

    pub fn present(&mut self, sync_interval: u32) -> Result<(), DxgiError> {
        self.vsync = sync_interval > 0;
        self.current_back_buffer = (self.current_back_buffer + 1) % self.buffer_count;
        Ok(())
    }

    pub fn get_current_back_buffer(&self) -> &DxgiBuffer {
        &self.buffers[self.current_back_buffer as usize]
    }

    pub fn resize_buffers(
        &mut self,
        width: u32,
        height: u32,
        format: DxgiFormat,
        buffer_count: u32,
    ) -> Result<(), DxgiError> {
        self.width = width;
        self.height = height;
        if format != DxgiFormat::Unknown {
            self.format = format;
        }
        let count = if buffer_count == 0 {
            self.buffer_count
        } else {
            buffer_count
        };
        self.buffer_count = count;
        self.buffers = (0..count)
            .map(|i| DxgiBuffer {
                handle: i as u64,
                width: self.width,
                height: self.height,
                format: self.format,
            })
            .collect();
        self.current_back_buffer = 0;
        Ok(())
    }

    pub fn to_raegfx_present_mode(&self) -> raegfx::PresentMode {
        if !self.vsync {
            raegfx::PresentMode::Immediate
        } else {
            match self.present_mode {
                DxgiPresentMode::FlipDiscard | DxgiPresentMode::Discard => {
                    raegfx::PresentMode::Mailbox
                }
                DxgiPresentMode::FlipSequential | DxgiPresentMode::Sequential => {
                    raegfx::PresentMode::Fifo
                }
            }
        }
    }
}

pub struct DxgiFactory {
    pub adapters: Vec<DxgiAdapter>,
    pub flags: u32,
}

impl DxgiFactory {
    pub fn new(flags: u32) -> Self {
        Self {
            adapters: vec![DxgiAdapter::raeen_default()],
            flags,
        }
    }

    pub fn enum_adapters(&self, index: u32) -> Option<&DxgiAdapter> {
        self.adapters.get(index as usize)
    }

    pub fn create_swap_chain(
        &self,
        width: u32,
        height: u32,
        format: DxgiFormat,
        buffer_count: u32,
    ) -> Result<DxgiSwapChain, DxgiError> {
        if width == 0 || height == 0 {
            return Err(DxgiError::InvalidParameter(
                "swap chain dimensions must be > 0",
            ));
        }
        Ok(DxgiSwapChain::new(
            width,
            height,
            format,
            buffer_count.max(2),
            DxgiPresentMode::FlipDiscard,
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Direct3D Feature Level & Common Enums
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum D3dFeatureLevel {
    Level9_1,
    Level9_2,
    Level9_3,
    Level10_0,
    Level10_1,
    Level11_0,
    Level11_1,
    Level12_0,
    Level12_1,
    Level12_2,
}

impl D3dFeatureLevel {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0x9100 => Self::Level9_1,
            0x9200 => Self::Level9_2,
            0x9300 => Self::Level9_3,
            0xa000 => Self::Level10_0,
            0xa100 => Self::Level10_1,
            0xb000 => Self::Level11_0,
            0xb100 => Self::Level11_1,
            0xc000 => Self::Level12_0,
            0xc100 => Self::Level12_1,
            0xc200 => Self::Level12_2,
            _ => Self::Level11_0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Direct3D 11 — Enums & State Structures
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11Usage {
    Default,
    Immutable,
    Dynamic,
    Staging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11MapType {
    Read,
    Write,
    ReadWrite,
    WriteDiscard,
    WriteNoOverwrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11PrimitiveTopology {
    Undefined,
    PointList,
    LineList,
    LineStrip,
    TriangleList,
    TriangleStrip,
    LineListAdj,
    LineStripAdj,
    TriangleListAdj,
    TriangleStripAdj,
}

impl D3d11PrimitiveTopology {
    pub fn to_raegfx(&self) -> raegfx::PrimitiveTopology {
        match self {
            Self::PointList => raegfx::PrimitiveTopology::PointList,
            Self::LineList | Self::LineListAdj => raegfx::PrimitiveTopology::LineList,
            Self::LineStrip | Self::LineStripAdj => raegfx::PrimitiveTopology::LineStrip,
            Self::TriangleList | Self::TriangleListAdj => raegfx::PrimitiveTopology::TriangleList,
            Self::TriangleStrip | Self::TriangleStripAdj => {
                raegfx::PrimitiveTopology::TriangleStrip
            }
            Self::Undefined => raegfx::PrimitiveTopology::TriangleList,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11Filter {
    MinMagMipPoint,
    MinMagPointMipLinear,
    MinPointMagLinearMipPoint,
    MinPointMagMipLinear,
    MinLinearMagMipPoint,
    MinLinearMagPointMipLinear,
    MinMagLinearMipPoint,
    MinMagMipLinear,
    Anisotropic,
    ComparisonMinMagMipLinear,
    ComparisonAnisotropic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11TextureAddressMode {
    Wrap,
    Mirror,
    Clamp,
    Border,
    MirrorOnce,
}

impl D3d11TextureAddressMode {
    pub fn to_raegfx(&self) -> raegfx::AddressMode {
        match self {
            Self::Wrap => raegfx::AddressMode::Repeat,
            Self::Mirror | Self::MirrorOnce => raegfx::AddressMode::MirroredRepeat,
            Self::Clamp => raegfx::AddressMode::ClampToEdge,
            Self::Border => raegfx::AddressMode::ClampToBorder,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11ComparisonFunc {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

impl D3d11ComparisonFunc {
    pub fn to_raegfx(&self) -> raegfx::CompareOp {
        match self {
            Self::Never => raegfx::CompareOp::Never,
            Self::Less => raegfx::CompareOp::Less,
            Self::Equal => raegfx::CompareOp::Equal,
            Self::LessEqual => raegfx::CompareOp::LessOrEqual,
            Self::Greater => raegfx::CompareOp::Greater,
            Self::NotEqual => raegfx::CompareOp::NotEqual,
            Self::GreaterEqual => raegfx::CompareOp::GreaterOrEqual,
            Self::Always => raegfx::CompareOp::Always,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11CullMode {
    None,
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11FillMode {
    Wireframe,
    Solid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d11ShaderType {
    Vertex,
    Pixel,
    Geometry,
    Hull,
    Domain,
    Compute,
}

// ═══════════════════════════════════════════════════════════════════════════
// D3D11 Pipeline State
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct D3d11Viewport {
    pub top_left_x: f32,
    pub top_left_y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3d11ScissorRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3d11RasterizerState {
    pub fill_mode: D3d11FillMode,
    pub cull_mode: D3d11CullMode,
    pub front_counter_clockwise: bool,
    pub depth_bias: i32,
    pub depth_bias_clamp: f32,
    pub slope_scaled_depth_bias: f32,
    pub depth_clip_enable: bool,
    pub scissor_enable: bool,
    pub multisample_enable: bool,
    pub antialiased_line_enable: bool,
}

impl Default for D3d11RasterizerState {
    fn default() -> Self {
        Self {
            fill_mode: D3d11FillMode::Solid,
            cull_mode: D3d11CullMode::Back,
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

impl D3d11RasterizerState {
    pub fn to_raegfx(&self) -> raegfx::RasterState {
        raegfx::RasterState {
            cull_mode: match self.cull_mode {
                D3d11CullMode::None => raegfx::CullMode::None,
                D3d11CullMode::Front => raegfx::CullMode::Front,
                D3d11CullMode::Back => raegfx::CullMode::Back,
            },
            front_face: if self.front_counter_clockwise {
                raegfx::FrontFace::CounterClockwise
            } else {
                raegfx::FrontFace::Clockwise
            },
            polygon_mode: match self.fill_mode {
                D3d11FillMode::Wireframe => raegfx::PolygonMode::Line,
                D3d11FillMode::Solid => raegfx::PolygonMode::Fill,
            },
            depth_bias: self.depth_bias as f32,
            depth_bias_slope: self.slope_scaled_depth_bias,
            line_width: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3d11BlendState {
    pub alpha_to_coverage: bool,
    pub independent_blend: bool,
    pub blend_enable: bool,
    pub src_blend: u32,
    pub dest_blend: u32,
    pub blend_op: u32,
    pub src_blend_alpha: u32,
    pub dest_blend_alpha: u32,
    pub blend_op_alpha: u32,
    pub write_mask: u8,
}

impl Default for D3d11BlendState {
    fn default() -> Self {
        Self {
            alpha_to_coverage: false,
            independent_blend: false,
            blend_enable: false,
            src_blend: 1,
            dest_blend: 0,
            blend_op: 1,
            src_blend_alpha: 1,
            dest_blend_alpha: 0,
            blend_op_alpha: 1,
            write_mask: 0x0F,
        }
    }
}

fn d3d11_blend_to_raegfx(val: u32) -> raegfx::BlendFactor {
    match val {
        1 => raegfx::BlendFactor::One,
        2 => raegfx::BlendFactor::Zero,
        3 => raegfx::BlendFactor::SrcColor,
        4 => raegfx::BlendFactor::OneMinusSrcColor,
        5 => raegfx::BlendFactor::SrcAlpha,
        6 => raegfx::BlendFactor::OneMinusSrcAlpha,
        7 => raegfx::BlendFactor::DstAlpha,
        8 => raegfx::BlendFactor::OneMinusDstAlpha,
        9 => raegfx::BlendFactor::DstColor,
        10 => raegfx::BlendFactor::OneMinusDstColor,
        _ => raegfx::BlendFactor::One,
    }
}

fn d3d11_blend_op_to_raegfx(val: u32) -> raegfx::BlendOp {
    match val {
        1 => raegfx::BlendOp::Add,
        2 => raegfx::BlendOp::Subtract,
        3 => raegfx::BlendOp::ReverseSubtract,
        4 => raegfx::BlendOp::Min,
        5 => raegfx::BlendOp::Max,
        _ => raegfx::BlendOp::Add,
    }
}

impl D3d11BlendState {
    pub fn to_raegfx(&self) -> raegfx::BlendState {
        raegfx::BlendState {
            enabled: self.blend_enable,
            src_factor: d3d11_blend_to_raegfx(self.src_blend),
            dst_factor: d3d11_blend_to_raegfx(self.dest_blend),
            op: d3d11_blend_op_to_raegfx(self.blend_op),
            src_alpha_factor: d3d11_blend_to_raegfx(self.src_blend_alpha),
            dst_alpha_factor: d3d11_blend_to_raegfx(self.dest_blend_alpha),
            alpha_op: d3d11_blend_op_to_raegfx(self.blend_op_alpha),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct D3d11DepthStencilState {
    pub depth_enable: bool,
    pub depth_write_mask: u32,
    pub depth_func: D3d11ComparisonFunc,
    pub stencil_enable: bool,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
}

impl Default for D3d11DepthStencilState {
    fn default() -> Self {
        Self {
            depth_enable: true,
            depth_write_mask: 1,
            depth_func: D3d11ComparisonFunc::Less,
            stencil_enable: false,
            stencil_read_mask: 0xFF,
            stencil_write_mask: 0xFF,
        }
    }
}

impl D3d11DepthStencilState {
    pub fn to_raegfx(&self) -> raegfx::DepthStencilState {
        raegfx::DepthStencilState {
            depth_test: self.depth_enable,
            depth_write: self.depth_write_mask != 0,
            depth_compare: self.depth_func.to_raegfx(),
            stencil_enabled: self.stencil_enable,
        }
    }
}

pub struct D3d11PipelineState {
    pub input_layout: Option<u64>,
    pub vertex_shader: Option<u64>,
    pub pixel_shader: Option<u64>,
    pub geometry_shader: Option<u64>,
    pub hull_shader: Option<u64>,
    pub domain_shader: Option<u64>,
    pub compute_shader: Option<u64>,
    pub rasterizer_state: D3d11RasterizerState,
    pub blend_state: D3d11BlendState,
    pub depth_stencil_state: D3d11DepthStencilState,
    pub viewports: Vec<D3d11Viewport>,
    pub scissors: Vec<D3d11ScissorRect>,
    pub render_targets: Vec<Option<u64>>,
    pub depth_stencil_view: Option<u64>,
    pub vertex_buffers: Vec<(u64, u32, u32)>,
    pub index_buffer: Option<(u64, DxgiFormat, u32)>,
    pub primitive_topology: D3d11PrimitiveTopology,
    pub constant_buffers: [Vec<Option<u64>>; 6],
    pub shader_resources: [Vec<Option<u64>>; 6],
    pub samplers: [Vec<Option<u64>>; 6],
}

impl D3d11PipelineState {
    pub fn new() -> Self {
        Self {
            input_layout: None,
            vertex_shader: None,
            pixel_shader: None,
            geometry_shader: None,
            hull_shader: None,
            domain_shader: None,
            compute_shader: None,
            rasterizer_state: D3d11RasterizerState::default(),
            blend_state: D3d11BlendState::default(),
            depth_stencil_state: D3d11DepthStencilState::default(),
            viewports: Vec::new(),
            scissors: Vec::new(),
            render_targets: Vec::new(),
            depth_stencil_view: None,
            vertex_buffers: Vec::new(),
            index_buffer: None,
            primitive_topology: D3d11PrimitiveTopology::Undefined,
            constant_buffers: [
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ],
            shader_resources: [
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ],
            samplers: [
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ],
        }
    }

    pub fn clear(&mut self) {
        self.input_layout = None;
        self.vertex_shader = None;
        self.pixel_shader = None;
        self.geometry_shader = None;
        self.hull_shader = None;
        self.domain_shader = None;
        self.compute_shader = None;
        self.rasterizer_state = D3d11RasterizerState::default();
        self.blend_state = D3d11BlendState::default();
        self.depth_stencil_state = D3d11DepthStencilState::default();
        self.viewports.clear();
        self.scissors.clear();
        self.render_targets.clear();
        self.depth_stencil_view = None;
        self.vertex_buffers.clear();
        self.index_buffer = None;
        self.primitive_topology = D3d11PrimitiveTopology::Undefined;
        for slot in &mut self.constant_buffers {
            slot.clear();
        }
        for slot in &mut self.shader_resources {
            slot.clear();
        }
        for slot in &mut self.samplers {
            slot.clear();
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D3D11 Resource Management
// ═══════════════════════════════════════════════════════════════════════════

pub struct D3d11Buffer {
    pub size: u64,
    pub usage: D3d11Usage,
    pub bind_flags: u32,
    pub cpu_access: u32,
    pub data: Vec<u8>,
    pub raegfx_handle: Option<u64>,
}

pub struct D3d11Texture2D {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub array_size: u32,
    pub format: DxgiFormat,
    pub sample_count: u32,
    pub usage: D3d11Usage,
    pub bind_flags: u32,
    pub data: Vec<u8>,
    pub raegfx_handle: Option<u64>,
}

pub struct D3d11RenderTargetView {
    pub resource: u64,
    pub format: DxgiFormat,
}

pub struct D3d11DepthStencilView {
    pub resource: u64,
    pub format: DxgiFormat,
}

pub struct D3d11ShaderResourceView {
    pub resource: u64,
    pub format: DxgiFormat,
}

#[derive(Debug, Clone)]
pub struct D3d11InputElementDesc {
    pub semantic_name: String,
    pub semantic_index: u32,
    pub format: DxgiFormat,
    pub input_slot: u32,
    pub byte_offset: u32,
    pub input_slot_class: u32,
    pub instance_data_step_rate: u32,
}

pub struct D3d11InputLayout {
    pub elements: Vec<D3d11InputElementDesc>,
}

pub struct D3d11SamplerState {
    pub filter: D3d11Filter,
    pub address_u: D3d11TextureAddressMode,
    pub address_v: D3d11TextureAddressMode,
    pub address_w: D3d11TextureAddressMode,
    pub max_anisotropy: u32,
    pub comparison: D3d11ComparisonFunc,
}

impl D3d11SamplerState {
    pub fn to_raegfx(&self) -> raegfx::SamplerDescriptor {
        let (min, mag, mip) = match self.filter {
            D3d11Filter::MinMagMipPoint => (
                raegfx::FilterMode::Nearest,
                raegfx::FilterMode::Nearest,
                raegfx::FilterMode::Nearest,
            ),
            D3d11Filter::MinMagMipLinear | D3d11Filter::ComparisonMinMagMipLinear => (
                raegfx::FilterMode::Linear,
                raegfx::FilterMode::Linear,
                raegfx::FilterMode::Linear,
            ),
            D3d11Filter::Anisotropic | D3d11Filter::ComparisonAnisotropic => {
                let aniso = raegfx::FilterMode::Anisotropic(self.max_anisotropy.min(16) as u8);
                (aniso, aniso, raegfx::FilterMode::Linear)
            }
            D3d11Filter::MinMagPointMipLinear => (
                raegfx::FilterMode::Nearest,
                raegfx::FilterMode::Nearest,
                raegfx::FilterMode::Linear,
            ),
            D3d11Filter::MinPointMagLinearMipPoint => (
                raegfx::FilterMode::Nearest,
                raegfx::FilterMode::Linear,
                raegfx::FilterMode::Nearest,
            ),
            D3d11Filter::MinPointMagMipLinear => (
                raegfx::FilterMode::Nearest,
                raegfx::FilterMode::Linear,
                raegfx::FilterMode::Linear,
            ),
            D3d11Filter::MinLinearMagMipPoint => (
                raegfx::FilterMode::Linear,
                raegfx::FilterMode::Nearest,
                raegfx::FilterMode::Nearest,
            ),
            D3d11Filter::MinLinearMagPointMipLinear => (
                raegfx::FilterMode::Linear,
                raegfx::FilterMode::Nearest,
                raegfx::FilterMode::Linear,
            ),
            D3d11Filter::MinMagLinearMipPoint => (
                raegfx::FilterMode::Linear,
                raegfx::FilterMode::Linear,
                raegfx::FilterMode::Nearest,
            ),
        };
        let compare = match self.filter {
            D3d11Filter::ComparisonMinMagMipLinear | D3d11Filter::ComparisonAnisotropic => {
                Some(self.comparison.to_raegfx())
            }
            _ => None,
        };
        raegfx::SamplerDescriptor {
            min_filter: min,
            mag_filter: mag,
            mipmap_filter: mip,
            address_u: self.address_u.to_raegfx(),
            address_v: self.address_v.to_raegfx(),
            address_w: self.address_w.to_raegfx(),
            max_anisotropy: self.max_anisotropy.min(16) as u8,
            compare,
            lod_min: 0.0,
            lod_max: 1000.0,
        }
    }
}

pub struct D3d11ResourceTable {
    next_handle: u64,
    buffers: BTreeMap<u64, D3d11Buffer>,
    textures: BTreeMap<u64, D3d11Texture2D>,
    rtvs: BTreeMap<u64, D3d11RenderTargetView>,
    dsvs: BTreeMap<u64, D3d11DepthStencilView>,
    srvs: BTreeMap<u64, D3d11ShaderResourceView>,
    input_layouts: BTreeMap<u64, D3d11InputLayout>,
    samplers: BTreeMap<u64, D3d11SamplerState>,
}

impl D3d11ResourceTable {
    pub fn new() -> Self {
        Self {
            next_handle: 1,
            buffers: BTreeMap::new(),
            textures: BTreeMap::new(),
            rtvs: BTreeMap::new(),
            dsvs: BTreeMap::new(),
            srvs: BTreeMap::new(),
            input_layouts: BTreeMap::new(),
            samplers: BTreeMap::new(),
        }
    }

    fn alloc_handle(&mut self) -> u64 {
        let h = self.next_handle;
        self.next_handle += 1;
        h
    }

    pub fn create_buffer(&mut self, buffer: D3d11Buffer) -> u64 {
        let h = self.alloc_handle();
        self.buffers.insert(h, buffer);
        h
    }

    pub fn create_texture(&mut self, texture: D3d11Texture2D) -> u64 {
        let h = self.alloc_handle();
        self.textures.insert(h, texture);
        h
    }

    pub fn create_rtv(&mut self, rtv: D3d11RenderTargetView) -> u64 {
        let h = self.alloc_handle();
        self.rtvs.insert(h, rtv);
        h
    }

    pub fn create_dsv(&mut self, dsv: D3d11DepthStencilView) -> u64 {
        let h = self.alloc_handle();
        self.dsvs.insert(h, dsv);
        h
    }

    pub fn create_srv(&mut self, srv: D3d11ShaderResourceView) -> u64 {
        let h = self.alloc_handle();
        self.srvs.insert(h, srv);
        h
    }

    pub fn create_input_layout(&mut self, layout: D3d11InputLayout) -> u64 {
        let h = self.alloc_handle();
        self.input_layouts.insert(h, layout);
        h
    }

    pub fn create_sampler(&mut self, sampler: D3d11SamplerState) -> u64 {
        let h = self.alloc_handle();
        self.samplers.insert(h, sampler);
        h
    }

    pub fn get_buffer(&self, handle: u64) -> Option<&D3d11Buffer> {
        self.buffers.get(&handle)
    }

    pub fn get_texture(&self, handle: u64) -> Option<&D3d11Texture2D> {
        self.textures.get(&handle)
    }

    pub fn get_rtv(&self, handle: u64) -> Option<&D3d11RenderTargetView> {
        self.rtvs.get(&handle)
    }

    pub fn destroy_buffer(&mut self, handle: u64) -> bool {
        self.buffers.remove(&handle).is_some()
    }

    pub fn destroy_texture(&mut self, handle: u64) -> bool {
        self.textures.remove(&handle).is_some()
    }

    pub fn resource_count(&self) -> usize {
        self.buffers.len()
            + self.textures.len()
            + self.rtvs.len()
            + self.dsvs.len()
            + self.srvs.len()
            + self.input_layouts.len()
            + self.samplers.len()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D3D11 Commands
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum D3d11Command {
    Draw(u32, u32),
    DrawIndexed(u32, u32, i32),
    DrawInstanced(u32, u32, u32, u32),
    DrawIndexedInstanced(u32, u32, u32, i32, u32),
    Dispatch(u32, u32, u32),
    ClearRenderTargetView(u64, [f32; 4]),
    ClearDepthStencilView(u64, u32, f32, u8),
    CopyResource(u64, u64),
    CopySubresourceRegion {
        dst: u64,
        dst_subresource: u32,
        dst_x: u32,
        dst_y: u32,
        dst_z: u32,
        src: u64,
        src_subresource: u32,
    },
    UpdateSubresource(u64, Vec<u8>),
    Map(u64, D3d11MapType),
    Unmap(u64),
    ResolveSubresource(u64, u64),
    GenerateMips(u64),
    SetPredication(Option<u64>, bool),
}

// ═══════════════════════════════════════════════════════════════════════════
// D3D11 Device & Context
// ═══════════════════════════════════════════════════════════════════════════

pub struct D3d11Device {
    pub feature_level: D3dFeatureLevel,
    pub creation_flags: u32,
    pub resources: D3d11ResourceTable,
}

impl D3d11Device {
    pub fn new(feature_level: D3dFeatureLevel, creation_flags: u32) -> Self {
        Self {
            feature_level,
            creation_flags,
            resources: D3d11ResourceTable::new(),
        }
    }

    pub fn create_buffer(
        &mut self,
        size: u64,
        usage: D3d11Usage,
        bind_flags: u32,
        cpu_access: u32,
        initial_data: Option<&[u8]>,
    ) -> u64 {
        let data = initial_data
            .map(Vec::from)
            .unwrap_or_else(|| vec![0u8; size as usize]);
        self.resources.create_buffer(D3d11Buffer {
            size,
            usage,
            bind_flags,
            cpu_access,
            data,
            raegfx_handle: None,
        })
    }

    pub fn create_texture_2d(
        &mut self,
        width: u32,
        height: u32,
        mip_levels: u32,
        array_size: u32,
        format: DxgiFormat,
        sample_count: u32,
        usage: D3d11Usage,
        bind_flags: u32,
    ) -> u64 {
        self.resources.create_texture(D3d11Texture2D {
            width,
            height,
            mip_levels,
            array_size,
            format,
            sample_count,
            usage,
            bind_flags,
            data: Vec::new(),
            raegfx_handle: None,
        })
    }

    pub fn check_feature_support(&self, feature: u32) -> bool {
        match feature {
            0 => true, // threading
            1 => true, // doubles
            2 => self.feature_level >= D3dFeatureLevel::Level10_0,
            9 => self.feature_level >= D3dFeatureLevel::Level11_0,
            _ => false,
        }
    }
}

pub struct D3d11DeviceContext {
    pub device: u64,
    pub state: D3d11PipelineState,
    pub command_list: Vec<D3d11Command>,
}

impl D3d11DeviceContext {
    pub fn new(device_handle: u64) -> Self {
        Self {
            device: device_handle,
            state: D3d11PipelineState::new(),
            command_list: Vec::new(),
        }
    }

    pub fn ia_set_input_layout(&mut self, layout: u64) {
        self.state.input_layout = Some(layout);
    }

    pub fn ia_set_primitive_topology(&mut self, topology: D3d11PrimitiveTopology) {
        self.state.primitive_topology = topology;
    }

    pub fn ia_set_vertex_buffers(&mut self, start_slot: u32, buffers: &[(u64, u32, u32)]) {
        let end = start_slot as usize + buffers.len();
        if self.state.vertex_buffers.len() < end {
            self.state.vertex_buffers.resize(end, (0, 0, 0));
        }
        for (i, vb) in buffers.iter().enumerate() {
            self.state.vertex_buffers[start_slot as usize + i] = *vb;
        }
    }

    pub fn ia_set_index_buffer(&mut self, buffer: u64, format: DxgiFormat, offset: u32) {
        self.state.index_buffer = Some((buffer, format, offset));
    }

    pub fn vs_set_shader(&mut self, shader: u64) {
        self.state.vertex_shader = Some(shader);
    }

    pub fn ps_set_shader(&mut self, shader: u64) {
        self.state.pixel_shader = Some(shader);
    }

    pub fn gs_set_shader(&mut self, shader: Option<u64>) {
        self.state.geometry_shader = shader;
    }

    pub fn hs_set_shader(&mut self, shader: Option<u64>) {
        self.state.hull_shader = shader;
    }

    pub fn ds_set_shader(&mut self, shader: Option<u64>) {
        self.state.domain_shader = shader;
    }

    pub fn cs_set_shader(&mut self, shader: Option<u64>) {
        self.state.compute_shader = shader;
    }

    pub fn rs_set_state(&mut self, state: D3d11RasterizerState) {
        self.state.rasterizer_state = state;
    }

    pub fn rs_set_viewports(&mut self, viewports: &[D3d11Viewport]) {
        self.state.viewports = Vec::from(viewports);
    }

    pub fn rs_set_scissor_rects(&mut self, rects: &[D3d11ScissorRect]) {
        self.state.scissors = Vec::from(rects);
    }

    pub fn om_set_blend_state(&mut self, state: D3d11BlendState) {
        self.state.blend_state = state;
    }

    pub fn om_set_depth_stencil_state(&mut self, state: D3d11DepthStencilState) {
        self.state.depth_stencil_state = state;
    }

    pub fn om_set_render_targets(&mut self, rtvs: &[Option<u64>], dsv: Option<u64>) {
        self.state.render_targets = Vec::from(rtvs);
        self.state.depth_stencil_view = dsv;
    }

    pub fn draw(&mut self, vertex_count: u32, start_vertex: u32) {
        self.command_list
            .push(D3d11Command::Draw(vertex_count, start_vertex));
    }

    pub fn draw_indexed(&mut self, index_count: u32, start_index: u32, base_vertex: i32) {
        self.command_list.push(D3d11Command::DrawIndexed(
            index_count,
            start_index,
            base_vertex,
        ));
    }

    pub fn draw_instanced(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        start_vertex: u32,
        start_instance: u32,
    ) {
        self.command_list.push(D3d11Command::DrawInstanced(
            vertex_count,
            instance_count,
            start_vertex,
            start_instance,
        ));
    }

    pub fn draw_indexed_instanced(
        &mut self,
        index_count: u32,
        instance_count: u32,
        start_index: u32,
        base_vertex: i32,
        start_instance: u32,
    ) {
        self.command_list.push(D3d11Command::DrawIndexedInstanced(
            index_count,
            instance_count,
            start_index,
            base_vertex,
            start_instance,
        ));
    }

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        self.command_list.push(D3d11Command::Dispatch(x, y, z));
    }

    pub fn clear_render_target_view(&mut self, rtv: u64, color: [f32; 4]) {
        self.command_list
            .push(D3d11Command::ClearRenderTargetView(rtv, color));
    }

    pub fn clear_depth_stencil_view(&mut self, dsv: u64, flags: u32, depth: f32, stencil: u8) {
        self.command_list.push(D3d11Command::ClearDepthStencilView(
            dsv, flags, depth, stencil,
        ));
    }

    pub fn copy_resource(&mut self, dst: u64, src: u64) {
        self.command_list.push(D3d11Command::CopyResource(dst, src));
    }

    pub fn update_subresource(&mut self, resource: u64, data: &[u8]) {
        self.command_list
            .push(D3d11Command::UpdateSubresource(resource, Vec::from(data)));
    }

    pub fn map_resource(&mut self, resource: u64, map_type: D3d11MapType) {
        self.command_list
            .push(D3d11Command::Map(resource, map_type));
    }

    pub fn unmap_resource(&mut self, resource: u64) {
        self.command_list.push(D3d11Command::Unmap(resource));
    }

    pub fn generate_mips(&mut self, srv: u64) {
        self.command_list.push(D3d11Command::GenerateMips(srv));
    }

    pub fn resolve_subresource(&mut self, dst: u64, src: u64) {
        self.command_list
            .push(D3d11Command::ResolveSubresource(dst, src));
    }

    pub fn flush(&mut self) -> Vec<D3d11Command> {
        let cmds = core::mem::take(&mut self.command_list);
        cmds
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D3D11 → RaeGFX Command Translation
// ═══════════════════════════════════════════════════════════════════════════

pub fn translate_d3d11_to_raegfx(
    commands: &[D3d11Command],
    state: &D3d11PipelineState,
) -> Vec<raegfx::DrawCommand> {
    let mut out = Vec::new();

    for vp in &state.viewports {
        out.push(raegfx::DrawCommand::SetViewport {
            x: vp.top_left_x,
            y: vp.top_left_y,
            width: vp.width,
            height: vp.height,
            min_depth: vp.min_depth,
            max_depth: vp.max_depth,
        });
    }

    for sc in &state.scissors {
        out.push(raegfx::DrawCommand::SetScissor {
            x: sc.left,
            y: sc.top,
            width: (sc.right - sc.left).max(0) as u32,
            height: (sc.bottom - sc.top).max(0) as u32,
        });
    }

    for (i, &(buf_handle, _stride, offset)) in state.vertex_buffers.iter().enumerate() {
        if buf_handle != 0 {
            out.push(raegfx::DrawCommand::BindVertexBuffer {
                slot: i as u32,
                buffer: raegfx::BufferHandle(buf_handle),
                offset: offset as u64,
            });
        }
    }

    if let Some((buf, fmt, offset)) = &state.index_buffer {
        let index_type = match fmt {
            DxgiFormat::R16Unorm | DxgiFormat::R16Uint => raegfx::IndexType::U16,
            _ => raegfx::IndexType::U32,
        };
        out.push(raegfx::DrawCommand::BindIndexBuffer {
            buffer: raegfx::BufferHandle(*buf),
            offset: *offset as u64,
            index_type,
        });
    }

    for cmd in commands {
        match cmd {
            D3d11Command::Draw(vertex_count, start_vertex) => {
                out.push(raegfx::DrawCommand::Draw {
                    vertex_count: *vertex_count,
                    instance_count: 1,
                    first_vertex: *start_vertex,
                    first_instance: 0,
                });
            }
            D3d11Command::DrawIndexed(index_count, start_index, base_vertex) => {
                out.push(raegfx::DrawCommand::DrawIndexed {
                    index_count: *index_count,
                    instance_count: 1,
                    first_index: *start_index,
                    vertex_offset: *base_vertex,
                    first_instance: 0,
                });
            }
            D3d11Command::DrawInstanced(vc, ic, sv, si) => {
                out.push(raegfx::DrawCommand::Draw {
                    vertex_count: *vc,
                    instance_count: *ic,
                    first_vertex: *sv,
                    first_instance: *si,
                });
            }
            D3d11Command::DrawIndexedInstanced(ic, inst, si, bv, sinst) => {
                out.push(raegfx::DrawCommand::DrawIndexed {
                    index_count: *ic,
                    instance_count: *inst,
                    first_index: *si,
                    vertex_offset: *bv,
                    first_instance: *sinst,
                });
            }
            D3d11Command::Dispatch(x, y, z) => {
                out.push(raegfx::DrawCommand::Dispatch {
                    x: *x,
                    y: *y,
                    z: *z,
                });
            }
            D3d11Command::ClearRenderTargetView(_, _)
            | D3d11Command::ClearDepthStencilView(_, _, _, _)
            | D3d11Command::CopyResource(_, _)
            | D3d11Command::CopySubresourceRegion { .. }
            | D3d11Command::UpdateSubresource(_, _)
            | D3d11Command::Map(_, _)
            | D3d11Command::Unmap(_)
            | D3d11Command::ResolveSubresource(_, _)
            | D3d11Command::GenerateMips(_)
            | D3d11Command::SetPredication(_, _) => {
                // Resource management commands are handled at a higher level
                // by the translation runtime, not emitted as draw commands.
            }
        }
    }

    out
}

// ═══════════════════════════════════════════════════════════════════════════
// Shader Translation (DXBC/DXIL → SPIR-V) — DXBC delegates to dxbc_spirv
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaderError {
    InvalidBytecode,
    UnsupportedShaderModel(u32),
    UnsupportedInstruction(u32),
    MissingSignature,
    TranslationFailed(&'static str),
    CacheFull,
}

impl fmt::Display for ShaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBytecode => write!(f, "invalid shader bytecode"),
            Self::UnsupportedShaderModel(v) => write!(f, "unsupported shader model: {v}"),
            Self::UnsupportedInstruction(op) => write!(f, "unsupported instruction opcode: {op}"),
            Self::MissingSignature => write!(f, "missing input/output signature"),
            Self::TranslationFailed(s) => write!(f, "translation failed: {s}"),
            Self::CacheFull => write!(f, "shader cache full"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentType {
    Float32,
    Int32,
    Uint32,
    Float16,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ShaderParameter {
    pub name: String,
    pub semantic: String,
    pub semantic_index: u32,
    pub register: u32,
    pub mask: u8,
    pub component_type: ComponentType,
}

pub struct DxbcShader {
    pub bytecode: Vec<u8>,
    pub shader_type: D3d11ShaderType,
    pub input_signature: Vec<ShaderParameter>,
    pub output_signature: Vec<ShaderParameter>,
}

impl DxbcShader {
    pub fn parse(bytecode: &[u8]) -> Result<Self, ShaderError> {
        if bytecode.len() < 20 {
            return Err(ShaderError::InvalidBytecode);
        }
        let magic = u32::from_le_bytes([bytecode[0], bytecode[1], bytecode[2], bytecode[3]]);
        if magic != 0x43425844 {
            return Err(ShaderError::InvalidBytecode);
        }
        // Stub: extract minimal info. Full DXBC parsing is a large undertaking.
        Ok(Self {
            bytecode: Vec::from(bytecode),
            shader_type: D3d11ShaderType::Vertex,
            input_signature: Vec::new(),
            output_signature: Vec::new(),
        })
    }

    pub fn shader_stage(&self) -> raegfx::ShaderStage {
        match self.shader_type {
            D3d11ShaderType::Vertex => raegfx::ShaderStage::Vertex,
            D3d11ShaderType::Pixel => raegfx::ShaderStage::Fragment,
            D3d11ShaderType::Geometry => raegfx::ShaderStage::Geometry,
            D3d11ShaderType::Hull => raegfx::ShaderStage::TessControl,
            D3d11ShaderType::Domain => raegfx::ShaderStage::TessEvaluation,
            D3d11ShaderType::Compute => raegfx::ShaderStage::Compute,
        }
    }
}

pub struct ShaderTranslator {
    cache: BTreeMap<[u8; 32], Vec<u8>>,
}

impl ShaderTranslator {
    pub fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
        }
    }

    /// Translate DXBC (SM4/SM5) bytecode to SPIR-V.
    ///
    /// Delegates to the one converged `dxbc_spirv::translate` (no local stub
    /// twin). Returns real SPIR-V for the supported opcode subset (mov/ret/dcl);
    /// unsupported opcodes surface as `ShaderError::UnsupportedInstruction`.
    pub fn translate_dxbc_to_spirv(&mut self, dxbc: &[u8]) -> Result<Vec<u8>, ShaderError> {
        if dxbc.len() < 4 {
            return Err(ShaderError::InvalidBytecode);
        }

        let hash = Self::hash_bytecode(dxbc);
        if let Some(cached) = self.cache.get(&hash) {
            return Ok(cached.clone());
        }

        let spirv =
            crate::dxbc_spirv::translate(dxbc, crate::dxbc_spirv::TranslateOpts::default())?.spirv;
        self.cache.insert(hash, spirv.clone());
        Ok(spirv)
    }

    /// Translate DXIL (SM6+, D3D12) bytecode to SPIR-V.
    ///
    /// DXIL = LLVM bitcode in a DXBC envelope; it needs an LLVM-bitcode reader
    /// (a separate multi-month subsystem, VKD3D-Proton is LGPL study-only).
    /// Explicitly unsupported in this workstream — returns a clean error rather
    /// than emitting garbage.
    pub fn translate_dxil_to_spirv(&mut self, dxil: &[u8]) -> Result<Vec<u8>, ShaderError> {
        if dxil.len() < 4 {
            return Err(ShaderError::InvalidBytecode);
        }
        Err(ShaderError::TranslationFailed(
            "DXIL/SM6 (D3D12) translation not implemented (slice 1 is DXBC/SM4-5)",
        ))
    }

    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    fn hash_bytecode(data: &[u8]) -> [u8; 32] {
        let mut hash = [0u8; 32];
        // Simple FNV-like hash spread across 32 bytes for keying.
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
}

// ═══════════════════════════════════════════════════════════════════════════
// Direct3D 12 — Basic Structures
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d12CommandListType {
    Direct,
    Bundle,
    Compute,
    Copy,
    VideoDecode,
    VideoProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d12DescriptorHeapType {
    CbvSrvUav,
    Sampler,
    Rtv,
    Dsv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d12ResourceBarrierType {
    Transition,
    Aliasing,
    Uav,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d12ResourceState {
    Common,
    VertexAndConstantBuffer,
    IndexBuffer,
    RenderTarget,
    UnorderedAccess,
    DepthWrite,
    DepthRead,
    NonPixelShaderResource,
    PixelShaderResource,
    CopyDest,
    CopySource,
    ResolveDest,
    ResolveSource,
    Present,
    GenericRead,
}

#[derive(Debug, Clone)]
pub struct D3d12ResourceBarrier {
    pub barrier_type: D3d12ResourceBarrierType,
    pub resource: u64,
    pub state_before: D3d12ResourceState,
    pub state_after: D3d12ResourceState,
    pub subresource: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3d12RootParameterType {
    DescriptorTable,
    Constants32Bit,
    Cbv,
    Srv,
    Uav,
}

#[derive(Debug, Clone)]
pub struct D3d12RootParameter {
    pub parameter_type: D3d12RootParameterType,
    pub shader_register: u32,
    pub register_space: u32,
    pub num_32bit_values: u32,
}

pub struct D3d12Device {
    pub node_count: u32,
    pub feature_level: D3dFeatureLevel,
}

impl D3d12Device {
    pub fn new(feature_level: D3dFeatureLevel) -> Self {
        Self {
            node_count: 1,
            feature_level,
        }
    }

    pub fn get_node_count(&self) -> u32 {
        self.node_count
    }
}

pub struct D3d12CommandQueue {
    pub queue_type: D3d12CommandListType,
}

pub struct D3d12CommandAllocator {
    pub list_type: D3d12CommandListType,
}

impl D3d12CommandAllocator {
    pub fn new(list_type: D3d12CommandListType) -> Self {
        Self { list_type }
    }

    pub fn reset(&self) -> Result<(), DxgiError> {
        Ok(())
    }
}

pub struct D3d12DescriptorHeap {
    pub heap_type: D3d12DescriptorHeapType,
    pub num_descriptors: u32,
    pub shader_visible: bool,
}

pub struct D3d12RootSignature {
    pub parameters: Vec<D3d12RootParameter>,
}

pub struct D3d12PipelineState {
    pub root_signature: u64,
    pub vertex_shader: Vec<u8>,
    pub pixel_shader: Vec<u8>,
    pub blend_state: D3d11BlendState,
    pub rasterizer_state: D3d11RasterizerState,
    pub depth_stencil_state: D3d11DepthStencilState,
    pub input_layout: Vec<D3d11InputElementDesc>,
    pub primitive_topology_type: u32,
    pub num_render_targets: u32,
    pub rtv_formats: [DxgiFormat; 8],
    pub dsv_format: DxgiFormat,
    pub sample_count: u32,
}

impl D3d12PipelineState {
    pub fn new() -> Self {
        Self {
            root_signature: 0,
            vertex_shader: Vec::new(),
            pixel_shader: Vec::new(),
            blend_state: D3d11BlendState::default(),
            rasterizer_state: D3d11RasterizerState::default(),
            depth_stencil_state: D3d11DepthStencilState::default(),
            input_layout: Vec::new(),
            primitive_topology_type: 4, // TRIANGLE
            num_render_targets: 1,
            rtv_formats: [DxgiFormat::R8G8B8A8Unorm; 8],
            dsv_format: DxgiFormat::D24UnormS8Uint,
            sample_count: 1,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D3D12 Commands
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum D3d12Command {
    DrawInstanced(u32, u32, u32, u32),
    DrawIndexedInstanced(u32, u32, u32, i32, u32),
    Dispatch(u32, u32, u32),
    ClearRenderTargetView(u64, [f32; 4]),
    ClearDepthStencilView(u64, f32, u8),
    ResourceBarrier(Vec<D3d12ResourceBarrier>),
    SetGraphicsRootSignature(u64),
    SetComputeRootSignature(u64),
    SetPipelineState(u64),
    SetGraphicsRoot32BitConstants(u32, Vec<u32>),
    CopyResource(u64, u64),
    CopyBufferRegion {
        dst: u64,
        dst_offset: u64,
        src: u64,
        src_offset: u64,
        num_bytes: u64,
    },
}

pub struct D3d12GraphicsCommandList {
    pub commands: Vec<D3d12Command>,
    pub allocator: u64,
    pub pipeline_state: Option<u64>,
    pub closed: bool,
}

impl D3d12GraphicsCommandList {
    pub fn new(allocator: u64) -> Self {
        Self {
            commands: Vec::new(),
            allocator,
            pipeline_state: None,
            closed: false,
        }
    }

    pub fn close(&mut self) -> Result<(), DxgiError> {
        self.closed = true;
        Ok(())
    }

    pub fn reset(&mut self, allocator: u64, initial_state: Option<u64>) -> Result<(), DxgiError> {
        self.commands.clear();
        self.allocator = allocator;
        self.pipeline_state = initial_state;
        self.closed = false;
        Ok(())
    }

    pub fn draw_instanced(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        start_vertex: u32,
        start_instance: u32,
    ) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands.push(D3d12Command::DrawInstanced(
            vertex_count,
            instance_count,
            start_vertex,
            start_instance,
        ));
        Ok(())
    }

    pub fn draw_indexed_instanced(
        &mut self,
        index_count: u32,
        instance_count: u32,
        start_index: u32,
        base_vertex: i32,
        start_instance: u32,
    ) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands.push(D3d12Command::DrawIndexedInstanced(
            index_count,
            instance_count,
            start_index,
            base_vertex,
            start_instance,
        ));
        Ok(())
    }

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands.push(D3d12Command::Dispatch(x, y, z));
        Ok(())
    }

    pub fn clear_render_target_view(&mut self, rtv: u64, color: [f32; 4]) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands
            .push(D3d12Command::ClearRenderTargetView(rtv, color));
        Ok(())
    }

    pub fn clear_depth_stencil_view(
        &mut self,
        dsv: u64,
        depth: f32,
        stencil: u8,
    ) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands
            .push(D3d12Command::ClearDepthStencilView(dsv, depth, stencil));
        Ok(())
    }

    pub fn resource_barrier(
        &mut self,
        barriers: Vec<D3d12ResourceBarrier>,
    ) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands.push(D3d12Command::ResourceBarrier(barriers));
        Ok(())
    }

    pub fn set_graphics_root_signature(&mut self, sig: u64) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands
            .push(D3d12Command::SetGraphicsRootSignature(sig));
        Ok(())
    }

    pub fn set_pipeline_state(&mut self, pso: u64) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.pipeline_state = Some(pso);
        self.commands.push(D3d12Command::SetPipelineState(pso));
        Ok(())
    }

    pub fn set_graphics_root_32bit_constants(
        &mut self,
        root_index: u32,
        data: &[u32],
    ) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands
            .push(D3d12Command::SetGraphicsRoot32BitConstants(
                root_index,
                Vec::from(data),
            ));
        Ok(())
    }

    pub fn copy_resource(&mut self, dst: u64, src: u64) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands.push(D3d12Command::CopyResource(dst, src));
        Ok(())
    }

    pub fn copy_buffer_region(
        &mut self,
        dst: u64,
        dst_offset: u64,
        src: u64,
        src_offset: u64,
        num_bytes: u64,
    ) -> Result<(), DxgiError> {
        if self.closed {
            return Err(DxgiError::CommandListClosed);
        }
        self.commands.push(D3d12Command::CopyBufferRegion {
            dst,
            dst_offset,
            src,
            src_offset,
            num_bytes,
        });
        Ok(())
    }

    pub fn command_count(&self) -> usize {
        self.commands.len()
    }
}

/// Translate D3D12 command list to RaeGFX draw commands.
pub fn translate_d3d12_to_raegfx(commands: &[D3d12Command]) -> Vec<raegfx::DrawCommand> {
    let mut out = Vec::new();

    for cmd in commands {
        match cmd {
            D3d12Command::DrawInstanced(vc, ic, sv, si) => {
                out.push(raegfx::DrawCommand::Draw {
                    vertex_count: *vc,
                    instance_count: *ic,
                    first_vertex: *sv,
                    first_instance: *si,
                });
            }
            D3d12Command::DrawIndexedInstanced(ic, inst, si, bv, sinst) => {
                out.push(raegfx::DrawCommand::DrawIndexed {
                    index_count: *ic,
                    instance_count: *inst,
                    first_index: *si,
                    vertex_offset: *bv,
                    first_instance: *sinst,
                });
            }
            D3d12Command::Dispatch(x, y, z) => {
                out.push(raegfx::DrawCommand::Dispatch {
                    x: *x,
                    y: *y,
                    z: *z,
                });
            }
            D3d12Command::SetGraphicsRoot32BitConstants(_, data) => {
                let mut bytes = Vec::with_capacity(data.len() * 4);
                for word in data {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
                out.push(raegfx::DrawCommand::PushConstants {
                    offset: 0,
                    data: bytes,
                });
            }
            D3d12Command::ClearRenderTargetView(_, _)
            | D3d12Command::ClearDepthStencilView(_, _, _)
            | D3d12Command::ResourceBarrier(_)
            | D3d12Command::SetGraphicsRootSignature(_)
            | D3d12Command::SetComputeRootSignature(_)
            | D3d12Command::SetPipelineState(_)
            | D3d12Command::CopyResource(_, _)
            | D3d12Command::CopyBufferRegion { .. } => {}
        }
    }

    out
}

// ═══════════════════════════════════════════════════════════════════════════
// Translation Statistics
// ═══════════════════════════════════════════════════════════════════════════

pub struct TranslationStats {
    pub frames_translated: u64,
    pub draw_calls_translated: u64,
    pub shaders_compiled: u64,
    pub shader_cache_hits: u64,
    pub shader_cache_misses: u64,
    pub resource_creates: u64,
    pub resource_destroys: u64,
    pub api_calls_total: u64,
    pub errors: u64,
    pub avg_frame_translation_us: u64,
}

impl TranslationStats {
    pub fn new() -> Self {
        Self {
            frames_translated: 0,
            draw_calls_translated: 0,
            shaders_compiled: 0,
            shader_cache_hits: 0,
            shader_cache_misses: 0,
            resource_creates: 0,
            resource_destroys: 0,
            api_calls_total: 0,
            errors: 0,
            avg_frame_translation_us: 0,
        }
    }

    pub fn record_draw_call(&mut self) {
        self.draw_calls_translated += 1;
        self.api_calls_total += 1;
    }

    pub fn record_shader_compile(&mut self, cache_hit: bool) {
        self.shaders_compiled += 1;
        self.api_calls_total += 1;
        if cache_hit {
            self.shader_cache_hits += 1;
        } else {
            self.shader_cache_misses += 1;
        }
    }

    pub fn record_resource_create(&mut self) {
        self.resource_creates += 1;
        self.api_calls_total += 1;
    }

    pub fn record_resource_destroy(&mut self) {
        self.resource_destroys += 1;
        self.api_calls_total += 1;
    }

    pub fn record_frame(&mut self, translation_us: u64) {
        self.frames_translated += 1;
        if self.frames_translated == 1 {
            self.avg_frame_translation_us = translation_us;
        } else {
            // Exponential moving average (α ≈ 0.05)
            self.avg_frame_translation_us =
                (self.avg_frame_translation_us * 19 + translation_us) / 20;
        }
    }

    pub fn record_error(&mut self) {
        self.errors += 1;
    }

    pub fn shader_cache_hit_rate(&self) -> f32 {
        let total = self.shader_cache_hits + self.shader_cache_misses;
        if total == 0 {
            return 0.0;
        }
        self.shader_cache_hits as f32 / total as f32
    }

    pub fn live_resources(&self) -> u64 {
        self.resource_creates.saturating_sub(self.resource_destroys)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// High-level Translation Runtime
// ═══════════════════════════════════════════════════════════════════════════

pub struct DxgiTranslationRuntime {
    pub factory: DxgiFactory,
    pub d3d11_device: Option<D3d11Device>,
    pub d3d11_context: Option<D3d11DeviceContext>,
    pub d3d12_device: Option<D3d12Device>,
    pub swap_chain: Option<DxgiSwapChain>,
    pub shader_translator: ShaderTranslator,
    pub stats: TranslationStats,
}

impl DxgiTranslationRuntime {
    pub fn new() -> Self {
        Self {
            factory: DxgiFactory::new(0),
            d3d11_device: None,
            d3d11_context: None,
            d3d12_device: None,
            swap_chain: None,
            shader_translator: ShaderTranslator::new(),
            stats: TranslationStats::new(),
        }
    }

    pub fn create_d3d11_device(
        &mut self,
        feature_level: D3dFeatureLevel,
        flags: u32,
    ) -> Result<(), DxgiError> {
        let device = D3d11Device::new(feature_level, flags);
        let ctx = D3d11DeviceContext::new(0);
        self.d3d11_device = Some(device);
        self.d3d11_context = Some(ctx);
        Ok(())
    }

    pub fn create_d3d12_device(&mut self, feature_level: D3dFeatureLevel) -> Result<(), DxgiError> {
        self.d3d12_device = Some(D3d12Device::new(feature_level));
        Ok(())
    }

    pub fn create_swap_chain(
        &mut self,
        width: u32,
        height: u32,
        format: DxgiFormat,
        buffer_count: u32,
    ) -> Result<(), DxgiError> {
        self.swap_chain =
            Some(
                self.factory
                    .create_swap_chain(width, height, format, buffer_count)?,
            );
        Ok(())
    }

    pub fn present(&mut self, sync_interval: u32) -> Result<(), DxgiError> {
        let sc = self
            .swap_chain
            .as_mut()
            .ok_or(DxgiError::Unimplemented("no swap chain"))?;
        sc.present(sync_interval)?;
        self.stats.record_frame(0);
        Ok(())
    }

    pub fn flush_d3d11(&mut self) -> Result<Vec<raegfx::DrawCommand>, DxgiError> {
        let ctx = self
            .d3d11_context
            .as_mut()
            .ok_or(DxgiError::Unimplemented("no D3D11 context"))?;
        let commands = ctx.flush();

        for cmd in &commands {
            match cmd {
                D3d11Command::Draw(_, _)
                | D3d11Command::DrawIndexed(_, _, _)
                | D3d11Command::DrawInstanced(_, _, _, _)
                | D3d11Command::DrawIndexedInstanced(_, _, _, _, _)
                | D3d11Command::Dispatch(_, _, _) => {
                    self.stats.record_draw_call();
                }
                _ => {
                    self.stats.api_calls_total += 1;
                }
            }
        }

        let raegfx_commands = translate_d3d11_to_raegfx(&commands, &ctx.state);
        Ok(raegfx_commands)
    }
}
