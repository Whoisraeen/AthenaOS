//! amdgpu DRM userspace ABI (uapi) — the surface **Mesa** talks to.
//!
//! Mesa's radeonsi (GL) / radv (Vulkan) drivers do NOT link the kernel-side DRM
//! crate ([`ath_drm`]). They are userspace apps that link **libdrm_amdgpu** and
//! reach the driver through **DRM ioctls** on a render node. This module is the
//! AthenaOS-side of that contract: the ioctl command ids, the `AMDGPU_INFO`
//! sub-queries, the HW-IP / GEM-domain constants, and the byte-exact
//! `drm_amdgpu_info_device` struct — all taken verbatim from the upstream
//! `include/uapi/drm/amdgpu_drm.h` (cited per item) so a real Mesa winsys reads
//! the layout it expects.
//!
//! Phase 1 (this): the `AMDGPU_INFO` `DEV_INFO` query — the FIRST thing Mesa
//! reads, populated from the bring-up [`Device`] for the fields we know
//! authoritatively (the rest stay 0, matching the uapi's own "older chips set 0"
//! semantics — never fabricate a value). Phases 2/3 (the GEM/CS/WAIT_CS ioctl
//! handlers that map onto `dma_alloc` / the ring submit / the fence-poll) are
//! mapped out in `docs/research/mesa-amdgpu-seam.md`.

use crate::bringup::Device;

// ── DRM amdgpu ioctl command numbers (DRM_AMDGPU_*, amdgpu_drm.h) ─────────────
pub const DRM_AMDGPU_GEM_CREATE: u32 = 0x00;
pub const DRM_AMDGPU_GEM_MMAP: u32 = 0x01;
pub const DRM_AMDGPU_CTX: u32 = 0x02;
pub const DRM_AMDGPU_CS: u32 = 0x04;
pub const DRM_AMDGPU_INFO: u32 = 0x05;
pub const DRM_AMDGPU_GEM_VA: u32 = 0x08;
pub const DRM_AMDGPU_WAIT_CS: u32 = 0x09;
pub const DRM_AMDGPU_FENCE_TO_HANDLE: u32 = 0x14;

// ── AMDGPU_INFO sub-query ids (the `query` field of struct drm_amdgpu_info) ───
pub const AMDGPU_INFO_HW_IP_INFO: u32 = 0x02;
pub const AMDGPU_INFO_TIMESTAMP: u32 = 0x05;
pub const AMDGPU_INFO_FW_VERSION: u32 = 0x0e;
pub const AMDGPU_INFO_VRAM_USAGE: u32 = 0x10;
pub const AMDGPU_INFO_DEV_INFO: u32 = 0x16;
pub const AMDGPU_INFO_MEMORY: u32 = 0x19;

// ── HW IP block ids (AMDGPU_HW_IP_*, for HW_IP_INFO + CS ring selection) ──────
pub const AMDGPU_HW_IP_GFX: u32 = 0;
pub const AMDGPU_HW_IP_COMPUTE: u32 = 1;
pub const AMDGPU_HW_IP_DMA: u32 = 2;

// ── GEM memory domains (AMDGPU_GEM_DOMAIN_*) ─────────────────────────────────
pub const AMDGPU_GEM_DOMAIN_GTT: u32 = 0x2;
pub const AMDGPU_GEM_DOMAIN_VRAM: u32 = 0x4;

/// gfx11 (RDNA3) wavefront width — an architectural constant Mesa needs.
pub const GFX11_WAVE_FRONT_SIZE: u32 = 32;
/// Standard GART/VM page size + VA alignment.
pub const GART_PAGE_SIZE: u32 = 4096;

/// `struct drm_amdgpu_info_device` — returned for `AMDGPU_INFO_DEV_INFO`. Byte
/// layout transcribed FIELD-FOR-FIELD from `amdgpu_drm.h`; `repr(C)` reproduces
/// the C ABI exactly (all fields are naturally-aligned u32/u64/arrays). The
/// trailing `pad` keeps the size 8-aligned, as in the C source.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrmAmdgpuInfoDevice {
    pub device_id: u32,
    pub chip_rev: u32,
    pub external_rev: u32,
    pub pci_rev: u32,
    pub family: u32,
    pub num_shader_engines: u32,
    pub num_shader_arrays_per_engine: u32,
    pub gpu_counter_freq: u32,
    pub max_engine_clock: u64,
    pub max_memory_clock: u64,
    pub cu_active_number: u32,
    pub cu_ao_mask: u32,
    pub cu_bitmap: [[u32; 4]; 4],
    pub enabled_rb_pipes_mask: u32,
    pub num_rb_pipes: u32,
    pub num_hw_gfx_contexts: u32,
    pub pcie_gen: u32,
    pub ids_flags: u64,
    pub virtual_address_offset: u64,
    pub virtual_address_max: u64,
    pub virtual_address_alignment: u32,
    pub pte_fragment_size: u32,
    pub gart_page_size: u32,
    pub ce_ram_size: u32,
    pub vram_type: u32,
    pub vram_bit_width: u32,
    pub vce_harvest_config: u32,
    pub gc_double_offchip_lds_buf: u32,
    pub prim_buf_gpu_addr: u64,
    pub pos_buf_gpu_addr: u64,
    pub cntl_sb_buf_gpu_addr: u64,
    pub param_buf_gpu_addr: u64,
    pub prim_buf_size: u32,
    pub pos_buf_size: u32,
    pub cntl_sb_buf_size: u32,
    pub param_buf_size: u32,
    pub wave_front_size: u32,
    pub num_shader_visible_vgprs: u32,
    pub num_cu_per_sh: u32,
    pub num_tcc_blocks: u32,
    pub gs_vgt_table_depth: u32,
    pub gs_prim_buffer_depth: u32,
    pub max_gs_waves_per_vgt: u32,
    pub pcie_num_lanes: u32,
    pub cu_ao_bitmap: [[u32; 4]; 4],
    pub high_va_offset: u64,
    pub high_va_max: u64,
    pub pa_sc_tile_steering_override: u32,
    pub tcc_disabled_mask: u64,
    pub min_engine_clock: u64,
    pub min_memory_clock: u64,
    // gfx11+ only; older chips report 0.
    pub tcp_cache_size: u32,
    pub num_sqc_per_wgp: u32,
    pub sqc_data_cache_size: u32,
    pub sqc_inst_cache_size: u32,
    pub gl1c_cache_size: u32,
    pub gl2c_cache_size: u32,
    pub mall_size: u64,
    pub enabled_rb_pipes_mask_hi: u32,
    pub shadow_size: u32,
    pub shadow_alignment: u32,
    pub csa_size: u32,
    pub csa_alignment: u32,
    pub userq_ip_mask: u32,
    pub pad: u32,
}

/// Build the `AMDGPU_INFO_DEV_INFO` reply from the bring-up [`Device`] — what
/// Mesa's winsys reads first to identify + configure the GPU. Only the fields we
/// know AUTHORITATIVELY are set: the PCI device id, the VBIOS-decoded bootup
/// engine/memory clocks (MHz → the field's KHz), and the gfx11 architectural
/// constants (wave size, page size). Everything else (CU/RB counts, VRAM type +
/// bit width, cache sizes) comes from the GPU's `gc_info` / the discovery table —
/// left 0 until iron rather than guessed, the same discipline as the rest of the
/// driver. `family` is the one chip-dispatch field that needs the kernel's
/// `AMDGPU_FAMILY_*` constant (header moved upstream); flagged, not fabricated.
pub fn query_dev_info(dev: &Device) -> DrmAmdgpuInfoDevice {
    DrmAmdgpuInfoDevice {
        device_id: dev.device as u32,
        // VBIOS firmware-info bootup clocks (stage 2); 0 on QEMU (no VBIOS).
        max_engine_clock: dev.bootup_sclk_mhz as u64 * 1000,
        max_memory_clock: dev.bootup_mclk_mhz as u64 * 1000,
        wave_front_size: GFX11_WAVE_FRONT_SIZE,
        gart_page_size: GART_PAGE_SIZE,
        virtual_address_alignment: GART_PAGE_SIZE,
        // family: AMDGPU_FAMILY_GC_11_0_1 — verify-pending (kernel header moved).
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bringup::{AMD_VENDOR, RADEON_760M};
    use core::mem::{offset_of, size_of};

    #[test]
    fn info_device_abi_layout() {
        // ABI guard: catch field-order / padding drift vs amdgpu_drm.h. The first
        // u64 lands after the 8 leading u32s; `family` is the 5th u32; the trailing
        // `pad` keeps the whole struct 8-aligned.
        assert_eq!(offset_of!(DrmAmdgpuInfoDevice, device_id), 0);
        assert_eq!(offset_of!(DrmAmdgpuInfoDevice, family), 16);
        assert_eq!(offset_of!(DrmAmdgpuInfoDevice, max_engine_clock), 32);
        assert_eq!(size_of::<DrmAmdgpuInfoDevice>() % 8, 0);
    }

    #[test]
    fn query_dev_info_fills_known_fields() {
        let dev = Device {
            handle: 1,
            vendor: AMD_VENDOR,
            device: RADEON_760M, // 0x15bf
            vram_base: 0,
            vram_size: 2048 * 1024 * 1024,
            bootup_sclk_mhz: 800,
            bootup_mclk_mhz: 1600,
        };
        let info = query_dev_info(&dev);
        assert_eq!(info.device_id, 0x15bf);
        assert_eq!(info.max_engine_clock, 800_000); // MHz -> KHz
        assert_eq!(info.max_memory_clock, 1_600_000);
        assert_eq!(info.wave_front_size, 32);
        assert_eq!(info.gart_page_size, 4096);
        // Unknown-on-host fields are 0, never fabricated.
        assert_eq!(info.num_shader_engines, 0);
        assert_eq!(info.vram_type, 0);
    }

    #[test]
    fn no_vbios_clocks_stay_zero() {
        // QEMU path: no VBIOS -> bootup clocks 0 -> the clock fields read 0.
        let dev = Device {
            handle: 1,
            vendor: AMD_VENDOR,
            device: RADEON_760M,
            vram_base: 0,
            vram_size: 0,
            bootup_sclk_mhz: 0,
            bootup_mclk_mhz: 0,
        };
        let info = query_dev_info(&dev);
        assert_eq!(info.max_engine_clock, 0);
        assert_eq!(info.max_memory_clock, 0);
    }
}
