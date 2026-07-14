//! Direct3D 12 API emulation for RaeBridge.
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{HResult, WinHandle};

// ---------------------------------------------------------------------------
// HRESULT constants
// ---------------------------------------------------------------------------

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_FAIL: i32 = -2147467259; // 0x80004005
pub const E_INVALIDARG: i32 = -2147024809; // 0x80070057
pub const E_OUTOFMEMORY: i32 = -2147024882; // 0x8007000E
pub const DXGI_ERROR_DEVICE_REMOVED: i32 = -2005270523; // 0x887A0005
pub const DXGI_ERROR_DEVICE_RESET: i32 = -2005270521; // 0x887A0007

// ---------------------------------------------------------------------------
// Resource states
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum D3D12ResourceState {
    Common = 0,
    VertexAndConstantBuffer = 0x1,
    IndexBuffer = 0x2,
    RenderTarget = 0x4,
    UnorderedAccess = 0x8,
    DepthWrite = 0x10,
    DepthRead = 0x20,
    NonPixelShaderResource = 0x40,
    PixelShaderResource = 0x80,
    StreamOut = 0x100,
    IndirectArgument = 0x200,
    CopyDest = 0x400,
    CopySource = 0x800,
    ResolveDest = 0x1000,
    ResolveSource = 0x2000,
    GenericRead = 0x1 | 0x2 | 0x40 | 0x80 | 0x200 | 0x800,
    Present = 0x4000,
    Predication = 0x8000,
}

// ---------------------------------------------------------------------------
// Resource dimension & types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12ResourceDimension {
    Unknown,
    Buffer,
    Texture1D,
    Texture2D,
    Texture3D,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12TextureLayout {
    Unknown,
    RowMajor,
    UndefinedSwizzle64KB,
    StandardSwizzle64KB,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D12ResourceFlags {
    None = 0,
    AllowRenderTarget = 0x1,
    AllowDepthStencil = 0x2,
    AllowUnorderedAccess = 0x4,
    DenyShaderResource = 0x8,
    AllowCrossAdapter = 0x10,
    AllowSimultaneousAccess = 0x20,
}

#[derive(Debug, Clone)]
pub struct D3D12ResourceDesc {
    pub dimension: D3D12ResourceDimension,
    pub alignment: u64,
    pub width: u64,
    pub height: u32,
    pub depth_or_array_size: u16,
    pub mip_levels: u16,
    pub format: DxgiFormat,
    pub sample_count: u32,
    pub sample_quality: u32,
    pub layout: D3D12TextureLayout,
    pub flags: u32,
}

// ---------------------------------------------------------------------------
// DXGI format (subset)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum DxgiFormat {
    Unknown = 0,
    R32G32B32A32Float = 2,
    R32G32B32A32Uint = 3,
    R32G32B32Float = 6,
    R16G16B16A16Float = 10,
    R16G16B16A16Unorm = 11,
    R32G32Float = 16,
    R32G32Uint = 17,
    R10G10B10A2Unorm = 24,
    R11G11B10Float = 26,
    R8G8B8A8Unorm = 28,
    R8G8B8A8UnormSrgb = 29,
    R16G16Float = 34,
    R16G16Unorm = 35,
    R32Float = 41,
    R32Uint = 42,
    D32Float = 40,
    R8G8Unorm = 49,
    R16Float = 54,
    R16Unorm = 56,
    R8Unorm = 61,
    D16Unorm = 55,
    D24UnormS8Uint = 45,
    D32FloatS8X24Uint = 20,
    B8G8R8A8Unorm = 87,
    B8G8R8A8UnormSrgb = 91,
}

// ---------------------------------------------------------------------------
// Heap types and tiers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12HeapType {
    Default,
    Upload,
    Readback,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12HeapTier {
    Tier1,
    Tier2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12CpuPageProperty {
    Unknown,
    NotAvailable,
    WriteCombine,
    WriteBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12MemoryPool {
    Unknown,
    L0,
    L1,
}

#[derive(Debug, Clone)]
pub struct D3D12HeapProperties {
    pub heap_type: D3D12HeapType,
    pub cpu_page_property: D3D12CpuPageProperty,
    pub memory_pool: D3D12MemoryPool,
    pub creation_node_mask: u32,
    pub visible_node_mask: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D12HeapFlags {
    None = 0,
    Shared = 0x1,
    DenyBuffers = 0x4,
    AllowDisplay = 0x8,
    SharedCrossAdapter = 0x20,
    DenyRtDsTextures = 0x40,
    DenyNonRtDsTextures = 0x80,
    AllowAllBuffersAndTextures = 0x100,
    AllowOnlyBuffers = 0xC0,
    AllowOnlyNonRtDsTextures = 0x44,
    AllowOnlyRtDsTextures = 0x84,
}

// ---------------------------------------------------------------------------
// Descriptor heaps
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum D3D12DescriptorHeapType {
    CbvSrvUav,
    Sampler,
    Rtv,
    Dsv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D12DescriptorHeapFlags {
    None = 0,
    ShaderVisible = 0x1,
}

#[derive(Debug, Clone)]
pub struct D3D12DescriptorHeapDesc {
    pub heap_type: D3D12DescriptorHeapType,
    pub num_descriptors: u32,
    pub flags: u32,
    pub node_mask: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct D3D12CpuDescriptorHandle {
    pub ptr: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct D3D12GpuDescriptorHandle {
    pub ptr: u64,
}

#[derive(Debug, Clone)]
pub struct DescriptorHeap {
    pub desc: D3D12DescriptorHeapDesc,
    pub handle: WinHandle,
    pub cpu_start: D3D12CpuDescriptorHandle,
    pub gpu_start: D3D12GpuDescriptorHandle,
    pub increment_size: u32,
    pub allocated_count: u32,
}

// ---------------------------------------------------------------------------
// Descriptor ranges & root signature
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12DescriptorRangeType {
    Srv,
    Uav,
    Cbv,
    Sampler,
}

#[derive(Debug, Clone)]
pub struct D3D12DescriptorRange {
    pub range_type: D3D12DescriptorRangeType,
    pub num_descriptors: u32,
    pub base_shader_register: u32,
    pub register_space: u32,
    pub offset_in_descriptors_from_table_start: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12RootParameterType {
    DescriptorTable,
    Constants32Bit,
    Cbv,
    Srv,
    Uav,
}

#[derive(Debug, Clone)]
pub struct D3D12RootParameter {
    pub parameter_type: D3D12RootParameterType,
    pub shader_visibility: D3D12ShaderVisibility,
    pub descriptor_table_ranges: Vec<D3D12DescriptorRange>,
    pub constants_shader_register: u32,
    pub constants_register_space: u32,
    pub constants_num_32bit_values: u32,
    pub descriptor_shader_register: u32,
    pub descriptor_register_space: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12ShaderVisibility {
    All,
    Vertex,
    Hull,
    Domain,
    Geometry,
    Pixel,
    Amplification,
    Mesh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12Filter {
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
pub enum D3D12TextureAddressMode {
    Wrap,
    Mirror,
    Clamp,
    Border,
    MirrorOnce,
}

#[derive(Debug, Clone)]
pub struct D3D12StaticSamplerDesc {
    pub filter: D3D12Filter,
    pub address_u: D3D12TextureAddressMode,
    pub address_v: D3D12TextureAddressMode,
    pub address_w: D3D12TextureAddressMode,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison_func: D3D12ComparisonFunc,
    pub border_color: D3D12StaticBorderColor,
    pub min_lod: f32,
    pub max_lod: f32,
    pub shader_register: u32,
    pub register_space: u32,
    pub shader_visibility: D3D12ShaderVisibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12ComparisonFunc {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12StaticBorderColor {
    TransparentBlack,
    OpaqueBlack,
    OpaqueWhite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D12RootSignatureFlags {
    None = 0,
    AllowInputAssemblerInputLayout = 0x1,
    DenyVertexShaderRootAccess = 0x2,
    DenyHullShaderRootAccess = 0x4,
    DenyDomainShaderRootAccess = 0x8,
    DenyGeometryShaderRootAccess = 0x10,
    DenyPixelShaderRootAccess = 0x20,
    AllowStreamOutput = 0x40,
    LocalRootSignature = 0x80,
}

#[derive(Debug, Clone)]
pub struct D3D12RootSignatureDesc {
    pub parameters: Vec<D3D12RootParameter>,
    pub static_samplers: Vec<D3D12StaticSamplerDesc>,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct RootSignature {
    pub handle: WinHandle,
    pub desc: D3D12RootSignatureDesc,
    pub serialized_blob: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Pipeline state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12PrimitiveTopologyType {
    Undefined,
    Point,
    Line,
    Triangle,
    Patch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12PrimitiveTopology {
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
    ControlPointPatchList(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12InputClassification {
    PerVertexData,
    PerInstanceData,
}

#[derive(Debug, Clone)]
pub struct D3D12InputElementDesc {
    pub semantic_name: &'static str,
    pub semantic_index: u32,
    pub format: DxgiFormat,
    pub input_slot: u32,
    pub aligned_byte_offset: u32,
    pub input_slot_class: D3D12InputClassification,
    pub instance_data_step_rate: u32,
}

#[derive(Debug, Clone)]
pub struct D3D12ShaderBytecode {
    pub bytecode: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12Blend {
    Zero,
    One,
    SrcColor,
    InvSrcColor,
    SrcAlpha,
    InvSrcAlpha,
    DestAlpha,
    InvDestAlpha,
    DestColor,
    InvDestColor,
    SrcAlphaSat,
    BlendFactor,
    InvBlendFactor,
    Src1Color,
    InvSrc1Color,
    Src1Alpha,
    InvSrc1Alpha,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12BlendOp {
    Add,
    Subtract,
    RevSubtract,
    Min,
    Max,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12RenderTargetBlendDesc {
    pub blend_enable: bool,
    pub logic_op_enable: bool,
    pub src_blend: D3D12Blend,
    pub dest_blend: D3D12Blend,
    pub blend_op: D3D12BlendOp,
    pub src_blend_alpha: D3D12Blend,
    pub dest_blend_alpha: D3D12Blend,
    pub blend_op_alpha: D3D12BlendOp,
    pub render_target_write_mask: u8,
}

#[derive(Debug, Clone)]
pub struct D3D12BlendDesc {
    pub alpha_to_coverage_enable: bool,
    pub independent_blend_enable: bool,
    pub render_target: [D3D12RenderTargetBlendDesc; 8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12FillMode {
    Wireframe,
    Solid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12CullMode {
    None,
    Front,
    Back,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12RasterizerDesc {
    pub fill_mode: D3D12FillMode,
    pub cull_mode: D3D12CullMode,
    pub front_counter_clockwise: bool,
    pub depth_bias: i32,
    pub depth_bias_clamp: f32,
    pub slope_scaled_depth_bias: f32,
    pub depth_clip_enable: bool,
    pub multisample_enable: bool,
    pub antialiased_line_enable: bool,
    pub forced_sample_count: u32,
    pub conservative_raster: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12DepthWriteMask {
    Zero,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12StencilOp {
    Keep,
    Zero,
    Replace,
    IncrSat,
    DecrSat,
    Invert,
    Incr,
    Decr,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12DepthStencilOpDesc {
    pub stencil_fail_op: D3D12StencilOp,
    pub stencil_depth_fail_op: D3D12StencilOp,
    pub stencil_pass_op: D3D12StencilOp,
    pub stencil_func: D3D12ComparisonFunc,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12DepthStencilDesc {
    pub depth_enable: bool,
    pub depth_write_mask: D3D12DepthWriteMask,
    pub depth_func: D3D12ComparisonFunc,
    pub stencil_enable: bool,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
    pub front_face: D3D12DepthStencilOpDesc,
    pub back_face: D3D12DepthStencilOpDesc,
}

#[derive(Debug, Clone)]
pub struct D3D12GraphicsPipelineStateDesc {
    pub root_signature: WinHandle,
    pub vs: D3D12ShaderBytecode,
    pub ps: D3D12ShaderBytecode,
    pub ds: Option<D3D12ShaderBytecode>,
    pub hs: Option<D3D12ShaderBytecode>,
    pub gs: Option<D3D12ShaderBytecode>,
    pub blend_state: D3D12BlendDesc,
    pub sample_mask: u32,
    pub rasterizer_state: D3D12RasterizerDesc,
    pub depth_stencil_state: D3D12DepthStencilDesc,
    pub input_layout: Vec<D3D12InputElementDesc>,
    pub primitive_topology_type: D3D12PrimitiveTopologyType,
    pub num_render_targets: u32,
    pub rtv_formats: [DxgiFormat; 8],
    pub dsv_format: DxgiFormat,
    pub sample_desc_count: u32,
    pub sample_desc_quality: u32,
}

#[derive(Debug, Clone)]
pub struct D3D12ComputePipelineStateDesc {
    pub root_signature: WinHandle,
    pub cs: D3D12ShaderBytecode,
}

#[derive(Debug, Clone)]
pub struct PipelineState {
    pub handle: WinHandle,
    pub is_compute: bool,
}

// ---------------------------------------------------------------------------
// Resource barriers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12ResourceBarrierType {
    Transition,
    Aliasing,
    Uav,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum D3D12ResourceBarrierFlags {
    None = 0,
    BeginOnly = 0x1,
    EndOnly = 0x2,
}

#[derive(Debug, Clone)]
pub struct D3D12ResourceTransitionBarrier {
    pub resource: WinHandle,
    pub subresource: u32,
    pub state_before: D3D12ResourceState,
    pub state_after: D3D12ResourceState,
}

#[derive(Debug, Clone)]
pub struct D3D12ResourceAliasingBarrier {
    pub resource_before: WinHandle,
    pub resource_after: WinHandle,
}

#[derive(Debug, Clone)]
pub struct D3D12ResourceUavBarrier {
    pub resource: WinHandle,
}

#[derive(Debug, Clone)]
pub struct D3D12ResourceBarrier {
    pub barrier_type: D3D12ResourceBarrierType,
    pub flags: u32,
    pub transition: Option<D3D12ResourceTransitionBarrier>,
    pub aliasing: Option<D3D12ResourceAliasingBarrier>,
    pub uav: Option<D3D12ResourceUavBarrier>,
}

// ---------------------------------------------------------------------------
// Viewport / Scissor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct D3D12Viewport {
    pub top_left_x: f32,
    pub top_left_y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

// ---------------------------------------------------------------------------
// Vertex / Index buffer views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct D3D12VertexBufferView {
    pub buffer_location: u64,
    pub size_in_bytes: u32,
    pub stride_in_bytes: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12IndexBufferView {
    pub buffer_location: u64,
    pub size_in_bytes: u32,
    pub format: DxgiFormat,
}

// ---------------------------------------------------------------------------
// Command queue & types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12CommandListType {
    Direct,
    Bundle,
    Compute,
    Copy,
    VideoDecode,
    VideoProcess,
    VideoEncode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12CommandQueuePriority {
    Normal,
    High,
    GlobalRealtime,
}

#[derive(Debug, Clone)]
pub struct D3D12CommandQueueDesc {
    pub queue_type: D3D12CommandListType,
    pub priority: D3D12CommandQueuePriority,
    pub flags: u32,
    pub node_mask: u32,
}

// ---------------------------------------------------------------------------
// Fence
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct D3D12Fence {
    pub handle: WinHandle,
    pub value: AtomicU64,
    pub is_signaled: AtomicBool,
}

impl D3D12Fence {
    pub fn new(handle: WinHandle, initial_value: u64) -> Self {
        Self {
            handle,
            value: AtomicU64::new(initial_value),
            is_signaled: AtomicBool::new(false),
        }
    }

    pub fn get_completed_value(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }

    pub fn signal(&self, value: u64) {
        self.value.store(value, Ordering::Release);
        self.is_signaled.store(true, Ordering::Release);
    }

    pub fn wait(&self, value: u64) -> HResult {
        if self.value.load(Ordering::Acquire) >= value {
            return HResult(S_OK);
        }
        // In emulation, spin or return pending
        HResult(S_OK)
    }
}

// ---------------------------------------------------------------------------
// Query heap
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12QueryHeapType {
    Occlusion,
    Timestamp,
    PipelineStatistics,
    SOStatistics,
}

#[derive(Debug, Clone)]
pub struct D3D12QueryHeapDesc {
    pub heap_type: D3D12QueryHeapType,
    pub count: u32,
    pub node_mask: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12QueryType {
    Occlusion,
    BinaryOcclusion,
    Timestamp,
    PipelineStatistics,
    SOStatisticsStream0,
    SOStatisticsStream1,
    SOStatisticsStream2,
    SOStatisticsStream3,
}

// ---------------------------------------------------------------------------
// Command signature
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12IndirectArgumentType {
    Draw,
    DrawIndexed,
    Dispatch,
    VertexBufferView,
    IndexBufferView,
    Constant,
    ConstantBufferView,
    ShaderResourceView,
    UnorderedAccessView,
}

#[derive(Debug, Clone)]
pub struct D3D12IndirectArgumentDesc {
    pub arg_type: D3D12IndirectArgumentType,
    pub root_parameter_index: u32,
    pub dest_offset_in_32bit_values: u32,
    pub num_32bit_values_to_set: u32,
}

#[derive(Debug, Clone)]
pub struct D3D12CommandSignatureDesc {
    pub byte_stride: u32,
    pub argument_descs: Vec<D3D12IndirectArgumentDesc>,
    pub node_mask: u32,
}

// ---------------------------------------------------------------------------
// Feature support
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12Feature {
    D3D12Options,
    Architecture,
    FeatureLevels,
    FormatSupport,
    MultisampleQualityLevels,
    FormatInfo,
    GpuVirtualAddressSupport,
    ShaderModel,
    D3D12Options1,
    RootSignature,
    Architecture1,
    D3D12Options2,
    ShaderCache,
    CommandQueuePriority,
    D3D12Options3,
    ExistingHeaps,
    D3D12Options4,
    Serialization,
    CrossNode,
    D3D12Options5,
    D3D12Options6,
    D3D12Options7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12ShaderModel {
    Sm5_1,
    Sm6_0,
    Sm6_1,
    Sm6_2,
    Sm6_3,
    Sm6_4,
    Sm6_5,
    Sm6_6,
    Sm6_7,
}

#[derive(Debug, Clone)]
pub struct D3D12FeatureDataOptions {
    pub double_precision_float_shader_ops: bool,
    pub output_merger_logic_op: bool,
    pub min_precision_support: u32,
    pub tiled_resources_tier: u32,
    pub resource_binding_tier: u32,
    pub ps_specified_stencil_ref_supported: bool,
    pub typed_uav_load_additional_formats: bool,
    pub rovs_supported: bool,
    pub conservative_rasterization_tier: u32,
    pub max_gpu_virtual_address_bits_per_resource: u32,
    pub standard_swizzle_64kb_supported: bool,
    pub cross_node_sharing_tier: u32,
    pub cross_adapter_row_major_texture_supported: bool,
    pub vp_and_rt_array_index_from_any_shader: bool,
    pub resource_heap_tier: u32,
}

// ---------------------------------------------------------------------------
// Resource allocation info
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct D3D12ResourceAllocationInfo {
    pub size_in_bytes: u64,
    pub alignment: u64,
}

// ---------------------------------------------------------------------------
// Memory budget
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct D3D12MemoryBudget {
    pub budget: u64,
    pub current_usage: u64,
    pub available_for_reservation: u64,
    pub current_reservation: u64,
}

// ---------------------------------------------------------------------------
// Resource object
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct D3D12Resource {
    pub handle: WinHandle,
    pub desc: D3D12ResourceDesc,
    pub current_state: D3D12ResourceState,
    pub heap_type: D3D12HeapType,
    pub gpu_virtual_address: u64,
    pub size_in_bytes: u64,
    pub mapped_ptr: Option<u64>,
    pub is_committed: bool,
    pub is_placed: bool,
    pub is_reserved: bool,
}

// ---------------------------------------------------------------------------
// ID3D12CommandAllocator
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CommandAllocator {
    pub handle: WinHandle,
    pub list_type: D3D12CommandListType,
    pub is_reset: bool,
}

// ---------------------------------------------------------------------------
// ID3D12GraphicsCommandList
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GraphicsCommandList {
    pub handle: WinHandle,
    pub list_type: D3D12CommandListType,
    pub allocator: WinHandle,
    pub pipeline_state: Option<WinHandle>,
    pub root_signature_graphics: Option<WinHandle>,
    pub root_signature_compute: Option<WinHandle>,
    pub is_recording: bool,
    pub is_closed: bool,
    pub viewports: Vec<D3D12Viewport>,
    pub scissor_rects: Vec<D3D12Rect>,
    pub render_targets: Vec<D3D12CpuDescriptorHandle>,
    pub depth_stencil: Option<D3D12CpuDescriptorHandle>,
    pub primitive_topology: D3D12PrimitiveTopology,
    pub vertex_buffers: Vec<D3D12VertexBufferView>,
    pub index_buffer: Option<D3D12IndexBufferView>,
    pub blend_factor: [f32; 4],
    pub stencil_ref: u32,
    pub descriptor_heaps: Vec<WinHandle>,
    pub recorded_commands: u64,
}

impl GraphicsCommandList {
    pub fn reset(&mut self, allocator: WinHandle, initial_state: Option<WinHandle>) -> HResult {
        self.allocator = allocator;
        self.pipeline_state = initial_state;
        self.is_recording = true;
        self.is_closed = false;
        self.recorded_commands = 0;
        self.viewports.clear();
        self.scissor_rects.clear();
        self.render_targets.clear();
        self.depth_stencil = None;
        self.vertex_buffers.clear();
        self.index_buffer = None;
        self.descriptor_heaps.clear();
        HResult(S_OK)
    }

    pub fn close(&mut self) -> HResult {
        if !self.is_recording {
            return HResult(E_FAIL);
        }
        self.is_closed = true;
        self.is_recording = false;
        HResult(S_OK)
    }

    pub fn clear_render_target_view(
        &mut self,
        rtv: D3D12CpuDescriptorHandle,
        _color: [f32; 4],
        _num_rects: u32,
        _rects: &[D3D12Rect],
    ) {
        let _ = rtv;
        self.recorded_commands += 1;
    }

    pub fn clear_depth_stencil_view(
        &mut self,
        dsv: D3D12CpuDescriptorHandle,
        _clear_flags: u32,
        _depth: f32,
        _stencil: u8,
        _num_rects: u32,
        _rects: &[D3D12Rect],
    ) {
        let _ = dsv;
        self.recorded_commands += 1;
    }

    pub fn draw_instanced(
        &mut self,
        vertex_count_per_instance: u32,
        instance_count: u32,
        start_vertex_location: u32,
        start_instance_location: u32,
    ) {
        let _ = (
            vertex_count_per_instance,
            instance_count,
            start_vertex_location,
            start_instance_location,
        );
        self.recorded_commands += 1;
    }

    pub fn draw_indexed_instanced(
        &mut self,
        index_count_per_instance: u32,
        instance_count: u32,
        start_index_location: u32,
        base_vertex_location: i32,
        start_instance_location: u32,
    ) {
        let _ = (
            index_count_per_instance,
            instance_count,
            start_index_location,
            base_vertex_location,
            start_instance_location,
        );
        self.recorded_commands += 1;
    }

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        let _ = (x, y, z);
        self.recorded_commands += 1;
    }

    pub fn copy_resource(&mut self, dst: WinHandle, src: WinHandle) {
        let _ = (dst, src);
        self.recorded_commands += 1;
    }

    pub fn copy_buffer_region(
        &mut self,
        dst: WinHandle,
        dst_offset: u64,
        src: WinHandle,
        src_offset: u64,
        num_bytes: u64,
    ) {
        let _ = (dst, dst_offset, src, src_offset, num_bytes);
        self.recorded_commands += 1;
    }

    pub fn copy_texture_region(
        &mut self,
        dst_location: &D3D12TextureCopyLocation,
        dst_x: u32,
        dst_y: u32,
        dst_z: u32,
        src_location: &D3D12TextureCopyLocation,
        src_box: Option<&D3D12Box>,
    ) {
        let _ = (dst_location, dst_x, dst_y, dst_z, src_location, src_box);
        self.recorded_commands += 1;
    }

    pub fn resource_barrier(&mut self, barriers: &[D3D12ResourceBarrier]) {
        let _ = barriers;
        self.recorded_commands += 1;
    }

    pub fn set_graphics_root_signature(&mut self, root_sig: WinHandle) {
        self.root_signature_graphics = Some(root_sig);
        self.recorded_commands += 1;
    }

    pub fn set_compute_root_signature(&mut self, root_sig: WinHandle) {
        self.root_signature_compute = Some(root_sig);
        self.recorded_commands += 1;
    }

    pub fn set_graphics_root_32bit_constants(
        &mut self,
        _root_parameter_index: u32,
        _num_32bit_values: u32,
        _src_data: u64,
        _dest_offset: u32,
    ) {
        self.recorded_commands += 1;
    }

    pub fn set_graphics_root_constant_buffer_view(&mut self, _index: u32, _gpu_addr: u64) {
        self.recorded_commands += 1;
    }

    pub fn set_graphics_root_shader_resource_view(&mut self, _index: u32, _gpu_addr: u64) {
        self.recorded_commands += 1;
    }

    pub fn set_graphics_root_unordered_access_view(&mut self, _index: u32, _gpu_addr: u64) {
        self.recorded_commands += 1;
    }

    pub fn set_graphics_root_descriptor_table(
        &mut self,
        _index: u32,
        _base_descriptor: D3D12GpuDescriptorHandle,
    ) {
        self.recorded_commands += 1;
    }

    pub fn ia_set_primitive_topology(&mut self, topology: D3D12PrimitiveTopology) {
        self.primitive_topology = topology;
        self.recorded_commands += 1;
    }

    pub fn ia_set_vertex_buffers(&mut self, start_slot: u32, views: &[D3D12VertexBufferView]) {
        let _ = start_slot;
        self.vertex_buffers = views.to_vec();
        self.recorded_commands += 1;
    }

    pub fn ia_set_index_buffer(&mut self, view: Option<&D3D12IndexBufferView>) {
        self.index_buffer = view.copied();
        self.recorded_commands += 1;
    }

    pub fn rs_set_viewports(&mut self, viewports: &[D3D12Viewport]) {
        self.viewports = viewports.to_vec();
        self.recorded_commands += 1;
    }

    pub fn rs_set_scissor_rects(&mut self, rects: &[D3D12Rect]) {
        self.scissor_rects = rects.to_vec();
        self.recorded_commands += 1;
    }

    pub fn om_set_render_targets(
        &mut self,
        render_target_descriptors: &[D3D12CpuDescriptorHandle],
        rts_single_handle_to_descriptor_range: bool,
        depth_stencil_descriptor: Option<&D3D12CpuDescriptorHandle>,
    ) {
        let _ = rts_single_handle_to_descriptor_range;
        self.render_targets = render_target_descriptors.to_vec();
        self.depth_stencil = depth_stencil_descriptor.copied();
        self.recorded_commands += 1;
    }

    pub fn om_set_blend_factor(&mut self, blend_factor: [f32; 4]) {
        self.blend_factor = blend_factor;
        self.recorded_commands += 1;
    }

    pub fn om_set_stencil_ref(&mut self, stencil_ref: u32) {
        self.stencil_ref = stencil_ref;
        self.recorded_commands += 1;
    }

    pub fn set_pipeline_state(&mut self, pso: WinHandle) {
        self.pipeline_state = Some(pso);
        self.recorded_commands += 1;
    }

    pub fn set_descriptor_heaps(&mut self, heaps: &[WinHandle]) {
        self.descriptor_heaps = heaps.to_vec();
        self.recorded_commands += 1;
    }

    pub fn so_set_targets(&mut self, _start_slot: u32, _views: &[D3D12StreamOutputBufferView]) {
        self.recorded_commands += 1;
    }

    pub fn begin_query(
        &mut self,
        _query_heap: WinHandle,
        _query_type: D3D12QueryType,
        _index: u32,
    ) {
        self.recorded_commands += 1;
    }

    pub fn end_query(&mut self, _query_heap: WinHandle, _query_type: D3D12QueryType, _index: u32) {
        self.recorded_commands += 1;
    }

    pub fn resolve_query_data(
        &mut self,
        _query_heap: WinHandle,
        _query_type: D3D12QueryType,
        _start_index: u32,
        _num_queries: u32,
        _dest_buffer: WinHandle,
        _aligned_dest_offset: u64,
    ) {
        self.recorded_commands += 1;
    }

    pub fn execute_indirect(
        &mut self,
        _command_signature: WinHandle,
        _max_command_count: u32,
        _argument_buffer: WinHandle,
        _argument_buffer_offset: u64,
        _count_buffer: Option<WinHandle>,
        _count_buffer_offset: u64,
    ) {
        self.recorded_commands += 1;
    }

    pub fn set_predication(
        &mut self,
        _buffer: Option<WinHandle>,
        _aligned_buffer_offset: u64,
        _operation: D3D12PredicationOp,
    ) {
        self.recorded_commands += 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12PredicationOp {
    EqualZero,
    NotEqualZero,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12StreamOutputBufferView {
    pub buffer_location: u64,
    pub size_in_bytes: u64,
    pub buffer_filled_size_location: u64,
}

#[derive(Debug, Clone)]
pub struct D3D12TextureCopyLocation {
    pub resource: WinHandle,
    pub subresource_index: u32,
    pub placed_footprint: Option<D3D12PlacedSubresourceFootprint>,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12PlacedSubresourceFootprint {
    pub offset: u64,
    pub format: DxgiFormat,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub row_pitch: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12Box {
    pub left: u32,
    pub top: u32,
    pub front: u32,
    pub right: u32,
    pub bottom: u32,
    pub back: u32,
}

// ---------------------------------------------------------------------------
// ID3D12CommandQueue
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct CommandQueue {
    pub handle: WinHandle,
    pub desc: D3D12CommandQueueDesc,
    pub submitted_lists: u64,
    pub timestamp_frequency: u64,
}

impl CommandQueue {
    pub fn execute_command_lists(&mut self, command_lists: &[WinHandle]) {
        self.submitted_lists += command_lists.len() as u64;
    }

    pub fn signal(&self, fence: &D3D12Fence, value: u64) -> HResult {
        fence.signal(value);
        HResult(S_OK)
    }

    pub fn wait(&self, fence: &D3D12Fence, value: u64) -> HResult {
        fence.wait(value)
    }

    pub fn get_timestamp_frequency(&self) -> u64 {
        self.timestamp_frequency
    }
}

// ---------------------------------------------------------------------------
// ID3D12Device
// ---------------------------------------------------------------------------

pub struct D3D12Device {
    pub handle: WinHandle,
    pub adapter_description: String,
    pub feature_level: u32,
    pub node_count: u32,
    pub heap_tier: D3D12HeapTier,
    pub resource_binding_tier: u32,
    pub next_handle: u64,
    pub resources: BTreeMap<u64, D3D12Resource>,
    pub descriptor_heaps: BTreeMap<u64, DescriptorHeap>,
    pub command_allocators: BTreeMap<u64, CommandAllocator>,
    pub command_lists: BTreeMap<u64, GraphicsCommandList>,
    pub command_queues: BTreeMap<u64, CommandQueue>,
    pub root_signatures: BTreeMap<u64, RootSignature>,
    pub pipeline_states: BTreeMap<u64, PipelineState>,
    pub fences: BTreeMap<u64, D3D12Fence>,
    pub query_heaps: BTreeMap<u64, D3D12QueryHeapDesc>,
    pub command_signatures: BTreeMap<u64, D3D12CommandSignatureDesc>,
    pub memory_budget: D3D12MemoryBudget,
    pub resident_resources: Vec<WinHandle>,
}

impl D3D12Device {
    fn alloc_handle(&mut self) -> WinHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        WinHandle(h)
    }

    pub fn create_command_queue(&mut self, desc: D3D12CommandQueueDesc) -> HResult {
        let handle = self.alloc_handle();
        self.command_queues.insert(
            handle.0,
            CommandQueue {
                handle,
                desc,
                submitted_lists: 0,
                timestamp_frequency: 10_000_000,
            },
        );
        HResult(S_OK)
    }

    pub fn create_command_allocator(&mut self, list_type: D3D12CommandListType) -> HResult {
        let handle = self.alloc_handle();
        self.command_allocators.insert(
            handle.0,
            CommandAllocator {
                handle,
                list_type,
                is_reset: true,
            },
        );
        HResult(S_OK)
    }

    pub fn create_graphics_pipeline_state(
        &mut self,
        _desc: &D3D12GraphicsPipelineStateDesc,
    ) -> HResult {
        let handle = self.alloc_handle();
        self.pipeline_states.insert(
            handle.0,
            PipelineState {
                handle,
                is_compute: false,
            },
        );
        HResult(S_OK)
    }

    pub fn create_compute_pipeline_state(
        &mut self,
        _desc: &D3D12ComputePipelineStateDesc,
    ) -> HResult {
        let handle = self.alloc_handle();
        self.pipeline_states.insert(
            handle.0,
            PipelineState {
                handle,
                is_compute: true,
            },
        );
        HResult(S_OK)
    }

    pub fn create_command_list(
        &mut self,
        node_mask: u32,
        list_type: D3D12CommandListType,
        allocator: WinHandle,
        initial_state: Option<WinHandle>,
    ) -> HResult {
        let _ = node_mask;
        let handle = self.alloc_handle();
        self.command_lists.insert(
            handle.0,
            GraphicsCommandList {
                handle,
                list_type,
                allocator,
                pipeline_state: initial_state,
                root_signature_graphics: None,
                root_signature_compute: None,
                is_recording: true,
                is_closed: false,
                viewports: Vec::new(),
                scissor_rects: Vec::new(),
                render_targets: Vec::new(),
                depth_stencil: None,
                primitive_topology: D3D12PrimitiveTopology::Undefined,
                vertex_buffers: Vec::new(),
                index_buffer: None,
                blend_factor: [1.0; 4],
                stencil_ref: 0,
                descriptor_heaps: Vec::new(),
                recorded_commands: 0,
            },
        );
        HResult(S_OK)
    }

    pub fn create_descriptor_heap(&mut self, desc: D3D12DescriptorHeapDesc) -> HResult {
        let handle = self.alloc_handle();
        let increment = match desc.heap_type {
            D3D12DescriptorHeapType::CbvSrvUav => 32,
            D3D12DescriptorHeapType::Sampler => 32,
            D3D12DescriptorHeapType::Rtv => 32,
            D3D12DescriptorHeapType::Dsv => 8,
        };
        let cpu_start = D3D12CpuDescriptorHandle {
            ptr: handle.0 * 0x10000,
        };
        let gpu_start = if desc.flags & (D3D12DescriptorHeapFlags::ShaderVisible as u32) != 0 {
            D3D12GpuDescriptorHandle {
                ptr: handle.0 * 0x10000,
            }
        } else {
            D3D12GpuDescriptorHandle { ptr: 0 }
        };
        self.descriptor_heaps.insert(
            handle.0,
            DescriptorHeap {
                desc,
                handle,
                cpu_start,
                gpu_start,
                increment_size: increment,
                allocated_count: 0,
            },
        );
        HResult(S_OK)
    }

    pub fn create_render_target_view(
        &mut self,
        _resource: WinHandle,
        _desc: Option<&D3D12RenderTargetViewDesc>,
        _dest_descriptor: D3D12CpuDescriptorHandle,
    ) {
    }

    pub fn create_depth_stencil_view(
        &mut self,
        _resource: WinHandle,
        _desc: Option<&D3D12DepthStencilViewDesc>,
        _dest_descriptor: D3D12CpuDescriptorHandle,
    ) {
    }

    pub fn create_shader_resource_view(
        &mut self,
        _resource: WinHandle,
        _desc: Option<&D3D12ShaderResourceViewDesc>,
        _dest_descriptor: D3D12CpuDescriptorHandle,
    ) {
    }

    pub fn create_unordered_access_view(
        &mut self,
        _resource: WinHandle,
        _counter_resource: Option<WinHandle>,
        _desc: Option<&D3D12UnorderedAccessViewDesc>,
        _dest_descriptor: D3D12CpuDescriptorHandle,
    ) {
    }

    pub fn create_constant_buffer_view(
        &mut self,
        _desc: &D3D12ConstantBufferViewDesc,
        _dest_descriptor: D3D12CpuDescriptorHandle,
    ) {
    }

    pub fn create_sampler(
        &mut self,
        _desc: &D3D12SamplerDesc,
        _dest_descriptor: D3D12CpuDescriptorHandle,
    ) {
    }

    pub fn create_root_signature(&mut self, _node_mask: u32, blob: &[u8]) -> HResult {
        let handle = self.alloc_handle();
        self.root_signatures.insert(
            handle.0,
            RootSignature {
                handle,
                desc: D3D12RootSignatureDesc {
                    parameters: Vec::new(),
                    static_samplers: Vec::new(),
                    flags: 0,
                },
                serialized_blob: blob.to_vec(),
            },
        );
        HResult(S_OK)
    }

    pub fn create_committed_resource(
        &mut self,
        heap_properties: &D3D12HeapProperties,
        _heap_flags: u32,
        desc: &D3D12ResourceDesc,
        initial_state: D3D12ResourceState,
    ) -> HResult {
        let handle = self.alloc_handle();
        let size = compute_resource_size(desc);
        let gpu_va = self.memory_budget.current_usage + 0x100000;
        self.memory_budget.current_usage += size;
        self.resources.insert(
            handle.0,
            D3D12Resource {
                handle,
                desc: desc.clone(),
                current_state: initial_state,
                heap_type: heap_properties.heap_type,
                gpu_virtual_address: gpu_va,
                size_in_bytes: size,
                mapped_ptr: None,
                is_committed: true,
                is_placed: false,
                is_reserved: false,
            },
        );
        HResult(S_OK)
    }

    pub fn create_placed_resource(
        &mut self,
        _heap: WinHandle,
        _heap_offset: u64,
        desc: &D3D12ResourceDesc,
        initial_state: D3D12ResourceState,
    ) -> HResult {
        let handle = self.alloc_handle();
        let size = compute_resource_size(desc);
        let gpu_va = self.memory_budget.current_usage + 0x100000;
        self.memory_budget.current_usage += size;
        self.resources.insert(
            handle.0,
            D3D12Resource {
                handle,
                desc: desc.clone(),
                current_state: initial_state,
                heap_type: D3D12HeapType::Default,
                gpu_virtual_address: gpu_va,
                size_in_bytes: size,
                mapped_ptr: None,
                is_committed: false,
                is_placed: true,
                is_reserved: false,
            },
        );
        HResult(S_OK)
    }

    pub fn create_reserved_resource(
        &mut self,
        desc: &D3D12ResourceDesc,
        initial_state: D3D12ResourceState,
    ) -> HResult {
        let handle = self.alloc_handle();
        let size = compute_resource_size(desc);
        self.resources.insert(
            handle.0,
            D3D12Resource {
                handle,
                desc: desc.clone(),
                current_state: initial_state,
                heap_type: D3D12HeapType::Default,
                gpu_virtual_address: 0,
                size_in_bytes: size,
                mapped_ptr: None,
                is_committed: false,
                is_placed: false,
                is_reserved: true,
            },
        );
        HResult(S_OK)
    }

    pub fn create_fence(&mut self, initial_value: u64) -> HResult {
        let handle = self.alloc_handle();
        self.fences
            .insert(handle.0, D3D12Fence::new(handle, initial_value));
        HResult(S_OK)
    }

    pub fn create_query_heap(&mut self, desc: D3D12QueryHeapDesc) -> HResult {
        let handle = self.alloc_handle();
        self.query_heaps.insert(handle.0, desc);
        HResult(S_OK)
    }

    pub fn get_descriptor_handle_increment_size(&self, heap_type: D3D12DescriptorHeapType) -> u32 {
        match heap_type {
            D3D12DescriptorHeapType::CbvSrvUav => 32,
            D3D12DescriptorHeapType::Sampler => 32,
            D3D12DescriptorHeapType::Rtv => 32,
            D3D12DescriptorHeapType::Dsv => 8,
        }
    }

    pub fn get_resource_allocation_info(
        &self,
        desc: &D3D12ResourceDesc,
    ) -> D3D12ResourceAllocationInfo {
        let size = compute_resource_size(desc);
        let alignment = if desc.dimension == D3D12ResourceDimension::Buffer {
            65536
        } else {
            65536
        };
        D3D12ResourceAllocationInfo {
            size_in_bytes: (size + alignment - 1) & !(alignment - 1),
            alignment,
        }
    }

    pub fn make_resident(&mut self, resources: &[WinHandle]) -> HResult {
        for &r in resources {
            self.resident_resources.push(r);
        }
        HResult(S_OK)
    }

    pub fn evict(&mut self, resources: &[WinHandle]) -> HResult {
        for r in resources {
            self.resident_resources.retain(|h| h != r);
        }
        HResult(S_OK)
    }

    pub fn check_feature_support(&self, _feature: D3D12Feature, _data: &mut [u8]) -> HResult {
        HResult(S_OK)
    }

    pub fn create_command_signature(&mut self, desc: D3D12CommandSignatureDesc) -> HResult {
        let handle = self.alloc_handle();
        self.command_signatures.insert(handle.0, desc);
        HResult(S_OK)
    }
}

// ---------------------------------------------------------------------------
// View descriptors (parameter types for create_*_view)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct D3D12RenderTargetViewDesc {
    pub format: DxgiFormat,
    pub view_dimension: D3D12RtvDimension,
    pub mip_slice: u32,
    pub first_array_slice: u32,
    pub array_size: u32,
    pub plane_slice: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12RtvDimension {
    Unknown,
    Buffer,
    Texture1D,
    Texture1DArray,
    Texture2D,
    Texture2DArray,
    Texture2DMs,
    Texture2DMsArray,
    Texture3D,
}

#[derive(Debug, Clone)]
pub struct D3D12DepthStencilViewDesc {
    pub format: DxgiFormat,
    pub view_dimension: D3D12DsvDimension,
    pub flags: u32,
    pub mip_slice: u32,
    pub first_array_slice: u32,
    pub array_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12DsvDimension {
    Unknown,
    Texture1D,
    Texture1DArray,
    Texture2D,
    Texture2DArray,
    Texture2DMs,
    Texture2DMsArray,
}

#[derive(Debug, Clone)]
pub struct D3D12ShaderResourceViewDesc {
    pub format: DxgiFormat,
    pub view_dimension: D3D12SrvDimension,
    pub shader_4_component_mapping: u32,
    pub most_detailed_mip: u32,
    pub mip_levels: u32,
    pub first_array_slice: u32,
    pub array_size: u32,
    pub plane_slice: u32,
    pub min_lod_clamp: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12SrvDimension {
    Unknown,
    Buffer,
    Texture1D,
    Texture1DArray,
    Texture2D,
    Texture2DArray,
    Texture2DMs,
    Texture2DMsArray,
    Texture3D,
    TextureCube,
    TextureCubeArray,
    RaytracingAccelerationStructure,
}

#[derive(Debug, Clone)]
pub struct D3D12UnorderedAccessViewDesc {
    pub format: DxgiFormat,
    pub view_dimension: D3D12UavDimension,
    pub mip_slice: u32,
    pub first_array_slice: u32,
    pub array_size: u32,
    pub first_element: u64,
    pub num_elements: u32,
    pub structure_byte_stride: u32,
    pub counter_offset_in_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D12UavDimension {
    Unknown,
    Buffer,
    Texture1D,
    Texture1DArray,
    Texture2D,
    Texture2DArray,
    Texture3D,
}

#[derive(Debug, Clone, Copy)]
pub struct D3D12ConstantBufferViewDesc {
    pub buffer_location: u64,
    pub size_in_bytes: u32,
}

#[derive(Debug, Clone)]
pub struct D3D12SamplerDesc {
    pub filter: D3D12Filter,
    pub address_u: D3D12TextureAddressMode,
    pub address_v: D3D12TextureAddressMode,
    pub address_w: D3D12TextureAddressMode,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison_func: D3D12ComparisonFunc,
    pub border_color: [f32; 4],
    pub min_lod: f32,
    pub max_lod: f32,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compute_resource_size(desc: &D3D12ResourceDesc) -> u64 {
    match desc.dimension {
        D3D12ResourceDimension::Buffer => desc.width,
        D3D12ResourceDimension::Texture1D => {
            desc.width
                * format_bytes_per_pixel(desc.format) as u64
                * desc.mip_levels as u64
                * desc.depth_or_array_size as u64
        }
        D3D12ResourceDimension::Texture2D => {
            desc.width
                * desc.height as u64
                * format_bytes_per_pixel(desc.format) as u64
                * desc.mip_levels as u64
                * desc.depth_or_array_size as u64
        }
        D3D12ResourceDimension::Texture3D => {
            desc.width
                * desc.height as u64
                * desc.depth_or_array_size as u64
                * format_bytes_per_pixel(desc.format) as u64
                * desc.mip_levels as u64
        }
        D3D12ResourceDimension::Unknown => 0,
    }
}

fn format_bytes_per_pixel(format: DxgiFormat) -> u32 {
    match format {
        DxgiFormat::R32G32B32A32Float | DxgiFormat::R32G32B32A32Uint => 16,
        DxgiFormat::R32G32B32Float => 12,
        DxgiFormat::R16G16B16A16Float | DxgiFormat::R16G16B16A16Unorm => 8,
        DxgiFormat::R32G32Float | DxgiFormat::R32G32Uint => 8,
        DxgiFormat::R10G10B10A2Unorm | DxgiFormat::R11G11B10Float => 4,
        DxgiFormat::R8G8B8A8Unorm | DxgiFormat::R8G8B8A8UnormSrgb => 4,
        DxgiFormat::B8G8R8A8Unorm | DxgiFormat::B8G8R8A8UnormSrgb => 4,
        DxgiFormat::R16G16Float | DxgiFormat::R16G16Unorm => 4,
        DxgiFormat::R32Float | DxgiFormat::R32Uint | DxgiFormat::D32Float => 4,
        DxgiFormat::D24UnormS8Uint => 4,
        DxgiFormat::R8G8Unorm => 2,
        DxgiFormat::R16Float | DxgiFormat::R16Unorm | DxgiFormat::D16Unorm => 2,
        DxgiFormat::R8Unorm => 1,
        DxgiFormat::D32FloatS8X24Uint => 8,
        DxgiFormat::Unknown => 1,
    }
}

// ---------------------------------------------------------------------------
// Global D3D12 runtime
// ---------------------------------------------------------------------------

pub struct D3D12Runtime {
    pub initialized: bool,
    pub device: Option<D3D12Device>,
    pub debug_layer_enabled: bool,
    pub gpu_based_validation: bool,
    pub dred_enabled: bool,
}

impl D3D12Runtime {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            device: None,
            debug_layer_enabled: false,
            gpu_based_validation: false,
            dred_enabled: false,
        }
    }

    pub fn init(&mut self) {
        if self.initialized {
            return;
        }
        self.device = Some(D3D12Device {
            handle: WinHandle(0xD3D12000),
            adapter_description: String::new(),
            feature_level: 0xC100, // D3D_FEATURE_LEVEL_12_1
            node_count: 1,
            heap_tier: D3D12HeapTier::Tier2,
            resource_binding_tier: 3,
            next_handle: 0xD3D12001,
            resources: BTreeMap::new(),
            descriptor_heaps: BTreeMap::new(),
            command_allocators: BTreeMap::new(),
            command_lists: BTreeMap::new(),
            command_queues: BTreeMap::new(),
            root_signatures: BTreeMap::new(),
            pipeline_states: BTreeMap::new(),
            fences: BTreeMap::new(),
            query_heaps: BTreeMap::new(),
            command_signatures: BTreeMap::new(),
            memory_budget: D3D12MemoryBudget {
                budget: 8 * 1024 * 1024 * 1024,
                current_usage: 0,
                available_for_reservation: 4 * 1024 * 1024 * 1024,
                current_reservation: 0,
            },
            resident_resources: Vec::new(),
        });
        self.initialized = true;
    }

    pub fn device(&self) -> Option<&D3D12Device> {
        self.device.as_ref()
    }

    pub fn device_mut(&mut self) -> Option<&mut D3D12Device> {
        self.device.as_mut()
    }

    pub fn enable_debug_layer(&mut self) {
        self.debug_layer_enabled = true;
    }

    pub fn enable_gpu_based_validation(&mut self) {
        self.gpu_based_validation = true;
    }

    pub fn enable_dred(&mut self) {
        self.dred_enabled = true;
    }
}

static mut D3D12_RUNTIME: D3D12Runtime = D3D12Runtime::new();

pub fn init() {
    unsafe {
        D3D12_RUNTIME.init();
    }
}

pub fn runtime() -> &'static D3D12Runtime {
    unsafe { &D3D12_RUNTIME }
}

pub fn runtime_mut() -> &'static mut D3D12Runtime {
    unsafe { &mut D3D12_RUNTIME }
}

// ---------------------------------------------------------------------------
// Root signature serialization
// ---------------------------------------------------------------------------

pub fn serialize_root_signature(
    desc: &D3D12RootSignatureDesc,
    _version: u32,
) -> Result<Vec<u8>, HResult> {
    let mut blob = Vec::new();
    blob.extend_from_slice(b"RTSB");
    blob.extend_from_slice(&(desc.parameters.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(desc.static_samplers.len() as u32).to_le_bytes());
    blob.extend_from_slice(&desc.flags.to_le_bytes());
    for param in &desc.parameters {
        blob.push(param.parameter_type as u8);
        blob.push(param.shader_visibility as u8);
    }
    Ok(blob)
}
