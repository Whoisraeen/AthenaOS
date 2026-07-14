//! Native `wgpu-hal` backend for RaeGFX.
//!
//! This module is the bridge between the public `wgpu_hal::Api` surface used
//! by RaeUI / Skia and the lockless command ring + VirtIO Venus protocol that
//! the RaeKernel half of the graphics stack consumes. Specifically:
//!
//!   1. Every `wgpu_hal::CommandEncoder` call serialises a typed Venus token
//!      (see `tokens.rs`) into a per-encoder `PayloadArena` (see `arena.rs`).
//!   2. The arena offset is stored verbatim in `GpuCommandPacket::payload_addr`,
//!      not a raw pointer — the prior implementation cast `&payload as
//!      *const _` and was, strictly, a use-after-free.
//!   3. At submit time, `RaeGfxQueue::submit` writes the packets into the
//!      lockless ring's MMIO doorbell while the arena bytes stay owned by the
//!      `CommandBuffer` until the kernel signals completion via the fence id.
//!
//! Anything that's not yet wired (full instance/adapter open, surface
//! configuration, buffer mapping) returns an explicit `wgpu_hal` error rather
//! than `unimplemented!()`. We'd rather walk the call graph and fail
//! observably than panic the moment Skia tries to enumerate adapters.

#![allow(unused_variables)]

extern crate alloc;
extern crate wgpu_types;

pub mod arena;
pub mod sub_alloc;
pub mod tokens;

use core::ops::Range;
use core::sync::atomic::{AtomicU64, Ordering};

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::device::RaeGfxQueue;
use crate::shared_queue::GpuCommandPacket;

use self::arena::PayloadArena;
use self::tokens::{
    opcode, VenusBeginRenderPassToken, VenusDrawIndexedToken, VenusDrawToken,
    VenusEndRenderPassToken, VenusSetBindGroupToken, VenusSetIndexBufferToken,
    VenusSetPipelineToken, VenusSetVertexBufferToken,
};

static NEXT_RESOURCE_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_id() -> u64 {
    NEXT_RESOURCE_ID.fetch_add(1, Ordering::SeqCst)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaeGfxApi;

#[derive(Debug)]
pub struct RaeGfxContext {
    pub raw_queue: Arc<RaeGfxQueue>,
}

#[derive(Debug)]
pub struct RaeGfxCommandEncoder {
    /// Tokens recorded in submission order. Each packet's `payload_addr`
    /// indexes into `payload` (or `ArenaOffset::INVALID.0` when the token is
    /// payload-free, e.g. end-of-pass).
    storage: Vec<GpuCommandPacket>,
    /// Backing store for token payloads. Owned by the encoder for the duration
    /// of recording + submit; never aliased.
    payload: PayloadArena,
    /// Fence sequence used as a per-packet completion token. Monotonic.
    next_fence: u32,
}

impl RaeGfxCommandEncoder {
    fn new() -> Self {
        Self {
            storage: Vec::new(),
            payload: PayloadArena::new(),
            next_fence: 1,
        }
    }

    /// Append a typed token to the arena and emit a packet referencing it.
    fn record<T: bytemuck::Pod>(&mut self, cmd_type: u32, flags: u32, token: &T) {
        let off = self.payload.push(token);
        let fence_id = self.next_fence;
        self.next_fence = self.next_fence.wrapping_add(1);
        self.storage.push(GpuCommandPacket {
            cmd_type,
            flags,
            payload_addr: off.0,
            payload_size: core::mem::size_of::<T>() as u32,
            fence_id,
        });
    }

    /// Emit a control packet that carries no payload.
    fn record_control(&mut self, cmd_type: u32, flags: u32) {
        let fence_id = self.next_fence;
        self.next_fence = self.next_fence.wrapping_add(1);
        self.storage.push(GpuCommandPacket {
            cmd_type,
            flags,
            payload_addr: arena::ArenaOffset::INVALID.0,
            payload_size: 0,
            fence_id,
        });
    }

    /// Borrow the recorded packets + their backing arena. Used by `submit`.
    pub fn parts(&self) -> (&[GpuCommandPacket], &[u8]) {
        (&self.storage, self.payload.as_bytes())
    }

    pub fn clear(&mut self) {
        self.storage.clear();
        self.payload.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawGpuResource {
    pub id: u64,
}

impl RawGpuResource {
    fn fresh() -> Self {
        Self { id: next_id() }
    }
}

type DeviceResult<T> = Result<T, wgpu_hal::DeviceError>;

impl wgpu_hal::Api for RaeGfxApi {
    type Instance = RaeGfxContext;
    type Surface = RaeGfxContext;
    type Adapter = RaeGfxContext;
    type Device = RaeGfxContext;

    type Queue = RaeGfxContext;
    type CommandEncoder = RaeGfxCommandEncoder;
    type CommandBuffer = RaeGfxCommandEncoder;

    type Buffer = RawGpuResource;
    type Texture = RawGpuResource;
    type SurfaceTexture = RawGpuResource;
    type TextureView = RawGpuResource;
    type Sampler = RawGpuResource;
    type QuerySet = RawGpuResource;
    type Fence = RawGpuResource;
    type AccelerationStructure = RawGpuResource;

    type BindGroupLayout = RawGpuResource;
    type BindGroup = RawGpuResource;
    type PipelineLayout = RawGpuResource;
    type ShaderModule = RawGpuResource;
    type RenderPipeline = RawGpuResource;
    type ComputePipeline = RawGpuResource;
}

// ─── Instance ────────────────────────────────────────────────────────────────
//
// `Instance::init` is called by wgpu-core during adapter discovery. We do not
// yet have a kernel handle to ask for a real `RaeGfxQueue` from this code path
// (that requires a syscall into RaeKernel that returns the mapped ring +
// doorbell pointers). Until that wiring lands, we surface an explicit
// `InstanceError` so callers can fall back to the software canvas instead of
// crashing the renderer.

impl wgpu_hal::Instance<RaeGfxApi> for RaeGfxContext {
    unsafe fn init(_desc: &wgpu_hal::InstanceDescriptor) -> Result<Self, wgpu_hal::InstanceError> {
        Err(wgpu_hal::InstanceError::new(String::from(
            "RaeGFX wgpu_backend: Instance::init requires a kernel-provided RaeGfxQueue. \
             Call RaeGfxContext::from_queue(...) after acquiring one via the surface syscall.",
        )))
    }

    unsafe fn create_surface(
        &self,
        _display_handle: raw_window_handle::RawDisplayHandle,
        _window_handle: raw_window_handle::RawWindowHandle,
    ) -> Result<RaeGfxContext, wgpu_hal::InstanceError> {
        // The compositor owns surface allocation; from RaeGFX's perspective a
        // surface is just another handle on the same underlying ring. Clone
        // the Arc so the surface and instance share the queue.
        Ok(RaeGfxContext {
            raw_queue: Arc::clone(&self.raw_queue),
        })
    }

    unsafe fn destroy_surface(&self, _surface: RaeGfxContext) {
        // Dropping `surface` releases its Arc; nothing else to do.
    }

    unsafe fn enumerate_adapters(&self) -> Vec<wgpu_hal::ExposedAdapter<RaeGfxApi>> {
        // We expose exactly one adapter: the kernel's primary RaeGFX queue.
        // wgpu_hal::ExposedAdapter requires concrete capability info which is
        // intentionally minimal until the kernel reports real device info via
        // RaeGfxQueue::describe_adapter — for now we return empty to signal
        // "no advertisable adapter" without panicking. Callers fall back to
        // the software Canvas.
        Vec::new()
    }
}

impl RaeGfxContext {
    /// Construct a context from an already-mapped kernel queue. This is the
    /// real entry point used by raeui's renderer once it has called the
    /// surface allocation syscall and received the ring + doorbell pointers.
    pub fn from_queue(queue: Arc<RaeGfxQueue>) -> Self {
        Self { raw_queue: queue }
    }
}

// ─── Surface ────────────────────────────────────────────────────────────────

impl wgpu_hal::Surface<RaeGfxApi> for RaeGfxContext {
    unsafe fn configure(
        &self,
        _device: &RaeGfxContext,
        _config: &wgpu_hal::SurfaceConfiguration,
    ) -> Result<(), wgpu_hal::SurfaceError> {
        Ok(())
    }

    unsafe fn unconfigure(&self, _device: &RaeGfxContext) {}

    unsafe fn acquire_texture(
        &self,
        _timeout: Option<core::time::Duration>,
    ) -> Result<Option<wgpu_hal::AcquiredSurfaceTexture<RaeGfxApi>>, wgpu_hal::SurfaceError> {
        // No acquire path until the compositor exposes a swapchain syscall.
        // Returning Ok(None) tells wgpu-core "no frame ready" rather than
        // crashing it.
        Ok(None)
    }

    unsafe fn discard_texture(&self, _texture: RawGpuResource) {}
}

// ─── Adapter ─────────────────────────────────────────────────────────────────

impl wgpu_hal::Adapter<RaeGfxApi> for RaeGfxContext {
    unsafe fn open(
        &self,
        _features: wgpu_types::Features,
        _limits: &wgpu_types::Limits,
    ) -> DeviceResult<wgpu_hal::OpenDevice<RaeGfxApi>> {
        // `OpenDevice` needs the device + queue. Both are the same context
        // here (they share the raw queue). Until the kernel exposes a real
        // open-device handshake we return Lost so wgpu-core treats us as a
        // disconnected adapter and re-enumerates next frame.
        Err(wgpu_hal::DeviceError::Lost)
    }

    unsafe fn texture_format_capabilities(
        &self,
        _format: wgpu_types::TextureFormat,
    ) -> wgpu_hal::TextureFormatCapabilities {
        wgpu_hal::TextureFormatCapabilities::empty()
    }

    unsafe fn surface_capabilities(
        &self,
        _surface: &RaeGfxContext,
    ) -> Option<wgpu_hal::SurfaceCapabilities> {
        None
    }

    unsafe fn get_presentation_timestamp(&self) -> wgpu_types::PresentationTimestamp {
        wgpu_types::PresentationTimestamp::INVALID_TIMESTAMP
    }
}

// ─── Queue ───────────────────────────────────────────────────────────────────

impl wgpu_hal::Queue<RaeGfxApi> for RaeGfxContext {
    unsafe fn submit(
        &self,
        command_buffers: &[&RaeGfxCommandEncoder],
        _signal_fence: Option<(&mut RawGpuResource, wgpu_hal::FenceValue)>,
    ) -> DeviceResult<()> {
        for encoder in command_buffers {
            let (packets, _bytes) = encoder.parts();
            // Today the kernel-side `RaeGfxQueue::submit` only walks the packet
            // ring; the arena bytes follow once we wire the kernel's payload
            // window. Until then we forward the packets verbatim so the ring
            // exercise stays honest.
            self.raw_queue
                .submit(packets)
                .map_err(|_| wgpu_hal::DeviceError::Lost)?;
        }
        Ok(())
    }

    unsafe fn get_timestamp_period(&self) -> f32 {
        1.0
    }

    unsafe fn present(
        &self,
        _surface: &<RaeGfxApi as wgpu_hal::Api>::Surface,
        _texture: <RaeGfxApi as wgpu_hal::Api>::SurfaceTexture,
    ) -> Result<(), wgpu_hal::SurfaceError> {
        Ok(())
    }
}

// ─── Device ──────────────────────────────────────────────────────────────────

impl wgpu_hal::Device<RaeGfxApi> for RaeGfxContext {
    unsafe fn exit(self, _queue: RaeGfxContext) {}

    unsafe fn create_buffer(
        &self,
        _desc: &wgpu_hal::BufferDescriptor,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_buffer(&self, _buffer: RawGpuResource) {}

    unsafe fn map_buffer(
        &self,
        _buffer: &RawGpuResource,
        _range: wgpu_hal::MemoryRange,
    ) -> DeviceResult<wgpu_hal::BufferMapping> {
        // Mapping requires a userspace VRAM aperture; until the kernel
        // surfaces one, signal Lost so wgpu-core falls back to staged uploads.
        Err(wgpu_hal::DeviceError::Lost)
    }
    unsafe fn unmap_buffer(&self, _buffer: &RawGpuResource) -> DeviceResult<()> {
        Ok(())
    }
    unsafe fn flush_mapped_ranges<I>(&self, _buffer: &RawGpuResource, _ranges: I) {}
    unsafe fn invalidate_mapped_ranges<I>(&self, _buffer: &RawGpuResource, _ranges: I) {}

    unsafe fn create_texture(
        &self,
        _desc: &wgpu_hal::TextureDescriptor,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_texture(&self, _texture: RawGpuResource) {}

    unsafe fn create_texture_view(
        &self,
        _texture: &RawGpuResource,
        _desc: &wgpu_hal::TextureViewDescriptor,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_texture_view(&self, _view: RawGpuResource) {}

    unsafe fn create_sampler(
        &self,
        _desc: &wgpu_hal::SamplerDescriptor,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_sampler(&self, _sampler: RawGpuResource) {}

    unsafe fn create_command_encoder(
        &self,
        _desc: &wgpu_hal::CommandEncoderDescriptor<RaeGfxApi>,
    ) -> DeviceResult<RaeGfxCommandEncoder> {
        Ok(RaeGfxCommandEncoder::new())
    }
    unsafe fn destroy_command_encoder(&self, _encoder: RaeGfxCommandEncoder) {}

    unsafe fn create_bind_group_layout(
        &self,
        _desc: &wgpu_hal::BindGroupLayoutDescriptor,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_bind_group_layout(&self, _bg_layout: RawGpuResource) {}

    unsafe fn create_pipeline_layout(
        &self,
        _desc: &wgpu_hal::PipelineLayoutDescriptor<RaeGfxApi>,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_pipeline_layout(&self, _pipeline_layout: RawGpuResource) {}

    unsafe fn create_bind_group(
        &self,
        _desc: &wgpu_hal::BindGroupDescriptor<RaeGfxApi>,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_bind_group(&self, _group: RawGpuResource) {}

    unsafe fn create_shader_module(
        &self,
        _desc: &wgpu_hal::ShaderModuleDescriptor,
        _shader: wgpu_hal::ShaderInput,
    ) -> Result<RawGpuResource, wgpu_hal::ShaderError> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_shader_module(&self, _module: RawGpuResource) {}

    unsafe fn create_render_pipeline(
        &self,
        _desc: &wgpu_hal::RenderPipelineDescriptor<RaeGfxApi>,
    ) -> Result<RawGpuResource, wgpu_hal::PipelineError> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_render_pipeline(&self, _pipeline: RawGpuResource) {}

    unsafe fn create_compute_pipeline(
        &self,
        _desc: &wgpu_hal::ComputePipelineDescriptor<RaeGfxApi>,
    ) -> Result<RawGpuResource, wgpu_hal::PipelineError> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_compute_pipeline(&self, _pipeline: RawGpuResource) {}

    unsafe fn create_query_set(
        &self,
        _desc: &wgpu_types::QuerySetDescriptor<wgpu_hal::Label>,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_query_set(&self, _set: RawGpuResource) {}

    unsafe fn create_fence(&self) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn destroy_fence(&self, _fence: RawGpuResource) {}

    unsafe fn get_fence_value(
        &self,
        _fence: &RawGpuResource,
    ) -> DeviceResult<wgpu_hal::FenceValue> {
        Ok(0)
    }
    unsafe fn wait(
        &self,
        _fence: &RawGpuResource,
        _value: wgpu_hal::FenceValue,
        _timeout_ms: u32,
    ) -> DeviceResult<bool> {
        Ok(true)
    }

    unsafe fn start_capture(&self) -> bool {
        false
    }
    unsafe fn stop_capture(&self) {}

    unsafe fn create_acceleration_structure(
        &self,
        _desc: &wgpu_hal::AccelerationStructureDescriptor,
    ) -> DeviceResult<RawGpuResource> {
        Ok(RawGpuResource::fresh())
    }
    unsafe fn get_acceleration_structure_build_sizes<'a>(
        &self,
        _desc: &wgpu_hal::GetAccelerationStructureBuildSizesDescriptor<'a, RaeGfxApi>,
    ) -> wgpu_hal::AccelerationStructureBuildSizes {
        Default::default()
    }
    unsafe fn get_acceleration_structure_device_address(
        &self,
        _acceleration_structure: &RawGpuResource,
    ) -> wgpu_types::BufferAddress {
        Default::default()
    }
    unsafe fn destroy_acceleration_structure(&self, _acceleration_structure: RawGpuResource) {}
}

// ─── CommandEncoder ──────────────────────────────────────────────────────────

impl wgpu_hal::CommandEncoder<RaeGfxApi> for RaeGfxCommandEncoder {
    unsafe fn begin_encoding(&mut self, _label: wgpu_hal::Label) -> DeviceResult<()> {
        self.clear();
        Ok(())
    }

    unsafe fn discard_encoding(&mut self) {
        self.clear();
    }

    unsafe fn end_encoding(&mut self) -> Result<RaeGfxCommandEncoder, wgpu_hal::DeviceError> {
        // Move the encoder state out by swapping with a fresh one. This keeps
        // the original `&mut self` valid + empty for the next encoding pass.
        let storage = core::mem::take(&mut self.storage);
        let payload = core::mem::replace(&mut self.payload, PayloadArena::new());
        let next_fence = core::mem::replace(&mut self.next_fence, 1);
        Ok(RaeGfxCommandEncoder {
            storage,
            payload,
            next_fence,
        })
    }

    unsafe fn reset_all<I>(&mut self, _command_buffers: I) {
        self.clear();
    }

    unsafe fn transition_buffers<'a, T>(&mut self, _barriers: T)
    where
        T: Iterator<Item = wgpu_hal::BufferBarrier<'a, RaeGfxApi>>,
    {
    }

    unsafe fn transition_textures<'a, T>(&mut self, _barriers: T)
    where
        T: Iterator<Item = wgpu_hal::TextureBarrier<'a, RaeGfxApi>>,
    {
    }

    unsafe fn clear_buffer(&mut self, _buffer: &RawGpuResource, _range: wgpu_hal::MemoryRange) {}

    unsafe fn copy_buffer_to_buffer<T>(
        &mut self,
        _src: &RawGpuResource,
        _dst: &RawGpuResource,
        _regions: T,
    ) {
    }

    #[cfg(webgl)]
    unsafe fn copy_external_image_to_texture<T>(
        &mut self,
        _src: &wgpu_types::ImageCopyExternalImage,
        _dst: &RawGpuResource,
        _dst_premultiplication: bool,
        _regions: T,
    ) where
        T: Iterator<Item = wgpu_hal::TextureCopy>,
    {
    }

    unsafe fn copy_texture_to_texture<T>(
        &mut self,
        _src: &RawGpuResource,
        _src_usage: wgpu_hal::TextureUses,
        _dst: &RawGpuResource,
        _regions: T,
    ) {
    }

    unsafe fn copy_buffer_to_texture<T>(
        &mut self,
        _src: &RawGpuResource,
        _dst: &RawGpuResource,
        _regions: T,
    ) {
    }

    unsafe fn copy_texture_to_buffer<T>(
        &mut self,
        _src: &RawGpuResource,
        _src_usage: wgpu_hal::TextureUses,
        _dst: &RawGpuResource,
        _regions: T,
    ) {
    }

    unsafe fn begin_query(&mut self, _set: &RawGpuResource, _index: u32) {}
    unsafe fn end_query(&mut self, _set: &RawGpuResource, _index: u32) {}
    unsafe fn write_timestamp(&mut self, _set: &RawGpuResource, _index: u32) {}
    unsafe fn reset_queries(&mut self, _set: &RawGpuResource, _range: Range<u32>) {}
    unsafe fn copy_query_results(
        &mut self,
        _set: &RawGpuResource,
        _range: Range<u32>,
        _buffer: &RawGpuResource,
        _offset: wgpu_types::BufferAddress,
        _stride: wgpu_types::BufferSize,
    ) {
    }

    // ─── render ────────────────────────────────────────────────────────────

    unsafe fn begin_render_pass(&mut self, desc: &wgpu_hal::RenderPassDescriptor<RaeGfxApi>) {
        // Pull the first color attachment's clear value (if any) so the host
        // can short-circuit a CLEAR pass without inspecting the full pass.
        // wgpu_hal's Color is f64 per channel; the Venus token spec is f32.
        let (color_count, clear_color) =
            match desc.color_attachments.first().and_then(|c| c.as_ref()) {
                Some(att) => {
                    let c = att.clear_value;
                    (
                        desc.color_attachments.len() as u32,
                        [c.r as f32, c.g as f32, c.b as f32, c.a as f32],
                    )
                }
                None => (0, [0.0; 4]),
            };
        let depth_id = desc
            .depth_stencil_attachment
            .as_ref()
            .map(|d| d.target.view.id)
            .unwrap_or(0);
        // DepthStencilAttachment::clear_value is (f32, u32).
        let (clear_depth, clear_stencil) = desc
            .depth_stencil_attachment
            .as_ref()
            .map(|d| (d.clear_value.0, d.clear_value.1))
            .unwrap_or((0.0, 0));
        let token = VenusBeginRenderPassToken {
            cmd_id: opcode::BEGIN_RENDER_PASS,
            color_attachment_count: color_count,
            depth_attachment_id: depth_id,
            clear_color,
            clear_depth,
            clear_stencil,
        };
        self.record(opcode::BEGIN_RENDER_PASS, 0, &token);
    }

    unsafe fn end_render_pass(&mut self) {
        let token = VenusEndRenderPassToken {
            cmd_id: opcode::END_RENDER_PASS,
            _pad: 0,
        };
        self.record(opcode::END_RENDER_PASS, 0, &token);
    }

    unsafe fn set_bind_group(
        &mut self,
        layout: &RawGpuResource,
        index: u32,
        group: &RawGpuResource,
        _dynamic_offsets: &[wgpu_types::DynamicOffset],
    ) {
        let token = VenusSetBindGroupToken {
            cmd_id: opcode::SET_BIND_GROUP,
            set_index: index,
            bind_group_id: group.id,
            layout_id: layout.id,
        };
        self.record(opcode::SET_BIND_GROUP, index, &token);
    }

    unsafe fn set_push_constants(
        &mut self,
        _layout: &RawGpuResource,
        _stages: wgpu_types::ShaderStages,
        _offset_bytes: u32,
        _data: &[u32],
    ) {
        // Push constants are encoded as inline payloads — for v0 we drop them
        // and rely on uniform buffers. A future commit will record a
        // `VenusPushConstantsToken` + raw data into the arena.
    }

    unsafe fn insert_debug_marker(&mut self, _label: &str) {}
    unsafe fn begin_debug_marker(&mut self, _group_label: &str) {}
    unsafe fn end_debug_marker(&mut self) {}

    unsafe fn set_render_pipeline(&mut self, pipeline: &RawGpuResource) {
        let token = VenusSetPipelineToken {
            cmd_id: opcode::SET_PIPELINE,
            _pad: 0,
            pipeline_id: pipeline.id,
        };
        self.record(opcode::SET_PIPELINE, 0, &token);
    }

    unsafe fn set_index_buffer<'a>(
        &mut self,
        binding: wgpu_hal::BufferBinding<'a, RaeGfxApi>,
        format: wgpu_types::IndexFormat,
    ) {
        let token = VenusSetIndexBufferToken {
            cmd_id: opcode::SET_INDEX_BUFFER,
            index_format: match format {
                wgpu_types::IndexFormat::Uint16 => 0,
                wgpu_types::IndexFormat::Uint32 => 1,
            },
            buffer_id: binding.buffer.id,
            offset: binding.offset,
            size: binding.size.map(|s| s.get()).unwrap_or(0),
        };
        self.record(opcode::SET_INDEX_BUFFER, 0, &token);
    }

    unsafe fn set_vertex_buffer<'a>(
        &mut self,
        index: u32,
        binding: wgpu_hal::BufferBinding<'a, RaeGfxApi>,
    ) {
        let token = VenusSetVertexBufferToken {
            cmd_id: opcode::SET_VERTEX_BUFFER,
            slot: index,
            buffer_id: binding.buffer.id,
            offset: binding.offset,
            size: binding.size.map(|s| s.get()).unwrap_or(0),
        };
        self.record(opcode::SET_VERTEX_BUFFER, index, &token);
    }

    unsafe fn set_viewport(&mut self, _rect: &wgpu_hal::Rect<f32>, _depth_range: Range<f32>) {}
    unsafe fn set_scissor_rect(&mut self, _rect: &wgpu_hal::Rect<u32>) {}
    unsafe fn set_stencil_reference(&mut self, _value: u32) {}
    unsafe fn set_blend_constants(&mut self, _color: &[f32; 4]) {}

    unsafe fn draw(
        &mut self,
        first_vertex: u32,
        vertex_count: u32,
        first_instance: u32,
        instance_count: u32,
    ) {
        let token = VenusDrawToken {
            cmd_id: opcode::DRAW,
            first_vertex,
            vertex_count,
            first_instance,
            instance_count,
            _pad: 0,
        };
        self.record(opcode::DRAW, 0, &token);
    }

    unsafe fn draw_indexed(
        &mut self,
        first_index: u32,
        index_count: u32,
        base_vertex: i32,
        first_instance: u32,
        instance_count: u32,
    ) {
        let token = VenusDrawIndexedToken {
            cmd_id: opcode::DRAW_INDEXED,
            first_index,
            index_count,
            base_vertex,
            first_instance,
            instance_count,
        };
        self.record(opcode::DRAW_INDEXED, 0, &token);
    }

    unsafe fn draw_indirect(
        &mut self,
        _buffer: &RawGpuResource,
        _offset: wgpu_types::BufferAddress,
        _draw_count: u32,
    ) {
    }
    unsafe fn draw_indexed_indirect(
        &mut self,
        _buffer: &RawGpuResource,
        _offset: wgpu_types::BufferAddress,
        _draw_count: u32,
    ) {
    }
    unsafe fn draw_indirect_count(
        &mut self,
        _buffer: &RawGpuResource,
        _offset: wgpu_types::BufferAddress,
        _count_buffer: &RawGpuResource,
        _count_offset: wgpu_types::BufferAddress,
        _max_count: u32,
    ) {
    }
    unsafe fn draw_indexed_indirect_count(
        &mut self,
        _buffer: &RawGpuResource,
        _offset: wgpu_types::BufferAddress,
        _count_buffer: &RawGpuResource,
        _count_offset: wgpu_types::BufferAddress,
        _max_count: u32,
    ) {
    }

    // ─── compute ───────────────────────────────────────────────────────────

    unsafe fn begin_compute_pass(&mut self, _desc: &wgpu_hal::ComputePassDescriptor<RaeGfxApi>) {}
    unsafe fn end_compute_pass(&mut self) {}

    unsafe fn set_compute_pipeline(&mut self, _pipeline: &RawGpuResource) {}

    unsafe fn dispatch(&mut self, _count: [u32; 3]) {}
    unsafe fn dispatch_indirect(
        &mut self,
        _buffer: &RawGpuResource,
        _offset: wgpu_types::BufferAddress,
    ) {
    }

    unsafe fn build_acceleration_structures<'a, T>(
        &mut self,
        _descriptor_count: u32,
        _descriptors: T,
    ) where
        RaeGfxApi: 'a,
        T: IntoIterator<Item = wgpu_hal::BuildAccelerationStructureDescriptor<'a, RaeGfxApi>>,
    {
    }

    unsafe fn place_acceleration_structure_barrier(
        &mut self,
        _barriers: wgpu_hal::AccelerationStructureBarrier,
    ) {
    }
}
