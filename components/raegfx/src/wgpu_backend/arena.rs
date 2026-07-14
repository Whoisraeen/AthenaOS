//! Per-encoder payload arena.
//!
//! Each `RaeGfxCommandEncoder` owns a `PayloadArena`: a contiguous byte buffer
//! that backs every `GpuCommandPacket::payload_addr` recorded during the
//! encoder's lifetime. The arena lives as long as the command buffer; it is
//! handed to the kernel verbatim at submit time, where the GPU-PV scanout (or
//! VirtIO-GPU host) reads the bytes directly through its DMA mapping.
//!
//! This replaces the previous, *broken* pattern of casting a stack-local
//! token to `*const _` and storing it in the ring — the moment `draw()`
//! returned, that pointer dangled. With an arena, the bytes outlive the
//! recording call and the offsets recorded in the packets are stable.

extern crate alloc;
use alloc::vec::Vec;
use bytemuck::Pod;

/// Byte offset into a `PayloadArena`. Stored verbatim in
/// `GpuCommandPacket::payload_addr`; the host resolves
/// `arena_base + offset` at execute time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaOffset(pub u64);

impl ArenaOffset {
    pub const INVALID: Self = ArenaOffset(u64::MAX);
}

/// Append-only byte buffer. We never reuse offsets — once a token is written
/// it stays put until the encoder is reset, which lets us hand out offsets
/// while still recording without invalidating earlier ones.
#[derive(Debug, Default)]
pub struct PayloadArena {
    buf: Vec<u8>,
}

impl PayloadArena {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    /// Copy `token` into the arena, return the offset it landed at.
    ///
    /// `T: Pod` guarantees the type is plain-old-data (no padding bytes that
    /// could leak undefined memory; no references that could dangle).
    /// `bytemuck::bytes_of` is a safe transmute to `&[u8]`.
    pub fn push<T: Pod>(&mut self, token: &T) -> ArenaOffset {
        // Pad to T's alignment so the GPU-side reader can do an aligned load.
        // For all current Venus tokens this is a no-op (each is 8-byte aligned
        // and we follow them with 8-byte aligned tokens), but the explicit
        // padding here means a future 16-byte-aligned token won't silently
        // corrupt the previous one.
        let align = core::mem::align_of::<T>();
        let pad = (align - (self.buf.len() % align)) % align;
        for _ in 0..pad {
            self.buf.push(0);
        }

        let offset = self.buf.len() as u64;
        self.buf.extend_from_slice(bytemuck::bytes_of(token));
        ArenaOffset(offset)
    }

    /// Bytes recorded so far. The kernel's submit path reads through this
    /// slice; do not mutate after submission.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Reset the arena, keeping the allocated capacity. Used when an encoder
    /// is recycled across frames.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wgpu_backend::tokens::{opcode, VenusDrawToken};

    #[test]
    fn arena_push_returns_monotonic_offsets() {
        let mut a = PayloadArena::new();
        let t1 = VenusDrawToken {
            cmd_id: opcode::DRAW,
            first_vertex: 0,
            vertex_count: 3,
            first_instance: 0,
            instance_count: 1,
            _pad: 0,
        };
        let t2 = VenusDrawToken {
            cmd_id: opcode::DRAW,
            first_vertex: 0,
            vertex_count: 6,
            first_instance: 0,
            instance_count: 1,
            _pad: 0,
        };
        let o1 = a.push(&t1);
        let o2 = a.push(&t2);
        assert_eq!(o1, ArenaOffset(0));
        assert_eq!(o2.0, core::mem::size_of::<VenusDrawToken>() as u64);
    }
}
