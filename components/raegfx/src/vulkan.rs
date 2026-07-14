#![allow(dead_code)]

//! Vulkan-compatible API surface mapped to AthGFX internals.
//!
//! This module provides the Vulkan API types and entry points that game engines
//! and renderers (including DXVK) target. Each Vulkan object type maps to the
//! AthGFX pipeline API, allowing transparent interception of draw calls, state
//! changes, and resource management.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// Result codes
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum VkResult {
    Success = 0,
    NotReady = 1,
    Timeout = 2,
    EventSet = 3,
    EventReset = 4,
    Incomplete = 5,
    ErrorOutOfHostMemory = -1,
    ErrorOutOfDeviceMemory = -2,
    ErrorInitializationFailed = -3,
    ErrorDeviceLost = -4,
    ErrorMemoryMapFailed = -5,
    ErrorLayerNotPresent = -6,
    ErrorExtensionNotPresent = -7,
    ErrorFeatureNotPresent = -8,
    ErrorIncompatibleDriver = -9,
    ErrorTooManyObjects = -10,
    ErrorFormatNotSupported = -11,
    ErrorFragmentedPool = -12,
    ErrorUnknown = -13,
    ErrorOutOfPoolMemory = -1000069000,
    ErrorInvalidExternalHandle = -1000072003,
    ErrorFragmentation = -1000161000,
    ErrorInvalidOpaqueCaptureAddress = -1000257000,
    ErrorSurfaceLostKhr = -1000000000,
    ErrorNativeWindowInUseKhr = -1000000001,
    SuboptimalKhr = 1000001003,
    ErrorOutOfDateKhr = -1000001004,
    ErrorValidationFailedExt = -1000011001,
}

impl VkResult {
    pub fn is_success(self) -> bool {
        self as i32 >= 0
    }
    pub fn is_error(self) -> bool {
        (self as i32) < 0
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Format enum — maps to raegfx::PixelFormat
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkFormat {
    Undefined = 0,
    R8Unorm = 9,
    R8Snorm = 10,
    R8Uint = 13,
    R8Sint = 14,
    R8G8Unorm = 16,
    R8G8Snorm = 17,
    R8G8B8A8Unorm = 37,
    R8G8B8A8Snorm = 38,
    R8G8B8A8Srgb = 43,
    B8G8R8A8Unorm = 44,
    B8G8R8A8Srgb = 50,
    A2B10G10R10UnormPack32 = 64,
    R16Sfloat = 76,
    R16G16Sfloat = 83,
    R16G16B16A16Sfloat = 97,
    R32Sfloat = 100,
    R32G32Sfloat = 103,
    R32G32B32Sfloat = 106,
    R32G32B32A32Sfloat = 109,
    B10G11R11UfloatPack32 = 122,
    D16Unorm = 124,
    D32Sfloat = 126,
    D24UnormS8Uint = 129,
    D32SfloatS8Uint = 130,
    Bc1RgbUnormBlock = 131,
    Bc1RgbaUnormBlock = 133,
    Bc3UnormBlock = 137,
    Bc7UnormBlock = 145,
}

impl VkFormat {
    pub fn to_raegfx(self) -> Option<crate::PixelFormat> {
        match self {
            Self::R8G8B8A8Unorm => Some(crate::PixelFormat::Rgba8Unorm),
            Self::B8G8R8A8Unorm => Some(crate::PixelFormat::Bgra8Unorm),
            Self::R8G8B8A8Srgb => Some(crate::PixelFormat::Rgba8Srgb),
            Self::B8G8R8A8Srgb => Some(crate::PixelFormat::Bgra8Srgb),
            Self::R16G16B16A16Sfloat => Some(crate::PixelFormat::Rgba16Float),
            Self::R32G32B32A32Sfloat => Some(crate::PixelFormat::Rgba32Float),
            Self::B10G11R11UfloatPack32 => Some(crate::PixelFormat::Rg11B10Float),
            Self::D24UnormS8Uint => Some(crate::PixelFormat::Depth24Stencil8),
            Self::D32Sfloat => Some(crate::PixelFormat::Depth32Float),
            Self::R8Unorm => Some(crate::PixelFormat::R8Unorm),
            Self::R8G8Unorm => Some(crate::PixelFormat::Rg8Unorm),
            Self::Bc1RgbUnormBlock | Self::Bc1RgbaUnormBlock => Some(crate::PixelFormat::Bc1Unorm),
            Self::Bc3UnormBlock => Some(crate::PixelFormat::Bc3Unorm),
            Self::Bc7UnormBlock => Some(crate::PixelFormat::Bc7Unorm),
            _ => None,
        }
    }

    pub fn from_raegfx(fmt: crate::PixelFormat) -> Self {
        match fmt {
            crate::PixelFormat::Rgba8Unorm => Self::R8G8B8A8Unorm,
            crate::PixelFormat::Bgra8Unorm => Self::B8G8R8A8Unorm,
            crate::PixelFormat::Rgba8Srgb => Self::R8G8B8A8Srgb,
            crate::PixelFormat::Bgra8Srgb => Self::B8G8R8A8Srgb,
            crate::PixelFormat::Rgba16Float => Self::R16G16B16A16Sfloat,
            crate::PixelFormat::Rgba32Float => Self::R32G32B32A32Sfloat,
            crate::PixelFormat::Rg11B10Float => Self::B10G11R11UfloatPack32,
            crate::PixelFormat::Depth24Stencil8 => Self::D24UnormS8Uint,
            crate::PixelFormat::Depth32Float => Self::D32Sfloat,
            crate::PixelFormat::R8Unorm => Self::R8Unorm,
            crate::PixelFormat::Rg8Unorm => Self::R8G8Unorm,
            crate::PixelFormat::Bc1Unorm => Self::Bc1RgbUnormBlock,
            crate::PixelFormat::Bc3Unorm => Self::Bc3UnormBlock,
            crate::PixelFormat::Bc7Unorm => Self::Bc7UnormBlock,
        }
    }

    pub fn is_depth(self) -> bool {
        matches!(
            self,
            Self::D16Unorm | Self::D32Sfloat | Self::D24UnormS8Uint | Self::D32SfloatS8Uint
        )
    }

    pub fn is_compressed(self) -> bool {
        matches!(
            self,
            Self::Bc1RgbUnormBlock
                | Self::Bc1RgbaUnormBlock
                | Self::Bc3UnormBlock
                | Self::Bc7UnormBlock
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Physical device types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VkPhysicalDeviceType {
    Other = 0,
    IntegratedGpu = 1,
    DiscreteGpu = 2,
    VirtualGpu = 3,
    Cpu = 4,
}

// ═══════════════════════════════════════════════════════════════════════════
// Physical device limits
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VkPhysicalDeviceLimits {
    pub max_image_dimension_1d: u32,
    pub max_image_dimension_2d: u32,
    pub max_image_dimension_3d: u32,
    pub max_image_dimension_cube: u32,
    pub max_image_array_layers: u32,
    pub max_texel_buffer_elements: u32,
    pub max_uniform_buffer_range: u32,
    pub max_storage_buffer_range: u32,
    pub max_push_constants_size: u32,
    pub max_memory_allocation_count: u32,
    pub max_bound_descriptor_sets: u32,
    pub max_per_stage_descriptor_samplers: u32,
    pub max_per_stage_descriptor_uniform_buffers: u32,
    pub max_per_stage_descriptor_storage_buffers: u32,
    pub max_per_stage_descriptor_sampled_images: u32,
    pub max_per_stage_descriptor_storage_images: u32,
    pub max_vertex_input_attributes: u32,
    pub max_vertex_input_bindings: u32,
    pub max_vertex_input_attribute_offset: u32,
    pub max_vertex_input_binding_stride: u32,
    pub max_fragment_output_attachments: u32,
    pub max_compute_shared_memory_size: u32,
    pub max_compute_work_group_count: [u32; 3],
    pub max_compute_work_group_invocations: u32,
    pub max_compute_work_group_size: [u32; 3],
    pub max_viewports: u32,
    pub max_framebuffer_width: u32,
    pub max_framebuffer_height: u32,
    pub max_framebuffer_layers: u32,
    pub max_color_attachments: u32,
    pub max_clip_distances: u32,
    pub max_cull_distances: u32,
    pub min_memory_map_alignment: u64,
    pub min_texel_buffer_offset_alignment: u64,
    pub min_uniform_buffer_offset_alignment: u64,
    pub min_storage_buffer_offset_alignment: u64,
    pub framebuffer_color_sample_counts: u32,
    pub framebuffer_depth_sample_counts: u32,
    pub timestamp_compute_and_graphics: bool,
    pub timestamp_period: f32,
}

impl Default for VkPhysicalDeviceLimits {
    fn default() -> Self {
        Self {
            max_image_dimension_1d: 16384,
            max_image_dimension_2d: 16384,
            max_image_dimension_3d: 2048,
            max_image_dimension_cube: 16384,
            max_image_array_layers: 2048,
            max_texel_buffer_elements: 134217728,
            max_uniform_buffer_range: 65536,
            max_storage_buffer_range: 2147483647,
            max_push_constants_size: 256,
            max_memory_allocation_count: 4096,
            max_bound_descriptor_sets: 8,
            max_per_stage_descriptor_samplers: 16,
            max_per_stage_descriptor_uniform_buffers: 15,
            max_per_stage_descriptor_storage_buffers: 16,
            max_per_stage_descriptor_sampled_images: 16,
            max_per_stage_descriptor_storage_images: 8,
            max_vertex_input_attributes: 32,
            max_vertex_input_bindings: 32,
            max_vertex_input_attribute_offset: 2047,
            max_vertex_input_binding_stride: 2048,
            max_fragment_output_attachments: 8,
            max_compute_shared_memory_size: 49152,
            max_compute_work_group_count: [65535, 65535, 65535],
            max_compute_work_group_invocations: 1024,
            max_compute_work_group_size: [1024, 1024, 64],
            max_viewports: 16,
            max_framebuffer_width: 16384,
            max_framebuffer_height: 16384,
            max_framebuffer_layers: 2048,
            max_color_attachments: 8,
            max_clip_distances: 8,
            max_cull_distances: 8,
            min_memory_map_alignment: 64,
            min_texel_buffer_offset_alignment: 256,
            min_uniform_buffer_offset_alignment: 256,
            min_storage_buffer_offset_alignment: 64,
            framebuffer_color_sample_counts: 0x0F,
            framebuffer_depth_sample_counts: 0x0F,
            timestamp_compute_and_graphics: true,
            timestamp_period: 1.0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Physical device features
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VkPhysicalDeviceFeatures {
    pub robust_buffer_access: bool,
    pub full_draw_index_uint32: bool,
    pub image_cube_array: bool,
    pub independent_blend: bool,
    pub geometry_shader: bool,
    pub tessellation_shader: bool,
    pub sample_rate_shading: bool,
    pub dual_src_blend: bool,
    pub logic_op: bool,
    pub multi_draw_indirect: bool,
    pub draw_indirect_first_instance: bool,
    pub depth_clamp: bool,
    pub depth_bias_clamp: bool,
    pub fill_mode_non_solid: bool,
    pub depth_bounds: bool,
    pub wide_lines: bool,
    pub large_points: bool,
    pub alpha_to_one: bool,
    pub multi_viewport: bool,
    pub sampler_anisotropy: bool,
    pub texture_compression_etc2: bool,
    pub texture_compression_astc_ldr: bool,
    pub texture_compression_bc: bool,
    pub occlusion_query_precise: bool,
    pub pipeline_statistics_query: bool,
    pub shader_storage_image_extended_formats: bool,
    pub shader_float64: bool,
    pub shader_int64: bool,
    pub shader_int16: bool,
    pub sparse_binding: bool,
    pub sparse_residency_buffer: bool,
    pub variable_multisample_rate: bool,
    pub inherited_queries: bool,
    // ── Completion to the full Vulkan VkPhysicalDeviceFeatures (55 fields) ──
    // The struct previously stopped at 33 fields; the remaining 22 are part of
    // the core Vulkan 1.0 feature set and are filled in here so callers can set
    // any standard feature (e.g. user_init's GPU probe).
    pub vertex_pipeline_stores_and_atomics: bool,
    pub fragment_stores_and_atomics: bool,
    pub shader_tessellation_and_geometry_point_size: bool,
    pub shader_image_gather_extended: bool,
    pub shader_storage_image_multisample: bool,
    pub shader_storage_image_read_without_format: bool,
    pub shader_storage_image_write_without_format: bool,
    pub shader_uniform_buffer_array_dynamic_indexing: bool,
    pub shader_sampled_image_array_dynamic_indexing: bool,
    pub shader_storage_buffer_array_dynamic_indexing: bool,
    pub shader_storage_image_array_dynamic_indexing: bool,
    pub shader_clip_distance: bool,
    pub shader_cull_distance: bool,
    pub shader_resource_residency: bool,
    pub shader_resource_min_lod: bool,
    pub sparse_residency_image2_d: bool,
    pub sparse_residency_image3_d: bool,
    pub sparse_residency2_samples: bool,
    pub sparse_residency4_samples: bool,
    pub sparse_residency8_samples: bool,
    pub sparse_residency16_samples: bool,
    pub sparse_residency_aliased: bool,
}

impl Default for VkPhysicalDeviceFeatures {
    fn default() -> Self {
        Self {
            robust_buffer_access: true,
            full_draw_index_uint32: true,
            image_cube_array: true,
            independent_blend: true,
            geometry_shader: true,
            tessellation_shader: true,
            sample_rate_shading: true,
            dual_src_blend: true,
            logic_op: true,
            multi_draw_indirect: true,
            draw_indirect_first_instance: true,
            depth_clamp: true,
            depth_bias_clamp: true,
            fill_mode_non_solid: true,
            depth_bounds: true,
            wide_lines: true,
            large_points: true,
            alpha_to_one: true,
            multi_viewport: true,
            sampler_anisotropy: true,
            texture_compression_etc2: false,
            texture_compression_astc_ldr: false,
            texture_compression_bc: true,
            occlusion_query_precise: true,
            pipeline_statistics_query: true,
            shader_storage_image_extended_formats: true,
            shader_float64: true,
            shader_int64: true,
            shader_int16: true,
            sparse_binding: true,
            sparse_residency_buffer: true,
            variable_multisample_rate: true,
            inherited_queries: true,
            // Completion fields (see struct def). Shader/vertex/fragment
            // capabilities advertised on; advanced sparse-residency modes off
            // (a software/bring-up backend cannot honor them).
            vertex_pipeline_stores_and_atomics: true,
            fragment_stores_and_atomics: true,
            shader_tessellation_and_geometry_point_size: true,
            shader_image_gather_extended: true,
            shader_storage_image_multisample: false,
            shader_storage_image_read_without_format: true,
            shader_storage_image_write_without_format: true,
            shader_uniform_buffer_array_dynamic_indexing: true,
            shader_sampled_image_array_dynamic_indexing: true,
            shader_storage_buffer_array_dynamic_indexing: true,
            shader_storage_image_array_dynamic_indexing: true,
            shader_clip_distance: true,
            shader_cull_distance: true,
            shader_resource_residency: false,
            shader_resource_min_lod: true,
            sparse_residency_image2_d: false,
            sparse_residency_image3_d: false,
            sparse_residency2_samples: false,
            sparse_residency4_samples: false,
            sparse_residency8_samples: false,
            sparse_residency16_samples: false,
            sparse_residency_aliased: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Memory types and heaps
// ═══════════════════════════════════════════════════════════════════════════

pub const VK_MEMORY_PROPERTY_DEVICE_LOCAL: u32 = 0x01;
pub const VK_MEMORY_PROPERTY_HOST_VISIBLE: u32 = 0x02;
pub const VK_MEMORY_PROPERTY_HOST_COHERENT: u32 = 0x04;
pub const VK_MEMORY_PROPERTY_HOST_CACHED: u32 = 0x08;
pub const VK_MEMORY_PROPERTY_LAZILY_ALLOCATED: u32 = 0x10;

pub const VK_MEMORY_HEAP_DEVICE_LOCAL: u32 = 0x01;
pub const VK_MEMORY_HEAP_MULTI_INSTANCE: u32 = 0x02;

#[derive(Debug, Clone)]
pub struct VkMemoryType {
    pub property_flags: u32,
    pub heap_index: u32,
}

#[derive(Debug, Clone)]
pub struct VkMemoryHeap {
    pub size: u64,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct VkPhysicalDeviceMemoryProperties {
    pub memory_types: Vec<VkMemoryType>,
    pub memory_heaps: Vec<VkMemoryHeap>,
}

impl Default for VkPhysicalDeviceMemoryProperties {
    fn default() -> Self {
        Self {
            memory_types: Vec::from([
                VkMemoryType {
                    property_flags: VK_MEMORY_PROPERTY_DEVICE_LOCAL,
                    heap_index: 0,
                },
                VkMemoryType {
                    property_flags: VK_MEMORY_PROPERTY_HOST_VISIBLE
                        | VK_MEMORY_PROPERTY_HOST_COHERENT,
                    heap_index: 1,
                },
                VkMemoryType {
                    property_flags: VK_MEMORY_PROPERTY_HOST_VISIBLE
                        | VK_MEMORY_PROPERTY_HOST_COHERENT
                        | VK_MEMORY_PROPERTY_HOST_CACHED,
                    heap_index: 1,
                },
                VkMemoryType {
                    property_flags: VK_MEMORY_PROPERTY_DEVICE_LOCAL
                        | VK_MEMORY_PROPERTY_HOST_VISIBLE
                        | VK_MEMORY_PROPERTY_HOST_COHERENT,
                    heap_index: 0,
                },
            ]),
            memory_heaps: Vec::from([
                VkMemoryHeap {
                    size: 8 * 1024 * 1024 * 1024,
                    flags: VK_MEMORY_HEAP_DEVICE_LOCAL,
                },
                VkMemoryHeap {
                    size: 16 * 1024 * 1024 * 1024,
                    flags: 0,
                },
            ]),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Queue families
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct VkQueueFlags(pub u32);

impl VkQueueFlags {
    pub const GRAPHICS: u32 = 1;
    pub const COMPUTE: u32 = 2;
    pub const TRANSFER: u32 = 4;
    pub const SPARSE_BINDING: u32 = 8;
    pub const PROTECTED: u32 = 16;
    pub const VIDEO_DECODE: u32 = 32;
    pub const VIDEO_ENCODE: u32 = 64;

    pub fn contains(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
    pub fn is_graphics(self) -> bool {
        self.contains(Self::GRAPHICS)
    }
    pub fn is_compute(self) -> bool {
        self.contains(Self::COMPUTE)
    }
    pub fn is_transfer(self) -> bool {
        self.contains(Self::TRANSFER)
    }
}

#[derive(Debug, Clone)]
pub struct VkQueueFamilyProperties {
    pub queue_flags: VkQueueFlags,
    pub queue_count: u32,
    pub timestamp_valid_bits: u32,
    pub min_image_transfer_granularity: [u32; 3],
}

// ═══════════════════════════════════════════════════════════════════════════
// Application info & Instance
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VkApplicationInfo {
    pub app_name: String,
    pub app_version: u32,
    pub engine_name: String,
    pub engine_version: u32,
    pub api_version: u32,
}

impl VkApplicationInfo {
    pub fn api_major(&self) -> u32 {
        (self.api_version >> 22) & 0x7F
    }
    pub fn api_minor(&self) -> u32 {
        (self.api_version >> 12) & 0x3FF
    }
    pub fn api_patch(&self) -> u32 {
        self.api_version & 0xFFF
    }
}

pub const VK_API_VERSION_1_0: u32 = (1 << 22) | (0 << 12);
pub const VK_API_VERSION_1_1: u32 = (1 << 22) | (1 << 12);
pub const VK_API_VERSION_1_2: u32 = (1 << 22) | (2 << 12);
pub const VK_API_VERSION_1_3: u32 = (1 << 22) | (3 << 12);

pub fn vk_make_api_version(variant: u32, major: u32, minor: u32, patch: u32) -> u32 {
    (variant << 29) | (major << 22) | (minor << 12) | patch
}

// ── Physical device ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VkPhysicalDeviceProperties {
    pub api_version: u32,
    pub driver_version: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub device_type: VkPhysicalDeviceType,
    pub device_name: String,
    pub limits: VkPhysicalDeviceLimits,
}

impl Default for VkPhysicalDeviceProperties {
    fn default() -> Self {
        Self {
            api_version: VK_API_VERSION_1_3,
            driver_version: 1,
            vendor_id: 0x8AEE_u32.wrapping_mul(1),
            device_id: 0x0001,
            device_type: VkPhysicalDeviceType::DiscreteGpu,
            device_name: String::from("AthGFX Virtual GPU"),
            limits: VkPhysicalDeviceLimits::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VkPhysicalDevice {
    pub properties: VkPhysicalDeviceProperties,
    pub features: VkPhysicalDeviceFeatures,
    pub memory_properties: VkPhysicalDeviceMemoryProperties,
    pub queue_families: Vec<VkQueueFamilyProperties>,
}

impl Default for VkPhysicalDevice {
    fn default() -> Self {
        Self {
            properties: VkPhysicalDeviceProperties::default(),
            features: VkPhysicalDeviceFeatures::default(),
            memory_properties: VkPhysicalDeviceMemoryProperties::default(),
            queue_families: Vec::from([
                VkQueueFamilyProperties {
                    queue_flags: VkQueueFlags(
                        VkQueueFlags::GRAPHICS | VkQueueFlags::COMPUTE | VkQueueFlags::TRANSFER,
                    ),
                    queue_count: 16,
                    timestamp_valid_bits: 64,
                    min_image_transfer_granularity: [1, 1, 1],
                },
                VkQueueFamilyProperties {
                    queue_flags: VkQueueFlags(VkQueueFlags::COMPUTE | VkQueueFlags::TRANSFER),
                    queue_count: 8,
                    timestamp_valid_bits: 64,
                    min_image_transfer_granularity: [1, 1, 1],
                },
                VkQueueFamilyProperties {
                    queue_flags: VkQueueFlags(VkQueueFlags::TRANSFER),
                    queue_count: 2,
                    timestamp_valid_bits: 64,
                    min_image_transfer_granularity: [16, 16, 16],
                },
            ]),
        }
    }
}

// ── Instance ─────────────────────────────────────────────────────────────

pub struct VkInstance {
    pub app_info: VkApplicationInfo,
    pub enabled_layers: Vec<String>,
    pub enabled_extensions: Vec<String>,
    pub devices: Vec<VkPhysicalDevice>,
}

pub fn vk_create_instance(
    info: &VkApplicationInfo,
    layers: &[&str],
    extensions: &[&str],
) -> Result<VkInstance, VkResult> {
    let enabled_layers: Vec<String> = layers.iter().map(|s| String::from(*s)).collect();
    let enabled_extensions: Vec<String> = extensions.iter().map(|s| String::from(*s)).collect();

    let devices = Vec::from([VkPhysicalDevice::default()]);

    Ok(VkInstance {
        app_info: info.clone(),
        enabled_layers,
        enabled_extensions,
        devices,
    })
}

pub fn vk_enumerate_physical_devices(instance: &VkInstance) -> Vec<VkPhysicalDevice> {
    instance.devices.clone()
}

pub fn vk_destroy_instance(_instance: VkInstance) {
    // Drop ownership — resources released
}

// ═══════════════════════════════════════════════════════════════════════════
// Logical device & queues
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VkQueue {
    pub family_index: u32,
    pub queue_index: u32,
    pub priority: f32,
    pub flags: VkQueueFlags,
}

pub struct VkDevice {
    pub physical_device: usize,
    pub queues: Vec<VkQueue>,
    pub enabled_features: VkPhysicalDeviceFeatures,
    pub enabled_extensions: Vec<String>,
    next_handle: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct VkDeviceQueueCreateInfo {
    pub queue_family_index: u32,
    pub queue_count: u32,
    pub priorities: Vec<f32>,
}

pub fn vk_create_device(
    physical_device: &VkPhysicalDevice,
    physical_device_index: usize,
    queue_infos: &[VkDeviceQueueCreateInfo],
    features: &VkPhysicalDeviceFeatures,
    extensions: &[&str],
) -> Result<VkDevice, VkResult> {
    let mut queues = Vec::new();

    for qi in queue_infos {
        if qi.queue_family_index as usize >= physical_device.queue_families.len() {
            return Err(VkResult::ErrorInitializationFailed);
        }
        let family = &physical_device.queue_families[qi.queue_family_index as usize];
        if qi.queue_count > family.queue_count {
            return Err(VkResult::ErrorInitializationFailed);
        }
        for i in 0..qi.queue_count {
            let priority = qi.priorities.get(i as usize).copied().unwrap_or(1.0);
            queues.push(VkQueue {
                family_index: qi.queue_family_index,
                queue_index: i,
                priority,
                flags: family.queue_flags,
            });
        }
    }

    Ok(VkDevice {
        physical_device: physical_device_index,
        queues,
        enabled_features: features.clone(),
        enabled_extensions: extensions.iter().map(|s| String::from(*s)).collect(),
        next_handle: AtomicU64::new(1),
    })
}

impl VkDevice {
    fn alloc_handle(&self) -> u64 {
        self.next_handle.fetch_add(1, Ordering::Relaxed)
    }

    pub fn get_queue(&self, family_index: u32, queue_index: u32) -> Option<&VkQueue> {
        self.queues
            .iter()
            .find(|q| q.family_index == family_index && q.queue_index == queue_index)
    }

    pub fn wait_idle(&self) -> VkResult {
        VkResult::Success
    }
}

pub fn vk_destroy_device(_device: VkDevice) {}

// ═══════════════════════════════════════════════════════════════════════════
// Device memory
// ═══════════════════════════════════════════════════════════════════════════

pub struct VkDeviceMemory {
    pub handle: u64,
    pub size: u64,
    pub memory_type_index: u32,
    pub mapped: bool,
    pub map_offset: u64,
    pub map_size: u64,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct VkMemoryAllocateInfo {
    pub allocation_size: u64,
    pub memory_type_index: u32,
}

impl VkDevice {
    pub fn allocate_memory(&self, info: &VkMemoryAllocateInfo) -> Result<VkDeviceMemory, VkResult> {
        let handle = self.alloc_handle();
        let mut data = Vec::new();
        data.resize(info.allocation_size as usize, 0u8);
        Ok(VkDeviceMemory {
            handle,
            size: info.allocation_size,
            memory_type_index: info.memory_type_index,
            mapped: false,
            map_offset: 0,
            map_size: 0,
            data,
        })
    }

    pub fn free_memory(&self, _memory: VkDeviceMemory) {}
}

impl VkDeviceMemory {
    pub fn map(&mut self, offset: u64, size: u64) -> Result<*mut u8, VkResult> {
        if self.mapped {
            return Err(VkResult::ErrorMemoryMapFailed);
        }
        let actual_size = if size == u64::MAX {
            self.size - offset
        } else {
            size
        };
        if offset + actual_size > self.size {
            return Err(VkResult::ErrorMemoryMapFailed);
        }
        self.mapped = true;
        self.map_offset = offset;
        self.map_size = actual_size;
        Ok(unsafe { self.data.as_mut_ptr().add(offset as usize) })
    }

    pub fn unmap(&mut self) {
        self.mapped = false;
        self.map_offset = 0;
        self.map_size = 0;
    }

    pub fn flush(&self, _offset: u64, _size: u64) -> VkResult {
        VkResult::Success
    }

    pub fn invalidate(&self, _offset: u64, _size: u64) -> VkResult {
        VkResult::Success
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Buffers
// ═══════════════════════════════════════════════════════════════════════════

pub const VK_BUFFER_USAGE_TRANSFER_SRC: u32 = 0x0001;
pub const VK_BUFFER_USAGE_TRANSFER_DST: u32 = 0x0002;
pub const VK_BUFFER_USAGE_UNIFORM_TEXEL_BUFFER: u32 = 0x0004;
pub const VK_BUFFER_USAGE_STORAGE_TEXEL_BUFFER: u32 = 0x0008;
pub const VK_BUFFER_USAGE_UNIFORM_BUFFER: u32 = 0x0010;
pub const VK_BUFFER_USAGE_STORAGE_BUFFER: u32 = 0x0020;
pub const VK_BUFFER_USAGE_INDEX_BUFFER: u32 = 0x0040;
pub const VK_BUFFER_USAGE_VERTEX_BUFFER: u32 = 0x0080;
pub const VK_BUFFER_USAGE_INDIRECT_BUFFER: u32 = 0x0100;

#[derive(Debug, Clone)]
pub struct VkBufferCreateInfo {
    pub size: u64,
    pub usage: u32,
    pub sharing_mode: VkSharingMode,
    pub queue_family_indices: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkSharingMode {
    Exclusive,
    Concurrent,
}

pub struct VkBuffer {
    pub handle: u64,
    pub size: u64,
    pub usage: u32,
    pub sharing_mode: VkSharingMode,
    pub memory_handle: Option<u64>,
    pub memory_offset: u64,
}

pub fn vk_create_buffer(
    device: &VkDevice,
    info: &VkBufferCreateInfo,
) -> Result<VkBuffer, VkResult> {
    Ok(VkBuffer {
        handle: device.alloc_handle(),
        size: info.size,
        usage: info.usage,
        sharing_mode: info.sharing_mode,
        memory_handle: None,
        memory_offset: 0,
    })
}

pub fn vk_destroy_buffer(_device: &VkDevice, _buffer: VkBuffer) {}

impl VkBuffer {
    pub fn bind_memory(&mut self, memory: &VkDeviceMemory, offset: u64) -> VkResult {
        if offset + self.size > memory.size {
            return VkResult::ErrorOutOfDeviceMemory;
        }
        self.memory_handle = Some(memory.handle);
        self.memory_offset = offset;
        VkResult::Success
    }

    pub fn memory_requirements(&self) -> VkMemoryRequirements {
        VkMemoryRequirements {
            size: self.size,
            alignment: 256,
            memory_type_bits: 0x0F,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VkMemoryRequirements {
    pub size: u64,
    pub alignment: u64,
    pub memory_type_bits: u32,
}

// ═══════════════════════════════════════════════════════════════════════════
// Images
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkImageType {
    Image1D,
    Image2D,
    Image3D,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkImageTiling {
    Optimal,
    Linear,
}

pub const VK_IMAGE_USAGE_TRANSFER_SRC: u32 = 0x0001;
pub const VK_IMAGE_USAGE_TRANSFER_DST: u32 = 0x0002;
pub const VK_IMAGE_USAGE_SAMPLED: u32 = 0x0004;
pub const VK_IMAGE_USAGE_STORAGE: u32 = 0x0008;
pub const VK_IMAGE_USAGE_COLOR_ATTACHMENT: u32 = 0x0010;
pub const VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT: u32 = 0x0020;
pub const VK_IMAGE_USAGE_TRANSIENT_ATTACHMENT: u32 = 0x0040;
pub const VK_IMAGE_USAGE_INPUT_ATTACHMENT: u32 = 0x0080;

#[derive(Debug, Clone)]
pub struct VkImageCreateInfo {
    pub image_type: VkImageType,
    pub format: VkFormat,
    pub extent: VkExtent3D,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub samples: u32,
    pub tiling: VkImageTiling,
    pub usage: u32,
    pub sharing_mode: VkSharingMode,
    pub initial_layout: VkImageLayout,
}

#[derive(Debug, Clone, Copy)]
pub struct VkExtent3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct VkExtent2D {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkImageLayout {
    Undefined,
    General,
    ColorAttachmentOptimal,
    DepthStencilAttachmentOptimal,
    DepthStencilReadOnlyOptimal,
    ShaderReadOnlyOptimal,
    TransferSrcOptimal,
    TransferDstOptimal,
    Preinitialized,
    PresentSrcKhr,
}

pub struct VkImage {
    pub handle: u64,
    pub image_type: VkImageType,
    pub format: VkFormat,
    pub extent: VkExtent3D,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub samples: u32,
    pub tiling: VkImageTiling,
    pub usage: u32,
    pub layout: VkImageLayout,
    pub memory_handle: Option<u64>,
    pub memory_offset: u64,
}

pub fn vk_create_image(device: &VkDevice, info: &VkImageCreateInfo) -> Result<VkImage, VkResult> {
    Ok(VkImage {
        handle: device.alloc_handle(),
        image_type: info.image_type,
        format: info.format,
        extent: info.extent,
        mip_levels: info.mip_levels,
        array_layers: info.array_layers,
        samples: info.samples,
        tiling: info.tiling,
        usage: info.usage,
        layout: info.initial_layout,
        memory_handle: None,
        memory_offset: 0,
    })
}

pub fn vk_destroy_image(_device: &VkDevice, _image: VkImage) {}

impl VkImage {
    pub fn bind_memory(&mut self, memory: &VkDeviceMemory, offset: u64) -> VkResult {
        self.memory_handle = Some(memory.handle);
        self.memory_offset = offset;
        VkResult::Success
    }

    pub fn memory_requirements(&self) -> VkMemoryRequirements {
        let texel_size = match self.format.to_raegfx() {
            Some(fmt) => fmt.bytes_per_pixel() as u64,
            None => 4,
        };
        let size = self.extent.width as u64
            * self.extent.height as u64
            * self.extent.depth as u64
            * self.array_layers as u64
            * texel_size;
        VkMemoryRequirements {
            size,
            alignment: 256,
            memory_type_bits: 0x0F,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Image views
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkImageViewType {
    View1D,
    View2D,
    View3D,
    ViewCube,
    View1DArray,
    View2DArray,
    ViewCubeArray,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkComponentSwizzle {
    Identity,
    Zero,
    One,
    R,
    G,
    B,
    A,
}

#[derive(Debug, Clone, Copy)]
pub struct VkComponentMapping {
    pub r: VkComponentSwizzle,
    pub g: VkComponentSwizzle,
    pub b: VkComponentSwizzle,
    pub a: VkComponentSwizzle,
}

impl Default for VkComponentMapping {
    fn default() -> Self {
        Self {
            r: VkComponentSwizzle::Identity,
            g: VkComponentSwizzle::Identity,
            b: VkComponentSwizzle::Identity,
            a: VkComponentSwizzle::Identity,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VkImageSubresourceRange {
    pub aspect_mask: u32,
    pub base_mip_level: u32,
    pub level_count: u32,
    pub base_array_layer: u32,
    pub layer_count: u32,
}

pub const VK_IMAGE_ASPECT_COLOR: u32 = 0x01;
pub const VK_IMAGE_ASPECT_DEPTH: u32 = 0x02;
pub const VK_IMAGE_ASPECT_STENCIL: u32 = 0x04;

pub struct VkImageView {
    pub handle: u64,
    pub image_handle: u64,
    pub view_type: VkImageViewType,
    pub format: VkFormat,
    pub components: VkComponentMapping,
    pub subresource_range: VkImageSubresourceRange,
}

pub fn vk_create_image_view(
    device: &VkDevice,
    image: &VkImage,
    view_type: VkImageViewType,
    format: VkFormat,
    components: VkComponentMapping,
    subresource_range: VkImageSubresourceRange,
) -> Result<VkImageView, VkResult> {
    Ok(VkImageView {
        handle: device.alloc_handle(),
        image_handle: image.handle,
        view_type,
        format,
        components,
        subresource_range,
    })
}

pub fn vk_destroy_image_view(_device: &VkDevice, _view: VkImageView) {}

// ═══════════════════════════════════════════════════════════════════════════
// Samplers
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkFilter {
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkSamplerMipmapMode {
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkSamplerAddressMode {
    Repeat,
    MirroredRepeat,
    ClampToEdge,
    ClampToBorder,
    MirrorClampToEdge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkBorderColor {
    FloatTransparentBlack,
    IntTransparentBlack,
    FloatOpaqueBlack,
    IntOpaqueBlack,
    FloatOpaqueWhite,
    IntOpaqueWhite,
}

#[derive(Debug, Clone)]
pub struct VkSamplerCreateInfo {
    pub mag_filter: VkFilter,
    pub min_filter: VkFilter,
    pub mipmap_mode: VkSamplerMipmapMode,
    pub address_mode_u: VkSamplerAddressMode,
    pub address_mode_v: VkSamplerAddressMode,
    pub address_mode_w: VkSamplerAddressMode,
    pub mip_lod_bias: f32,
    pub anisotropy_enable: bool,
    pub max_anisotropy: f32,
    pub compare_enable: bool,
    pub compare_op: VkCompareOp,
    pub min_lod: f32,
    pub max_lod: f32,
    pub border_color: VkBorderColor,
    pub unnormalized_coordinates: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkCompareOp {
    Never,
    Less,
    Equal,
    LessOrEqual,
    Greater,
    NotEqual,
    GreaterOrEqual,
    Always,
}

pub struct VkSampler {
    pub handle: u64,
    pub info: VkSamplerCreateInfo,
}

pub fn vk_create_sampler(
    device: &VkDevice,
    info: &VkSamplerCreateInfo,
) -> Result<VkSampler, VkResult> {
    Ok(VkSampler {
        handle: device.alloc_handle(),
        info: info.clone(),
    })
}

pub fn vk_destroy_sampler(_device: &VkDevice, _sampler: VkSampler) {}

// ═══════════════════════════════════════════════════════════════════════════
// Descriptor set layout & descriptor pool
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkDescriptorType {
    Sampler,
    CombinedImageSampler,
    SampledImage,
    StorageImage,
    UniformTexelBuffer,
    StorageTexelBuffer,
    UniformBuffer,
    StorageBuffer,
    UniformBufferDynamic,
    StorageBufferDynamic,
    InputAttachment,
}

#[derive(Debug, Clone)]
pub struct VkDescriptorSetLayoutBinding {
    pub binding: u32,
    pub descriptor_type: VkDescriptorType,
    pub descriptor_count: u32,
    pub stage_flags: u32,
}

pub struct VkDescriptorSetLayout {
    pub handle: u64,
    pub bindings: Vec<VkDescriptorSetLayoutBinding>,
}

pub fn vk_create_descriptor_set_layout(
    device: &VkDevice,
    bindings: &[VkDescriptorSetLayoutBinding],
) -> Result<VkDescriptorSetLayout, VkResult> {
    Ok(VkDescriptorSetLayout {
        handle: device.alloc_handle(),
        bindings: Vec::from(bindings),
    })
}

pub fn vk_destroy_descriptor_set_layout(_device: &VkDevice, _layout: VkDescriptorSetLayout) {}

#[derive(Debug, Clone)]
pub struct VkDescriptorPoolSize {
    pub descriptor_type: VkDescriptorType,
    pub descriptor_count: u32,
}

pub struct VkDescriptorPool {
    pub handle: u64,
    pub max_sets: u32,
    pub pool_sizes: Vec<VkDescriptorPoolSize>,
    pub allocated_sets: u32,
}

pub fn vk_create_descriptor_pool(
    device: &VkDevice,
    max_sets: u32,
    pool_sizes: &[VkDescriptorPoolSize],
) -> Result<VkDescriptorPool, VkResult> {
    Ok(VkDescriptorPool {
        handle: device.alloc_handle(),
        max_sets,
        pool_sizes: Vec::from(pool_sizes),
        allocated_sets: 0,
    })
}

pub fn vk_destroy_descriptor_pool(_device: &VkDevice, _pool: VkDescriptorPool) {}

pub struct VkDescriptorSet {
    pub handle: u64,
    pub layout_handle: u64,
    pub bindings: BTreeMap<u32, VkDescriptorBinding>,
}

pub struct VkDescriptorBinding {
    pub descriptor_type: VkDescriptorType,
    pub buffer_handle: Option<u64>,
    pub image_view_handle: Option<u64>,
    pub sampler_handle: Option<u64>,
    pub offset: u64,
    pub range: u64,
}

impl VkDescriptorPool {
    pub fn allocate_set(
        &mut self,
        device: &VkDevice,
        layout: &VkDescriptorSetLayout,
    ) -> Result<VkDescriptorSet, VkResult> {
        if self.allocated_sets >= self.max_sets {
            return Err(VkResult::ErrorOutOfPoolMemory);
        }
        self.allocated_sets += 1;
        Ok(VkDescriptorSet {
            handle: device.alloc_handle(),
            layout_handle: layout.handle,
            bindings: BTreeMap::new(),
        })
    }

    pub fn reset(&mut self) -> VkResult {
        self.allocated_sets = 0;
        VkResult::Success
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Pipeline layout
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VkPushConstantRange {
    pub stage_flags: u32,
    pub offset: u32,
    pub size: u32,
}

pub struct VkPipelineLayout {
    pub handle: u64,
    pub set_layout_handles: Vec<u64>,
    pub push_constant_ranges: Vec<VkPushConstantRange>,
}

pub fn vk_create_pipeline_layout(
    device: &VkDevice,
    set_layouts: &[&VkDescriptorSetLayout],
    push_constant_ranges: &[VkPushConstantRange],
) -> Result<VkPipelineLayout, VkResult> {
    Ok(VkPipelineLayout {
        handle: device.alloc_handle(),
        set_layout_handles: set_layouts.iter().map(|l| l.handle).collect(),
        push_constant_ranges: Vec::from(push_constant_ranges),
    })
}

pub fn vk_destroy_pipeline_layout(_device: &VkDevice, _layout: VkPipelineLayout) {}

// ═══════════════════════════════════════════════════════════════════════════
// Shader modules
// ═══════════════════════════════════════════════════════════════════════════

pub struct VkShaderModule {
    pub handle: u64,
    pub spirv: Vec<u8>,
    pub code_size: usize,
}

pub fn vk_create_shader_module(
    device: &VkDevice,
    spirv: &[u8],
) -> Result<VkShaderModule, VkResult> {
    if spirv.len() < 4 || spirv.len() % 4 != 0 {
        return Err(VkResult::ErrorInvalidExternalHandle);
    }
    Ok(VkShaderModule {
        handle: device.alloc_handle(),
        spirv: Vec::from(spirv),
        code_size: spirv.len(),
    })
}

pub fn vk_destroy_shader_module(_device: &VkDevice, _module: VkShaderModule) {}

// ═══════════════════════════════════════════════════════════════════════════
// Render pass
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkAttachmentLoadOp {
    Load,
    Clear,
    DontCare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkAttachmentStoreOp {
    Store,
    DontCare,
}

#[derive(Debug, Clone)]
pub struct VkAttachmentDescription {
    pub format: VkFormat,
    pub samples: u32,
    pub load_op: VkAttachmentLoadOp,
    pub store_op: VkAttachmentStoreOp,
    pub stencil_load_op: VkAttachmentLoadOp,
    pub stencil_store_op: VkAttachmentStoreOp,
    pub initial_layout: VkImageLayout,
    pub final_layout: VkImageLayout,
}

#[derive(Debug, Clone)]
pub struct VkAttachmentReference {
    pub attachment: u32,
    pub layout: VkImageLayout,
}

#[derive(Debug, Clone)]
pub struct VkSubpassDescription {
    pub pipeline_bind_point: VkPipelineBindPoint,
    pub input_attachments: Vec<VkAttachmentReference>,
    pub color_attachments: Vec<VkAttachmentReference>,
    pub resolve_attachments: Vec<VkAttachmentReference>,
    pub depth_stencil_attachment: Option<VkAttachmentReference>,
    pub preserve_attachments: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkPipelineBindPoint {
    Graphics,
    Compute,
}

#[derive(Debug, Clone)]
pub struct VkSubpassDependency {
    pub src_subpass: u32,
    pub dst_subpass: u32,
    pub src_stage_mask: u32,
    pub dst_stage_mask: u32,
    pub src_access_mask: u32,
    pub dst_access_mask: u32,
    pub dependency_flags: u32,
}

pub const VK_SUBPASS_EXTERNAL: u32 = u32::MAX;

pub struct VkRenderPass {
    pub handle: u64,
    pub attachments: Vec<VkAttachmentDescription>,
    pub subpasses: Vec<VkSubpassDescription>,
    pub dependencies: Vec<VkSubpassDependency>,
}

pub fn vk_create_render_pass(
    device: &VkDevice,
    attachments: &[VkAttachmentDescription],
    subpasses: &[VkSubpassDescription],
    dependencies: &[VkSubpassDependency],
) -> Result<VkRenderPass, VkResult> {
    Ok(VkRenderPass {
        handle: device.alloc_handle(),
        attachments: Vec::from(attachments),
        subpasses: Vec::from(subpasses),
        dependencies: Vec::from(dependencies),
    })
}

pub fn vk_destroy_render_pass(_device: &VkDevice, _pass: VkRenderPass) {}

// ═══════════════════════════════════════════════════════════════════════════
// Graphics pipeline
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkPrimitiveTopology {
    PointList,
    LineList,
    LineStrip,
    TriangleList,
    TriangleStrip,
    TriangleFan,
    LineListWithAdjacency,
    LineStripWithAdjacency,
    TriangleListWithAdjacency,
    TriangleStripWithAdjacency,
    PatchList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkPolygonMode {
    Fill,
    Line,
    Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkCullModeFlags {
    None,
    Front,
    Back,
    FrontAndBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkFrontFace {
    CounterClockwise,
    Clockwise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkBlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    ConstantColor,
    OneMinusConstantColor,
    ConstantAlpha,
    OneMinusConstantAlpha,
    SrcAlphaSaturate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkBlendOp {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkDynamicState {
    Viewport,
    Scissor,
    LineWidth,
    DepthBias,
    BlendConstants,
    DepthBounds,
    StencilCompareMask,
    StencilWriteMask,
    StencilReference,
}

#[derive(Debug, Clone)]
pub struct VkPipelineShaderStageCreateInfo {
    pub stage: u32,
    pub module_handle: u64,
    pub entry_point: String,
}

pub const VK_SHADER_STAGE_VERTEX: u32 = 0x01;
pub const VK_SHADER_STAGE_TESS_CONTROL: u32 = 0x02;
pub const VK_SHADER_STAGE_TESS_EVAL: u32 = 0x04;
pub const VK_SHADER_STAGE_GEOMETRY: u32 = 0x08;
pub const VK_SHADER_STAGE_FRAGMENT: u32 = 0x10;
pub const VK_SHADER_STAGE_COMPUTE: u32 = 0x20;
pub const VK_SHADER_STAGE_ALL_GRAPHICS: u32 = 0x1F;
pub const VK_SHADER_STAGE_ALL: u32 = 0x7FFFFFFF;

#[derive(Debug, Clone)]
pub struct VkVertexInputBindingDescription {
    pub binding: u32,
    pub stride: u32,
    pub input_rate: VkVertexInputRate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkVertexInputRate {
    Vertex,
    Instance,
}

#[derive(Debug, Clone)]
pub struct VkVertexInputAttributeDescription {
    pub location: u32,
    pub binding: u32,
    pub format: VkFormat,
    pub offset: u32,
}

#[derive(Debug, Clone)]
pub struct VkPipelineVertexInputStateCreateInfo {
    pub binding_descriptions: Vec<VkVertexInputBindingDescription>,
    pub attribute_descriptions: Vec<VkVertexInputAttributeDescription>,
}

#[derive(Debug, Clone)]
pub struct VkPipelineInputAssemblyStateCreateInfo {
    pub topology: VkPrimitiveTopology,
    pub primitive_restart_enable: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct VkViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct VkRect2D {
    pub offset_x: i32,
    pub offset_y: i32,
    pub extent: VkExtent2D,
}

#[derive(Debug, Clone)]
pub struct VkPipelineViewportStateCreateInfo {
    pub viewports: Vec<VkViewport>,
    pub scissors: Vec<VkRect2D>,
}

#[derive(Debug, Clone)]
pub struct VkPipelineRasterizationStateCreateInfo {
    pub depth_clamp_enable: bool,
    pub rasterizer_discard_enable: bool,
    pub polygon_mode: VkPolygonMode,
    pub cull_mode: VkCullModeFlags,
    pub front_face: VkFrontFace,
    pub depth_bias_enable: bool,
    pub depth_bias_constant_factor: f32,
    pub depth_bias_clamp: f32,
    pub depth_bias_slope_factor: f32,
    pub line_width: f32,
}

#[derive(Debug, Clone)]
pub struct VkPipelineMultisampleStateCreateInfo {
    pub rasterization_samples: u32,
    pub sample_shading_enable: bool,
    pub min_sample_shading: f32,
    pub alpha_to_coverage_enable: bool,
    pub alpha_to_one_enable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkStencilOp {
    Keep,
    Zero,
    Replace,
    IncrementAndClamp,
    DecrementAndClamp,
    Invert,
    IncrementAndWrap,
    DecrementAndWrap,
}

#[derive(Debug, Clone)]
pub struct VkStencilOpState {
    pub fail_op: VkStencilOp,
    pub pass_op: VkStencilOp,
    pub depth_fail_op: VkStencilOp,
    pub compare_op: VkCompareOp,
    pub compare_mask: u32,
    pub write_mask: u32,
    pub reference: u32,
}

#[derive(Debug, Clone)]
pub struct VkPipelineDepthStencilStateCreateInfo {
    pub depth_test_enable: bool,
    pub depth_write_enable: bool,
    pub depth_compare_op: VkCompareOp,
    pub depth_bounds_test_enable: bool,
    pub stencil_test_enable: bool,
    pub front: VkStencilOpState,
    pub back: VkStencilOpState,
    pub min_depth_bounds: f32,
    pub max_depth_bounds: f32,
}

#[derive(Debug, Clone)]
pub struct VkPipelineColorBlendAttachmentState {
    pub blend_enable: bool,
    pub src_color_blend_factor: VkBlendFactor,
    pub dst_color_blend_factor: VkBlendFactor,
    pub color_blend_op: VkBlendOp,
    pub src_alpha_blend_factor: VkBlendFactor,
    pub dst_alpha_blend_factor: VkBlendFactor,
    pub alpha_blend_op: VkBlendOp,
    pub color_write_mask: u32,
}

pub const VK_COLOR_COMPONENT_R: u32 = 0x01;
pub const VK_COLOR_COMPONENT_G: u32 = 0x02;
pub const VK_COLOR_COMPONENT_B: u32 = 0x04;
pub const VK_COLOR_COMPONENT_A: u32 = 0x08;
pub const VK_COLOR_COMPONENT_ALL: u32 = 0x0F;

#[derive(Debug, Clone)]
pub struct VkPipelineColorBlendStateCreateInfo {
    pub logic_op_enable: bool,
    pub logic_op: VkLogicOp,
    pub attachments: Vec<VkPipelineColorBlendAttachmentState>,
    pub blend_constants: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkLogicOp {
    Clear,
    And,
    AndReverse,
    Copy,
    AndInverted,
    NoOp,
    Xor,
    Or,
    Nor,
    Equivalent,
    Invert,
    OrReverse,
    CopyInverted,
    OrInverted,
    Nand,
    Set,
}

#[derive(Debug, Clone)]
pub struct VkGraphicsPipelineCreateInfo {
    pub stages: Vec<VkPipelineShaderStageCreateInfo>,
    pub vertex_input_state: VkPipelineVertexInputStateCreateInfo,
    pub input_assembly_state: VkPipelineInputAssemblyStateCreateInfo,
    pub viewport_state: VkPipelineViewportStateCreateInfo,
    pub rasterization_state: VkPipelineRasterizationStateCreateInfo,
    pub multisample_state: VkPipelineMultisampleStateCreateInfo,
    pub depth_stencil_state: Option<VkPipelineDepthStencilStateCreateInfo>,
    pub color_blend_state: VkPipelineColorBlendStateCreateInfo,
    pub dynamic_states: Vec<VkDynamicState>,
    pub layout_handle: u64,
    pub render_pass_handle: u64,
    pub subpass: u32,
}

pub struct VkPipeline {
    pub handle: u64,
    pub bind_point: VkPipelineBindPoint,
    pub layout_handle: u64,
    pub render_pass_handle: u64,
}

pub fn vk_create_graphics_pipeline(
    device: &VkDevice,
    info: &VkGraphicsPipelineCreateInfo,
) -> Result<VkPipeline, VkResult> {
    Ok(VkPipeline {
        handle: device.alloc_handle(),
        bind_point: VkPipelineBindPoint::Graphics,
        layout_handle: info.layout_handle,
        render_pass_handle: info.render_pass_handle,
    })
}

pub fn vk_create_compute_pipeline(
    device: &VkDevice,
    stage: &VkPipelineShaderStageCreateInfo,
    layout_handle: u64,
) -> Result<VkPipeline, VkResult> {
    Ok(VkPipeline {
        handle: device.alloc_handle(),
        bind_point: VkPipelineBindPoint::Compute,
        layout_handle,
        render_pass_handle: 0,
    })
}

pub fn vk_destroy_pipeline(_device: &VkDevice, _pipeline: VkPipeline) {}

// ═══════════════════════════════════════════════════════════════════════════
// Framebuffer
// ═══════════════════════════════════════════════════════════════════════════

pub struct VkFramebuffer {
    pub handle: u64,
    pub render_pass_handle: u64,
    pub attachment_handles: Vec<u64>,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

pub fn vk_create_framebuffer(
    device: &VkDevice,
    render_pass: &VkRenderPass,
    attachments: &[&VkImageView],
    width: u32,
    height: u32,
    layers: u32,
) -> Result<VkFramebuffer, VkResult> {
    Ok(VkFramebuffer {
        handle: device.alloc_handle(),
        render_pass_handle: render_pass.handle,
        attachment_handles: attachments.iter().map(|a| a.handle).collect(),
        width,
        height,
        layers,
    })
}

pub fn vk_destroy_framebuffer(_device: &VkDevice, _framebuffer: VkFramebuffer) {}

// ═══════════════════════════════════════════════════════════════════════════
// Command pool & command buffers
// ═══════════════════════════════════════════════════════════════════════════

pub struct VkCommandPool {
    pub handle: u64,
    pub queue_family_index: u32,
    pub flags: u32,
    pub allocated_buffers: u32,
}

pub const VK_COMMAND_POOL_CREATE_TRANSIENT: u32 = 0x01;
pub const VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER: u32 = 0x02;

pub fn vk_create_command_pool(
    device: &VkDevice,
    queue_family_index: u32,
    flags: u32,
) -> Result<VkCommandPool, VkResult> {
    Ok(VkCommandPool {
        handle: device.alloc_handle(),
        queue_family_index,
        flags,
        allocated_buffers: 0,
    })
}

pub fn vk_destroy_command_pool(_device: &VkDevice, _pool: VkCommandPool) {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkCommandBufferLevel {
    Primary,
    Secondary,
}

pub struct VkCommandBuffer {
    pub handle: u64,
    pub pool_handle: u64,
    pub level: VkCommandBufferLevel,
    pub recording: bool,
    pub executable: bool,
    commands: Vec<VkCmd>,
}

#[derive(Debug, Clone)]
pub enum VkCmd {
    BeginRenderPass {
        render_pass_handle: u64,
        framebuffer_handle: u64,
        clear_values: Vec<VkClearValue>,
    },
    EndRenderPass,
    BindPipeline {
        bind_point: VkPipelineBindPoint,
        pipeline_handle: u64,
    },
    BindVertexBuffers {
        first_binding: u32,
        buffer_handles: Vec<u64>,
        offsets: Vec<u64>,
    },
    BindIndexBuffer {
        buffer_handle: u64,
        offset: u64,
        index_type: VkIndexType,
    },
    BindDescriptorSets {
        bind_point: VkPipelineBindPoint,
        layout_handle: u64,
        first_set: u32,
        set_handles: Vec<u64>,
        dynamic_offsets: Vec<u32>,
    },
    Draw {
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    },
    DrawIndexed {
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    },
    Dispatch {
        group_count_x: u32,
        group_count_y: u32,
        group_count_z: u32,
    },
    SetViewport {
        first_viewport: u32,
        viewports: Vec<VkViewport>,
    },
    SetScissor {
        first_scissor: u32,
        scissors: Vec<VkRect2D>,
    },
    PushConstants {
        layout_handle: u64,
        stage_flags: u32,
        offset: u32,
        data: Vec<u8>,
    },
    CopyBuffer {
        src_handle: u64,
        dst_handle: u64,
        regions: Vec<VkBufferCopy>,
    },
    CopyBufferToImage {
        src_handle: u64,
        dst_handle: u64,
        dst_layout: VkImageLayout,
        regions: Vec<VkBufferImageCopy>,
    },
    PipelineBarrier {
        src_stage_mask: u32,
        dst_stage_mask: u32,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct VkClearValue {
    pub color: [f32; 4],
    pub depth: f32,
    pub stencil: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkIndexType {
    Uint16,
    Uint32,
}

#[derive(Debug, Clone, Copy)]
pub struct VkBufferCopy {
    pub src_offset: u64,
    pub dst_offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct VkBufferImageCopy {
    pub buffer_offset: u64,
    pub buffer_row_length: u32,
    pub buffer_image_height: u32,
    pub image_offset: [i32; 3],
    pub image_extent: VkExtent3D,
    pub image_mip_level: u32,
    pub image_base_array_layer: u32,
    pub image_layer_count: u32,
}

impl VkCommandPool {
    pub fn allocate_command_buffer(
        &mut self,
        device: &VkDevice,
        level: VkCommandBufferLevel,
    ) -> VkCommandBuffer {
        self.allocated_buffers += 1;
        VkCommandBuffer {
            handle: device.alloc_handle(),
            pool_handle: self.handle,
            level,
            recording: false,
            executable: false,
            commands: Vec::new(),
        }
    }

    pub fn reset(&mut self) -> VkResult {
        self.allocated_buffers = 0;
        VkResult::Success
    }
}

impl VkCommandBuffer {
    pub fn begin(&mut self) -> VkResult {
        if self.recording {
            return VkResult::ErrorUnknown;
        }
        self.recording = true;
        self.executable = false;
        self.commands.clear();
        VkResult::Success
    }

    pub fn end(&mut self) -> VkResult {
        if !self.recording {
            return VkResult::ErrorUnknown;
        }
        self.recording = false;
        self.executable = true;
        VkResult::Success
    }

    pub fn reset(&mut self) -> VkResult {
        self.recording = false;
        self.executable = false;
        self.commands.clear();
        VkResult::Success
    }

    pub fn cmd_begin_render_pass(
        &mut self,
        render_pass_handle: u64,
        framebuffer_handle: u64,
        clear_values: &[VkClearValue],
    ) {
        self.commands.push(VkCmd::BeginRenderPass {
            render_pass_handle,
            framebuffer_handle,
            clear_values: Vec::from(clear_values),
        });
    }

    pub fn cmd_end_render_pass(&mut self) {
        self.commands.push(VkCmd::EndRenderPass);
    }

    pub fn cmd_bind_pipeline(&mut self, bind_point: VkPipelineBindPoint, pipeline_handle: u64) {
        self.commands.push(VkCmd::BindPipeline {
            bind_point,
            pipeline_handle,
        });
    }

    pub fn cmd_bind_vertex_buffers(
        &mut self,
        first_binding: u32,
        buffer_handles: &[u64],
        offsets: &[u64],
    ) {
        self.commands.push(VkCmd::BindVertexBuffers {
            first_binding,
            buffer_handles: Vec::from(buffer_handles),
            offsets: Vec::from(offsets),
        });
    }

    pub fn cmd_bind_index_buffer(
        &mut self,
        buffer_handle: u64,
        offset: u64,
        index_type: VkIndexType,
    ) {
        self.commands.push(VkCmd::BindIndexBuffer {
            buffer_handle,
            offset,
            index_type,
        });
    }

    pub fn cmd_bind_descriptor_sets(
        &mut self,
        bind_point: VkPipelineBindPoint,
        layout_handle: u64,
        first_set: u32,
        set_handles: &[u64],
        dynamic_offsets: &[u32],
    ) {
        self.commands.push(VkCmd::BindDescriptorSets {
            bind_point,
            layout_handle,
            first_set,
            set_handles: Vec::from(set_handles),
            dynamic_offsets: Vec::from(dynamic_offsets),
        });
    }

    pub fn cmd_draw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        self.commands.push(VkCmd::Draw {
            vertex_count,
            instance_count,
            first_vertex,
            first_instance,
        });
    }

    pub fn cmd_draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    ) {
        self.commands.push(VkCmd::DrawIndexed {
            index_count,
            instance_count,
            first_index,
            vertex_offset,
            first_instance,
        });
    }

    pub fn cmd_dispatch(&mut self, group_count_x: u32, group_count_y: u32, group_count_z: u32) {
        self.commands.push(VkCmd::Dispatch {
            group_count_x,
            group_count_y,
            group_count_z,
        });
    }

    pub fn cmd_set_viewport(&mut self, first_viewport: u32, viewports: &[VkViewport]) {
        self.commands.push(VkCmd::SetViewport {
            first_viewport,
            viewports: Vec::from(viewports),
        });
    }

    pub fn cmd_set_scissor(&mut self, first_scissor: u32, scissors: &[VkRect2D]) {
        self.commands.push(VkCmd::SetScissor {
            first_scissor,
            scissors: Vec::from(scissors),
        });
    }

    pub fn cmd_push_constants(
        &mut self,
        layout_handle: u64,
        stage_flags: u32,
        offset: u32,
        data: &[u8],
    ) {
        self.commands.push(VkCmd::PushConstants {
            layout_handle,
            stage_flags,
            offset,
            data: Vec::from(data),
        });
    }

    pub fn cmd_copy_buffer(&mut self, src_handle: u64, dst_handle: u64, regions: &[VkBufferCopy]) {
        self.commands.push(VkCmd::CopyBuffer {
            src_handle,
            dst_handle,
            regions: Vec::from(regions),
        });
    }

    pub fn cmd_copy_buffer_to_image(
        &mut self,
        src_handle: u64,
        dst_handle: u64,
        dst_layout: VkImageLayout,
        regions: &[VkBufferImageCopy],
    ) {
        self.commands.push(VkCmd::CopyBufferToImage {
            src_handle,
            dst_handle,
            dst_layout,
            regions: Vec::from(regions),
        });
    }

    pub fn cmd_pipeline_barrier(&mut self, src_stage_mask: u32, dst_stage_mask: u32) {
        self.commands.push(VkCmd::PipelineBarrier {
            src_stage_mask,
            dst_stage_mask,
        });
    }

    pub fn command_count(&self) -> usize {
        self.commands.len()
    }
    pub fn commands(&self) -> &[VkCmd] {
        &self.commands
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Pipeline stage flags
// ═══════════════════════════════════════════════════════════════════════════

pub const VK_PIPELINE_STAGE_TOP_OF_PIPE: u32 = 0x00000001;
pub const VK_PIPELINE_STAGE_DRAW_INDIRECT: u32 = 0x00000002;
pub const VK_PIPELINE_STAGE_VERTEX_INPUT: u32 = 0x00000004;
pub const VK_PIPELINE_STAGE_VERTEX_SHADER: u32 = 0x00000008;
pub const VK_PIPELINE_STAGE_TESS_CONTROL_SHADER: u32 = 0x00000010;
pub const VK_PIPELINE_STAGE_TESS_EVAL_SHADER: u32 = 0x00000020;
pub const VK_PIPELINE_STAGE_GEOMETRY_SHADER: u32 = 0x00000040;
pub const VK_PIPELINE_STAGE_FRAGMENT_SHADER: u32 = 0x00000080;
pub const VK_PIPELINE_STAGE_EARLY_FRAGMENT_TESTS: u32 = 0x00000100;
pub const VK_PIPELINE_STAGE_LATE_FRAGMENT_TESTS: u32 = 0x00000200;
pub const VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT: u32 = 0x00000400;
pub const VK_PIPELINE_STAGE_COMPUTE_SHADER: u32 = 0x00000800;
pub const VK_PIPELINE_STAGE_TRANSFER: u32 = 0x00001000;
pub const VK_PIPELINE_STAGE_BOTTOM_OF_PIPE: u32 = 0x00002000;
pub const VK_PIPELINE_STAGE_ALL_GRAPHICS: u32 = 0x00008000;
pub const VK_PIPELINE_STAGE_ALL_COMMANDS: u32 = 0x00010000;

// ═══════════════════════════════════════════════════════════════════════════
// Synchronization primitives
// ═══════════════════════════════════════════════════════════════════════════

pub struct VkSemaphore {
    pub handle: u64,
    pub signaled: bool,
}

pub fn vk_create_semaphore(device: &VkDevice) -> Result<VkSemaphore, VkResult> {
    Ok(VkSemaphore {
        handle: device.alloc_handle(),
        signaled: false,
    })
}

pub fn vk_destroy_semaphore(_device: &VkDevice, _sem: VkSemaphore) {}

pub struct VkFence {
    pub handle: u64,
    pub signaled: bool,
}

pub fn vk_create_fence(device: &VkDevice, signaled: bool) -> Result<VkFence, VkResult> {
    Ok(VkFence {
        handle: device.alloc_handle(),
        signaled,
    })
}

pub fn vk_destroy_fence(_device: &VkDevice, _fence: VkFence) {}

impl VkFence {
    pub fn reset(&mut self) {
        self.signaled = false;
    }
    pub fn signal(&mut self) {
        self.signaled = true;
    }
    pub fn is_signaled(&self) -> bool {
        self.signaled
    }

    pub fn wait(&self, timeout_ns: u64) -> VkResult {
        if self.signaled {
            VkResult::Success
        } else if timeout_ns == 0 {
            VkResult::NotReady
        } else {
            VkResult::Timeout
        }
    }
}

pub fn vk_wait_for_fences(fences: &[&VkFence], wait_all: bool, _timeout_ns: u64) -> VkResult {
    if wait_all {
        if fences.iter().all(|f| f.signaled) {
            VkResult::Success
        } else {
            VkResult::Timeout
        }
    } else {
        if fences.iter().any(|f| f.signaled) {
            VkResult::Success
        } else {
            VkResult::Timeout
        }
    }
}

pub fn vk_reset_fences(fences: &mut [&mut VkFence]) {
    for f in fences.iter_mut() {
        f.signaled = false;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Queue submission
// ═══════════════════════════════════════════════════════════════════════════

pub struct VkSubmitInfo<'a> {
    pub wait_semaphores: &'a [&'a VkSemaphore],
    pub wait_dst_stage_masks: &'a [u32],
    pub command_buffers: &'a [&'a VkCommandBuffer],
    pub signal_semaphores: &'a [&'a mut VkSemaphore],
}

pub fn vk_queue_submit(
    _queue: &VkQueue,
    submits: &[VkSubmitInfo<'_>],
    signal_fence: Option<&mut VkFence>,
) -> VkResult {
    for submit in submits {
        for cb in submit.command_buffers {
            if !cb.executable {
                return VkResult::ErrorUnknown;
            }
        }
    }
    if let Some(fence) = signal_fence {
        fence.signal();
    }
    VkResult::Success
}

pub fn vk_queue_wait_idle(_queue: &VkQueue) -> VkResult {
    VkResult::Success
}

// ═══════════════════════════════════════════════════════════════════════════
// Swapchain (KHR)
// ═══════════════════════════════════════════════════════════════════════════

pub struct VkSurfaceKhr {
    pub handle: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct VkSurfaceCapabilitiesKhr {
    pub min_image_count: u32,
    pub max_image_count: u32,
    pub current_extent: VkExtent2D,
    pub min_image_extent: VkExtent2D,
    pub max_image_extent: VkExtent2D,
    pub max_image_array_layers: u32,
    pub supported_transforms: u32,
    pub current_transform: u32,
    pub supported_composite_alpha: u32,
    pub supported_usage_flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VkPresentModeKhr {
    Immediate,
    Mailbox,
    Fifo,
    FifoRelaxed,
}

#[derive(Debug, Clone)]
pub struct VkSwapchainCreateInfoKhr {
    pub surface_handle: u64,
    pub min_image_count: u32,
    pub image_format: VkFormat,
    pub image_extent: VkExtent2D,
    pub image_array_layers: u32,
    pub image_usage: u32,
    pub image_sharing_mode: VkSharingMode,
    pub pre_transform: u32,
    pub composite_alpha: u32,
    pub present_mode: VkPresentModeKhr,
    pub clipped: bool,
    pub old_swapchain_handle: Option<u64>,
}

pub struct VkSwapchainKhr {
    pub handle: u64,
    pub images: Vec<VkImage>,
    pub image_format: VkFormat,
    pub extent: VkExtent2D,
    pub current_image_index: u32,
}

pub fn vk_create_swapchain(
    device: &VkDevice,
    info: &VkSwapchainCreateInfoKhr,
) -> Result<VkSwapchainKhr, VkResult> {
    let image_count = info.min_image_count.max(2).min(8);
    let mut images = Vec::new();
    for _ in 0..image_count {
        images.push(VkImage {
            handle: device.alloc_handle(),
            image_type: VkImageType::Image2D,
            format: info.image_format,
            extent: VkExtent3D {
                width: info.image_extent.width,
                height: info.image_extent.height,
                depth: 1,
            },
            mip_levels: 1,
            array_layers: info.image_array_layers,
            samples: 1,
            tiling: VkImageTiling::Optimal,
            usage: info.image_usage,
            layout: VkImageLayout::Undefined,
            memory_handle: None,
            memory_offset: 0,
        });
    }

    Ok(VkSwapchainKhr {
        handle: device.alloc_handle(),
        images,
        image_format: info.image_format,
        extent: info.image_extent,
        current_image_index: 0,
    })
}

pub fn vk_destroy_swapchain(_device: &VkDevice, _swapchain: VkSwapchainKhr) {}

impl VkSwapchainKhr {
    pub fn acquire_next_image(
        &mut self,
        _timeout_ns: u64,
        _semaphore: Option<&mut VkSemaphore>,
        _fence: Option<&mut VkFence>,
    ) -> Result<u32, VkResult> {
        let idx = self.current_image_index;
        self.current_image_index = (self.current_image_index + 1) % self.images.len() as u32;
        Ok(idx)
    }

    pub fn image_count(&self) -> u32 {
        self.images.len() as u32
    }

    pub fn get_image(&self, index: u32) -> Option<&VkImage> {
        self.images.get(index as usize)
    }
}

pub fn vk_queue_present(
    _queue: &VkQueue,
    _wait_semaphores: &[&VkSemaphore],
    swapchain: &VkSwapchainKhr,
    _image_index: u32,
) -> VkResult {
    if swapchain.images.is_empty() {
        return VkResult::ErrorDeviceLost;
    }
    VkResult::Success
}

// ═══════════════════════════════════════════════════════════════════════════
// Utility: find memory type
// ═══════════════════════════════════════════════════════════════════════════

pub fn vk_find_memory_type(
    mem_properties: &VkPhysicalDeviceMemoryProperties,
    type_filter: u32,
    property_flags: u32,
) -> Option<u32> {
    for (i, mem_type) in mem_properties.memory_types.iter().enumerate() {
        if (type_filter & (1 << i)) != 0
            && (mem_type.property_flags & property_flags) == property_flags
        {
            return Some(i as u32);
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════
// Extension & layer enumeration
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VkExtensionProperties {
    pub extension_name: String,
    pub spec_version: u32,
}

#[derive(Debug, Clone)]
pub struct VkLayerProperties {
    pub layer_name: String,
    pub spec_version: u32,
    pub implementation_version: u32,
    pub description: String,
}

pub fn vk_enumerate_instance_extension_properties() -> Vec<VkExtensionProperties> {
    Vec::from([
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_surface"),
            spec_version: 25,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_display"),
            spec_version: 23,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_EXT_debug_utils"),
            spec_version: 2,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_get_physical_device_properties2"),
            spec_version: 2,
        },
    ])
}

pub fn vk_enumerate_device_extension_properties() -> Vec<VkExtensionProperties> {
    Vec::from([
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_swapchain"),
            spec_version: 70,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_maintenance1"),
            spec_version: 2,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_maintenance2"),
            spec_version: 1,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_maintenance3"),
            spec_version: 1,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_multiview"),
            spec_version: 1,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_dedicated_allocation"),
            spec_version: 3,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_descriptor_update_template"),
            spec_version: 1,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_push_descriptor"),
            spec_version: 2,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_KHR_dynamic_rendering"),
            spec_version: 1,
        },
        VkExtensionProperties {
            extension_name: String::from("VK_EXT_descriptor_indexing"),
            spec_version: 2,
        },
    ])
}

pub fn vk_enumerate_instance_layer_properties() -> Vec<VkLayerProperties> {
    Vec::from([VkLayerProperties {
        layer_name: String::from("VK_LAYER_RAEGFX_validation"),
        spec_version: VK_API_VERSION_1_3,
        implementation_version: 1,
        description: String::from("AthGFX Vulkan validation layer"),
    }])
}
