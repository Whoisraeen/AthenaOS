//! raeen_amdgpu — pure AMD GPU command/ROM logic for the `amdgpud` daemon.
//!
//! This crate holds the parts of the AMD GPU bring-up that are *pure functions
//! of bytes* — PM4 command-stream construction, ATOMBIOS ROM parsing, and the
//! GC 11.0.1 register map — with NO dependency on the LinuxKPI syscall shim. So
//! it builds on the host and every encoding is validated by `cargo test -p
//! raeen_amdgpu` (the same host-KAT discipline used for `rae_crypto`), rather
//! than only being exercisable on the Athena Radeon 780M.
//!
//! What lives here:
//! * [`pm4`] — Type-3 (PKT3) command packet builders the GFX command processor
//!   consumes (NOP, WRITE_DATA, SET_SH_REG, EVENT_WRITE, RELEASE_MEM,
//!   INDIRECT_BUFFER, DISPATCH_DIRECT).
//! * [`atombios`] — VBIOS/ATOMBIOS ROM header + master-table-offset parsing
//!   (`amdgpu_atombios.c` `amdgpu_atombios_init` does this on iron).
//! * [`gc11`] — GC 11.0.1 (Phoenix) MMIO register offsets used by GFX/CP ring
//!   programming.
//! * [`bringup`] — the `amdgpu_device_init` → `*_ip_init` STAGE SEQUENCE,
//!   expressed over a [`bringup::GpuOps`] trait so the exact ordering/handshake
//!   logic runs in BOTH the live `amdgpud` daemon (real LinuxKPI syscalls) and
//!   the host harness (a mock register file) — no QEMU/iron needed to catch a
//!   sequencing bug.
//!
//! `amdgpud` (the `no_std` daemon) implements `GpuOps` over the LinuxKPI shim and
//! runs `bringup::bringup`, building command streams with `pm4`/`gc11` and
//! submitting them through the LinuxKPI DMA rings.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod atombios;
pub mod bringup;
pub mod discovery;
pub mod gart;
pub mod gc11;
pub mod imu;
pub mod mes;
pub mod pm4;
pub mod regs;
pub mod rlc_autoload;
pub mod sdma;
pub mod uapi;
