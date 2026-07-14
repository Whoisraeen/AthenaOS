//! VirtIO Venus protocol token layouts.
//!
//! Each token is a `#[repr(C)]`, `Pod`+`Zeroable` struct that mirrors the
//! binary command the host Venus renderer (or the GPU-PV scanout when running
//! on real hardware) expects to dequeue from our shared command ring.
//!
//! The encoder serialises these into a per-encoder `PayloadArena` (a
//! `Vec<u8>`), then records the *byte offset* into the ring's
//! `GpuCommandPacket::payload_addr` field. At submit time the kernel half of
//! the queue resolves `arena_base + offset` and DMAs the right bytes.
//!
//! Why not raw pointers? Two reasons:
//!
//! 1. The previous `&payload as *const _` pattern was a use-after-free —
//!    the stack-allocated payload disappeared the moment `draw()` returned.
//! 2. The kernel-side consumer has no way to validate or remap a userspace
//!    pointer; an offset into a known buffer it has explicitly mapped is
//!    both safer and faster (no per-token IOMMU lookup).
//!
//! `cmd_id` values follow the Venus opcode space (see external Venus spec
//! mirror in `docs/components/raegfx.md`). The leading high byte distinguishes
//! resource ops (`0x01`), state ops (`0x02`), draws (`0x02xx`) and pass
//! control (`0x03xx`) so a single switch in the dispatcher can route them.

use bytemuck::{Pod, Zeroable};

// ─── Resource creation / destruction ────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusCreateResourceToken {
    pub cmd_id: u32,
    pub _pad: u32,
    pub resource_id: u64,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub usage: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusDestroyResourceToken {
    pub cmd_id: u32,
    pub _pad: u32,
    pub resource_id: u64,
}

// ─── Render pass control ────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusBeginRenderPassToken {
    pub cmd_id: u32,
    pub color_attachment_count: u32,
    pub depth_attachment_id: u64, // 0 == none
    pub clear_color: [f32; 4],
    pub clear_depth: f32,
    pub clear_stencil: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusEndRenderPassToken {
    pub cmd_id: u32,
    pub _pad: u32,
}

// ─── State change tokens ────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusSetPipelineToken {
    pub cmd_id: u32,
    pub _pad: u32,
    pub pipeline_id: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusSetBindGroupToken {
    pub cmd_id: u32,
    pub set_index: u32,
    pub bind_group_id: u64,
    pub layout_id: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusSetVertexBufferToken {
    pub cmd_id: u32,
    pub slot: u32,
    pub buffer_id: u64,
    pub offset: u64,
    pub size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusSetIndexBufferToken {
    pub cmd_id: u32,
    pub index_format: u32, // 0 = U16, 1 = U32
    pub buffer_id: u64,
    pub offset: u64,
    pub size: u64,
}

// ─── Draw tokens ────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusDrawToken {
    pub cmd_id: u32,
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub first_instance: u32,
    pub instance_count: u32,
    pub _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VenusDrawIndexedToken {
    pub cmd_id: u32,
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
    pub instance_count: u32,
}

// ─── Command opcodes ────────────────────────────────────────────────────────
//
// These IDs are duplicated into `GpuCommandPacket::cmd_type` so the dispatcher
// can route without first dereferencing the payload. Keep the constants here
// and below in sync.

pub mod opcode {
    // Resource lifecycle (0x01xx)
    pub const CREATE_RESOURCE: u32 = 0x0100;
    pub const DESTROY_RESOURCE: u32 = 0x0101;

    // Draws (0x02xx)
    pub const DRAW: u32 = 0x0200;
    pub const DRAW_INDEXED: u32 = 0x0201;

    // Render pass + state (0x03xx)
    pub const BEGIN_RENDER_PASS: u32 = 0x0300;
    pub const END_RENDER_PASS: u32 = 0x0301;
    pub const SET_BIND_GROUP: u32 = 0x0302;
    pub const SET_PIPELINE: u32 = 0x0303;
    pub const SET_INDEX_BUFFER: u32 = 0x0304;
    pub const SET_VERTEX_BUFFER: u32 = 0x0305;
}

// ─── Compile-time invariants ────────────────────────────────────────────────
//
// Tokens are transmitted to the GPU as raw bytes; their on-the-wire size is
// part of the ABI. If a layout drift sneaks in (someone reorders a field, or
// the compiler picks a different alignment on a new target), these constants
// will refuse to compile rather than silently corrupt the ring.

const _: () = {
    assert!(core::mem::size_of::<VenusCreateResourceToken>() == 32);
    assert!(core::mem::size_of::<VenusDestroyResourceToken>() == 16);
    assert!(core::mem::size_of::<VenusBeginRenderPassToken>() == 40);
    assert!(core::mem::size_of::<VenusEndRenderPassToken>() == 8);
    assert!(core::mem::size_of::<VenusSetPipelineToken>() == 16);
    assert!(core::mem::size_of::<VenusSetBindGroupToken>() == 24);
    assert!(core::mem::size_of::<VenusSetVertexBufferToken>() == 32);
    assert!(core::mem::size_of::<VenusSetIndexBufferToken>() == 32);
    assert!(core::mem::size_of::<VenusDrawToken>() == 24);
    assert!(core::mem::size_of::<VenusDrawIndexedToken>() == 24);
};
