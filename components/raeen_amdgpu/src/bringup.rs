//! amdgpu device bring-up — the real `amdgpu_device_init` → `*_ip_init` stage
//! sequence, expressed over a [`GpuOps`] trait so it runs in BOTH the live
//! `amdgpud` daemon (real LinuxKPI syscalls) and the host harness (a mock GPU
//! register file). This is the part that used to live in `amdgpud/src/main.rs`
//! tightly coupled to the syscall shim; extracted here so the ORDERING/HANDSHAKE
//! logic is host-testable with no QEMU/iron (`tools/linuxkpi_harness`).
//!
//! `#![forbid(unsafe_code)]` (crate-wide): the trait surface is safe — DMA
//! buffers are opaque tokens the impl interprets, never dereferenced here.

use crate::{atombios, discovery, gart, gc11, pm4, sdma};
use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

/// Whether THIS init has already sent `PPSMC_MSG_EnableGfxImu` (any path). The
/// working driver sends it exactly ONCE per init (cold mmiotrace t=43.6479);
/// iron boot #2 of 2026-07-02 took the cold-GFX path (power-cycled GPU), where
/// the PSP-autoload block sends EnableGfxImu — and the machine hard-reset
/// seconds later, right where the first-light branch would have sent it a
/// SECOND time onto the just-powered domain. A double EnableGfxImu is
/// un-oracle'd PMFW input on an APU whose PMFW owns the SoC rails, so every
/// send site records itself here and the first-light branch only sends when no
/// earlier path did. Cleared at [`probe`] (one probe per init; the host-KAT
/// full sequence enters through probe as well).
static ENABLE_GFX_IMU_SENT: AtomicBool = AtomicBool::new(false);

/// AMD PCI vendor; Phoenix1 APU iGPU device id (Radeon 760M/780M — Athena's
/// Ryzen 5 7640HS reports 0x15BF, confirmed by its boot log: `c4:00.0 1002:15bf`).
pub const AMD_VENDOR: u16 = 0x1002;
pub const RADEON_760M: u16 = 0x15BF;
/// PCI display-controller class code (for match-mode probe).
pub const PCI_CLASS_DISPLAY: u8 = 0x03;

/// A DMA allocation handed back by [`GpuOps::dma_alloc`]. Opaque to this crate:
/// `id` is impl-private (real: the LinuxKPI token; mock: a buffer index), and
/// `dma_addr` is what gets programmed into the hardware ring registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaBuf {
    pub dma_addr: u64,
    pub size: usize,
    pub id: u64,
}

/// The platform operations amdgpu bring-up needs. The live daemon implements
/// these over `raeen_linuxkpi` (real syscalls); the harness implements them over
/// a mock register file + a hardware-reaction model.
pub trait GpuOps {
    /// Claim a PCI device by bus:dev.func; `Some(handle)` or `None` if absent.
    fn pci_enable(&mut self, bus: u8, dev: u8, func: u8) -> Option<u64>;
    /// Claim the first device matching PCI `class` + `vendor` (0 = any vendor)
    /// — the Linux id-table binding model, so the probe finds the GPU wherever
    /// the firmware put it (00:01.0 on QEMU, c4:00.0 on Athena). Default `None`
    /// keeps BDF-only impls (the harness mock) working via the fallback list.
    fn pci_enable_match(&mut self, _class: u8, _vendor: u16) -> Option<u64> {
        None
    }
    /// Read a 32-bit PCI config dword.
    fn config_read_dword(&mut self, handle: u64, offset: u16) -> u32;
    /// Map a BAR's MMIO register aperture; `true` if mappable. Subsequent
    /// `reg_read`/`reg_write` target the most-recently-mapped register BAR.
    fn map_register_bar(&mut self, handle: u64, bar: u8) -> bool;
    /// MMIO register read/write by byte offset within the mapped register BAR.
    fn reg_read(&mut self, off: u32) -> u32;
    fn reg_write(&mut self, off: u32, val: u32);
    /// Delay ~`usec` microseconds, YIELDING the CPU. Default no-op so the mock +
    /// QEMU run at full speed; the daemon implements it via `raeen_linuxkpi::msleep`
    /// so the SMU mailbox poll loops actually span the PMFW's ~1s response budget
    /// (the real `smu_cmn` handshake `udelay(1)`s up to `usec_timeout`) instead of
    /// firing a few thousand back-to-back MMIO reads in microseconds — and so the
    /// wait yields CPU 0 to the bootlog-flush thread rather than busy-spinning it.
    fn delay_us(&mut self, _usec: u32) {}
    /// Read up to `max_len` bytes of the expansion-ROM (VBIOS), if mappable.
    fn read_vbios_rom(&mut self, handle: u64, max_len: usize) -> Option<Vec<u8>>;
    /// Allocate physically-contiguous, IOMMU-sandboxed DMA memory.
    fn dma_alloc(&mut self, handle: u64, size: usize) -> Option<DmaBuf>;
    /// Write dwords into a previously-allocated DMA buffer at a dword offset.
    fn dma_write(&mut self, buf: &DmaBuf, offset_dw: usize, data: &[u32]);
    /// Copy raw bytes into a DMA buffer at `byte_offset` — for the multi-MiB,
    /// byte-oriented RLC autoload buffer (not the dword `dma_write`). Default
    /// no-op; the daemon memcpys into the buffer's CPU mapping.
    fn dma_write_bytes(&mut self, _buf: &DmaBuf, _byte_offset: usize, _data: &[u8]) {}
    /// Read `out.len()` dwords from a DMA buffer at `offset_dw` — used to poll a
    /// fence/writeback the GPU posts to memory. Default fills zeros (impls that
    /// don't expose readable DMA memory); the mock and the real daemon read the
    /// CPU-mapped buffer.
    fn dma_read(&mut self, _buf: &DmaBuf, _offset_dw: usize, out: &mut [u32]) {
        out.iter_mut().for_each(|x| *x = 0);
    }
    /// Ring a hardware doorbell: a 64-bit write of `value` at `byte_offset` into the
    /// GPU's doorbell BAR (a SEPARATE BAR from the register BAR5 — BAR2 at phys
    /// 0xdc000000 on Athena). This is how the SDMA/CP engines are woken to consume a
    /// ring (the live amdgpu wakes SDMA0 QUEUE0 via a 64-bit write to doorbell+0x800
    /// = WPTR-in-bytes). Default no-op (QEMU/host have no doorbell BAR); the daemon
    /// ioremaps BAR2 and does the 64-bit write.
    fn ring_doorbell(&mut self, _byte_offset: u32, _value: u64) {}
    /// Load a firmware blob by name (e.g. "amdgpu/gc_11_0_1_pfp.bin").
    fn request_firmware(&mut self, name: &str) -> bool;
    /// Load a firmware blob by name and return its BYTES, for blobs the driver
    /// must parse rather than merely hand to hardware (e.g. the VFCT-captured
    /// VBIOS on APUs, which have no expansion ROM). Default `None` so impls
    /// without byte access (the harness mock) skip byte-level parsing.
    fn request_firmware_bytes(&mut self, _name: &str) -> Option<Vec<u8>> {
        None
    }
    /// The VRAM carve-out size in MB, as published by the CONFIG_MEMSIZE
    /// register (Linux nbio `get_memsize`). `None` (the default) when the
    /// platform can't read it — QEMU, or iron before the register offset is
    /// confirmed. Returning `None` keeps `init_gmc` from reading a *guessed*
    /// MMIO offset on real hardware (which could have read side effects); it
    /// falls back to a conservative default instead.
    fn config_memsize_mb(&mut self) -> Option<u32> {
        None
    }
    /// The SMU mailbox register offsets for this ASIC, once confirmed. `None`
    /// (the default) until the iron offsets are verified — so `init_smu` does
    /// not *write* guessed MMIO on real hardware. The daemon returns `Some`
    /// after the offsets are dumped from Athena.
    fn smu_mailbox(&mut self) -> Option<SmuMailbox> {
        None
    }
    /// The PSP (MP0) mailbox + ring register offsets for this ASIC (`psp_v13_0`).
    /// `None` (the default) until the daemon confirms discovery — so the PSP
    /// firmware-load path never poke-guesses MMIO on iron. This is the channel that
    /// cold-starts GFX on a PSP-load APU (boot 041507: no host/SMU power-up exists).
    fn psp_regs(&mut self) -> Option<PspRegs> {
        None
    }
    /// The GPU MC base address of VRAM (the FB aperture, `0x8000000000` on Phoenix,
    /// = MMHUB `MMMC_VM_FB_LOCATION_BASE << 24`, Athena-oracle-verified: Linux's PSP
    /// ring sits at MC 0x8000339000). PSP buffers (ring/cmd/fw) must carry a GPU MC
    /// address, not a CPU/bus address. `None` until the daemon maps the VRAM aperture
    /// (BAR0), so QEMU never runs the PSP ring path.
    fn vram_mc_base(&mut self) -> Option<u64> {
        None
    }
    /// Zero `len` bytes of VRAM at `offset` through the CPU VRAM aperture (BAR0).
    /// Used to clear a PSP buffer before handing the GPU MC address to the PSP.
    /// Default no-op (QEMU / no aperture). VRAM is the GPU's own carve-out, so this
    /// cannot corrupt host OS memory — only unused VRAM.
    fn vram_zero(&mut self, _offset: u64, _len: usize) {}
    /// Write dwords into VRAM at byte `offset` through the CPU VRAM aperture (BAR0).
    /// Used to stage the PSP GPCOM command buffer + the ring frame (the PSP DMAs them
    /// from VRAM). Default no-op (QEMU / no aperture) — keeps [`psp_submit_gpcom`] from
    /// running until the daemon maps BAR0. VRAM-only, cannot touch host OS memory.
    fn vram_write(&mut self, _offset: u64, _data: &[u32]) {}
    /// Read dwords from VRAM at byte `offset` (BAR0). Used to poll the PSP completion
    /// fence + read back the command response. Default fills zeros (so the fence never
    /// matches → `psp_submit_gpcom` times out gracefully until the daemon maps BAR0).
    fn vram_read(&mut self, _offset: u64, out: &mut [u32]) {
        out.iter_mut().for_each(|x| *x = 0);
    }
    /// The GPU MC address + byte size of the firmware TOC blob the daemon has staged
    /// in VRAM (from the RLC firmware's `rlc_toc`). `None` (default) until the daemon
    /// loads + stages it — gates [`psp_load_gfx_firmware`] off on QEMU/iron-without-fw.
    fn psp_toc_blob(&mut self) -> Option<(u64, u32)> {
        None
    }
    /// The GPU MC base address for the Trusted Memory Region (Athena oracle:
    /// 0x8078000000). `None` (default) until the daemon reserves it; the PSP-load
    /// firmware handshake places the authenticated firmware here.
    fn psp_tmr_base(&mut self) -> Option<u64> {
        None
    }
    /// The interrupt-handler (IH) ring register offsets for this ASIC, once
    /// confirmed. `None` (the default) until iron-verified — so `init_ih` does
    /// not write guessed MMIO on real hardware; it allocates the ring only.
    fn ih_ring(&mut self) -> Option<IhRing> {
        None
    }
    /// The `RLC_SAFE_MODE` register offset for this ASIC, once confirmed. `None`
    /// (the default) until iron-verified — so the GFXOFF-ungate path writes NO
    /// guessed MMIO on real hardware and the CP stage proceeds unchanged. The
    /// daemon returns `Some` after the offset is dumped from Athena.
    fn rlc_safe_mode(&mut self) -> Option<RlcSafeMode> {
        None
    }
    /// The SMU `PPSMC_MSG_DisallowGfxOff` message id for this PMFW, once
    /// confirmed. `None` (default) until iron-verified — version-specific, so we
    /// never send a guessed message id. Used with [`GpuOps::smu_mailbox`] to stop
    /// the PMFW re-gating GFX while the CP is programmed.
    fn gfx_off_disable_msg(&mut self) -> Option<u32> {
        None
    }
    /// The SMU `PPSMC_MSG_EnableGfxImu` id (`0x16` on smu_v13_0_4, header-verified).
    ///
    /// This is the cold GFX power-up: `smu_v13_0_set_gfx_power_up_by_imu` sends it with
    /// `ENABLE_IMU_ARG_GFXOFF_ENABLE=1`. On the DIRECT-load path it is sent ASYNC
    /// (`smu_msg_send_async_locked`) — the PMFW does NOT post a response, so boot
    /// 185829's "resp None" was the async semantics, not a failure (removing it was the
    /// mistake). Stage 6 re-sends it via [`smu_send_msg_async`] before the I-RAM load
    /// to bring the gated GFX domain alive (boot 034417 proved the high GC registers
    /// read 0xffffffff). `None` (default) keeps QEMU from sending a guessed id.
    fn enable_gfx_imu_msg(&mut self) -> Option<u32> {
        None
    }
    /// The raw `gc_*_imu.bin` blob, for DIRECT-streaming the IMU ucode into the IMU's
    /// I-RAM/D-RAM ([`imu_load_microcode`]). On this Phoenix APU the PSP loads the PMFW but
    /// NOT the GFX/IMU firmware, so unless we stream the IMU ucode ourselves the IMU core has
    /// no program (CORE_STATUS=0) and GFX never leaves reset. `None` (default) on QEMU / before
    /// the daemon loads it.
    fn imu_fw_blob(&mut self) -> Option<Vec<u8>> {
        None
    }
    /// The SMU `PPSMC_MSG_GfxDeviceDriverReset` id (`0x11` on smu_v13_0_4,
    /// header-verified). amdgpu sends this with arg `SMU_RESET_MODE_2` to scrub a
    /// "dirty" GFX/compute/SDMA domain — the state a non-cold (warm) boot inherits from
    /// the previous boot that a cold power-up alone cannot clear (and a PCI FLR does NOT
    /// clear on this APU). Used ONLY on the GFX-DOWN branch, so a cold boot (GFX up via
    /// PSP autoload) never sends it. `None` (default) keeps QEMU from sending a guessed
    /// id.
    fn gfx_device_driver_reset_msg(&mut self) -> Option<u32> {
        None
    }
    /// SOC15-correct gfx11 CP/GRBM register offsets, resolved from the IP
    /// discovery table ([`crate::discovery`] + [`crate::regs::gfx_regs`]). `None`
    /// (the default) means "not resolved" — QEMU, or iron before discovery is
    /// read — and stage 6 falls back to the `gc11` constants, preserving current
    /// behavior. On iron the daemon returns `Some` (discovery-resolved), which
    /// fixes the CP-ring readback MISMATCH caused by `gc11`'s LEGACY (pre-SOC15)
    /// offsets addressing the wrong gfx11 registers.
    fn gfx_regs(&mut self) -> Option<GfxRegs> {
        None
    }
    /// The OR of the gfx11 RS64 `CP_ME_CNTL` halt bits (PFP + ME, and the compute
    /// MEC halt) for this ASIC — clearing them releases the CP from halt
    /// ([`cp_gfx_enable`]). `None` (the default) until the mask is confirmed from
    /// `gc_11_0_0_sh_mask.h` (the `CP_ME_CNTL__*_HALT_MASK` values), so the enable
    /// is a NO-OP rather than a *guessed* write to the live CP register on real
    /// hardware (the CP drives the GOP display — a wrong write blanks the screen).
    /// The daemon returns `Some` once the mask is harvested (the same WSL
    /// `curl`+`grep` path that sourced `regs.rs`); the host mock supplies it so the
    /// enable logic is still FAIL-able off-target.
    fn cp_me_cntl_halt_mask(&mut self) -> Option<u32> {
        None
    }
    /// SOC15-resolved SDMA0 QUEUE0 ring register offsets ([`crate::regs::sdma_regs`]).
    /// `None` (the default) means "not resolved" (QEMU / pre-discovery) — stage 6
    /// then builds the SDMA command stream but does NOT program/submit the ring,
    /// so no guessed MMIO is written. The daemon returns `Some` once discovery is
    /// active. See [`program_sdma_ring`].
    fn sdma_regs(&mut self) -> Option<SdmaRegs> {
        None
    }
    /// The remaining gfx11 CP gfx-ring registers (`CP_RB_ACTIVE`, `CP_RB_VMID`,
    /// the RPTR/WPTR writeback addresses) that complete `gfx_v11_0_cp_gfx_resume`.
    /// `None` (the default) means "not resolved" (QEMU / pre-discovery) — stage 6
    /// then programs only BASE/CNTL/WPTR (legacy partial path). The daemon returns
    /// `Some` once discovery is active, so the ring is fully programmed +
    /// ACTIVATED (`CP_RB_ACTIVE = 1`). See [`crate::regs::cp_gfx_ring_regs`].
    fn cp_gfx_ring_regs(&mut self) -> Option<CpGfxRingRegs> {
        None
    }
    /// RS64 CP startup register offsets (`gfx_v11_0_config_gfx_rs64`). `None`
    /// (default) until discovery resolves them — so [`config_gfx_rs64`] never
    /// pokes the live CP with a guessed offset. The daemon returns `Some`.
    fn rs64_cp_regs(&mut self) -> Option<Rs64CpRegs> {
        None
    }
    /// The PFP/ME/MEC RS64 program-counter START addresses (from the fw headers).
    /// `None` (default) until the daemon parses the gfx11 RS64 ucode headers — so
    /// [`config_gfx_rs64`] writes no guessed program counter. Gating both this and
    /// [`Self::rs64_cp_regs`] keeps the RS64 start a NO-OP on QEMU / pre-discovery.
    fn rs64_ucode_starts(&mut self) -> Option<Rs64UcodeStarts> {
        None
    }
    /// gfxhub GPUVM state register offsets (read-only). `None` (default) until
    /// discovery resolves them; the daemon returns `Some` so [`log_gmc_vm_state`]
    /// can dump the firmware's VM config for the GART-inheritance investigation.
    fn gmc_vm_regs(&mut self) -> Option<GmcVmRegs> {
        None
    }
    /// MES engine-enable register offsets ([`crate::mes::build_mes_enable_sequence`]).
    /// `None` (default) until discovery resolves them — so MES start never pokes a
    /// guessed offset pre-iron. The daemon returns `Some`.
    fn mes_enable_regs(&mut self) -> Option<crate::mes::MesEnableRegs> {
        None
    }
    /// The MES microengine entry points `(pipe0 scheduler start, optional pipe1 KIQ
    /// start)`, parsed from the mes_2.bin / mes1.bin headers
    /// ([`crate::mes::parse_mes_uc_start_addr`]). `None` (default) until the daemon
    /// parses them — so MES start never loads a guessed program counter.
    fn mes_uc_starts(&mut self) -> Option<(u64, Option<u64>)> {
        None
    }
    /// MES instruction/data-cache base register offsets
    /// ([`crate::mes::build_mes_load_sequence`]). `None` (default) until discovery
    /// resolves them. The daemon returns `Some`.
    fn mes_load_regs(&mut self) -> Option<crate::mes::MesLoadRegs> {
        None
    }
    /// The MES scheduler (pipe 0) ucode + data blobs `(ucode, data)` extracted from
    /// mes_2.bin ([`crate::rlc_autoload::extract_mes_ucode_data`]) — RaeenOS DIRECT-
    /// loads them (the PSP-autoloaded copy's address is unknown) so the MES has code
    /// at a GART-mappable address. `None` (default) until the daemon supplies them.
    fn mes_ucode_blobs(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        None
    }
    /// The MES **KIQ** (pipe 1) ucode + data blobs `(ucode, data)` from mes1.bin. The
    /// KIQ is the bootstrap queue that maps the SCHED ring; `mes_v11_0_kiq_hw_init`
    /// loads BOTH pipes before enable, so without this the KIQ microengine (pipe 1)
    /// has no code and never services its ring. `None` (default) until the daemon
    /// supplies them (and when None the bring-up loads only pipe 0, as before).
    fn mes_kiq_ucode_blobs(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        None
    }
    /// The 8-segment IP register bases for GC / MMHUB / OSSSYS (`reg_offset[HWIP][0]
    /// [0..8]`), discovery-resolved — the MES `set_hw_resources` packet hands these to
    /// the scheduler so it can drive the hardware. `None` (default) until discovery.
    fn mes_hqd_regs(&mut self) -> Option<crate::mes::MesHqdRegs> {
        None
    }
    /// `(gc_base[8], mmhub_base[8], osssys_base[8])` for `set_hw_resources`.
    fn mes_ip_bases(&mut self) -> Option<([u32; 8], [u32; 8], [u32; 8])> {
        None
    }
    /// `regRCC_DEV0_EPF0_RCC_DOORBELL_APER_EN` (NBIO) — set bit 0 to ROUTE BAR2
    /// doorbell writes to the engines. `None` (default) until discovery resolves it.
    fn doorbell_aper_en_reg(&mut self) -> Option<u32> {
        None
    }
    /// `regCP_MEC_DOORBELL_RANGE_LOWER`/`UPPER` (GC) — the compute/MES doorbell range the
    /// MEC/MES monitors so a KIQ/MES doorbell ring WAKES the microengine. Returns the
    /// resolved `(lower, upper)` offsets; `None` (default) until discovery resolves the GC
    /// block (so QEMU/pre-iron never writes a guessed offset). The daemon returns Some.
    fn cp_mec_doorbell_range_regs(&mut self) -> Option<(u32, u32)> {
        None
    }
    /// gfxhub GART-build register offsets ([`crate::gart::build_gart_enable_sequence`]).
    /// `None` (default) until discovery resolves them — so [`init_gart`] is a no-op
    /// on QEMU/pre-iron and never writes guessed GMC offsets. The daemon returns Some.
    fn gfxhub_gart_regs(&mut self) -> Option<crate::gart::GfxhubGartRegs> {
        None
    }
    /// Resolved DCN HUBP0/OTG0 scanout register offsets, once discovery confirms the
    /// DMU block. `None` (default) until iron-verified. The DCN is a SEPARATE, already
    /// firmware-lit power domain, so these read live on a WARM boot — used to probe the
    /// firmware's current scanout state (and later to flip the panel to an amdgpu buffer).
    fn dcn_scanout_regs(&mut self) -> Option<crate::regs::DcnScanout> {
        None
    }
    /// Commit a scanout/modeset to the display pipeline.
    fn commit_scanout(&mut self, width: u32, height: u32, pitch: u32, gpu_addr: u64) -> bool;
    /// Emit a driver log line.
    fn log(&mut self, msg: &str);
}

/// amdgpu device state, mirroring the bits of `struct amdgpu_device` the
/// bring-up sequence threads between stages.
#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub handle: u64,
    pub vendor: u16,
    pub device: u16,
    pub vram_base: u64,
    pub vram_size: u64,
    /// Bootup engine/memory clocks (MHz) decoded from the VBIOS firmware-info
    /// table in stage 2; 0 until read (e.g. on QEMU, which has no VBIOS).
    pub bootup_sclk_mhz: u32,
    pub bootup_mclk_mhz: u32,
}

/// Which stages completed. `device_present == false` means no AMD GPU at any
/// probe BDF (the expected QEMU result) — not a failure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BringupReport {
    pub device_present: bool,
    pub vbios: bool,
    pub gmc: bool,
    pub ih: bool,
    pub smu: bool,
    pub rings: bool,
    pub display: bool,
    /// VRAM carve-out size (MiB) the GMC stage resolved — from CONFIG_MEMSIZE
    /// when available, else the fallback default. Observability, not a gate.
    pub vram_mib: u32,
}
impl BringupReport {
    /// True only when a device was found AND every IP block initialized.
    pub fn all_ok(&self) -> bool {
        self.device_present
            && self.vbios
            && self.gmc
            && self.ih
            && self.smu
            && self.rings
            && self.display
    }
}

/// Phoenix1 (Radeon 760M/780M) firmware set, matching linux-firmware names.
/// Phoenix is PSP 13.0.4 / SMU IP 13.0.4 (13.0.8 is the Mendocino family), and
/// as an APU ships a PSP `_toc` (table-of-contents) blob, not the dGPU `_sos`.
/// There is NO `smu_13_0_4.bin` in linux-firmware: on APUs the SMU/PMFW image
/// lives in the system BIOS and the PSP bootloader loads it from there — the
/// driver never `request_firmware`s it. GFX11 additionally needs IMU, RLC and
/// the two MES (MicroEngine Scheduler) blobs.
pub const FW_PHOENIX: &[&str] = &[
    "amdgpu/psp_13_0_4_toc.bin",
    "amdgpu/psp_13_0_4_ta.bin",
    "amdgpu/gc_11_0_1_imu.bin",
    "amdgpu/gc_11_0_1_me.bin",
    "amdgpu/gc_11_0_1_pfp.bin",
    "amdgpu/gc_11_0_1_mec.bin",
    "amdgpu/gc_11_0_1_rlc.bin",
    "amdgpu/gc_11_0_1_mes_2.bin",
    "amdgpu/gc_11_0_1_mes1.bin",
    "amdgpu/sdma_6_0_1.bin",
    "amdgpu/dcn_3_1_4_dmcub.bin",
    "amdgpu/vcn_4_0_2.bin",
];

/// Stage 1 — `amdgpu_pci_probe`: claim the device, verify AMD vendor, map the
/// BAR5 register aperture. Tries an id-table match (class 0x03 + AMD vendor)
/// first — the way Linux binds — then falls back to the fixed BDF list for
/// impls without match support (the harness mock).
pub fn probe<O: GpuOps>(ops: &mut O, bdfs: &[(u8, u8, u8)]) -> Option<Device> {
    // Fresh init: no EnableGfxImu has been sent yet (see the static's doc).
    ENABLE_GFX_IMU_SENT.store(false, AtomicOrdering::Relaxed);
    if let Some(handle) = ops.pci_enable_match(PCI_CLASS_DISPLAY, AMD_VENDOR) {
        if let Some(dev) = verify_claimed(ops, handle) {
            return Some(dev);
        }
    }
    for &(bus, dev, func) in bdfs {
        let Some(handle) = ops.pci_enable(bus, dev, func) else {
            continue;
        };
        if let Some(found) = verify_claimed(ops, handle) {
            return Some(found);
        }
    }
    None
}

/// Post-claim verification shared by the match and BDF probe paths: confirm
/// AMD vendor, map the BAR5 register aperture, touch a known register.
fn verify_claimed<O: GpuOps>(ops: &mut O, handle: u64) -> Option<Device> {
    // WALL-9 DIAG (2026-07-10): rip lands in this fn's Some(Device) return with a
    // NULL out-ptr, but the fault is reached during the C init where nothing calls
    // verify_claimed (probe already returned). Mark entry so the next boot shows how
    // many times verify_claimed is genuinely ENTERED — if the crash count exceeds the
    // entry count, it is a wild control transfer INTO this code, not a real call.
    ops.log("[amdgpu] VC-ENTER");
    let id = ops.config_read_dword(handle, 0x00);
    let vendor = (id & 0xFFFF) as u16;
    let device = ((id >> 16) & 0xFFFF) as u16;
    if vendor != AMD_VENDOR {
        ops.log("[amdgpu] probe: device is not AMD; skipping");
        return None;
    }
    if !ops.map_register_bar(handle, 5) {
        ops.log("[amdgpu] probe: ioremap(BAR5 registers) failed");
        return None;
    }
    // PROBE-TIME MAILBOX CHECK (2026-07-02, was "GFXOFF hold"): GetSmuVersion proves
    // the always-on MP1 mailbox decodes BEFORE the first GC-domain read (bootstrap
    // offsets, Phoenix-gated — see regs::PHOENIX_SMU_MB_*), bounded so a silent PMFW
    // can never hang the probe. The DisallowGfxOff that used to ride along is GONE:
    // the working driver's cold mmiotrace sends NO GFXOFF message at any point before
    // the MES is up (its only GFXOFF traffic is AllowGfxOff ~150 ms AFTER set_hw_res
    // acks) — on Phoenix, GFXOFF is boot-DISALLOWED until the driver's first
    // AllowGfxOff, which RaeenOS never sends. The "early-probe fabric wedge" this
    // guarded against was root-caused to a theme_engine lock self-deadlock (9c4749a),
    // not GFXOFF; the 06-29 GFX regate was self-inflicted by the mistranscribed
    // PrepareMp1ForUnload (0x0C) of that era.
    if device == RADEON_760M {
        let mb = SmuMailbox {
            msg_reg: crate::regs::PHOENIX_SMU_MB_MSG,
            arg_reg: crate::regs::PHOENIX_SMU_MB_ARG,
            resp_reg: crate::regs::PHOENIX_SMU_MB_RESP,
        };
        let ver = smu_send_msg(ops, &mb, 0x02, 0, 200_000); // GetSmuVersion
        ops.log(&format!(
            "[amdgpu] probe-time SMU mailbox check: GetSmuVersion->{ver:?} (no GFXOFF messages — oracle: working driver sends none pre-MES; GFXOFF is boot-disallowed)"
        ));
    }
    // Touch a known register to prove the mapping is live before IP init.
    let _ = ops.reg_read(gc11::MM_GRBM_STATUS);
    if device == RADEON_760M {
        ops.log("[amdgpu] stage 1 probe + BAR5 OK (Radeon 760M/780M / Phoenix1)");
    } else {
        ops.log("[amdgpu] stage 1 probe + BAR5 OK");
    }
    // WALL-9 DIAG: the instruction that faults (mov %rax,(%rdx)) is this struct's
    // return write. If VC-SOME prints once per successful probe but the fault still
    // fires with no matching VC-SOME, control reached 0x5bcc6 WITHOUT running this
    // line — i.e. a wild jump, and the rip is a red herring.
    ops.log("[amdgpu] VC-SOME");
    Some(Device {
        handle,
        vendor,
        device,
        vram_base: 0,
        vram_size: 0,
        bootup_sclk_mhz: 0,
        bootup_mclk_mhz: 0,
    })
}

/// Stage 2 — `amdgpu_get_bios`: read + parse the ATOMBIOS image. Source order
/// mirrors Linux: PCI expansion ROM first, then the ACPI-VFCT-published image —
/// APUs (incl. Athena's 760M) have NO expansion ROM and only the VFCT path
/// works. Until the daemon can read ACPI tables at runtime, the VFCT image is
/// vendored device-keyed (`firmware/vbios/<vvvv>-<dddd>.bin`, extracted from the
/// captured table — see `atombios::parse_vfct` + its real-data KAT). Absent
/// VBIOS is non-fatal (returns true) — firmware-info just stays defaulted.
pub fn read_vbios<O: GpuOps>(ops: &mut O, dev: &mut Device) -> bool {
    let rom = ops.read_vbios_rom(dev.handle, 64 * 1024).or_else(|| {
        let name = format!("vbios/{:04x}-{:04x}.bin", dev.vendor, dev.device);
        let bytes = ops.request_firmware_bytes(&name);
        if bytes.is_some() {
            ops.log("[amdgpu] stage 2 VBIOS: no expansion ROM; using VFCT-captured image");
        }
        bytes
    });
    if let Some(rom) = rom {
        match atombios::parse_rom(&rom) {
            Ok(r) => {
                let m = format!(
                    "[amdgpu] stage 2 ATOMBIOS OK: hdr@{:#x} cmd_tbl={:#x} data_tbl={:#x}",
                    r.header_ptr, r.master_command_table_offset, r.master_data_table_offset
                );
                ops.log(&m);
                match atombios::parse_firmware_info(&rom, &r) {
                    Ok(fi) => {
                        dev.bootup_sclk_mhz = fi.bootup_sclk_mhz();
                        dev.bootup_mclk_mhz = fi.bootup_mclk_mhz();
                        ops.log(&format!(
                            "[amdgpu] stage 2 firmware_info: rev={:#010x} sclk={} MHz mclk={} MHz cap={:#010x}",
                            fi.firmware_revision,
                            dev.bootup_sclk_mhz,
                            dev.bootup_mclk_mhz,
                            fi.firmware_capability
                        ));
                    }
                    Err(_) => ops.log("[amdgpu] stage 2 firmware_info table absent/undecodable"),
                }
                // Integrated-system-info is the APU memory-config table (type,
                // channels, clocks). Present on Athena, absent on dGPU/QEMU —
                // log it only when found so QEMU boots stay quiet.
                if let Ok(isi) = atombios::integrated_system_info(&rom, &r) {
                    ops.log(&format!(
                        "[amdgpu] stage 2 integrated_system_info@{:#x} v{}.{} ({} B) — APU memory config",
                        isi.offset,
                        isi.header.format_revision,
                        isi.header.content_revision,
                        isi.header.structure_size
                    ));
                }
            }
            Err(_) => ops.log("[amdgpu] stage 2 ATOMBIOS parse failed"),
        }
    } else {
        ops.log("[amdgpu] stage 2 VBIOS: no ROM and no captured image (expected on QEMU)");
    }
    true
}

/// Stage 3 — `gmc_v*_sw_init`: memory controller + GPUVM. On an APU, VRAM is a
/// system-DRAM (UMA) carve-out whose size in MB the firmware publishes in the
/// CONFIG_MEMSIZE register (Linux nbio `get_memsize`). Prefer that value,
/// sanity-bounded; fall back to a conservative default when it is unavailable
/// (QEMU / pre-iron) or implausible. Athena measured truth
/// (docs/ATHENA_GROUND_TRUTH.md): the BIOS carve-out is 2048 MiB, BAR0 aperture
/// 0x7C_0000_0000/256 MiB, BAR5 regs 0xDC50_0000.
pub fn init_gmc<O: GpuOps>(ops: &mut O, dev: &mut Device) -> bool {
    /// Conservative carve-out used when CONFIG_MEMSIZE is unavailable.
    const DEFAULT_VRAM_MB: u32 = 512;
    /// Plausibility window for a UMA carve-out (guards a wrong/garbage read).
    const MIN_VRAM_MB: u32 = 16;
    const MAX_VRAM_MB: u32 = 16 * 1024; // 16 GiB ceiling

    dev.vram_base = 0;
    let mb = match ops.config_memsize_mb() {
        Some(mb) if (MIN_VRAM_MB..=MAX_VRAM_MB).contains(&mb) => {
            ops.log(&format!(
                "[amdgpu] stage 3 GMC/GPUVM init (VRAM {mb} MiB from CONFIG_MEMSIZE)"
            ));
            mb
        }
        _ => {
            ops.log(&format!(
                "[amdgpu] stage 3 GMC/GPUVM init (CONFIG_MEMSIZE unavailable; default {DEFAULT_VRAM_MB} MiB)"
            ));
            DEFAULT_VRAM_MB
        }
    };
    dev.vram_size = (mb as u64) * 1024 * 1024;
    // GART-inheritance investigation (roadmap §4): read-only dump of the firmware's
    // gfxhub GPUVM state. Tells us whether VMID0 + the system aperture already let
    // the CP reach a system-RAM ring (inherit) or we must build GART. Gated on the
    // discovery-resolved offsets — no-op on QEMU / pre-iron.
    if let Some(vm) = ops.gmc_vm_regs() {
        log_gmc_vm_state(ops, &vm);
    }
    true
}

/// Interrupt-handler (IH) ring register offsets: the ring base address and the
/// read/write pointers the GPU and driver use to hand off interrupt cookies.
/// ASIC-specific (oss/ih block), so supplied at runtime by the daemon
/// ([`GpuOps::ih_ring`]) once confirmed on iron — this crate hardcodes none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IhRing {
    pub rb_base: u32,
    pub rb_base_hi: u32,
    pub rb_rptr: u32,
    pub rb_wptr: u32,
}

/// One IH ring entry is 8 dwords (32 bytes) on GFX11 (oss_v6).
pub const IH_ENTRY_BYTES: u32 = 32;

/// Drain the IH ring: advance the read pointer toward the GPU's write pointer —
/// both are byte offsets that wrap at `ring_bytes` — and write RPTR back so the
/// GPU sees the freed space. Returns the number of interrupt entries consumed.
/// This is the steady-state IRQ-thread handshake (WPTR is moved by the GPU as it
/// posts cookies; RPTR chases it).
pub fn ih_drain<O: GpuOps>(ops: &mut O, ring: &IhRing, ring_bytes: u32) -> u32 {
    if ring_bytes < IH_ENTRY_BYTES {
        return 0;
    }
    let wptr = ops.reg_read(ring.rb_wptr) % ring_bytes;
    let mut rptr = ops.reg_read(ring.rb_rptr) % ring_bytes;
    let mut consumed = 0u32;
    while rptr != wptr {
        rptr = (rptr + IH_ENTRY_BYTES) % ring_bytes;
        consumed += 1;
    }
    ops.reg_write(ring.rb_rptr, rptr);
    consumed
}

/// Stage 4 — `*_ih_sw_init`: the interrupt-handler ring (a DMA buffer the GPU
/// writes interrupt cookies into; the IRQ thread drains it via [`ih_drain`]).
pub fn init_ih<O: GpuOps>(ops: &mut O, dev: &Device) -> bool {
    const IH_RING_BYTES: usize = 256 * 1024;
    let Some(ring) = ops.dma_alloc(dev.handle, IH_RING_BYTES) else {
        ops.log("[amdgpu] stage 4 IH ring alloc FAILED");
        return false;
    };
    ops.log("[amdgpu] stage 4 IH ring allocated (256 KiB, IOMMU-sandboxed)");
    // Program the ring base + clear the pointers when the register offsets are
    // confirmed (default None on QEMU/pre-iron leaves it allocate-only).
    if let Some(ihr) = ops.ih_ring() {
        ops.reg_write(ihr.rb_base, (ring.dma_addr & 0xFFFF_FFFF) as u32);
        ops.reg_write(ihr.rb_base_hi, (ring.dma_addr >> 32) as u32);
        ops.reg_write(ihr.rb_rptr, 0);
        ops.reg_write(ihr.rb_wptr, 0);
        ops.log("[amdgpu] stage 4 IH ring base programmed + RPTR/WPTR cleared");
    }
    true
}

/// The SMU (System Management Unit) mailbox — the C2PMSG registers the PMFW
/// polls. The C2PMSG indices (66 = message, 82 = argument, 90 = response) are
/// stable across AMD generations; only the absolute byte offsets (MP1 SMN base
/// plus `index << 2`) are ASIC-specific, so they are supplied at runtime by the
/// daemon ([`GpuOps::smu_mailbox`]) once confirmed on iron — this crate
/// hardcodes no SMU offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmuMailbox {
    pub msg_reg: u32,
    pub arg_reg: u32,
    pub resp_reg: u32,
}

/// The PSP (MP0) mailbox + command-ring registers — the C2PMSG registers
/// `psp_v13_0` uses. Boot 041507 proved this APU's GFX can ONLY be cold-started by
/// the PSP loading authenticated firmware (no host/SMU power-up exists), so this is
/// the channel for the firmware-load sequence. Resolved from discovery
/// ([`crate::regs::psp_regs`], MP0 seg 0) and supplied by the daemon — no hardcoded
/// PSP offset. `sol` (C2PMSG_81) is the sOS sign-of-life (`!= 0` => secure OS up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PspRegs {
    pub sol: u32,
    pub sos_fw_version: u32,
    pub bl_status: u32,
    pub vmbx_status: u32,
    pub bl_arg: u32,
    pub ring_cmd: u32,
    pub ring_lo: u32,
    pub ring_hi: u32,
    pub ring_size: u32,
    /// C2PMSG_67 — the ring write pointer (DWORDS); bumped to submit a GPCOM frame
    /// (`psp_v13_0_ring_set_wptr`). Athena oracle live value 0x1b0.
    pub ring_wptr: u32,
}

/// A created PSP command ring (`psp_v13_0_ring_create`). The GPU MC address of the
/// ring buffer (VRAM) + its size; the firmware-load commands (increment 4) write
/// 64-byte frames into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PspRing {
    pub ring_mc: u64,
    pub ring_size: u32,
}

/// PSP `C2PMSG_64` mailbox status bits (`psp_v13_0`, `MBOX_TOS_*`): bit31 = the sOS
/// is ready / the command completed; the low 16 bits carry an error code on a
/// completed command (0 = success).
pub const PSP_MBOX_FLAG: u32 = 0x8000_0000;
pub const PSP_MBOX_ERR_MASK: u32 = 0x0000_FFFF;
/// PSP KM (kernel-mode / GPCOM) ring type — `ring_type<<16` is the C2PMSG_64
/// ring-init command, which for KM(=2) equals `GFX_CTRL_CMD_ID_INIT_GPCOM_RING`
/// (0x00020000), exactly as `psp_v13_0_ring_create` builds it.
pub const PSP_RING_TYPE_KM: u32 = 2;
/// VRAM byte offset for the PSP ring. Past a 1080p framebuffer (~8 MiB) and well
/// below the firmware TMR (MC 0x8078000000), inside BAR0's 256 MiB CPU window. The
/// ring is the GPU's own carve-out (stolen RAM), so a write here cannot touch host
/// OS data — only unused VRAM.
pub const PSP_RING_VRAM_OFFSET: u64 = 0x0400_0000; // 64 MiB
/// PSP KM ring size (bytes) — matches the live Athena value (`C2PMSG_71=0x1000`);
/// 4 KiB = 64 frames, ample for the ~15-command firmware-load sequence.
pub const PSP_RING_SIZE: u32 = 0x1000;
/// PSP ring-create poll budgets (`psp_wait_for` uses ~1 s). Ready should be instant
/// (sOS already up); completion is after the 20 ms handshake delay.
pub const PSP_RING_READY_TIMEOUT_US: u32 = 1_000_000;
pub const PSP_RING_DONE_TIMEOUT_US: u32 = 1_000_000;

/// Pure: the C2PMSG_64 ring-init command for a ring type (`ring_type << 16`).
/// Host-KAT'able — KM(2) must equal GFX_CTRL_CMD_ID_INIT_GPCOM_RING (0x00020000).
pub fn psp_ring_init_cmd(ring_type: u32) -> u32 {
    ring_type << 16
}

// ── PSP GPCOM command protocol (psp_gfx_if.h) — increments 3 & 4 ──────────────
// The firmware-load handshake after ring-create: build a `psp_gfx_cmd_resp`
// command buffer (1024 B), point a 64-byte `psp_gfx_rb_frame` at it, bump the ring
// write pointer (C2PMSG_67), and poll the fence the PSP posts to memory. amdgpu
// (`psp_cmd_submit_buf`) memsets the whole cmd buffer to 0 then sets ONLY `cmd_id`
// (+8) and the command union (+28) — `buf_size`/`buf_version` stay 0 for GPCOM (the
// "must be PSP_GFX_CMD_BUF_VERSION" header note is RBI-only). All addresses are GPU
// MC addresses (cmd buffer + fence are GART/`fw_pri_mc_addr`; the ring is VRAM).

/// GFX command IDs (`enum psp_gfx_cmd_id`, psp_gfx_if.h).
pub const GFX_CMD_ID_SETUP_TMR: u32 = 0x05;
pub const GFX_CMD_ID_LOAD_IP_FW: u32 = 0x06;
pub const GFX_CMD_ID_LOAD_TOC: u32 = 0x20;
pub const GFX_CMD_ID_AUTOLOAD_RLC: u32 = 0x21;

/// FW types for `GFX_CMD_ID_LOAD_IP_FW` (`enum psp_gfx_fw_type`, psp_gfx_if.h). IMU
/// FIRST on a PSP-load APU (it powers up the GFX domain), then RLC/CP/MES/SDMA.
pub const GFX_FW_TYPE_CP_ME: u32 = 1;
pub const GFX_FW_TYPE_CP_PFP: u32 = 2;
pub const GFX_FW_TYPE_CP_MEC: u32 = 4;
pub const GFX_FW_TYPE_RLC_G: u32 = 8;
// The RLC save/restore + power-management restore lists (rlc.bin v2_1 payloads).
// amdgpu's iron psp_cmd_submit_buf trace loads BOTH right after IMU, before RLC_IRAM
// (ftype 20 sz=2560, ftype 21 sz=21104 on Athena). RaeenOS omitted them — without the
// SRM/GPM lists the RLC can't fully establish the GFX state the MES per-pipe context
// depends on. (psp_gfx_if.h GFX_FW_TYPE_RLC_RESTORE_LIST_{GPM,SRM}_MEM.)
pub const GFX_FW_TYPE_RLC_RESTORE_LIST_GPM_MEM: u32 = 20;
pub const GFX_FW_TYPE_RLC_RESTORE_LIST_SRM_MEM: u32 = 21;
pub const GFX_FW_TYPE_SDMA0: u32 = 9; // old VG/RV type — PSP rejects on SOC21 (use TH0/TH1)
pub const GFX_FW_TYPE_RLC_P: u32 = 25;
pub const GFX_FW_TYPE_RLC_IRAM: u32 = 26;
pub const GFX_FW_TYPE_RLC_DRAM_BOOT: u32 = 48;
pub const GFX_FW_TYPE_IMU_I: u32 = 68;
pub const GFX_FW_TYPE_IMU_D: u32 = 69;
pub const GFX_FW_TYPE_SDMA_UCODE_TH0: u32 = 71; // SOC21 SDMA ctx thread
pub const GFX_FW_TYPE_SDMA_UCODE_TH1: u32 = 72; // SOC21 SDMA ctl thread
                                                // MES (MicroEngine Scheduler) — gfx11 needs MES in the autoload chain (RLC->CP->MES).
                                                // `amdgpu_ucode.c::psp_get_fw_type`: pipe0 (scheduler, mes_2.bin) ucode=RS64_MES /
                                                // data=RS64_MES_STACK; pipe1 (KIQ, mes1.bin) ucode=RS64_KIQ / data=RS64_KIQ_STACK.
                                                // The working Athena amdgpu loads MES(0x88)+MES_KIQ(0x109) — RaeenOS was missing both,
                                                // a candidate reason RLC_BOOTLOAD never completes (the autoload waits on MES).
pub const GFX_FW_TYPE_RS64_MES: u32 = 76;
pub const GFX_FW_TYPE_RS64_MES_STACK: u32 = 77;
pub const GFX_FW_TYPE_RS64_KIQ: u32 = 78;
pub const GFX_FW_TYPE_RS64_KIQ_STACK: u32 = 79;
// MES fw types the Athena's PSP actually ACCEPTS — iron-captured 2026-06-28 from the
// working amdgpu's `psp_cmd_submit_buf` payload (docs/gpu-oracle/MES-FWTYPE-FIX-2026-06-28.md).
// The RS64_* values 76-79 above were REJECTED (0xffff0006): wrong fw-type numbers for THIS
// PSP/firmware. These are the working ones, identified by size (pipe0 ucode 127040 B, KIQ
// ucode 104016 B = RaeenOS's extracted sizes). pipe0 = mes_2.bin (scheduler), pipe1 = mes1.bin (KIQ).
pub const GFX_FW_TYPE_MES_PIPE0_UCODE: u32 = 33;
pub const GFX_FW_TYPE_MES_PIPE0_DATA: u32 = 34;
pub const GFX_FW_TYPE_MES_PIPE1_UCODE: u32 = 81;
pub const GFX_FW_TYPE_MES_PIPE1_DATA: u32 = 82;

/// Meaningful dword prefix of a GPCOM `psp_gfx_cmd_resp`: dword 2 = `cmd_id` (byte
/// +8), dwords 7..11 = the command union (byte +28). The rest of the 1024-byte
/// buffer is zero (the writer memsets it first, like amdgpu). dwords 0/1 = `buf_size`
/// / `buf_version` (left 0); 3..7 = the RBI-only resp fields (0 for GPCOM).
pub const PSP_CMD_PREFIX_DWORDS: usize = 11;

/// Pure: build a GPCOM command-buffer prefix — `cmd_id` at dword 2 and up to four
/// union args at dwords 7..11. Host-KAT'able against the psp_gfx_if.h offsets.
fn psp_gpcom_cmd(cmd_id: u32, a0: u32, a1: u32, a2: u32, a3: u32) -> [u32; PSP_CMD_PREFIX_DWORDS] {
    let mut c = [0u32; PSP_CMD_PREFIX_DWORDS];
    c[2] = cmd_id; // psp_gfx_cmd_resp.cmd_id @ +8
    c[7] = a0; // union @ +28
    c[8] = a1; // +32
    c[9] = a2; // +36
    c[10] = a3; // +40
    c
}

/// Pure: `GFX_CMD_ID_SETUP_TMR` — point the PSP at the Trusted Memory Region (GPU MC
/// addr + size, both 4 KiB aligned). flags=0 (no SR-IOV, no virt+phys). Athena oracle:
/// TMR = 64 MiB @ MC 0x8078000000.
pub fn psp_cmd_setup_tmr(tmr_mc: u64, tmr_size: u32) -> [u32; PSP_CMD_PREFIX_DWORDS] {
    psp_gpcom_cmd(
        GFX_CMD_ID_SETUP_TMR,
        tmr_mc as u32,
        (tmr_mc >> 32) as u32,
        tmr_size,
        0,
    )
}

/// Pure: `GFX_CMD_ID_LOAD_TOC` — hand the PSP the table-of-contents blob; the PSP
/// replies with the required TMR size (read from the cmd buffer's resp area). Must
/// precede SETUP_TMR (amdgpu `psp_load_toc` → `psp_tmr_init`).
pub fn psp_cmd_load_toc(toc_mc: u64, toc_size: u32) -> [u32; PSP_CMD_PREFIX_DWORDS] {
    psp_gpcom_cmd(
        GFX_CMD_ID_LOAD_TOC,
        toc_mc as u32,
        (toc_mc >> 32) as u32,
        toc_size,
        0,
    )
}

/// Pure: `GFX_CMD_ID_LOAD_IP_FW` — the PSP authenticates a signed firmware blob (GPU
/// MC addr + size) and backdoors it into the gated GFX SRAM. `fw_type` selects the IP
/// (IMU_I/IMU_D first → GFX powers up). This is the command that cold-starts GFX.
pub fn psp_cmd_load_ip_fw(fw_mc: u64, fw_size: u32, fw_type: u32) -> [u32; PSP_CMD_PREFIX_DWORDS] {
    psp_gpcom_cmd(
        GFX_CMD_ID_LOAD_IP_FW,
        fw_mc as u32,
        (fw_mc >> 32) as u32,
        fw_size,
        fw_type,
    )
}

/// Pure: build the 64-byte `psp_gfx_rb_frame` ring entry that points the PSP at a
/// command buffer + the fence it should post on completion. Layout (psp_gfx_if.h):
/// cmd_buf_addr_lo/hi @ +0/+4, cmd_buf_size @ +8, fence_addr_lo/hi @ +12/+16,
/// fence_value @ +20; the SID/vmid/frame_type/reserved tail (+24..+64) is 0 for a
/// GPCOM KM frame. Host-KAT'able.
pub fn psp_rb_frame(
    cmd_buf_mc: u64,
    cmd_buf_size: u32,
    fence_mc: u64,
    fence_value: u32,
) -> [u32; 16] {
    let mut f = [0u32; 16]; // 64 bytes
    f[0] = cmd_buf_mc as u32; // cmd_buf_addr_lo
    f[1] = (cmd_buf_mc >> 32) as u32; // cmd_buf_addr_hi
    f[2] = cmd_buf_size; // cmd_buf_size
    f[3] = fence_mc as u32; // fence_addr_lo
    f[4] = (fence_mc >> 32) as u32; // fence_addr_hi
    f[5] = fence_value; // fence_value
    f
}

/// Dword indices of the PSP response (`psp_gfx_resp` @ `psp_gfx_cmd_resp.resp`, byte
/// +864) within the command buffer: `status` @ byte +864, `tmr_size` @ byte +880. The
/// PSP writes these after processing the frame (poll the fence before reading).
pub const PSP_RESP_STATUS_DWORD: usize = 864 / 4; // 216
pub const PSP_RESP_TMR_SIZE_DWORD: usize = 880 / 4; // 220

/// Pure: read `(status, tmr_size)` from a completed GPCOM command buffer read back
/// from GPU memory. `status == 0` => the command succeeded; `tmr_size` is the TMR
/// byte size the PSP wants reserved (valid in the LOAD_TOC response — feed it to
/// [`psp_cmd_setup_tmr`]). `None` if the buffer is too short to contain the resp.
pub fn psp_resp_status_tmr(cmd_buf: &[u32]) -> Option<(u32, u32)> {
    let status = *cmd_buf.get(PSP_RESP_STATUS_DWORD)?;
    let tmr_size = *cmd_buf.get(PSP_RESP_TMR_SIZE_DWORD)?;
    Some((status, tmr_size))
}

/// `psp_gfx_rb_frame` size (psp_gfx_if.h): 64 bytes = 16 dwords.
pub const PSP_RB_FRAME_BYTES: u32 = 64;
pub const PSP_RB_FRAME_DWORDS: u32 = 16;
/// GPCOM command buffer (`psp_gfx_cmd_resp`) size: 1024 bytes.
pub const PSP_CMD_BUF_BYTES: usize = 1024;
/// VRAM byte offsets (relative to `vram_mc_base`) for the GPCOM command buffer and its
/// completion fence — past the ring ([`PSP_RING_VRAM_OFFSET`] + its 4 KiB), each 4 KiB
/// aligned (the PSP requires it). All in BAR0's CPU-mapped window; the GPU's own
/// carve-out, so writes here cannot touch host OS memory.
pub const PSP_CMD_VRAM_OFFSET: u64 = PSP_RING_VRAM_OFFSET + 0x1000; // ring is 0x1000
pub const PSP_FENCE_VRAM_OFFSET: u64 = PSP_RING_VRAM_OFFSET + 0x2000;
/// GPCOM completion timeout (amdgpu `psp_timeout` is multi-second; the sOS answers a
/// ring command in ms).
pub const PSP_CMD_TIMEOUT_US: u32 = 1_000_000;
/// VRAM byte offset of the PSP firmware-staging buffer (`fw_pri_buf` in amdgpu): each
/// firmware blob is copied here in turn and LOAD_IP_FW points at its MC address. Past
/// the ring/cmd/fence region, with room for the largest ucode, inside BAR0's 256 MiB
/// CPU window.
pub const PSP_FWPRI_VRAM_OFFSET: u64 = 0x0500_0000; // 80 MiB

/// Stage a byte blob into VRAM at `offset` as little-endian dwords (zero-padding a
/// trailing partial dword) — for copying firmware ucode into the PSP fw_pri buffer.
fn psp_vram_write_bytes<O: GpuOps>(ops: &mut O, offset: u64, bytes: &[u8]) {
    let mut dwords = Vec::with_capacity(bytes.len().div_ceil(4));
    for chunk in bytes.chunks(4) {
        let mut d = [0u8; 4];
        d[..chunk.len()].copy_from_slice(chunk);
        dwords.push(u32::from_le_bytes(d));
    }
    ops.vram_write(offset, &dwords);
}

/// Pure: byte offset within the PSP ring where the next 64-byte frame is written,
/// given the current write pointer (C2PMSG_67, in DWORDS) and the ring size in dwords.
/// Mirrors `psp_ring_cmd_submit`: frame index = `wptr_dw / PSP_RB_FRAME_DWORDS`, byte
/// offset = index * 64; a wptr that is a multiple of `ring_size_dw` wraps to slot 0.
pub fn psp_ring_frame_offset(wptr_dw: u32, ring_size_dw: u32) -> u32 {
    if ring_size_dw == 0 || wptr_dw % ring_size_dw == 0 {
        0
    } else {
        (wptr_dw / PSP_RB_FRAME_DWORDS) * PSP_RB_FRAME_BYTES
    }
}

/// Pure: the write pointer after submitting one frame — advances by one frame (in
/// DWORDS) and wraps at `ring_size_dw` (`psp_ring_cmd_submit`: `(wptr + frame_dw) %
/// ring_size_dw`). Written back to C2PMSG_67 to tell the PSP to process the frame.
pub fn psp_ring_advance_wptr(wptr_dw: u32, ring_size_dw: u32) -> u32 {
    if ring_size_dw == 0 {
        return 0;
    }
    (wptr_dw + PSP_RB_FRAME_DWORDS) % ring_size_dw
}

/// PSP path — submit ONE GPCOM command frame and wait for completion
/// (`psp_ring_cmd_submit` + `psp_cmd_submit_buf`'s fence poll). Stages the
/// command buffer + fence in VRAM, writes the 64-byte `psp_gfx_rb_frame` into the ring
/// at the current C2PMSG_67 slot, bumps the write pointer, polls the fence in memory
/// until it equals `index`, then reads back `(status, tmr_size)` from the command
/// buffer's response area. Returns the resp on success, `None` on timeout/short read.
///
/// Gated on [`GpuOps::vram_write`]/[`vram_read`] (default no-op/zero) + the
/// daemon-resolved `ring_wptr`, so it never runs on QEMU and never pokes guessed MMIO
/// — on iron the daemon supplies the BAR0 VRAM window. `index` must be a fresh,
/// monotonically increasing fence value (amdgpu `atomic_inc_return`, starts at 1).
pub fn psp_submit_gpcom<O: GpuOps>(
    ops: &mut O,
    psp: &PspRegs,
    ring: &PspRing,
    vram_base: u64,
    cmd_prefix: &[u32],
    index: u32,
) -> Option<(u32, u32)> {
    let cmd_mc = vram_base.wrapping_add(PSP_CMD_VRAM_OFFSET);
    let fence_mc = vram_base.wrapping_add(PSP_FENCE_VRAM_OFFSET);
    // Stage the command buffer (zero the full 1024 B, then the cmd_id + union prefix —
    // matching amdgpu's memset-then-set) and clear the fence cell.
    ops.vram_zero(PSP_CMD_VRAM_OFFSET, PSP_CMD_BUF_BYTES);
    ops.vram_write(PSP_CMD_VRAM_OFFSET, cmd_prefix);
    ops.vram_write(PSP_FENCE_VRAM_OFFSET, &[0]);
    // Write the ring frame at the current write-pointer slot, then bump C2PMSG_67.
    let wptr = ops.reg_read(psp.ring_wptr);
    let ring_size_dw = ring.ring_size / 4;
    let frame_off = PSP_RING_VRAM_OFFSET + psp_ring_frame_offset(wptr, ring_size_dw) as u64;
    let frame = psp_rb_frame(cmd_mc, PSP_CMD_BUF_BYTES as u32, fence_mc, index);
    ops.vram_write(frame_off, &frame);
    let new_wptr = psp_ring_advance_wptr(wptr, ring_size_dw);
    ops.reg_write(psp.ring_wptr, new_wptr);
    // Poll the fence (the PSP writes `index` to fence_mc on completion).
    let mut waited = 0u32;
    let mut fence = [0u32; 1];
    loop {
        ops.vram_read(PSP_FENCE_VRAM_OFFSET, &mut fence);
        if fence[0] == index {
            break;
        }
        if waited >= PSP_CMD_TIMEOUT_US {
            ops.log(&format!(
                "[amdgpu] PSP GPCOM submit TIMEOUT (fence={:#x} != {index:#x}, wptr {wptr}->{new_wptr})",
                fence[0]
            ));
            return None;
        }
        ops.delay_us(SMU_POLL_STEP_US);
        waited = waited.saturating_add(SMU_POLL_STEP_US);
    }
    // Read back the response area (up to tmr_size @ dword 220).
    let mut buf = [0u32; PSP_RESP_TMR_SIZE_DWORD + 1];
    ops.vram_read(PSP_CMD_VRAM_OFFSET, &mut buf);
    psp_resp_status_tmr(&buf)
}

/// PSP path increments 3+4 — the firmware-load handshake on top of a created GPCOM
/// ring, byte-exact to amdgpu `psp_load_non_psp_fw`: **LOAD_TOC** (the PSP parses the
/// table-of-contents and returns the required TMR size) → **SETUP_TMR**(size) → for
/// each gfx ucode in `fw_blobs`, stage it in the fw_pri buffer and submit
/// **LOAD_IP_FW** (the PSP authenticates the signed blob into the gated GFX SRAM/TMR);
/// **AUTOLOAD_RLC** fires right after the RLC_G blob loads (the amdgpu trigger — "start
/// rlc autoload after psp received all the gfx firmware"), so the RLC brings the GFX
/// engines up. gfx11 uses BOTH per-blob LOAD_IP_FW *and* the final AUTOLOAD_RLC (not
/// one-or-the-other) — caller orders `fw_blobs` with RLC_G last.
///
/// Gated on the daemon staging the TOC blob ([`GpuOps::psp_toc_blob`]) + BAR0 VRAM
/// access; skips (returns false) on QEMU / before the firmware is staged. Returns true
/// iff AUTOLOAD_RLC is accepted (resp.status 0) — the point GFX should power up and the
/// high GC registers come alive (verify with the seg1 decode-probe afterward).
pub fn psp_load_gfx_firmware<O: GpuOps>(
    ops: &mut O,
    psp: &PspRegs,
    ring: &PspRing,
    fw_blobs: &[(u32, &[u8])],
) -> bool {
    let Some(vram_base) = ops.vram_mc_base() else {
        ops.log("[amdgpu] PSP fw-load SKIPPED — VRAM aperture unmapped (QEMU/no discovery)");
        return false;
    };
    let Some((toc_mc, toc_size)) = ops.psp_toc_blob() else {
        ops.log("[amdgpu] PSP fw-load SKIPPED — TOC blob not staged (daemon fw staging pending)");
        return false;
    };
    let mut fence = 1u32;

    // (1) LOAD_TOC — the PSP parses the TOC and returns the TMR size to reserve.
    let toc_cmd = psp_cmd_load_toc(toc_mc, toc_size);
    let (status, tmr_size) = match psp_submit_gpcom(ops, psp, ring, vram_base, &toc_cmd, fence) {
        Some(r) => r,
        None => {
            ops.log("[amdgpu] PSP LOAD_TOC: no completion (fence timeout)");
            return false;
        }
    };
    if status != 0 {
        ops.log(&format!(
            "[amdgpu] PSP LOAD_TOC REJECTED — status {status:#x}"
        ));
        return false;
    }
    ops.log(&format!(
        "[amdgpu] PSP LOAD_TOC OK — TMR size {tmr_size:#x}; NEXT: SETUP_TMR"
    ));
    fence += 1;

    // (2) SETUP_TMR — reserve the Trusted Memory Region the PSP just sized.
    let tmr_mc = ops
        .psp_tmr_base()
        .unwrap_or(vram_base.wrapping_add(0x0800_0000));
    let tmr_cmd = psp_cmd_setup_tmr(tmr_mc, tmr_size);
    match psp_submit_gpcom(ops, psp, ring, vram_base, &tmr_cmd, fence) {
        Some((0, _)) => ops.log(&format!(
            "[amdgpu] PSP SETUP_TMR OK @ MC {tmr_mc:#x}; NEXT: LOAD_IP_FW x{}",
            fw_blobs.len()
        )),
        Some((s, _)) => {
            ops.log(&format!("[amdgpu] PSP SETUP_TMR REJECTED — status {s:#x}"));
            return false;
        }
        None => {
            ops.log("[amdgpu] PSP SETUP_TMR: no completion (fence timeout)");
            return false;
        }
    }
    fence += 1;

    // (3) LOAD_IP_FW per gfx ucode; AUTOLOAD_RLC right after RLC_G (amdgpu trigger).
    let fw_pri_mc = vram_base.wrapping_add(PSP_FWPRI_VRAM_OFFSET);
    for &(fw_type, bytes) in fw_blobs {
        psp_vram_write_bytes(ops, PSP_FWPRI_VRAM_OFFSET, bytes);
        let cmd = psp_cmd_load_ip_fw(fw_pri_mc, bytes.len() as u32, fw_type);
        match psp_submit_gpcom(ops, psp, ring, vram_base, &cmd, fence) {
            Some((0, _)) => {}
            Some((s, _)) => {
                ops.log(&format!(
                    "[amdgpu] PSP LOAD_IP_FW(type {fw_type}) REJECTED — status {s:#x}"
                ));
                // MES (types 76-79) is NON-FATAL — like SDMA, this Phoenix PSP may not
                // take MES via the gfx LOAD_IP_FW path. A rejection here must NOT abort
                // the loop before RLC_G (that would skip AUTOLOAD_RLC entirely, a
                // regression). Skip the rejected MES blob and continue; a rejected
                // GFX-core blob (IMU/RLC/CP) stays fatal (the autoload can't run without
                // them). The fall-through `fence += 1` keeps the PSP fence in step.
                if !(GFX_FW_TYPE_RS64_MES..=GFX_FW_TYPE_RS64_KIQ_STACK).contains(&fw_type) {
                    return false;
                }
            }
            None => {
                ops.log(&format!(
                    "[amdgpu] PSP LOAD_IP_FW(type {fw_type}): no completion"
                ));
                return false;
            }
        }
        fence += 1;

        // The PSP starts the RLC autoload once it has all the gfx firmware — amdgpu
        // fires it right after RLC_G (the load order puts RLC_G last).
        if fw_type == GFX_FW_TYPE_RLC_G {
            let auto = psp_gpcom_cmd(GFX_CMD_ID_AUTOLOAD_RLC, 0, 0, 0, 0);
            match psp_submit_gpcom(ops, psp, ring, vram_base, &auto, fence) {
                Some((0, _)) => {
                    ops.log("[amdgpu] PSP AUTOLOAD_RLC OK — RLC autoload armed; powering GFX next");
                    // *** GFX power-up — the missing step (cold-trace 2026-06-29,
                    // docs/gpu-oracle/netlog-imuload-20260629.txt analysis). ***
                    // amdgpu's SMU sends EnableGfxImu(0x16, arg=1) as its 3rd message
                    // during SMU init (right after GetPmfwVersion), BEFORE the GFX
                    // autoload completes. It powers the GFX/IMU domain so the
                    // PSP-staged RLC autoload can actually EXECUTE. Order on a PSP-load
                    // APU: PSP stages fw + arms autoload -> SMU EnableGfxImu -> GFX
                    // hw_init polls for complete. RaeenOS was polling immediately on an
                    // UNPOWERED GFX (-> RLC_BOOTLOAD_STATUS stayed 0, the ~14-boot
                    // timeout) and only sent EnableGfxImu later in the backdoor
                    // try_imu_core_start fallback — too late, after this poll already
                    // failed. Send it HERE, between arm and poll. Async: the PMFW posts
                    // no response to EnableGfxImu (a sync poll wedges).
                    if let (Some(mb), Some(msg)) = (ops.smu_mailbox(), ops.enable_gfx_imu_msg()) {
                        smu_send_msg_async(ops, &mb, msg, 1, 1_000_000);
                        ENABLE_GFX_IMU_SENT.store(true, AtomicOrdering::Relaxed);
                        ops.log(&format!(
                            "[amdgpu] PSP autoload: EnableGfxImu({msg:#x}) arg=1 sent — GFX/IMU domain powering up before the completion poll"
                        ));
                    } else {
                        ops.log(
                            "[amdgpu] PSP autoload: EnableGfxImu UNAVAILABLE (no SMU mailbox / msg id) — autoload will likely time out",
                        );
                    }
                    // gfx_v11_0_wait_for_rlc_autoload_complete: the RLC autoload runs
                    // for real time (the RLC microcontroller loads each engine from the
                    // TMR + brings GFX out of reset). Poll RLC_RLCS_BOOTLOAD_STATUS for
                    // BOOTLOAD_COMPLETE (bit 31) up to ~2s — checking once immediately
                    // (the old bug) always saw it false. Gated on gfx_regs (skipped on
                    // the mock/QEMU).
                    if let Some(gg) = ops.gfx_regs() {
                        let mut waited = 0u32;
                        loop {
                            let boot = ops.reg_read(gg.rlc_bootload_status);
                            if boot & crate::regs::RLC_BOOTLOAD_COMPLETE_MASK != 0 {
                                ops.log(&format!(
                                    "[amdgpu] PSP autoload COMPLETE — RLC_BOOTLOAD_STATUS={boot:#010x}: GFX is UP (first light)"
                                ));
                                break;
                            }
                            if waited >= 2_000_000 {
                                ops.log(&format!(
                                    "[amdgpu] PSP autoload poll TIMEOUT (2s) — RLC_BOOTLOAD_STATUS={boot:#010x} BOOTLOAD_COMPLETE clear (autoload accepted but engines not up — may need rlc_resume/cp_resume or more fw)"
                                ));
                                break;
                            }
                            ops.delay_us(SMU_POLL_STEP_US);
                            waited = waited.saturating_add(SMU_POLL_STEP_US);
                        }
                        // Consolidated FIRST-LIGHT verdict + flush. reset_ctrl & 0x1f ==
                        // 0x1f = the IMU released GFX from reset (first light); BOOTLOAD
                        // bit 31 = the RLC autoload finished. Logged + yielded HERE so the
                        // netlog broadcasts the ANSWER before the newly-live init_rings
                        // path runs — on a powered GFX that path can wedge CPU0 and starve
                        // the netlog/auto-return threads (2026-06-29: the first reorder
                        // boot hung past here with ZERO post-marker capture). delay_us
                        // yields CPU0 so the safe-progress broadcast captures this line.
                        let rst = ops.reg_read(gg.gfx_imu_reset_ctrl);
                        let boot = ops.reg_read(gg.rlc_bootload_status);
                        let complete = boot & crate::regs::RLC_BOOTLOAD_COMPLETE_MASK != 0;
                        let reset_done = rst & 0x1f == 0x1f;
                        ops.log(&format!(
                            "[amdgpu] FIRST-LIGHT verdict: RLC_BOOTLOAD_STATUS={boot:#010x} complete={complete} GFX_IMU_GFX_RESET_CTRL={rst:#010x} reset_done={reset_done} -> first_light={}",
                            complete || reset_done
                        ));
                        ops.delay_us(3_000_000);
                    }
                    return true;
                }
                Some((s, _)) => {
                    ops.log(&format!(
                        "[amdgpu] PSP AUTOLOAD_RLC REJECTED — status {s:#x}"
                    ));
                    return false;
                }
                None => {
                    ops.log("[amdgpu] PSP AUTOLOAD_RLC: no completion (fence timeout)");
                    return false;
                }
            }
        }
    }
    // All blobs loaded but RLC_G wasn't in the list — autoload was never triggered.
    ops.log("[amdgpu] PSP fw-load: all blobs loaded but no RLC_G — AUTOLOAD_RLC NOT started");
    false
}

/// PMFW success response code (written to the response register when a message
/// completes OK).
pub const SMU_RESP_OK: u32 = 1;

/// The `RLC_SAFE_MODE` register (one offset). Writing it asks the RLC to hold the
/// GFX block powered + clocked so CP/GFX register writes STICK even when GFXOFF
/// would otherwise gate the block. Offset is ASIC-specific → iron-pending
/// ([`GpuOps::rlc_safe_mode`]); the CMD/MESSAGE bit layout lives in `gc11`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RlcSafeMode {
    pub reg: u32,
}

/// SOC15-correct gfx11 CP/GRBM register offsets (absolute MMIO byte offsets),
/// resolved from IP discovery by [`crate::regs::gfx_regs`]. Supplied via
/// [`GpuOps::gfx_regs`]; stage 6 uses these instead of the `gc11` LEGACY
/// constants when present. The `gc11` values match pre-SOC15 GCN and address the
/// wrong registers on real gfx11 — the suspected CP-ring readback-mismatch root
/// cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GfxRegs {
    pub grbm_status: u32,
    pub cp_rb0_base: u32,
    pub cp_rb0_base_hi: u32,
    pub cp_rb0_cntl: u32,
    pub cp_rb0_rptr: u32,
    pub cp_rb0_wptr: u32,
    /// `CP_ME_CNTL` — the master CP enable/halt register (PFP/ME/MEC halt bits).
    /// On gfx11 the CP microcode is RS64 and, on Phoenix (an APU), PSP-loaded — so
    /// the driver's remaining CP step is enable/resume (release from halt), NOT a
    /// ucode upload. See [`cp_gfx_enable`].
    pub cp_me_cntl: u32,
    /// `RLC_RLCS_BOOTLOAD_STATUS` — bit 31 (`BOOTLOAD_COMPLETE`) is the PSP's
    /// "RLC autoload finished" flag: GFX firmware (IMU/RLC/CP ucode) is loaded.
    /// The authoritative "is the GFX firmware up?" read for stage 6.
    pub rlc_bootload_status: u32,
    /// `GFX_IMU_GFX_RESET_CTRL` — `& 0x1f == 0x1f` means the IMU released GFX from
    /// reset (imu_v11_0_wait_for_reset_status). Clear ⇒ GFX held in reset ⇒ CP/GFX
    /// register writes are silently dropped. The authoritative "is GFX awake?" read.
    pub gfx_imu_reset_ctrl: u32,
    /// `GFX_IMU_CORE_CTRL` — the IMU-core start register. The DOWN-branch wake clears
    /// bit0 (CRESET) to release the IMU core (`imu_v11_0_start`); the IMU then brings
    /// GFX out of reset. No-op if the core is already running.
    pub gfx_imu_core_ctrl: u32,
    /// `GFX_IMU_I_RAM_ADDR` / `_DATA` — the IMU instruction-RAM window. DIRECT-load
    /// streams the IMU ucode here (`imu_v11_0_load_microcode`): ADDR=0, push dwords
    /// through DATA (auto-incrementing), ADDR=fw_version.
    pub gfx_imu_i_ram_addr: u32,
    pub gfx_imu_i_ram_data: u32,
    /// `GFX_IMU_D_RAM_ADDR` / `_DATA` — the IMU data-RAM window (same protocol).
    pub gfx_imu_d_ram_addr: u32,
    pub gfx_imu_d_ram_data: u32,
    /// `GFX_IMU_RLC_BOOTLOADER_ADDR_LO/HI/SIZE` — point the IMU at the RLC_G
    /// ucode inside the autoload buffer (rlc_backdoor_autoload_enable).
    pub rlc_bootloader_addr_lo: u32,
    pub rlc_bootloader_addr_hi: u32,
    pub rlc_bootloader_size: u32,
    /// `GFX_IMU_C2PMSG_ACCESS_CTRL0/1` + `GFX_IMU_SCRATCH_10` — setup_imu writes.
    pub imu_access_ctrl0: u32,
    pub imu_access_ctrl1: u32,
    pub imu_scratch_10: u32,
    /// `RLC_SRM_CNTL` — the RLC Save/Restore Machine enable, written in the PSP-load
    /// `gfx_v11_0_rlc_resume` (enable_srm). One of the steps stage 6 was skipping.
    pub rlc_srm_cntl: u32,
    /// `RLC_PG_CNTL` — the RLC GFX power-gating enables. amdgpu clears it to 0 early in
    /// bring-up so the RLC hardware cannot gate GFX (the SMU DisallowGfxOff alone does
    /// not hold). RaeenOS was skipping this -> GFX gated -> GRBM/CP/gfxhub read 0.
    pub rlc_pg_cntl: u32,
    /// `RLC_CSIB_ADDR_LO/HI` + `RLC_CSIB_LENGTH` — the RLC Clear-State Buffer descriptor
    /// (`gfx_v11_0_init_csb`). amdgpu programs these RIGHT BEFORE the MES enable; RaeenOS
    /// skipped them and the MES stalls one instruction into boot. Set ADDR to the CSB
    /// buffer + LENGTH=0x3c0.
    pub rlc_csib_addr_lo: u32,
    pub rlc_csib_addr_hi: u32,
    pub rlc_csib_length: u32,
}

/// SOC15-correct `SDMA0_QUEUE0_RB_*` ring register offsets (absolute MMIO byte
/// offsets), resolved from IP discovery by [`crate::regs::sdma_regs`]. Supplied
/// via [`GpuOps::sdma_regs`]; used by [`program_sdma_ring`]. On gfx11 the SDMA
/// registers are part of the GC IP block (base_idx 0, like the CP RB regs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SdmaRegs {
    pub rb_cntl: u32,
    pub rb_base: u32,
    pub rb_base_hi: u32,
    pub rb_rptr: u32,
    pub rb_rptr_hi: u32,
    pub rb_wptr: u32,
    pub rb_wptr_hi: u32,
    /// `SDMA0_QUEUE0_RB_WPTR_POLL_ADDR_{LO,HI}` — the system-memory address the F32
    /// firmware polls for the write pointer when `RB_CNTL.F32_WPTR_POLL_ENABLE` is
    /// set. The live Athena amdgpu submits this way (RB_WPTR register stays 0), so
    /// this is how the autoloaded gfx11 SDMA actually takes work.
    pub rb_wptr_poll_addr_hi: u32,
    pub rb_wptr_poll_addr_lo: u32,
    /// `SDMA0_QUEUE0_DOORBELL` (`.ENABLE` bit 28) + `_DOORBELL_OFFSET` (the queue's
    /// byte offset in the doorbell BAR). The live amdgpu wakes the engine with a
    /// 64-bit write to the doorbell aperture; F32_WPTR_POLL alone did not (boot
    /// 170257: RB_RPTR stayed 0).
    pub doorbell: u32,
    pub doorbell_offset: u32,
    /// `SDMA0_BROADCAST_UCODE_{ADDR,DATA}` (GC seg1) — the RS64 microcode-load window.
    /// This Phoenix PSP rejects SDMA via LOAD_IP_FW, so we direct-load the dual-thread
    /// ucode here (write ADDR, then stream dwords to DATA, which auto-increments).
    pub broadcast_ucode_addr: u32,
    pub broadcast_ucode_data: u32,
    /// `SDMA0_F32_CNTL` (GC seg1) — the engine halt/enable register; clear `HALT`
    /// (bit 0) AND both thread resets, and SET `TH0_ENABLE`/`TH1_ENABLE`, to start
    /// the dual-thread RS64 engine. (The GOP leaves TH0_ENABLE=0, so clearing HALT
    /// alone leaves the ring-draining thread disabled.)
    pub f32_cntl: u32,
    /// `SDMA0_UTCL1_CNTL` (GC seg0) — the engine's UTC L1 translation-cache control.
    /// `sdma_v6_0_gfx_resume_instance` sets RESP_MODE=3 + REDO_DELAY=9 so the engine
    /// resolves its ring/WPTR/fence GPU addresses through VMID0; unprogrammed, the
    /// engine cannot translate and never fetches (RB_RPTR stays 0).
    pub utcl1_cntl: u32,
}

/// The remaining gfx11 CP gfx-ring registers `gfx_v11_0_cp_gfx_resume` programs
/// beyond BASE/CNTL/WPTR (resolved from IP discovery by
/// [`crate::regs::cp_gfx_ring_regs`]). Supplied via [`GpuOps::cp_gfx_ring_regs`].
/// `rb_active` is the load-bearing one — writing `CP_RB_ACTIVE = 1` ACTIVATES the
/// ring; the driver never omits it, and our earlier stage-6 did. The RPTR/WPTR
/// writeback addresses tell the CP where to post the read pointer + poll the write
/// pointer (GPU addresses). All GC IP block, BASE_IDX 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpGfxRingRegs {
    pub rb_active: u32,
    pub rb_vmid: u32,
    pub rb0_rptr_addr: u32,
    pub rb0_rptr_addr_hi: u32,
    pub rb_wptr_poll_addr_lo: u32,
    pub rb_wptr_poll_addr_hi: u32,
    /// `CP_RB_DOORBELL_CONTROL` — gfx11 CP gfx ring wakes on a doorbell, not a
    /// bare WPTR-register write. `gfx_v11_0_cp_gfx_set_doorbell` sets DOORBELL_EN
    /// (bit 30) + DOORBELL_OFFSET (the ring's doorbell index, GFX_RING0 = 0).
    pub doorbell_control: u32,
    /// `CP_RB_DOORBELL_RANGE_LOWER`/`UPPER` — the doorbell-aperture byte range
    /// the CP routes to the gfx ring. Without it the doorbell write is ignored.
    pub doorbell_range_lower: u32,
    pub doorbell_range_upper: u32,
    /// `CP_MAX_CONTEXT` / `CP_DEVICE_ID` — the CP-init registers `gfx_v11_0_cp_gfx_
    /// start` writes (MAX_CONTEXT = max_hw_contexts-1 = 7 on gfx11, DEVICE_ID = 1)
    /// BEFORE the first ring submit. CP_DEVICE_ID is the classic "the CP begins
    /// processing" kick — without it the (loaded, unhalted) CP never fetches the ring.
    pub max_context: u32,
    pub device_id: u32,
}

/// RS64 CP startup registers (`gfx_v11_0_config_gfx_rs64`), resolved from IP
/// discovery by [`crate::regs::rs64_cp_regs`]. Supplied via [`GpuOps::rs64_cp_regs`].
/// On gfx11 the CP is RS64 (RISC cores) — clearing the `CP_ME_CNTL` halt bits is
/// NOT enough; each core needs its program-counter START set (from the fw header)
/// + a pipe reset before it executes the PSP-loaded ucode. See [`config_gfx_rs64`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rs64CpRegs {
    /// `GRBM_GFX_CNTL` — the me/pipe/queue/vmid selector (`grbm_select`).
    pub grbm_gfx_cntl: u32,
    /// `CP_ME_CNTL` — carries the PFP/ME pipe-reset bits.
    pub me_cntl: u32,
    pub pfp_start: u32,
    pub pfp_start_hi: u32,
    pub me_start: u32,
    pub me_start_hi: u32,
    pub mec_start: u32,
    pub mec_start_hi: u32,
    /// `CP_MEC_RS64_CNTL` — carries the MEC pipe-reset bits.
    pub mec_cntl: u32,
}

/// The RS64 CP program-counter START addresses, parsed from the
/// `gfx_firmware_header_v2_0.ucode_start_addr_{lo,hi}` of the PFP/ME/MEC blobs
/// (offset 0x34/0x38 after the 32-byte common header). Supplied via
/// [`GpuOps::rs64_ucode_starts`] — `config_gfx_rs64` writes NO start address
/// without these (never a guessed program counter into the live CP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rs64UcodeStarts {
    pub pfp: u64,
    pub me: u64,
    pub mec: u64,
}

/// gfxhub GMC/GPUVM state registers (read-only diagnostic), resolved from IP
/// discovery by [`crate::regs::gmc_vm_regs`]. Used by [`log_gmc_vm_state`] to dump
/// what the BIOS/GOP firmware ALREADY configured — the key to deciding whether we
/// can INHERIT the firmware's VM (cheap: if the system aperture covers the ring's
/// system-RAM address and VMID0 is enabled, the CP can reach the ring without us
/// building GART) or must build `gfxhub_v3_0_gart_enable` from scratch (roadmap §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GmcVmRegs {
    pub fb_location_base: u32,
    pub fb_location_top: u32,
    pub agp_base: u32,
    pub sys_aperture_low: u32,
    pub sys_aperture_high: u32,
    pub mx_l1_tlb_cntl: u32,
    pub context0_cntl: u32,
    pub context0_ptb_lo32: u32,
    pub context0_ptb_hi32: u32,
}

/// Read-only dump of the firmware's gfxhub GPUVM state — the GART-inheritance
/// investigation (roadmap §4). Reads the registers the BIOS/GOP left configured
/// and logs them so one iron flash tells us: is VMID0 enabled? where's the page
/// table? does the system aperture [low,high] cover our ring's system-RAM address
/// (i.e. can the CP reach it WITHOUT us building GART)? Pure reads — never writes.
pub fn log_gmc_vm_state<O: GpuOps>(ops: &mut O, r: &GmcVmRegs) {
    let fb_base = ops.reg_read(r.fb_location_base);
    let fb_top = ops.reg_read(r.fb_location_top);
    let agp = ops.reg_read(r.agp_base);
    let sa_lo = ops.reg_read(r.sys_aperture_low);
    let sa_hi = ops.reg_read(r.sys_aperture_high);
    let tlb = ops.reg_read(r.mx_l1_tlb_cntl);
    let ctx0 = ops.reg_read(r.context0_cntl);
    let ptb_lo = ops.reg_read(r.context0_ptb_lo32);
    let ptb_hi = ops.reg_read(r.context0_ptb_hi32);
    ops.log(&format!(
        "[amdgpu] gfxhub VM: FB[{fb_base:#x}..{fb_top:#x}] AGP={agp:#x} SYS_APERTURE[{sa_lo:#x}..{sa_hi:#x}] L1_TLB_CNTL={tlb:#010x}"
    ));
    ops.log(&format!(
        "[amdgpu] gfxhub VM: CONTEXT0_CNTL={ctx0:#010x} PAGE_TABLE_BASE={ptb_hi:#010x}:{ptb_lo:#010x} (CNTL!=0 + page table set => VMID0 live; inherit-GART candidate)"
    ));
}

/// Runtime "inherit GART" check: is the ring's system-memory (DMA/bus) address
/// reachable through the firmware's EXISTING system aperture? The BIOS/GOP set up
/// `GCMC_VM_SYSTEM_APERTURE_[LOW,HIGH]` (in 256 KiB units, `addr >> 18`) as a flat
/// window onto system RAM. If our ring's address falls inside it, the started CP
/// (config_gfx_rs64) can fetch the ring with NO GART build and NO risky GMC
/// reprogramming — the cheap, SAFE inherit path. Returns true (reachable) / false
/// (would need a GART build or aperture extend). Reads + logs only; never writes.
pub fn gfx_ring_reachable_via_aperture<O: GpuOps>(
    ops: &mut O,
    vm: &GmcVmRegs,
    ring_dma_addr: u64,
) -> bool {
    let lo = ops.reg_read(vm.sys_aperture_low) as u64;
    let hi = ops.reg_read(vm.sys_aperture_high) as u64;
    let unit = ring_dma_addr >> 18; // aperture regs are in 256 KiB units
    let reachable = lo != 0 && unit >= lo && unit <= hi;
    ops.log(&format!(
        "[amdgpu] GART inherit check: ring@{ring_dma_addr:#x} (unit {unit:#x}) vs firmware SYS_APERTURE[{lo:#x}..{hi:#x}] -> {}",
        if reachable {
            "REACHABLE — CP can fetch the ring via the firmware aperture (inherit; no GART build)"
        } else {
            "OUT OF APERTURE — needs a GART build / aperture extend (roadmap §4.1, gated on this verdict)"
        }
    ));
    reachable
}

/// Enter RLC safe mode: ask the RLC to hold GFX powered/clocked so the CP ring
/// registers are writable (the gfx11 fix for stage-6 writes silently dropped by a
/// gated GFX block). Writes `CMD | MESSAGE`, then polls until the RLC clears
/// `CMD` (entered). Returns true on success — OR immediately true when the RLC
/// offset isn't confirmed yet (`None`), so the caller proceeds and we never write
/// a guessed offset on iron. A real timeout (RLC never acks) returns false.
pub fn rlc_enter_safe_mode<O: GpuOps>(ops: &mut O, max_polls: u32) -> bool {
    let Some(rlc) = ops.rlc_safe_mode() else {
        return true;
    };
    ops.reg_write(
        rlc.reg,
        gc11::RLC_SAFE_MODE_CMD | gc11::RLC_SAFE_MODE_MESSAGE,
    );
    for _ in 0..max_polls {
        if ops.reg_read(rlc.reg) & gc11::RLC_SAFE_MODE_CMD == 0 {
            return true;
        }
    }
    false
}

/// Exit RLC safe mode (let the RLC resume normal GFXOFF/clock-gating management).
/// No-op until the RLC offset is confirmed.
pub fn rlc_exit_safe_mode<O: GpuOps>(ops: &mut O) {
    if let Some(rlc) = ops.rlc_safe_mode() {
        ops.reg_write(rlc.reg, gc11::RLC_SAFE_MODE_MESSAGE);
    }
}

/// Microsecond step per poll of the SMU response register — paces the wait via
/// [`GpuOps::delay_us`] so the loop spans real time (1 ms granularity, fine over a
/// ~1 s budget) and yields the CPU between reads.
pub const SMU_POLL_STEP_US: u32 = 1000;

/// Poll the SMU response register until it reads non-zero (the PMFW posted a
/// status) or `timeout_us` elapses. Reads at least once, then paces with
/// [`GpuOps::delay_us`]. Returns the response value, or `None` on timeout. This is
/// `__smu_cmn_poll_stat`: used BOTH to wait for the mailbox to go idle before a
/// send AND for the message's response after.
pub fn smu_poll_response<O: GpuOps>(ops: &mut O, mb: &SmuMailbox, timeout_us: u32) -> Option<u32> {
    let mut waited = 0u32;
    loop {
        let resp = ops.reg_read(mb.resp_reg);
        if resp != 0 {
            return Some(resp);
        }
        if waited >= timeout_us {
            return None;
        }
        ops.delay_us(SMU_POLL_STEP_US);
        waited = waited.saturating_add(SMU_POLL_STEP_US);
    }
}

/// Send one SMU message and wait for the PMFW to answer — the
/// `smu_cmn_send_smc_msg_with_param` handshake. Faithful sequence: (1) WAIT for the
/// mailbox to be idle/ready (the PMFW posts a non-zero status to the response reg
/// after boot / its prior command — skipping this drops the very first message);
/// (2) clear the response, write the argument, then write the message id (which
/// triggers the PMFW); (3) wait for THIS message's response. `timeout_us` bounds
/// each wait (the real driver uses ~1 s). Returns the response code, or `None` on
/// timeout (the PMFW never answered — a real, surfaceable hang). If the pre-poll
/// times out the mailbox still has an IN-FLIGHT command: writing a new message id
/// on top of it is undefined PMFW behavior (smu_cmn aborts with "pre-check failed"
/// for exactly this reason), so we ABORT without sending. Iron 2026-07-01 hit the
/// stomp for real: a never-acking SetHardMinGfxClk followed by more sends left the
/// PMFW in an undefined state for the whole MES window.
pub fn smu_send_msg<O: GpuOps>(
    ops: &mut O,
    mb: &SmuMailbox,
    msg: u32,
    arg: u32,
    timeout_us: u32,
) -> Option<u32> {
    // Mailbox must be idle/ready before we may claim it (smu_cmn pre-check).
    smu_poll_response(ops, mb, timeout_us)?;
    ops.reg_write(mb.resp_reg, 0); // clear the prior response
    ops.reg_write(mb.arg_reg, arg);
    ops.reg_write(mb.msg_reg, msg); // writing the message id triggers the PMFW
    smu_poll_response(ops, mb, timeout_us)
}

/// Fire-and-forget SMU send (`smu_msg_send_async_locked`): write the argument + the
/// message id and return WITHOUT polling for a response. The DIRECT-load
/// `EnableGfxImu` is sent this way — the PMFW powers up GFX but never posts a
/// response, so boot 185829's "resp None" was this async semantics, NOT a failure
/// (which is why removing the message was wrong). Pre-polls for mailbox-idle so it
/// can't stomp an in-flight command (the prior GetSmuVersion ack leaves the resp
/// reg non-zero, so the pre-poll returns at once).
pub fn smu_send_msg_async<O: GpuOps>(
    ops: &mut O,
    mb: &SmuMailbox,
    msg: u32,
    arg: u32,
    ready_timeout_us: u32,
) {
    let _ready = smu_poll_response(ops, mb, ready_timeout_us);
    ops.reg_write(mb.resp_reg, 0);
    ops.reg_write(mb.arg_reg, arg);
    ops.reg_write(mb.msg_reg, msg);
}

/// Classify the PSP sign-of-life reading (`psp_v13_0_is_sos_alive`: sOS up when
/// `C2PMSG_81 != 0`). Pure → host-KAT'able. `0xffffffff` means the MP0 mailbox
/// aperture doesn't decode (wrong seg/base); `0` means the secure OS isn't exposing
/// C2PMSG; anything else is a live sOS version = the PSP channel is reachable.
pub fn psp_sol_verdict(sol: u32) -> &'static str {
    match sol {
        0xFFFF_FFFF => "DEAD (MP0 aperture does not decode — wrong seg/base)",
        0 => "SILENT (secure OS not exposing C2PMSG_81)",
        _ => "ALIVE (PSP secure-OS up — channel reachable)",
    }
}

/// PSP path increment 1 — `psp_v13_0_is_sos_alive`: read the PSP (MP0) mailbox
/// sign-of-life + bootloader status. If `C2PMSG_81` is a live value the PSP secure
/// OS is up and the firmware-load sequence (ring → TMR → LOAD_IP_FW) — the only way
/// to cold-start GFX on this PSP-load APU — can be built on top. READS ONLY; gated
/// on the daemon-confirmed MP0 offsets, so it never pokes guessed MMIO.
pub fn psp_sign_of_life<O: GpuOps>(ops: &mut O) {
    let Some(psp) = ops.psp_regs() else {
        ops.log("[amdgpu] PSP sign-of-life SKIPPED — MP0 mailbox unresolved (QEMU/no discovery)");
        return;
    };
    let sol = ops.reg_read(psp.sol);
    let sos_ver = ops.reg_read(psp.sos_fw_version);
    let bl = ops.reg_read(psp.bl_status);
    let vmbx = ops.reg_read(psp.vmbx_status);
    let verdict = psp_sol_verdict(sol);
    ops.log(&format!(
        "[amdgpu] PSP sign-of-life: C2PMSG_81(sol)={sol:#010x} sos_fw_ver(58)={sos_ver:#010x} bl_status(35)={bl:#010x} vmbx(33)={vmbx:#010x} -> {verdict}"
    ));
    ops.log(&format!(
        "[amdgpu] PSP mailbox resolved: sol@{:#07x} cmd@{:#07x} ring_lo@{:#07x} (MP0 seg0) — next: ring create + TMR + LOAD_IP_FW",
        psp.sol, psp.ring_cmd, psp.ring_lo
    ));
}

/// PSP path increment 2 — `psp_v13_0_ring_create` (non-SR-IOV / bare-metal KM ring).
/// Places a 4 KiB GPCOM command ring in VRAM (via BAR0, MC = `vram_mc_base +
/// PSP_RING_VRAM_OFFSET`) and asks the sOS to init it: poll C2PMSG_64 for sOS-ready,
/// write the ring MC addr lo/hi + size to C2PMSG_69/70/71, write the INIT_GPCOM_RING
/// command (`KM<<16`) to C2PMSG_64, delay, then poll C2PMSG_64 for completion (bit31
/// set, low-16 error code == 0). On success the firmware-load commands (increment 4)
/// can be submitted into this ring. Gated on the daemon-confirmed PSP regs + the
/// mapped VRAM aperture, so it never runs on QEMU. Register values are
/// Athena-oracle-confirmed (live ring @ MC 0x8000339000, size 0x1000).
pub fn psp_ring_create<O: GpuOps>(ops: &mut O) -> Option<PspRing> {
    let psp = ops.psp_regs()?;
    let Some(vram_base) = ops.vram_mc_base() else {
        ops.log("[amdgpu] PSP ring-create SKIPPED — VRAM aperture unmapped (QEMU/no discovery)");
        return None;
    };
    let ring_mc = vram_base.wrapping_add(PSP_RING_VRAM_OFFSET);
    // Clear the ring region in VRAM before the sOS reads it.
    ops.vram_zero(PSP_RING_VRAM_OFFSET, PSP_RING_SIZE as usize);

    // (1) Wait for the sOS to be ready for ring creation (C2PMSG_64 bit31).
    let ready = psp_poll_mbox(ops, psp.ring_cmd, PSP_RING_READY_TIMEOUT_US);
    if !ready {
        let v = ops.reg_read(psp.ring_cmd);
        ops.log(&format!(
            "[amdgpu] PSP ring-create ABORT — sOS not ready (C2PMSG_64={v:#010x} bit31 clear)"
        ));
        return None;
    }
    // (2) Program the ring buffer address + size, then issue INIT_GPCOM_RING.
    ops.reg_write(psp.ring_lo, (ring_mc & 0xFFFF_FFFF) as u32);
    ops.reg_write(psp.ring_hi, (ring_mc >> 32) as u32);
    ops.reg_write(psp.ring_size, PSP_RING_SIZE);
    let cmd = psp_ring_init_cmd(PSP_RING_TYPE_KM); // KM<<16 = 0x00020000
    ops.reg_write(psp.ring_cmd, cmd);
    ops.log(&format!(
        "[amdgpu] PSP ring-create: ring@MC {ring_mc:#x} size {:#x} -> C2PMSG_69/70/71, cmd {cmd:#010x} to C2PMSG_64",
        PSP_RING_SIZE
    ));
    // (3) Handshake delay (amdgpu mdelay(20)), then poll for completion.
    ops.delay_us(20_000);
    if !psp_poll_mbox(ops, psp.ring_cmd, PSP_RING_DONE_TIMEOUT_US) {
        let v = ops.reg_read(psp.ring_cmd);
        ops.log(&format!(
            "[amdgpu] PSP ring-create TIMEOUT — no completion (C2PMSG_64={v:#010x})"
        ));
        return None;
    }
    let resp = ops.reg_read(psp.ring_cmd);
    let err = resp & PSP_MBOX_ERR_MASK;
    if err != 0 {
        ops.log(&format!(
            "[amdgpu] PSP ring-create REJECTED — C2PMSG_64={resp:#010x} error=0x{err:04x}"
        ));
        return None;
    }
    ops.log(&format!(
        "[amdgpu] PSP ring-create OK — GPCOM ring live @ MC {ring_mc:#x} (C2PMSG_64={resp:#010x}); NEXT: LOAD_TOC + SETUP_TMR"
    ));
    Some(PspRing {
        ring_mc,
        ring_size: PSP_RING_SIZE,
    })
}

/// Poll a PSP mailbox register for its ready/done flag (bit31). Paced with
/// `delay_us` so it spans real time and yields CPU0 (the PSP can take ms to answer).
fn psp_poll_mbox<O: GpuOps>(ops: &mut O, reg: u32, timeout_us: u32) -> bool {
    let mut waited = 0u32;
    loop {
        if ops.reg_read(reg) & PSP_MBOX_FLAG != 0 {
            return true;
        }
        if waited >= timeout_us {
            return false;
        }
        ops.delay_us(SMU_POLL_STEP_US);
        waited = waited.saturating_add(SMU_POLL_STEP_US);
    }
}

/// Stage 5 — `psp_*`/`smu_*`: secure-processor + power firmware. Absent blobs
/// are non-fatal here. APU note: the SMU/PMFW image is part of the system BIOS
/// (the PSP bootloader loads it from there), so only the PSP TOC + TA blobs come
/// from the FS. The mailbox handshake runs only when the ASIC's offsets are
/// confirmed (see [`GpuOps::smu_mailbox`]).
pub fn init_smu<O: GpuOps>(ops: &mut O, dev: &Device) -> bool {
    let toc = ops.request_firmware("amdgpu/psp_13_0_4_toc.bin");
    let ta = ops.request_firmware("amdgpu/psp_13_0_4_ta.bin");
    if toc && ta {
        ops.log("[amdgpu] stage 5 PSP firmware loaded (APU PMFW from BIOS); mailbox power-up next");
    } else {
        ops.log("[amdgpu] stage 5 PSP firmware missing (add to firmware/amdgpu/)");
    }
    // The VBIOS bootup clocks (decoded in stage 2) are the SMU's starting point
    // before DPM ramps them; 0 on QEMU (no VBIOS), real on Athena.
    if dev.bootup_sclk_mhz != 0 {
        ops.log(&format!(
            "[amdgpu] stage 5 SMU bootup clocks from VBIOS: sclk={} MHz mclk={} MHz",
            dev.bootup_sclk_mhz, dev.bootup_mclk_mhz
        ));
    }
    // SMU mailbox handshake (PMFW request-response). Attempted only when the
    // ASIC's mailbox offsets are confirmed; otherwise skipped so we never write
    // guessed MMIO on real hardware. A timeout/non-OK is logged, not fatal yet
    // (full power-up wiring is a later step).
    if let Some(mb) = ops.smu_mailbox() {
        const PPSMC_MSG_GET_SMU_VERSION: u32 = 0x02;
        /// ~1 s PMFW response budget, matching amdgpu's `usec_timeout` (the old
        /// 1000 bare reads finished in microseconds — far too short for the PMFW).
        const SMU_TIMEOUT_US: u32 = 1_000_000;
        // Pre-read the response reg: a LIVE PMFW mailbox is non-zero here (it posted
        // a status after its boot command). A stuck 0x0 strongly implies the offset
        // is wrong (not just a slow PMFW). Logged with the resolved offsets so one
        // flash tells us which.
        let pre_resp = ops.reg_read(mb.resp_reg);
        ops.log(&format!(
            "[amdgpu] stage 5 SMU mailbox @ msg={:#x} arg={:#x} resp={:#x} pre-resp={:#010x}",
            mb.msg_reg, mb.arg_reg, mb.resp_reg, pre_resp
        ));
        match smu_send_msg(ops, &mb, PPSMC_MSG_GET_SMU_VERSION, 0, SMU_TIMEOUT_US) {
            Some(SMU_RESP_OK) => {
                // On success the PMFW returns the value in the argument register.
                let ver = ops.reg_read(mb.arg_reg);
                ops.log(&format!(
                    "[amdgpu] stage 5 SMU mailbox OK (GetSmuVersion = {ver:#010x})"
                ));
            }
            Some(resp) => ops.log(&format!(
                "[amdgpu] stage 5 SMU mailbox non-OK resp {resp:#x}"
            )),
            None => {
                // Read back what we wrote. If msg reads our 0x2, the aperture is
                // writable -> the PMFW is genuinely silent (readiness/timing/version
                // path). If msg reads 0x0/garbage, the resolved offset is WRONG.
                let (m, a, r) = (
                    ops.reg_read(mb.msg_reg),
                    ops.reg_read(mb.arg_reg),
                    ops.reg_read(mb.resp_reg),
                );
                ops.log(&format!(
                    "[amdgpu] stage 5 SMU mailbox TIMEOUT — readback msg={m:#x} arg={a:#x} resp={r:#x} (msg==0x2 => offset OK/PMFW silent; else offset WRONG)"
                ));
            }
        }
    }
    true
}

/// Poll `GRBM_STATUS.GUI_ACTIVE` until the GFX pipe reports idle, or give up
/// after `max_polls`. The `gfx_v*` bring-up must confirm the pipe is idle before
/// programming the command processor — and must NOT spin forever if the GPU
/// never quiesces, so a timeout returns false (a real, surfaceable failure).
pub fn wait_for_gfx_idle<O: GpuOps>(ops: &mut O, max_polls: u32) -> bool {
    // Discovery-resolved SOC15 GRBM_STATUS when available, else the gc11 legacy
    // offset (QEMU/pre-discovery).
    let grbm = ops
        .gfx_regs()
        .map_or(gc11::MM_GRBM_STATUS, |g| g.grbm_status);
    for _ in 0..max_polls {
        if ops.reg_read(grbm) & gc11::GRBM_STATUS_GUI_ACTIVE == 0 {
            return true;
        }
    }
    false
}

/// `gfx_v11_0_cp_gfx_enable` — release the CP from halt (`enable=true`) or halt
/// it (`enable=false`) via `CP_ME_CNTL`. On gfx11 the CP microcode (PFP/ME, and
/// the compute MEC) is RS64 and **PSP-loaded on Phoenix**, so this halt-bit
/// toggle is the driver's CP enable step — NOT a microcode upload (the legacy
/// `CP_PFP_UCODE_ADDR/DATA` port-write path does not exist on gfx11). Read-modify-
/// write so only the halt bits change; other `CP_ME_CNTL` fields are preserved.
///
/// Copy a byte blob (firmware ucode/data) into a DMA buffer via `dma_write` (which
/// takes u32 words): pack the bytes little-endian into u32s, zero-padding the final
/// partial word, and write them at dword 0. Used to direct-load the MES ucode/data
/// into GART-mapped buffers (the MES reads them through VMID0).
fn dma_write_bytes<O: GpuOps>(ops: &mut O, buf: &DmaBuf, bytes: &[u8]) {
    let mut words: Vec<u32> = Vec::with_capacity((bytes.len() + 3) / 4);
    for c in bytes.chunks(4) {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        words.push(u32::from_le_bytes(b));
    }
    ops.dma_write(buf, 0, &words);
}

/// Submit one MES API packet on the MES command ring + poll its completion fence
/// (`mes_v11_0_submit_pkt_and_poll_completion`). Writes the 64-dword packet into the
/// ring at the current wptr (the ring is 16384 dwords, packets 64 — no wrap concern
/// for the first few), advances wptr, writes it to the wptr-poll dword, rings the MES
/// doorbell, then polls `fence` dword 0 for `fence_value` (MES writes the packet's
/// api_completion_fence). Returns true when the MES acks. Bounded so a silent MES
/// never hangs the boot.
#[allow(clippy::too_many_arguments)]
fn mes_submit_and_poll<O: GpuOps>(
    ops: &mut O,
    ring: &DmaBuf,
    wptr: &mut u32,
    packet: &[u32],
    wptr_poll: &DmaBuf,
    wptr_poll_dw: usize,
    doorbell_byte: u32,
    fence: &DmaBuf,
    fence_dw: usize,
    fence_gart_va: u64,
    fence_value: u32,
    max_polls: u32,
) -> bool {
    let at = *wptr as usize;
    // Write the API frame, THEN append a QUERY_SCHEDULER_STATUS frame and poll ITS fence
    // — this is amdgpu's `mes_v11_0_submit_pkt_and_poll_completion` protocol: it pairs a
    // QUERY with EVERY submission and waits on the QUERY's completion fence, never the
    // preceding packet's. The MES processes the ring up to the QUERY, which flushes the
    // batch and writes the fence. We had submitted the lone packet and polled ITS fence,
    // which the MES never wrote (iron: rptr advanced past our frame, no ack). The QUERY's
    // api_completion_fence points at `fence_gart_va`/value `fence_value` = the loc we poll.
    // No NOP padding: the live working SCHED ring holds CONSECUTIVE 64-dword frames.
    ops.dma_write(ring, at, packet);
    let query = crate::mes::build_mes_query_scheduler_status(fence_gart_va, fence_value as u64);
    ops.dma_write(ring, at + packet.len(), &query);
    *wptr = wptr.wrapping_add((packet.len() + query.len()) as u32);
    ops.dma_write(wptr_poll, wptr_poll_dw, &[*wptr]);
    ops.ring_doorbell(doorbell_byte, *wptr as u64);
    let mut got = [0u32; 1];
    for _ in 0..max_polls {
        ops.dma_read(fence, fence_dw, &mut got);
        if got[0] == fence_value {
            return true;
        }
        ops.delay_us(1);
    }
    false
}

/// Push a PM4/MES packet onto a ring + ring its doorbell, WITHOUT polling a fence —
/// for the KIQ `MAP_QUEUES` (which carries no completion fence; the proof that it
/// worked is the SCHED ring's later `set_hw_resources` ACK).
fn mes_ring_push<O: GpuOps>(
    ops: &mut O,
    ring: &DmaBuf,
    wptr: &mut u32,
    packet: &[u32],
    wptr_poll: &DmaBuf,
    wptr_poll_dw: usize,
    doorbell_byte: u32,
) {
    let at = *wptr as usize;
    ops.dma_write(ring, at, packet);
    *wptr = wptr.wrapping_add(packet.len() as u32);
    ops.dma_write(wptr_poll, wptr_poll_dw, &[*wptr]);
    ops.ring_doorbell(doorbell_byte, *wptr as u64);
}

/// GATED: a no-op returning `false` until [`GpuOps::cp_me_cntl_halt_mask`] is
/// confirmed (`None` by default), so a guessed bit pattern is NEVER written to the
/// live CP on real hardware. Returns `true` only when it actually wrote.
pub fn cp_gfx_enable<O: GpuOps>(ops: &mut O, cp_me_cntl: u32, enable: bool) -> bool {
    let Some(halt_mask) = ops.cp_me_cntl_halt_mask() else {
        return false;
    };
    let cur = ops.reg_read(cp_me_cntl);
    let new = if enable {
        cur & !halt_mask // clear the halt bits -> CP runs
    } else {
        cur | halt_mask // set the halt bits -> CP halted
    };
    ops.reg_write(cp_me_cntl, new);
    true
}

/// `soc21_grbm_select` — select which CP me/pipe/queue/vmid subsequent indexed CP
/// register writes target, by writing `GRBM_GFX_CNTL`. Field layout (gc_11_0_0):
/// PIPEID[1:0] | MEID[3:2] | VMID[7:4] | QUEUEID[10:8].
fn grbm_select<O: GpuOps>(
    ops: &mut O,
    grbm_gfx_cntl: u32,
    me: u32,
    pipe: u32,
    queue: u32,
    vmid: u32,
) {
    let v = (pipe & 0x3) | ((me & 0x3) << 2) | ((vmid & 0xf) << 4) | ((queue & 0x7) << 8);
    ops.reg_write(grbm_gfx_cntl, v);
}

// CP_ME_CNTL / CP_MEC_RS64_CNTL pipe-reset bit masks (gc_11_0_0_sh_mask.h).
const CP_ME_CNTL_PFP_PIPE_RESET: u32 = (1 << 18) | (1 << 19); // PFP_PIPE0/1_RESET
const CP_ME_CNTL_ME_PIPE_RESET: u32 = (1 << 20) | (1 << 21); // ME_PIPE0/1_RESET
const CP_MEC_RS64_CNTL_PIPE_RESET: u32 = 0xF << 16; // MEC_PIPE0..3_RESET

/// Convert an RS64 ucode_start_addr (the combined `hi:lo` from the fw header) into
/// the `(PRGRM_CNTR_START, PRGRM_CNTR_START_HI)` register pair the RS64 CP expects.
///
/// The PC register holds a **DWORD** (>>2) instruction address, NOT a byte address:
/// `gfx_v11_0_config_gfx_rs64` writes `START = (addr_lo >> 2) | (addr_hi << 30)` and
/// `START_HI = addr_hi >> 2`. Writing the raw byte address (the prior bug here)
/// starts the RS64 core at 4× its real entry point, so the PFP/ME *runs but is
/// stuck* — `CP_STAT` shows PFP_BUSY and the gfx ring never advances (RPTR stays 0,
/// no fetch). This split is the gfx11-exact computation.
fn rs64_pc_start(addr: u64) -> (u32, u32) {
    let lo = addr as u32;
    let hi = (addr >> 32) as u32;
    let start = (lo >> 2) | (hi << 30);
    let start_hi = hi >> 2;
    (start, start_hi)
}

/// `gfx_v11_0_config_gfx_rs64` — start the RS64 CP cores. For each PFP/ME pipe and
/// each MEC pipe: `grbm_select` it, write its program-counter START (from the fw
/// header ucode_start_addr), then pulse the pipe RESET bit (1→0) so the core
/// (re)starts executing the PSP-loaded ucode at that address. This is the gfx11
/// step that makes `CP_ME_CNTL` halt-clear meaningful — without it the RS64 cores
/// never run and the CP ring registers stay dead (read back 0). Gated by the
/// caller on BOTH `rs64_cp_regs` (offsets) and `rs64_ucode_starts` (addresses) so
/// no guessed program counter is ever written to the live CP.
pub fn config_gfx_rs64<O: GpuOps>(ops: &mut O, regs: &Rs64CpRegs, starts: &Rs64UcodeStarts) {
    let g = regs.grbm_gfx_cntl;
    // PC-start values are DWORD addresses (>>2 with the hi carry) — see rs64_pc_start.
    let (pfp_s, pfp_s_hi) = rs64_pc_start(starts.pfp);
    let (me_s, me_s_hi) = rs64_pc_start(starts.me);
    let (mec_s, mec_s_hi) = rs64_pc_start(starts.mec);
    // PFP pipes 0,1: set start address.
    for pipe in 0..2 {
        grbm_select(ops, g, 0, pipe, 0, 0);
        ops.reg_write(regs.pfp_start, pfp_s);
        ops.reg_write(regs.pfp_start_hi, pfp_s_hi);
    }
    // Reset PFP pipes (1 then 0).
    let tmp = ops.reg_read(regs.me_cntl);
    ops.reg_write(regs.me_cntl, tmp | CP_ME_CNTL_PFP_PIPE_RESET);
    ops.reg_write(regs.me_cntl, tmp & !CP_ME_CNTL_PFP_PIPE_RESET);
    // ME pipes 0,1: set start address.
    for pipe in 0..2 {
        grbm_select(ops, g, 0, pipe, 0, 0);
        ops.reg_write(regs.me_start, me_s);
        ops.reg_write(regs.me_start_hi, me_s_hi);
    }
    // Reset ME pipes.
    let tmp = ops.reg_read(regs.me_cntl);
    ops.reg_write(regs.me_cntl, tmp | CP_ME_CNTL_ME_PIPE_RESET);
    ops.reg_write(regs.me_cntl, tmp & !CP_ME_CNTL_ME_PIPE_RESET);
    // MEC pipes 0-3: set start address.
    for pipe in 0..4 {
        grbm_select(ops, g, 1, pipe, 0, 0);
        ops.reg_write(regs.mec_start, mec_s);
        ops.reg_write(regs.mec_start_hi, mec_s_hi);
    }
    // Reset MEC pipes.
    let tmp = ops.reg_read(regs.mec_cntl);
    ops.reg_write(regs.mec_cntl, tmp | CP_MEC_RS64_CNTL_PIPE_RESET);
    ops.reg_write(regs.mec_cntl, tmp & !CP_MEC_RS64_CNTL_PIPE_RESET);
    // Restore the default grbm selection.
    grbm_select(ops, g, 0, 0, 0, 0);
}

/// Parse `gfx_firmware_header_v2_0.ucode_start_addr_{lo,hi}` (blob offset 0x34/
/// 0x38, after the 32-byte common header) for an RS64 ucode blob. `None` unless
/// the header is v2 (`header_version_major == 2` at offset 8) and long enough —
/// so a non-RS64 / truncated blob never yields a bogus program counter.
pub fn parse_rs64_ucode_start(blob: &[u8]) -> Option<u64> {
    if blob.len() < 0x3c {
        return None;
    }
    let rd16 = |o: usize| u16::from_le_bytes([blob[o], blob[o + 1]]);
    let rd32 = |o: usize| u32::from_le_bytes([blob[o], blob[o + 1], blob[o + 2], blob[o + 3]]);
    if rd16(8) != 2 {
        return None; // header_version_major must be 2 (RS64 v2_0)
    }
    let lo = rd32(0x34) as u64;
    let hi = rd32(0x38) as u64;
    Some((hi << 32) | lo)
}

/// Direct-load the gfx11 dual-thread RS64 SDMA microcode (`sdma_v6_0_load_microcode`,
/// broadcast mode). This Phoenix PSP REJECTS SDMA via the gfx LOAD_IP_FW path
/// (0xffff0010, amdgpud line 241), so the F32 engine had no firmware and never ran
/// (iron boots 170257/183116: RB_RPTR stayed 0 across every kick). We load it
/// ourselves while GFX stays PSP-loaded: halt the engine, stream TH0 (context) into
/// the broadcast window at ADDR=0, then TH1 (control) at ADDR=0x8000. The DATA
/// register auto-increments. Returns false on a malformed blob or unresolved offsets.
pub fn load_sdma_microcode<O: GpuOps>(ops: &mut O, sdma: &SdmaRegs, blob: &[u8]) -> bool {
    let Some((th0, th1)) = sdma::sdma_ucode_slices(blob) else {
        return false;
    };
    // Halt the engine before loading (sdma_v6_0_enable(false)).
    let f32 = ops.reg_read(sdma.f32_cntl);
    ops.reg_write(sdma.f32_cntl, f32 | sdma::SDMA_F32_CNTL_HALT);
    // TH0 (context) at ADDR=0, then TH1 (control) at ADDR=0x8000.
    for (addr, image) in [
        (sdma::SDMA_UCODE_ADDR_TH0, th0),
        (sdma::SDMA_UCODE_ADDR_TH1, th1),
    ] {
        ops.reg_write(sdma.broadcast_ucode_addr, addr);
        for c in image.chunks_exact(4) {
            ops.reg_write(
                sdma.broadcast_ucode_data,
                u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            );
        }
    }
    // sdma_v6_0_enable(true) — un-halt the F32 so it boots the loaded ucode BEFORE the
    // ring is programmed (amdgpu's start order: load_microcode -> enable(true) ->
    // gfx_resume). Clear HALT (bit 0) AND TH1_RESET (bit 13) for the dual-thread RS64.
    let f32 = ops.reg_read(sdma.f32_cntl);
    ops.reg_write(
        sdma.f32_cntl,
        f32 & !sdma::SDMA_F32_CNTL_HALT & !sdma::SDMA_F32_CNTL_TH1_RESET,
    );
    true
}

/// `sdma_v6_0_gfx_resume_instance` (essential subset) — program the SDMA0 QUEUE0
/// ring registers and submit a job via the **F32 WPTR-poll** mechanism the live
/// Athena amdgpu actually uses (confirmed by `umr`: RB_CNTL=0x841817 has
/// F32_WPTR_POLL_ENABLE=1, RB_PRIV=1, and the RB_WPTR register stays 0):
///   * `RB_BASE`/`RB_BASE_HI` = ring GPU address `>> 8` / `>> 40` (the SDMA ring
///     base is 256-byte-shifted, UNLIKE the CP ring's raw low/high split);
///   * `RB_WPTR_POLL_ADDR_{LO,HI}` = the `wptr_poll` memory dword's address — the
///     F32 firmware polls THIS for the write pointer, not the `RB_WPTR` register;
///   * `RB_CNTL` = `RB_ENABLE | (log2(ring DWORDS) << RB_SIZE_SHIFT) |
///     F32_WPTR_POLL_ENABLE | RB_PRIV`;
///   * un-halt the engine, then SUBMIT by writing `wptr_dwords << 2` (the byte
///     offset) into the `wptr_poll` memory — the polling F32 sees it and drains the
///     ring. Writing the `RB_WPTR` register (the old path) was silently ignored by
///     this autoloaded gfx11 SDMA, so the fence never posted (iron boot 152447).
///
/// `wptr_poll` must be a GART-mapped buffer (the engine reads it through VMID0);
/// `wptr_poll_dw` is its DWORD offset. Verifies `RB_BASE` reads back.
pub fn program_sdma_ring<O: GpuOps>(
    ops: &mut O,
    sdma: &SdmaRegs,
    ring_dma_addr: u64,
    ring_bytes: u32,
    wptr_dwords: u32,
    wptr_poll: &DmaBuf,
    wptr_poll_dw: usize,
    gart_delta: u64,
) -> bool {
    let Some(bufsz) = sdma::ring_size_log2_dwords(ring_bytes) else {
        return false;
    };
    // Clear the pointers for a fresh ring.
    ops.reg_write(sdma.rb_rptr, 0);
    ops.reg_write(sdma.rb_rptr_hi, 0);
    ops.reg_write(sdma.rb_wptr, 0);
    ops.reg_write(sdma.rb_wptr_hi, 0);
    // Point the F32 WPTR-poll at the memory dword + zero it (fresh ring). The register
    // takes the GPU VA = phys + gart_delta (the GART aperture), while the dma_write
    // below uses the DmaBuf (phys) — `ring_dma_addr` is likewise the caller's GART VA.
    let poll_addr = wptr_poll
        .dma_addr
        .wrapping_add((wptr_poll_dw as u64) * 4)
        .wrapping_add(gart_delta);
    ops.reg_write(sdma.rb_wptr_poll_addr_lo, (poll_addr & 0xFFFF_FFFF) as u32);
    ops.reg_write(sdma.rb_wptr_poll_addr_hi, (poll_addr >> 32) as u32);
    ops.dma_write(wptr_poll, wptr_poll_dw, &[0]);
    // Ring base (256-byte shifted) + size/enable + F32 WPTR-poll + privileged ring.
    let want_base = (ring_dma_addr >> 8) as u32;
    ops.reg_write(sdma.rb_base, want_base);
    ops.reg_write(sdma.rb_base_hi, (ring_dma_addr >> 40) as u32);
    ops.reg_write(
        sdma.rb_cntl,
        sdma::SDMA_RB_CNTL_RB_ENABLE
            | (bufsz << sdma::SDMA_RB_CNTL_RB_SIZE_SHIFT)
            | sdma::SDMA_RB_CNTL_F32_WPTR_POLL_ENABLE
            | sdma::SDMA_RB_CNTL_RB_PRIV,
    );
    // Route the queue to the doorbell aperture (the live amdgpu's wake path — boot
    // 170257 proved F32_WPTR_POLL + the WPTR-in-memory alone left RB_RPTR=0). Set
    // this queue's doorbell byte-offset (Athena cold-trace SDMA0 QUEUE0 slot = 0x800)
    // + enable the doorbell; the actual ring (a 64-bit write to that aperture slot)
    // happens at submit, below.
    const SDMA0_Q0_DOORBELL_OFF: u32 = 0x800;
    ops.reg_write(sdma.doorbell_offset, SDMA0_Q0_DOORBELL_OFF);
    ops.reg_write(sdma.doorbell, sdma::SDMA_DOORBELL_ENABLE);
    // Program the UTC L1 (the engine's address-translation cache) BEFORE unhalting:
    // RESP_MODE=3 + REDO_DELAY=9, exactly as sdma_v6_0_gfx_resume_instance. Without
    // it the engine cannot resolve its ring/WPTR/fence GPU addresses through VMID0,
    // so it never fetches the ring (iron readback: ring armed + engine unhalted yet
    // RB_RPTR=0). RMW-preserve the other UTCL1 fields.
    let utcl1 = ops.reg_read(sdma.utcl1_cntl);
    ops.reg_write(
        sdma.utcl1_cntl,
        (utcl1 & !sdma::SDMA_UTCL1_CNTL_REDO_DELAY_MASK & !sdma::SDMA_UTCL1_CNTL_RESP_MODE_MASK)
            | sdma::SDMA_UTCL1_CNTL_VALUE,
    );
    // Start the SDMA engine (sdma_v6_0_enable(true) + the RS64 dual-thread run
    // controls): clear HALT (bit 0) and BOTH thread resets, and SET TH0_ENABLE +
    // TH1_ENABLE. The GOP leaves the engine with TH0_ENABLE=0 (iron readback
    // F32_CNTL=0x08084000), so merely clearing HALT left thread 0 — the thread that
    // drains the ring — disabled, and the engine never advanced RB_RPTR.
    let f32 = ops.reg_read(sdma.f32_cntl);
    ops.reg_write(
        sdma.f32_cntl,
        (f32 & !sdma::SDMA_F32_CNTL_HALT
            & !sdma::SDMA_F32_CNTL_TH0_RESET
            & !sdma::SDMA_F32_CNTL_TH1_RESET)
            | sdma::SDMA_F32_CNTL_TH0_ENABLE
            | sdma::SDMA_F32_CNTL_TH1_ENABLE,
    );
    // SUBMIT. THE FIX (iron 2026-07-01): the WPTR must be written to the RB_WPTR
    // REGISTER DIRECTLY — the F32 WPTR-poll memory + doorbell aperture ALONE left
    // RB_WPTR=0 (engine saw an empty ring, RB_RPTR never advanced). With the direct
    // register write the F32 picks up the WPTR, drains the ring, executes the
    // CONSTANT_FILL, and posts its completion fence ("SDMA fill COMPLETE"). Write
    // all three (poll mem + register + doorbell) to match the live amdgpu wake path.
    let wptr_bytes = wptr_dwords << 2;
    ops.dma_write(wptr_poll, wptr_poll_dw, &[wptr_bytes]);
    ops.reg_write(sdma.rb_wptr, wptr_bytes);
    ops.reg_write(sdma.rb_wptr_hi, 0);
    ops.ring_doorbell(SDMA0_Q0_DOORBELL_OFF, wptr_bytes as u64);
    let wptr_rb = ops.reg_read(sdma.rb_wptr);
    ops.log(&format!(
        "[amdgpu] stage 6 SDMA WPTR direct-write: wrote {wptr_bytes:#x} -> RB_WPTR reads {wptr_rb:#x} (direct register write is what makes the F32 drain the ring)"
    ));
    // Verify the base programmed (catches a wrong offset / a gated block).
    ops.reg_read(sdma.rb_base) == want_base
}

/// Submit the SDMA job already written into `ring` and WAIT for the engine to
/// post `fence_value` into `fence` — the SDMA equivalent of
/// [`submit_and_wait_fence`]. [`program_sdma_ring`] rings the queue (advancing
/// WPTR); the engine drains the ring (the CONSTANT_FILL, then the FENCE packet
/// that writes `fence_value` to `fence`), and this polls that memory. Returns
/// true on completion, false on a program failure OR a poll timeout — so on iron
/// the bootlog distinguishes "the SDMA engine actually executed the fill" from
/// "submitted but the engine never drained" (a wedged queue must surface, not
/// hang the boot). On QEMU/host there is no engine, so a timeout is expected.
#[allow(clippy::too_many_arguments)]
pub fn sdma_submit_and_wait<O: GpuOps>(
    ops: &mut O,
    sdma: &SdmaRegs,
    ring_dma_addr: u64,
    ring_bytes: u32,
    wptr_dwords: u32,
    fence: &DmaBuf,
    fence_value: u32,
    max_polls: u32,
    wptr_poll: &DmaBuf,
    wptr_poll_dw: usize,
    gart_delta: u64,
) -> bool {
    if !program_sdma_ring(
        ops,
        sdma,
        ring_dma_addr,
        ring_bytes,
        wptr_dwords,
        wptr_poll,
        wptr_poll_dw,
        gart_delta,
    ) {
        return false;
    }
    let mut got = [0u32; 1];
    for _ in 0..max_polls {
        ops.dma_read(fence, 0, &mut got);
        if got[0] == fence_value {
            return true;
        }
    }
    false
}

/// Build + enable the GFX GART (gfxhub GPUVM) so the CP can fetch its ring. The
/// Phoenix firmware leaves GFX GPUVM unconfigured (boot 162824: every gfxhub VM
/// reg reads 0), so VMID0 has no mapping and the CP ring registers read back 0.
/// This: allocates a flat page table, maps GART VA 0.. onto the ring's system
/// pages, programs the gfxhub enable sequence (L2 / aperture-disabled / VMID0
/// page table / L1 TLB / TLB-invalidate via [`crate::gart`]), and returns the
/// ring's GART VA (0) for the CP ring base. `None` when the offsets aren't resolved
/// (QEMU / pre-discovery) — then NO GMC register is touched. GATED + SAFE-ish: it
/// writes the GFX hub, which is SEPARATE from the display hub (DCN/MMHUB), so a
/// wrong layout faults the CP — never the live display (the bootlog still persists).
pub fn init_gart<O: GpuOps>(
    ops: &mut O,
    dev: &Device,
    ring_dma_addr: u64,
    ring_bytes: u64,
) -> Option<u64> {
    let regs = ops.gfxhub_gart_regs()?;
    let ring_pages = gart::pages_for(ring_bytes).max(1);
    // GART VA page i -> the ring's system page i, so GART VA 0 == the ring start.
    let table = gart::build_identity_gart(ring_dma_addr, ring_pages);
    let table_bytes = (table.len() * 8).max(4096);
    let tbl = ops.dma_alloc(dev.handle, table_bytes)?;
    // The GMC reads PTEs as little-endian 64-bit; write each as a u32 lo/hi pair.
    let mut words: Vec<u32> = Vec::with_capacity(table.len() * 2);
    for pte in &table {
        words.push((*pte & 0xFFFF_FFFF) as u32);
        words.push((*pte >> 32) as u32);
    }
    ops.dma_write(&tbl, 0, &words);
    // Place the GART aperture at a NON-ZERO GPU VA so CP_RB0_BASE reads back a
    // distinguishable value (VA 0 would make the base 0, trivially matching the
    // gated-0 readback). The flat page table is indexed (va - start)>>12, so VA
    // GART_VA_BASE -> table[0] -> the ring's first page.
    const GART_VA_BASE: u64 = 0x0080_0000; // 8 MiB
    let gart_va_end = GART_VA_BASE + (ring_pages as u64) * gart::GPU_PAGE_SIZE;
    let config = gart::GartConfig {
        table_phys: tbl.dma_addr,
        gart_va_start: GART_VA_BASE,
        gart_va_end,
        fb_base: 0,
        fb_top: 0,
        agp_bot: 0,
        agp_top: 0,
        // Empty system aperture (low > high) so EVERY VA routes through our page
        // table — the ring is reached purely via our PTEs, with no dependency on
        // the (unknown) APU FB/AGP carve-out layout.
        sys_low: u32::MAX as u64,
        sys_high: 0,
        default_page: ring_dma_addr,
    };
    for (reg, val) in gart::build_gart_enable_sequence(&regs, &config) {
        ops.reg_write(reg, val);
    }
    ops.log(&format!(
        "[amdgpu] GART built: table@{:#x} ({} PTEs) maps GART VA {:#x}..{:#x} -> ring@{:#x}; VMID0 enabled, ring base uses GART VA",
        tbl.dma_addr,
        table.len(),
        GART_VA_BASE,
        gart_va_end,
        ring_dma_addr
    ));
    Some(GART_VA_BASE)
}

/// amdgpu's GFX GART aperture base — the HIGH GPU VA where the gfxhub maps kernel
/// ring buffers (`gmc.gart_start`). The Athena working driver (umr 2026-06-27) puts
/// its gfx ring at CP_RB0_BASE_HI=0x7f (GPU VA 0x7fff_xxxx_xxxx) with
/// GCVM_CONTEXT0_PAGE_TABLE_START = 0x7fff_0000_0000 (>>12 = 0x7_fff00000). The
/// gfxhub routes ONLY this high region through the VMID0 page table — a LOW
/// identity VA (our old scheme, BASE_HI=0) is never translated, so the CP's PFP
/// could never fetch the ring (iron: RPTR stayed 0, CP_STAT PFP_BUSY). Mapping the
/// ring at this aperture is THE CP-fetch fix.
pub const GFX_GART_APERTURE_BASE: u64 = 0x0000_7fff_0000_0000;

/// Build the GFXHUB VMID0 GART, mapping the page-aligned physical span `[lo, hi)`
/// into the GART APERTURE at [`GFX_GART_APERTURE_BASE`] (matching amdgpu). GART VA
/// `GFX_GART_APERTURE_BASE + (phys - lo)` resolves to `phys`, so every buffer's GPU
/// VA is `phys + (GFX_GART_APERTURE_BASE - lo)` — the uniform `delta` the caller
/// applies to ring/writeback/fence addresses. This replaces the old VA==PA identity
/// window, which the iron proved the gfxhub never translates (low VAs bypass the
/// page table). amdgpu's own gart_start is exactly this aperture base.
///
/// MUST run AFTER first light (the RLC autoload completing): the gfxhub registers
/// sit in the GFX power domain, so writes issued while GFX is still in reset are
/// silently dropped — exactly the bug the iron bootlog (boot 032117) showed, where
/// the GART was "built" before first light and every gfxhub VM reg read back 0. The
/// Athena cold trace shows amdgpu programs the gfxhub VM immediately after it reads
/// RLC_BOOTLOAD_STATUS = 0xc000001f. Returns true once VMID0 is enabled; `false`
/// when the gfxhub offsets are unresolved (QEMU/pre-discovery) — then NO GMC
/// register is touched.
pub fn init_gart_identity<O: GpuOps>(ops: &mut O, dev: &Device, lo: u64, hi: u64) -> bool {
    let Some(regs) = ops.gfxhub_gart_regs() else {
        return false;
    };
    let lo = lo & !(gart::GPU_PAGE_SIZE - 1);
    let hi = (hi + gart::GPU_PAGE_SIZE - 1) & !(gart::GPU_PAGE_SIZE - 1);
    let pages = ((hi - lo) / gart::GPU_PAGE_SIZE).max(1) as usize;
    // Aperture PTEs: GART page i (at VA GFX_GART_APERTURE_BASE + i*4K) -> physical
    // lo + i*4K. The window is placed at the HIGH aperture (not VA==PA) because the
    // gfxhub only translates that region; the caller offsets ring/buffer addresses
    // by delta = GFX_GART_APERTURE_BASE - lo to match.
    let table = gart::build_identity_gart(lo, pages);
    let table_bytes = (table.len() * 8).max(4096);
    let Some(tbl) = ops.dma_alloc(dev.handle, table_bytes) else {
        return false;
    };
    // The GMC reads PTEs as little-endian 64-bit; write each as a u32 lo/hi pair.
    let mut words: Vec<u32> = Vec::with_capacity(table.len() * 2);
    for pte in &table {
        words.push((*pte & 0xFFFF_FFFF) as u32);
        words.push((*pte >> 32) as u32);
    }
    ops.dma_write(&tbl, 0, &words);
    // System aperture = the VRAM FB range (amdgpu gfxhub_v3_0_init_system_aperture:
    // fb_start/end >> 18), so the GPU reaches the framebuffer + VRAM DIRECTLY — the
    // missing piece for the GPU drawing to the display. Athena live umr confirms
    // SYSTEM_APERTURE_LOW/HIGH = 0x200000/0x201fff (fb_start=0x80_0000_0000, 2 GiB).
    // SAFE for the SDMA path: our system-RAM ring/fence sit ~15 GiB up, FAR below the
    // 512 GiB FB base, so they stay OUTSIDE this aperture and keep resolving through
    // the page table exactly as before — only VRAM access is newly enabled. None
    // (QEMU / pre-GMC) -> empty aperture (everything via the page table, unchanged).
    let (sys_low, sys_high) = match ops.vram_mc_base() {
        Some(fb) if fb != 0 && dev.vram_size != 0 => (fb, fb + dev.vram_size - 1),
        _ => (u32::MAX as u64, 0),
    };
    // FB_LOCATION = the same VRAM MC window as the system aperture. The working
    // driver INHERITS this from POST (no write in the cold mmiotrace; live umr reads
    // BASE=0x8000 = MC 0x80_0000_0000) — but our GFX domain arrives power-cycled and
    // we then wrote 0/0 over it (netlog-MESFIX-20260701: "FB[0x0..0x0]" while
    // SYS_APERTURE/L1_TLB held our values). A zero FB window points the hub's FB
    // range at MC [0,16MB) — a real divergence from the working translation state in
    // effect exactly when the MES set_hw_resources handler freezes. When the window
    // is unknown (QEMU/no vram_mc_base), pass 0/0 — build_gart_enable_sequence now
    // SKIPS the FB write instead of clobbering the inherited value.
    let (fb_base, fb_top) = if sys_low <= sys_high {
        (sys_low, sys_high)
    } else {
        (0, 0)
    };
    let config = gart::GartConfig {
        table_phys: tbl.dma_addr,
        gart_va_start: GFX_GART_APERTURE_BASE,
        // inclusive last byte, matching amdgpu's END register; window length = span.
        gart_va_end: GFX_GART_APERTURE_BASE + (hi - lo) - 1,
        fb_base,
        fb_top,
        agp_bot: 0,
        agp_top: 0,
        sys_low,
        sys_high,
        default_page: lo,
    };
    for (reg, val) in gart::build_gart_enable_sequence(&regs, &config) {
        ops.reg_write(reg, val);
    }
    // POLL the invalidate ACK — amdgpu_gmc_fw_reg_write_reg_wait waits for the GMC to
    // confirm the VMID0 TLB flush before any engine fetches through it. Skipping this
    // (our old code only wrote REQ) left the walker on stale/empty entries, so the CP
    // and SDMA both faulted with RPTR=0 despite a correct page table. Bounded so a
    // silent GMC never hangs the boot; log whether it completed.
    let mut acked = false;
    for _ in 0..10_000 {
        if ops.reg_read(regs.invalidate_eng0_ack) & gart::GCVM_INVALIDATE_VMID0_ACK != 0 {
            acked = true;
            break;
        }
        ops.delay_us(1);
    }
    ops.log(if acked {
        "[amdgpu] GART TLB invalidate ACKed (VMID0 flush complete — walker live)"
    } else {
        "[amdgpu] GART TLB invalidate NOT ACKed (timeout) — VMID0 walker may be stale"
    });
    ops.log(&format!(
        "[amdgpu] GART (aperture) built post-first-light: table@{:#x} ({} PTEs) maps GPU VA {:#x}.. -> phys {:#x}..{:#x}; FB aperture [{:#x}..{:#x}] (VRAM reachable); VMID0 enabled",
        tbl.dma_addr,
        table.len(),
        GFX_GART_APERTURE_BASE,
        lo,
        hi,
        sys_low,
        sys_high
    ));
    true
}

/// Wait for the GFX block to be AWAKE before programming the CP ring. Root cause
/// of the persistent CP-ring readback-0 (boot 170417: GART built + ring base =
/// GART VA, BASE still read 0): by stage 6 the idle GFX has entered GFXOFF
/// (power-gated), so register writes are dropped and reads return 0 — independent
/// of GART. DisallowGfxOff asks the SMU to wake GFX, but the clock ramp takes time;
/// `probe_reg` (a live GFX reg the GOP left non-zero, e.g. CP_RB0_WPTR=0x3ffa208)
/// reads 0 while gated and non-zero once awake. Polling it both DETECTS the wake
/// and helps trigger it (a GFX register access nudges the GFXOFF state machine).
/// Returns true when awake. No-op-true if `probe_reg` is 0 (QEMU / no GOP ring).
pub fn wait_for_gfx_awake<O: GpuOps>(ops: &mut O, probe_reg: u32, timeout_us: u32) -> bool {
    let mut waited = 0u32;
    loop {
        if ops.reg_read(probe_reg) != 0 {
            return true;
        }
        if waited >= timeout_us {
            return false;
        }
        ops.delay_us(1000);
        waited = waited.saturating_add(1000);
    }
}

/// DIRECT-load step 1 (`imu_v11_0_load_microcode`): stream the IMU ucode into the
/// IMU's instruction- and data-RAM via the `GFX_IMU_{I,D}_RAM_{ADDR,DATA}` windows.
/// On Phoenix the PSP leaves GFX cold (iron boot 233600: `RLC_BOOTLOAD_STATUS=0`),
/// so the driver must load the firmware itself; with no ucode the IMU core has
/// nothing to run and `start_imu` (the CRESET release) times out. `blob` is
/// `gc_*_imu.bin` ([`GpuOps::request_firmware_bytes`]). Protocol per region: write
/// ADDR=0, push every dword through DATA (the address auto-increments), then write
/// ADDR=fw_version. Returns false if the header doesn't parse (no writes happen).
/// The caller gates the offsets on discovery (SOC15-resolved) — never a guessed MMIO.
pub fn load_imu_microcode<O: GpuOps>(
    ops: &mut O,
    i_ram_addr: u32,
    i_ram_data: u32,
    d_ram_addr: u32,
    d_ram_data: u32,
    blob: &[u8],
) -> bool {
    let Some(layout) = crate::imu::parse_imu_ucode_layout(blob) else {
        ops.log("[amdgpu] stage 6 IMU load: gc_*_imu.bin header parse FAILED");
        return false;
    };
    // I-RAM: reset the write address, stream the instruction ucode, set fw_version.
    ops.reg_write(i_ram_addr, 0);
    for i in 0..layout.iram_dwords() {
        let off = layout.iram_offset + i * 4;
        let dw = u32::from_le_bytes([blob[off], blob[off + 1], blob[off + 2], blob[off + 3]]);
        ops.reg_write(i_ram_data, dw);
    }
    ops.reg_write(i_ram_addr, layout.fw_version);
    // D-RAM: same protocol for the data ucode.
    ops.reg_write(d_ram_addr, 0);
    for i in 0..layout.dram_dwords() {
        let off = layout.dram_offset + i * 4;
        let dw = u32::from_le_bytes([blob[off], blob[off + 1], blob[off + 2], blob[off + 3]]);
        ops.reg_write(d_ram_data, dw);
    }
    ops.reg_write(d_ram_addr, layout.fw_version);
    ops.log(&format!(
        "[amdgpu] stage 6 IMU ucode loaded into SRAM: I-RAM {} dwords + D-RAM {} dwords (fw_version={:#x})",
        layout.iram_dwords(),
        layout.dram_dwords(),
        layout.fw_version
    ));
    true
}

/// Pure verdict for the IMU I-RAM readback diagnostic — host-KAT'able so the
/// classification logic can FAIL on the dev box, while the reads happen on iron.
/// Splits boot 024544's IMU-wake timeout into its two possible causes:
///   * `LANDED`   — readback == the ucode we streamed: the GFX_IMU register block
///     accepts writes, so the IMU has its program. If GFX still won't leave reset
///     the blocker is downstream (autoload-buffer MC address / contents).
///   * `ALL-ZERO` — readback is all 0: the whole GFX_IMU block is dropping writes
///     (gated), so the IMU never got its ucode. The real blocker is GFX-clock
///     ungating, NOT the autoload buffer.
///   * `ALL-ONES` — readback is all 0xffffffff: the register aperture is dead /
///     mis-mapped (wrong offset).
///   * `MISMATCH` — partial / wrong values: addressing or auto-increment is off.
pub fn imu_iram_verdict(got: &[u32], expect: &[u32]) -> &'static str {
    if got.is_empty() {
        "EMPTY (no ucode to check)"
    } else if got == expect {
        "LANDED (GFX_IMU writes stick — IMU has its ucode)"
    } else if got.iter().all(|&w| w == 0) {
        "ALL-ZERO (GFX_IMU block gated — writes dropped, IMU never programmed)"
    } else if got.iter().all(|&w| w == 0xFFFF_FFFF) {
        "ALL-ONES (register aperture dead/unmapped — wrong offset)"
    } else {
        "MISMATCH (partial/wrong addressing or auto-increment)"
    }
}

/// Byte address of a GC seg1 register, anchored off the already-resolved CORE_CTRL
/// byte (`(base+0x40b6)<<2`) so the sweep uses the live discovery base without
/// re-resolving: `seg1_base_byte + dword*4`. Pure → host-KAT'able.
pub fn seg1_probe_byte(core_ctrl_byte: u32, dword: u32) -> u32 {
    core_ctrl_byte
        .wrapping_sub(0x40b6 << 2)
        .wrapping_add(dword << 2)
}

/// DIAGNOSTIC (boot 031738 follow-up): the IMU I-RAM window (0x5f90/0x5f91, GC seg1
/// byte 0x3fe40) read 0xffffffff while CORE_CTRL (0x40b6, byte 0x382d8) read real —
/// both inside the mapped 512 KiB BAR5, same BASE_IDX 1, base 0xa000. Sweep KNOWN
/// GC-seg1 registers (IMU + non-IMU) across 0x4000..0x5f91 to localize the
/// 0xffffffff onset. If the non-IMU regs that bracket the IMU block (GRBM_SEC_CNTL
/// 0x5e0d, GCMC_VM_MARC 0x5e48) DECODE but the IMU bootloader/SRAM regs (0x5f8x/
/// 0x5f9x) do NOT, the IMU SRAM block is power-gated (not an aperture cutoff) and the
/// fix is a GFX/IMU power-up before the ucode load; a clean byte cutoff that also
/// kills the non-IMU regs ⇒ an rmmio sub-aperture (indirect access). Reads only.
pub fn probe_seg1_decode_boundary<O: GpuOps>(ops: &mut O, core_ctrl_byte: u32) {
    // (name, dword offset, is this a GFX_IMU register?)
    const PROBES: &[(&str, u32, bool)] = &[
        ("GFX_IMU_C2PMSG_0", 0x4000, true),
        ("GFX_IMU_CORE_CTRL", 0x40b6, true),
        ("GFX_IMU_ACCESS_CTRL0", 0x4040, true),
        ("GFX_IMU_D_RAM_ADDR", 0x40fc, true),
        ("GRBM_SEC_CNTL", 0x5e0d, false),
        ("GCMC_VM_MARC_BASE_LO_0", 0x5e48, false),
        ("GFX_IMU_RLC_BOOTLOADER_HI", 0x5f81, true),
        ("GFX_IMU_I_RAM_ADDR", 0x5f90, true),
        ("GFX_IMU_I_RAM_DATA", 0x5f91, true),
    ];
    for &(name, dw, is_imu) in PROBES {
        let byte = seg1_probe_byte(core_ctrl_byte, dw);
        let v = ops.reg_read(byte);
        let tag = if v == 0xFFFF_FFFF { "DEAD" } else { "ok" };
        let kind = if is_imu { "imu" } else { "non-imu" };
        ops.log(&format!(
            "[amdgpu] seg1-probe {name} ({kind}) dword={dw:#06x} byte={byte:#07x} = {v:#010x} [{tag}]"
        ));
    }
}

/// DIAGNOSTIC (iron localizer, boot 024544 follow-up): stream-verify that the IMU
/// ucode actually reached IMU SRAM by reading it back through the same I-RAM
/// window. Resets the window to 0, reads the first few `I_RAM_DATA` dwords (the
/// window auto-increments per read), compares to the blob's instruction ucode, and
/// logs the verdict, then restores the window to the load's end-state
/// (`fw_version`). Reads only + a window-pointer restore; runs BEFORE the IMU core
/// is released, so it cannot perturb a running IMU. Gated on discovery.
pub fn verify_imu_iram<O: GpuOps>(ops: &mut O, i_ram_addr: u32, i_ram_data: u32, blob: &[u8]) {
    let Some(layout) = crate::imu::parse_imu_ucode_layout(blob) else {
        return;
    };
    let n = layout.iram_dwords().min(4);
    let mut expect = [0u32; 4];
    for (i, slot) in expect.iter_mut().enumerate().take(n) {
        let off = layout.iram_offset + i * 4;
        *slot = u32::from_le_bytes([blob[off], blob[off + 1], blob[off + 2], blob[off + 3]]);
    }
    // Read back through the window: reset to 0, then read DATA n times.
    ops.reg_write(i_ram_addr, 0);
    let mut got = [0u32; 4];
    for slot in got.iter_mut().take(n) {
        *slot = ops.reg_read(i_ram_data);
    }
    // Restore the window to the load's end-state so the IMU sees the same setup.
    ops.reg_write(i_ram_addr, layout.fw_version);
    let verdict = imu_iram_verdict(&got[..n], &expect[..n]);
    ops.log(&format!(
        "[amdgpu] stage 6 IMU I-RAM readback: expect={:x?} got={:x?} -> {verdict}",
        &expect[..n],
        &got[..n]
    ));
}

/// DIRECT-load step (imu_v11_0_setup): enable IMU debug access + disable the
/// Rtavfs/SmsRepair/DfllBTC/ClkB features, so the IMU runs the loaded ucode. Three
/// register writes; runs between load_microcode and start_imu. Gated on discovery.
pub fn setup_imu<O: GpuOps>(ops: &mut O, gg: &GfxRegs) {
    ops.reg_write(gg.imu_access_ctrl0, 0x00ff_ffff);
    ops.reg_write(gg.imu_access_ctrl1, 0x0000_ffff);
    let v = ops.reg_read(gg.imu_scratch_10);
    ops.reg_write(gg.imu_scratch_10, v | 0x0001_0007);
    ops.log("[amdgpu] stage 6 setup_imu done (IMU debug access + clock features configured)");
}

/// `imu_v11_0_load_microcode` — stream the IMU instruction (I-RAM) + data (D-RAM) ucode
/// from the `gc_*_imu.bin` blob into the IMU's on-die SRAM via the I_RAM/D_RAM ADDR/DATA
/// register windows. THE missing first-light step: on this Phoenix APU the PSP loads the
/// PMFW but NOT the GFX/IMU firmware, so without streaming the IMU ucode ourselves the IMU
/// core boots with empty I-RAM, never executes (CORE_STATUS=0 / CBUSY=false), GFX never
/// leaves reset, and first light fails — flaky only because residual SRAM occasionally
/// survived (Athena 2026-06-29: GFX momentarily hit reset_ctrl=0x1f then gated back).
/// amdgpu runs this as RLC-backdoor autoload sequence 3 (`gfx_v11_0_rlc_backdoor_autoload_enable`),
/// BEFORE [`setup_imu`] + [`try_imu_core_start`]. Each region: ADDR=0, stream every dword to
/// DATA, then ADDR=fw_version (the post-load address the IMU expects). Returns false if the
/// blob header doesn't parse (the daemon then logs + the IMU stays unloaded, same as before).
pub fn imu_load_microcode<O: GpuOps>(ops: &mut O, gg: &GfxRegs, imu: &[u8]) -> bool {
    let Some(l) = crate::imu::parse_imu_ucode_layout(imu) else {
        return false;
    };
    // I-RAM (instruction) ucode: ADDR=0, push each dword, ADDR=fw_version.
    ops.reg_write(gg.gfx_imu_i_ram_addr, 0);
    for w in imu[l.iram_offset..l.iram_offset + l.iram_size].chunks_exact(4) {
        ops.reg_write(
            gg.gfx_imu_i_ram_data,
            u32::from_le_bytes([w[0], w[1], w[2], w[3]]),
        );
    }
    ops.reg_write(gg.gfx_imu_i_ram_addr, l.fw_version);
    // D-RAM (data) ucode: ADDR=0, push each dword, ADDR=fw_version.
    ops.reg_write(gg.gfx_imu_d_ram_addr, 0);
    for w in imu[l.dram_offset..l.dram_offset + l.dram_size].chunks_exact(4) {
        ops.reg_write(
            gg.gfx_imu_d_ram_data,
            u32::from_le_bytes([w[0], w[1], w[2], w[3]]),
        );
    }
    ops.reg_write(gg.gfx_imu_d_ram_addr, l.fw_version);
    // Diagnostic: read back the first 2 I-RAM dwords (ADDR=0, DATA reads auto-increment) to
    // confirm the stream actually LANDED in the IMU SRAM vs the writes being silently dropped
    // by a gated GFX domain. Restore ADDR=fw_version after. 0/garbage vs the firmware = dropped
    // = the IMU has no program even though we "streamed" it.
    ops.reg_write(gg.gfx_imu_i_ram_addr, 0);
    let rb0 = ops.reg_read(gg.gfx_imu_i_ram_data);
    let rb1 = ops.reg_read(gg.gfx_imu_i_ram_data);
    ops.reg_write(gg.gfx_imu_i_ram_addr, l.fw_version);
    let exp = &imu[l.iram_offset..l.iram_offset + 4];
    let exp0 = u32::from_le_bytes([exp[0], exp[1], exp[2], exp[3]]);
    ops.log(&format!(
        "[amdgpu] IMU I-RAM readback: [0]={rb0:#010x} [1]={rb1:#010x} (expect[0]={exp0:#010x}; 0/mismatch = writes DROPPED)"
    ));
    true
}

/// DIRECT-load: build the RLC autoload buffer from the firmware blobs, DMA it into
/// a GPU-readable BO, and point the IMU's RLC bootloader at the RLC_G ucode inside
/// it (rlc_backdoor_autoload_enable). Without this the IMU has its own ucode but
/// nothing to autoload into RLC/CP, so GFX stays in reset (boot 002046). Returns
/// false (logged) on a missing blob, a TOC parse failure, or a BO-alloc failure.
/// Open the gfxhub AGP + system aperture so the GPU's memory controller can read a
/// system-memory buffer directly (no GPUVM) — `gfxhub_v3_0_init_system_aperture_regs`.
/// Iron boot 022858 proved the IMU couldn't reach the autoload buffer because the
/// system aperture was [0,0] (init_gart sized it for the tiny GART VA, not the
/// buffer's ~15 GiB bus address). `AGP_BASE=0` makes the MC address == the system
/// physical address (pass-through), and AGP_TOP/SYS_APERTURE_HIGH are widened to
/// cover `[0, cover_end]`. Gated on discovery (resolved offsets).
pub fn configure_gmc_aperture<O: GpuOps>(
    ops: &mut O,
    r: &crate::gart::GfxhubGartRegs,
    cover_end: u64,
) {
    ops.reg_write(r.agp_base, 0);
    ops.reg_write(r.agp_bot, 0);
    ops.reg_write(r.agp_top, (cover_end >> 24) as u32);
    ops.reg_write(r.sys_aperture_low, 0);
    ops.reg_write(r.sys_aperture_high, (cover_end >> 18) as u32);
    ops.log(&format!(
        "[amdgpu] stage 6 GMC aperture opened: AGP/SYS cover [0..{cover_end:#x}] so the IMU can read the autoload buffer"
    ));
}

pub fn load_rlc_autoload_buffer<O: GpuOps>(ops: &mut O, dev: &Device, gg: &GfxRegs) -> bool {
    let toc = ops.request_firmware_bytes("amdgpu/psp_13_0_4_toc.bin");
    let rlc = ops.request_firmware_bytes("amdgpu/gc_11_0_1_rlc.bin");
    let sdma = ops.request_firmware_bytes("amdgpu/sdma_6_0_1.bin");
    let pfp = ops.request_firmware_bytes("amdgpu/gc_11_0_1_pfp.bin");
    let me = ops.request_firmware_bytes("amdgpu/gc_11_0_1_me.bin");
    let mec = ops.request_firmware_bytes("amdgpu/gc_11_0_1_mec.bin");
    let mes2 = ops.request_firmware_bytes("amdgpu/gc_11_0_1_mes_2.bin");
    let mes1 = ops.request_firmware_bytes("amdgpu/gc_11_0_1_mes1.bin");
    let (Some(toc), Some(rlc), Some(sdma), Some(pfp), Some(me), Some(mec), Some(mes2), Some(mes1)) =
        (toc, rlc, sdma, pfp, me, mec, mes2, mes1)
    else {
        ops.log("[amdgpu] stage 6 autoload buffer SKIPPED — a firmware blob is unavailable");
        return false;
    };
    let blobs = crate::rlc_autoload::AutoloadBlobs {
        toc: &toc,
        rlc: &rlc,
        sdma: &sdma,
        pfp: &pfp,
        me: &me,
        mec: &mec,
        mes_p0: &mes2,
        mes_p1: &mes1,
    };
    ops.log("[amdgpu] stage 6 autoload: all 8 fw blobs present, building buffer...");
    let Some(buffer) = crate::rlc_autoload::build_autoload_buffer(&blobs) else {
        ops.log("[amdgpu] stage 6 autoload buffer build FAILED (TOC parse)");
        return false;
    };
    // GRANULAR LOGGING (the autoload-buffer build had ONE log, at the very end, and HANGS on
    // iron somewhere before it). These bracket the suspected steps so the next boot pinpoints
    // the hang: if "allocating BO" appears but "BO @" doesn't => dma_alloc (6.4 MiB coherent)
    // hangs; if "BO @" but not "DMA write done" => dma_write_bytes hangs.
    ops.log(&format!(
        "[amdgpu] stage 6 autoload: buffer built ({} B), allocating BO...",
        buffer.len()
    ));
    let Some(bo) = ops.dma_alloc(dev.handle, buffer.len()) else {
        ops.log("[amdgpu] stage 6 autoload BO alloc FAILED (needs ~6.4 MiB contiguous)");
        return false;
    };
    ops.log(&format!(
        "[amdgpu] stage 6 autoload: BO @ {:#x} ({} B), DMA-writing...",
        bo.dma_addr, bo.size
    ));
    ops.dma_write_bytes(&bo, 0, &buffer);
    ops.log("[amdgpu] stage 6 autoload: DMA write done, opening GMC aperture");
    // Open the GMC aperture so the GPU MC can read this system-memory buffer at its
    // bus address before we point the IMU bootloader at it (boot 022858 blocker).
    if let Some(gart_regs) = ops.gfxhub_gart_regs() {
        configure_gmc_aperture(ops, &gart_regs, bo.dma_addr.wrapping_add(bo.size as u64));
    }
    let Some(entries) = crate::rlc_autoload::parse_psp_toc(&toc) else {
        return false;
    };
    let Some(rlc_g) =
        crate::rlc_autoload::entry_for(&entries, crate::rlc_autoload::FW_ID_RLC_G_UCODE)
    else {
        return false;
    };
    let addr = bo.dma_addr.wrapping_add(rlc_g.offset as u64);
    ops.reg_write(gg.rlc_bootloader_addr_lo, (addr & 0xFFFF_FFFF) as u32);
    ops.reg_write(gg.rlc_bootloader_addr_hi, (addr >> 32) as u32);
    ops.reg_write(gg.rlc_bootloader_size, rlc_g.size);
    ops.log(&format!(
        "[amdgpu] stage 6 RLC autoload buffer: {} bytes @ bus {:#x}; RLC_G @ {:#x} size {} -> bootloader regs",
        buffer.len(), bo.dma_addr, addr, rlc_g.size
    ));
    true
}

/// DOWN-branch GFX wake (`imu_v11_0_start` + `imu_v11_0_wait_for_reset_status`): when the
/// GFX-up probe says GFX is in reset, release the IMU core by clearing
/// `GFX_IMU_CORE_CTRL.CRESET` (bit0) so it runs the PSP-loaded ucode and brings GFX out
/// of reset, then poll `GFX_IMU_GFX_RESET_CTRL & 0x1f == 0x1f`. This is the documented
/// IMU start (`imu_v11_0_start`): clear `GFX_IMU_CORE_CTRL.CRESET` to release the IMU
/// core, then — `if AMD_IS_APU` (Phoenix is) — send `EnableGfxImu` to power up the GFX
/// rails (amdgpu sends this here, post-CRESET/pre-wait; PSP-load proved it is REQUIRED,
/// the old "PSP-load never sends it" note was wrong), then poll for GFX out of reset.
/// Paced with `delay_us` so the poll spans real time and yields CPU0. Returns true once
/// GFX is out of reset. Safe: clearing CRESET on an already-running core is a no-op, and
/// the register is the GFX hub, never the DCN display the GOP framebuffer uses.
pub fn try_imu_core_start<O: GpuOps>(
    ops: &mut O,
    core_ctrl_reg: u32,
    reset_ctrl_reg: u32,
    timeout_us: u32,
) -> bool {
    let v = ops.reg_read(core_ctrl_reg);
    ops.reg_write(core_ctrl_reg, v & !crate::regs::GFX_IMU_CORE_CTRL_CRESET);
    // APU GFX POWER-UP — the missing step (amdgpu `imu_v11_0_start`): after clearing
    // CRESET and BEFORE waiting for reset status, `if (adev->flags & AMD_IS_APU)` it calls
    // `amdgpu_dpm_set_gfx_power_up_by_imu`, which (`smu_v13_0_set_gfx_power_up_by_imu`) sends
    // PPSMC_MSG_EnableGfxImu(arg=ENABLE_IMU_ARG_GFXOFF_ENABLE=1). Without it the IMU core is
    // released but GFX is never told to power its rails up, so it sits in reset forever
    // (Athena 2026-06-29: reset_ctrl frozen at 0x10, RESET_DONE never reaching 0x1f = flaky
    // first light). The PMFW posts NO response for this msg, so async is correct (a sync
    // response-poll wedges). We formerly fired it only in the GFX-DOWN recovery branch AFTER
    // the poll already failed — too late; this is amdgpu's exact placement.
    if let (Some(mb), Some(msg)) = (ops.smu_mailbox(), ops.enable_gfx_imu_msg()) {
        smu_send_msg_async(ops, &mb, msg, 1, 1_000_000);
        ENABLE_GFX_IMU_SENT.store(true, AtomicOrdering::Relaxed);
        ops.log(&format!(
            "[amdgpu] try_imu_core_start: EnableGfxImu({msg:#x}) arg=1 sent (APU GFX power-up, post-CRESET/pre-wait)"
        ));
    }
    let mut waited = 0u32;
    // PERIODIC progress log (every 0.25s LOGICAL): this poll is otherwise SILENT until
    // it returns, so when CPU starvation stretches it across the whole capture window
    // (Athena 2026-06-29: bootlog stalled mid-poll, no return line) we can't tell SLOW
    // (RESET_DONE eventually sets) from FAILED (GFX never leaves reset = first-light
    // dead). Logging the reset_ctrl readback over wall-clock distinguishes them.
    let mut next_log = 0u32;
    // GFX_IMU_CORE_STATUS (CORE_CTRL+4 bytes): a working IMU reads 0x3 (CBUSY+PWAIT). Peak
    // 0x0 over the whole poll => the IMU NEVER executed (any reset-clear came from the PMFW's
    // EnableGfxImu, not the IMU running our streamed firmware). Peak 0x3 => IMU ran.
    let core_status_reg = core_ctrl_reg.wrapping_add(4);
    let mut peak_core_status = 0u32;
    loop {
        let rst = ops.reg_read(reset_ctrl_reg);
        let cs = ops.reg_read(core_status_reg);
        if cs > peak_core_status {
            peak_core_status = cs;
        }
        if rst & crate::regs::GFX_IMU_GFX_RESET_DONE_MASK
            == crate::regs::GFX_IMU_GFX_RESET_DONE_MASK
        {
            ops.log(&format!(
                "[amdgpu] try_imu_core_start: GFX OUT OF RESET (reset_ctrl={rst:#010x}, {waited}us, peak IMU CORE_STATUS={peak_core_status:#x} [0x3=IMU ran])"
            ));
            return true;
        }
        if waited >= next_log {
            ops.log(&format!(
                "[amdgpu] try_imu_core_start polling: reset_ctrl={rst:#010x} CORE_STATUS={cs:#x} {waited}/{timeout_us}us"
            ));
            next_log = next_log.saturating_add(250_000);
        }
        if waited >= timeout_us {
            ops.log(&format!(
                "[amdgpu] try_imu_core_start TIMEOUT: GFX never left reset (reset_ctrl={rst:#010x}, peak CORE_STATUS={peak_core_status:#x} — 0x0=IMU NEVER executed)"
            ));
            return false;
        }
        ops.delay_us(SMU_POLL_STEP_US);
        waited = waited.saturating_add(SMU_POLL_STEP_US);
    }
}

/// Stage 6 — `gfx_v*_sw_init`: wait for the GFX pipe to go idle, allocate the
/// GFX + SDMA command rings, build a real PM4 stream (NOP + WRITE_DATA fence +
/// RELEASE_MEM) into the GFX ring, and program the CP ring registers (gc11
/// `CP_RB0_*`). Verifies the programmed base/wptr read back — the exact ordering
/// a real CP needs.
/// STEP-0 DIAGNOSTIC (post-first-light refactor): read `GFX_IMU_GFX_RESET_CTRL`
/// (`& 0x1f == 0x1f` ⇒ GFX out of reset) + `RLC_BOOTLOAD_STATUS` at a labeled
/// point, to pinpoint EXACTLY which step drops GFX `0x1f -> 0x10` during the ring
/// setup (the drop happens before the existing stage-6 GFX-up probe; we don't know
/// which op). Re-acquires `gfx_regs` each call (callers sit outside the
/// `if let Some(gg)` scopes). Read-only; logged so the netlog shows the up→down
/// transition step by step.
fn probe_gfx_alive<O: GpuOps>(ops: &mut O, label: &str) {
    if let Some(gg) = ops.gfx_regs() {
        let rst = ops.reg_read(gg.gfx_imu_reset_ctrl);
        let boot = ops.reg_read(gg.rlc_bootload_status);
        ops.log(&format!(
            "[amdgpu] GFX-ALIVE @ {label}: GFX_IMU_GFX_RESET_CTRL={rst:#010x} (up={}) RLC_BOOTLOAD_STATUS={boot:#010x}",
            rst & 0x1f == 0x1f
        ));
    }
}

pub fn init_rings<O: GpuOps>(ops: &mut O, dev: &Device) -> bool {
    const RING_BYTES: usize = 64 * 1024;
    // Real gfx_v11 ordering: never touch the CP while the pipe is busy.
    if !wait_for_gfx_idle(ops, 1000) {
        ops.log(
            "[amdgpu] stage 6 GFX still busy (GRBM_STATUS.GUI_ACTIVE stuck) — aborting ring init",
        );
        return false;
    }
    ops.log("[amdgpu] stage 6 GFX pipe idle (GRBM_STATUS.GUI_ACTIVE clear)");
    let Some(gfx) = ops.dma_alloc(dev.handle, RING_BYTES) else {
        ops.log("[amdgpu] stage 6 GFX ring alloc FAILED");
        return false;
    };
    let Some(sdma_ring) = ops.dma_alloc(dev.handle, RING_BYTES) else {
        ops.log("[amdgpu] stage 6 SDMA ring alloc FAILED");
        return false;
    };

    let imu = ops.request_firmware("amdgpu/gc_11_0_1_imu.bin");
    let me = ops.request_firmware("amdgpu/gc_11_0_1_me.bin");
    let pfp = ops.request_firmware("amdgpu/gc_11_0_1_pfp.bin");
    let mec = ops.request_firmware("amdgpu/gc_11_0_1_mec.bin");
    let rlc = ops.request_firmware("amdgpu/gc_11_0_1_rlc.bin");
    let sdma_fw = ops.request_firmware("amdgpu/sdma_6_0_1.bin");
    if imu && me && pfp && mec && rlc && sdma_fw {
        ops.log("[amdgpu] stage 6 GFX/SDMA microcode loaded (incl. IMU)");
    } else {
        ops.log("[amdgpu] stage 6 GFX/SDMA microcode missing (add to firmware/amdgpu/)");
    }

    // LINUX gfx_v11_0_hw_init ORDER: power up GFX *before* any MES/CP setup. The MES is a
    // GFX-engine component; touching it while GFX is gated wedges the bus intermittently —
    // cold boots 2026-06-28 kept stalling at the MES load, BEFORE the GFX power-up that used
    // to run later (after the MES). So bring GFX up FIRST, exactly like Linux:
    // program_rlc_ram (mark the RLC RAM valid; Phoenix=11_0_1 has no golden table, just
    // GFX_IMU_RLC_RAM_INDEX=0x2 | RAM_VALID) → setup_imu → start_imu (release CRESET) so the
    // IMU runs the autoload → wait_for_rlc_autoload_complete (poll BOOTLOAD_COMPLETE). IMU
    // regs stay alive when GFX is gated, so this can't wedge. (GFX_IMU_RLC_RAM_INDEX dword
    // 0x40ac = GFX_IMU_CORE_CTRL 0x40b6 - 0xA dwords = -0x28 bytes.)
    if let Some(gg) = ops.gfx_regs() {
        // FIRST-LIGHT GATE (2026-06-29): on this Phoenix PSP-load APU the PSP autoload
        // already powers GFX up (RLC_BOOTLOAD_STATUS bit 31 + GFX_IMU_GFX_RESET_CTRL & 0x1f
        // == 0x1f — verified on iron: 0xc000001f / 0x1f). amdgpu runs NO driver-side IMU
        // bring-up on the PSP path: program_rlc_ram / imu_load_microcode / setup_imu /
        // start_imu are RLC_BACKDOOR_AUTO + DIRECT only (gfx_v11_0_hw_init). Re-poking the
        // IMU on a now-LIVE GFX wedges CPU 0 on a stuck MMIO access — the first-light boot
        // hung EXACTLY here, at imu_load_microcode. So when first light already holds, SKIP
        // the whole backdoor IMU block and go straight to the CP/MES ring setup below.
        let fl_boot = ops.reg_read(gg.rlc_bootload_status);
        let fl_rst = ops.reg_read(gg.gfx_imu_reset_ctrl);
        let first_light =
            (fl_boot & crate::regs::RLC_BOOTLOAD_COMPLETE_MASK != 0) && (fl_rst & 0x1f == 0x1f);
        if first_light {
            // SMU QUIESCE (2026-07-02) — the working driver's COMPLETE cold-init SMU
            // stream (cold_init_named-20260624.txt) sends NO DisallowGfxOff (0x1A), NO
            // SetHardMinGfxClk (0x1C), NO SetSoftMinGfxclk (0x09) — ever. GFXOFF is
            // boot-DISALLOWED on Phoenix (its only GFXOFF message is AllowGfxOff 0x19,
            // ~150 ms AFTER the MES acks set_hw_resources), and it never touches the
            // gfxclk DPM during init. Our extra messages were worse than redundant: on
            // EVERY halting boot SetHardMinGfxClk never acked (500 ms timeout), so we
            // entered the MES window with an IN-FLIGHT PMFW command that follow-up sends
            // then stomped — undefined PMFW state exactly when pipe0's set_hw_resources
            // handler needs PMFW-managed clocks (the RE-proven 0x7654 mid-fetch
            // clock-stop). Match the oracle: send NOTHING the working driver doesn't.
            let pg_before = ops.reg_read(gg.rlc_pg_cntl);
            ops.reg_write(gg.rlc_pg_cntl, 0);
            let pg_after = ops.reg_read(gg.rlc_pg_cntl);
            ops.log(&format!(
                "[amdgpu] stage 6 pre-MES (post-first-light): RLC_PG_CNTL {pg_before:#010x} -> {pg_after:#010x} (SMU QUIESCED — no DisallowGfxOff/no gfxclk floor, = working driver's silent pre-MES mailbox)"
            ));
            // EnableGfxImu(arg=1) — the ONE PMFW message the working driver sends on
            // every cold init on this exact hardware (trace t=43.6479, its 3rd message,
            // before any GFX work) that the first-light path used to SKIP. PSP autoload
            // covers the FIRMWARE, but this message is `smu_v13_0_set_gfx_power_up_by_imu`
            // — it tells the PMFW the driver owns GFX and establishes the power/clock
            // state the MES scheduler pipe runs under; a register diff can never see it.
            // ONCE PER INIT: on a cold-GFX boot the PSP-autoload block already sent it
            // (the oracle sends it exactly once; iron boot #2 2026-07-02 hard-reset
            // right where a second send would have landed on the just-powered domain —
            // see ENABLE_GFX_IMU_SENT). Only the warm/GOP-lit path, where no earlier
            // block sent it, sends it here. ASYNC (never poll the response across a
            // possible power transition), ~30 ms settle (the working trace gap is
            // 29.4 ms), then re-probe the domain: if a live-GFX send were to drop first
            // light, the probe pins the blame here.
            if ENABLE_GFX_IMU_SENT.load(AtomicOrdering::Relaxed) {
                ops.log("[amdgpu] stage 6 EnableGfxImu SKIPPED — already sent this init (PSP-autoload/cold path); oracle sends it once");
            } else if let (Some(mb), Some(msg)) = (ops.smu_mailbox(), ops.enable_gfx_imu_msg()) {
                smu_send_msg_async(ops, &mb, msg, 1, 200_000);
                ENABLE_GFX_IMU_SENT.store(true, AtomicOrdering::Relaxed);
                for _ in 0..30 {
                    ops.delay_us(1000);
                }
                let rst_after = ops.reg_read(gg.gfx_imu_reset_ctrl);
                let core_status = ops.reg_read(gg.gfx_imu_core_ctrl.wrapping_add(4));
                ops.log(&format!(
                    "[amdgpu] stage 6 EnableGfxImu({msg:#x}) arg=1 on first-light GFX (oracle: working driver ALWAYS sends this pre-MES): RESET_CTRL={rst_after:#010x} (&0x1f==0x1f => still up) IMU CORE_STATUS={core_status:#010x}"
                ));
            }
            ops.log(&format!(
                "[amdgpu] stage 6 backdoor IMU bring-up SKIPPED — PSP autoload already at first light (RLC_BOOTLOAD_STATUS={fl_boot:#010x}, GFX_IMU_GFX_RESET_CTRL={fl_rst:#010x}); amdgpu PSP path runs no driver IMU steps"
            ));
        } else {
            let rlc_ram_index = gg.gfx_imu_core_ctrl.wrapping_sub(0x28);
            ops.reg_write(rlc_ram_index, 0x2);
            let v = ops.reg_read(rlc_ram_index);
            ops.reg_write(rlc_ram_index, v | 0x8000_0000); // RAM_VALID (bit 31)
            let rb = ops.reg_read(rlc_ram_index);
            ops.log(&format!(
            "[amdgpu] stage 6 program_rlc_ram (BEFORE MES): GFX_IMU_RLC_RAM_INDEX <- 0x2|RAM_VALID (readback {rb:#010x})"
        ));
            // rlc_backdoor_autoload_enable (Linux order: AFTER program_rlc_ram, BEFORE start_imu) —
            // THE missing piece. Build the RLC autoload buffer, open the GMC aperture, and point
            // the IMU bootloader regs at the RLC_G ucode inside it. Without a VALID buffer to read,
            // releasing the IMU (start_imu below) had nothing to run and WEDGED the bus. This used
            // to run only in the down-branch fallback — AFTER this IMU release, far too late.
            let autoload_ok = load_rlc_autoload_buffer(ops, dev, &gg);
            ops.log(&format!(
            "[amdgpu] stage 6 rlc_backdoor_autoload_enable (BEFORE start_imu): autoload buffer + bootloader regs -> {autoload_ok}"
        ));
            // RLC-backdoor autoload sequence 3 (amdgpu gfx_v11_0_rlc_backdoor_autoload_enable):
            // DIRECT-stream the IMU ucode into I-RAM/D-RAM. THE missing first-light step — the PSP
            // leaves the IMU empty on this Phoenix APU, so without this the IMU core never executes
            // (CORE_STATUS=0) and GFX won't stay out of reset. Runs BEFORE setup_imu + start_imu.
            match ops.imu_fw_blob() {
            Some(imu) => {
                let n = imu.len();
                let loaded = imu_load_microcode(ops, &gg, &imu);
                ops.log(&format!(
                    "[amdgpu] stage 6 IMU load_microcode: streamed {n}B imu.bin -> I-RAM/D-RAM = {loaded} (empty IMU = GFX stuck in reset)"
                ));
            }
            None => ops
                .log("[amdgpu] stage 6 IMU load_microcode SKIPPED — no imu.bin blob (QEMU/pre-discovery)"),
        }
            setup_imu(ops, &gg);
            let woke =
                try_imu_core_start(ops, gg.gfx_imu_core_ctrl, gg.gfx_imu_reset_ctrl, 2_000_000);
            let mut w = 0u32;
            loop {
                let bl = ops.reg_read(gg.rlc_bootload_status);
                if bl & crate::regs::RLC_BOOTLOAD_COMPLETE_MASK != 0 || w >= 2_000_000 {
                    break;
                }
                ops.delay_us(20_000);
                w = w.saturating_add(20_000);
            }
            ops.log(&format!(
            "[amdgpu] stage 6 GFX power-up (BEFORE MES): start_imu out-of-reset={woke}, waited {}ms for RLC BOOTLOAD_COMPLETE",
            w / 1000
        ));
        } // end else (backdoor IMU bring-up — skipped when first light already holds)
    }
    probe_gfx_alive(ops, "A:post-gate/hold-gfx-awake (expect up=true)");

    // CP completion fence target: a DRIVER-OWNED buffer, NOT an arbitrary VRAM
    // offset (the RELEASE_MEM below writes into it, and a stray write to low VRAM
    // could clobber the GOP framebuffer / firmware on iron — same footgun the SDMA
    // fill fixed). Allocated AFTER the rings so the gfx/sdma ring DMA addresses
    // are unchanged.
    let Some(cp_fence_buf) = ops.dma_alloc(dev.handle, 4096) else {
        ops.log("[amdgpu] stage 6 CP fence-buf alloc FAILED");
        return false;
    };
    // SDMA scratch + fence buffers — allocated HERE (before the GART aperture is
    // computed) so every engine-visible buffer is known when we derive `delta`. The
    // SDMA engine fills DRIVER-OWNED scratch (never an arbitrary VRAM offset: a stray
    // fill could clobber the GOP framebuffer / firmware), fenced into a buffer it
    // posts completion into.
    let Some(sdma_scratch) = ops.dma_alloc(dev.handle, 4096) else {
        ops.log("[amdgpu] stage 6 SDMA scratch alloc FAILED");
        return false;
    };
    let Some(sdma_fence_buf) = ops.dma_alloc(dev.handle, 4096) else {
        ops.log("[amdgpu] stage 6 SDMA fence-buf alloc FAILED");
        return false;
    };

    // RLC Clear-State Buffer (init_csb). amdgpu programs RLC_CSIB_ADDR/LENGTH right
    // before the MES enable; the MES stalls one instruction into boot without it. The
    // buffer joins the GART span so the RLC reaches it through VMID0. 4 KiB holds the
    // 0x3c0-dword (3840 B) CSB. Content is zeroed for the first test (the hypothesis is
    // the MES polls for the CSB being SET UP); if that doesn't unblock it, the real
    // clear-state content goes here next.
    let csb_buf: Option<DmaBuf> = ops.dma_alloc(dev.handle, 4096);

    // MES scheduler (pipe 0) ucode + data — DIRECT-loaded into GART-mapped buffers so
    // the MES has code at an addressable GPU VA (rung 1 completion: the IC base). The
    // PSP autoloads the MES ucode into a PSP-managed location we can't address, so we
    // copy our own and point CP_MES_IC_BASE at it. Optional: only when the daemon
    // supplies the blobs + the entry point. Allocated here so the buffers fall inside
    // the GART span below (the MES reads them THROUGH VMID0, like the gfx ring).
    let mes_load: Option<(DmaBuf, DmaBuf, u64)> = match (ops.mes_ucode_blobs(), ops.mes_uc_starts())
    {
        (Some((ucode, data)), Some((p0, _))) => {
            match (
                ops.dma_alloc(dev.handle, ucode.len().max(4096)),
                ops.dma_alloc(dev.handle, data.len().max(4096)),
            ) {
                (Some(ub), Some(db)) => {
                    dma_write_bytes(ops, &ub, &ucode);
                    dma_write_bytes(ops, &db, &data);
                    ops.log(&format!(
                            "[amdgpu] stage 6 MES ucode direct-loaded: ucode={} B @ {:#x}, data={} B @ {:#x}",
                            ucode.len(), ub.dma_addr, data.len(), db.dma_addr
                        ));
                    Some((ub, db, p0))
                }
                _ => {
                    ops.log("[amdgpu] stage 6 MES ucode buffer alloc FAILED — MES load skipped");
                    None
                }
            }
        }
        _ => None,
    };

    // MES KIQ (pipe 1) ucode + data — the KIQ microengine that maps the SCHED ring.
    // mes_v11_0_kiq_hw_init loads BOTH pipes before enable; without pipe 1's code the
    // KIQ never services its MAP_QUEUES. Direct-loaded like pipe 0; gated on the daemon
    // supplying mes1.bin's blobs + p1 entry (None on QEMU / if mes1.bin is absent).
    let mes_kiq_load: Option<(DmaBuf, DmaBuf, u64)> = match (
        ops.mes_kiq_ucode_blobs(),
        ops.mes_uc_starts(),
    ) {
        (Some((ucode, data)), Some((_, Some(p1)))) => {
            match (
                ops.dma_alloc(dev.handle, ucode.len().max(4096)),
                ops.dma_alloc(dev.handle, data.len().max(4096)),
            ) {
                (Some(ub), Some(db)) => {
                    dma_write_bytes(ops, &ub, &ucode);
                    dma_write_bytes(ops, &db, &data);
                    ops.log(&format!(
                            "[amdgpu] stage 6 MES KIQ ucode direct-loaded: ucode={} B @ {:#x}, data={} B @ {:#x}",
                            ucode.len(), ub.dma_addr, data.len(), db.dma_addr
                        ));
                    Some((ub, db, p1))
                }
                _ => {
                    ops.log(
                        "[amdgpu] stage 6 MES KIQ ucode buffer alloc FAILED — KIQ load skipped",
                    );
                    None
                }
            }
        }
        _ => None,
    };
    probe_gfx_alive(ops, "B:post-MES+KIQ-ucode-dma-write");

    // MES command-ring queue buffers (rungs 2-4: SCHEDULE the gfx queue via the alive
    // MES). The MES command ring carries set_hw_resources + map_legacy_queue; the gfx
    // MQD is the descriptor map_legacy_queue points at. Allocated here so they join the
    // GART span (the MES + CP reach them through VMID0). Gated on the MES ucode load +
    // the HQD/ip-base resolution (None on QEMU / pre-discovery).
    // The SCHED ring (pipe0) carries set_hw_resources + map_legacy_queue; the KIQ ring
    // (pipe1) carries the MAP_QUEUES that activates the SCHED ring (the SCHED ring is
    // NEVER direct-register-written — only the KIQ is). Each MES pipe needs its own
    // MQD + EOP. The gfx MQD is the descriptor map_legacy_queue points at.
    struct MesQueue {
        cmd_ring: DmaBuf, // SCHED ring buffer (pipe0)
        eop: DmaBuf,      // SCHED EOP
        mqd: DmaBuf,      // SCHED MQD
        sch_ctx: DmaBuf,
        fence: DmaBuf,
        gfx_mqd: DmaBuf,
        kiq_ring: DmaBuf, // KIQ ring buffer (pipe1)
        kiq_eop: DmaBuf,  // KIQ EOP
        kiq_mqd: DmaBuf,  // KIQ MQD
    }
    let mes_queue: Option<MesQueue> =
        if mes_load.is_some() && ops.mes_hqd_regs().is_some() && ops.mes_ip_bases().is_some() {
            match (
                ops.dma_alloc(dev.handle, 64 * 1024),
                ops.dma_alloc(dev.handle, 4096),
                ops.dma_alloc(dev.handle, 4096),
                ops.dma_alloc(dev.handle, 4096),
                ops.dma_alloc(dev.handle, 4096),
                ops.dma_alloc(dev.handle, 4096),
                ops.dma_alloc(dev.handle, 4096),
                ops.dma_alloc(dev.handle, 4096),
                ops.dma_alloc(dev.handle, 4096),
            ) {
                (
                    Some(cmd_ring),
                    Some(eop),
                    Some(mqd),
                    Some(sch_ctx),
                    Some(fence),
                    Some(gfx_mqd),
                    Some(kiq_ring),
                    Some(kiq_eop),
                    Some(kiq_mqd),
                ) => Some(MesQueue {
                    cmd_ring,
                    eop,
                    mqd,
                    sch_ctx,
                    fence,
                    gfx_mqd,
                    kiq_ring,
                    kiq_eop,
                    kiq_mqd,
                }),
                _ => {
                    ops.log("[amdgpu] stage 6 MES queue buffer alloc FAILED — queue map skipped");
                    None
                }
            }
        } else {
            None
        };

    // ── GFX GART aperture mapper. Every engine-visible buffer is reached at a HIGH
    // GPU VA (GFX_GART_APERTURE_BASE + offset) that the gfxhub VMID0 page table
    // translates — NOT its physical address. delta = APERTURE_BASE - lo; the GART
    // (init_gart_identity, after first light) maps phys [lo,hi) onto that aperture.
    // THE CP-fetch fix: amdgpu's gfx ring reads CP_RB0_BASE_HI=0x7f; ours read 0 and
    // the PFP never fetched. `gart_va(phys)` is the GPU address baked into every
    // ring base / writeback / fence; the CPU keeps using the DmaBuf (phys) for dma_*.
    // The MES ucode/data buffers join the span so the MES reaches them through VMID0.
    let mut span: Vec<(u64, u64)> = alloc::vec![
        (gfx.dma_addr, gfx.dma_addr + RING_BYTES as u64),
        (sdma_ring.dma_addr, sdma_ring.dma_addr + RING_BYTES as u64),
        (cp_fence_buf.dma_addr, cp_fence_buf.dma_addr + 4096),
        (sdma_scratch.dma_addr, sdma_scratch.dma_addr + 4096),
        (sdma_fence_buf.dma_addr, sdma_fence_buf.dma_addr + 4096),
    ];
    if let Some(c) = &csb_buf {
        span.push((c.dma_addr, c.dma_addr + c.size as u64));
    }
    if let Some((ub, db, _)) = &mes_load {
        span.push((ub.dma_addr, ub.dma_addr + ub.size as u64));
        span.push((db.dma_addr, db.dma_addr + db.size as u64));
    }
    if let Some((ub, db, _)) = &mes_kiq_load {
        span.push((ub.dma_addr, ub.dma_addr + ub.size as u64));
        span.push((db.dma_addr, db.dma_addr + db.size as u64));
    }
    if let Some(q) = &mes_queue {
        for b in [
            &q.cmd_ring,
            &q.eop,
            &q.mqd,
            &q.sch_ctx,
            &q.fence,
            &q.gfx_mqd,
            &q.kiq_ring,
            &q.kiq_eop,
            &q.kiq_mqd,
        ] {
            span.push((b.dma_addr, b.dma_addr + b.size as u64));
        }
    }
    let gart_lo =
        span.iter().map(|&(s, _)| s).min().unwrap_or(gfx.dma_addr) & !(gart::GPU_PAGE_SIZE - 1);
    let gart_hi = span
        .iter()
        .map(|&(_, e)| e)
        .max()
        .unwrap_or(gfx.dma_addr + RING_BYTES as u64);
    // delta turns a buffer's phys into its GART VA (= phys + delta). Both the closure
    // (for addresses computed here) and the raw delta (threaded into the SDMA helper,
    // which derives its WPTR-poll address from a DmaBuf's phys) are needed.
    let gart_delta = GFX_GART_APERTURE_BASE.wrapping_sub(gart_lo);
    let gart_va = |phys: u64| phys.wrapping_add(gart_delta);

    // Build the initial PM4 command stream into the GFX ring. The fence target is a
    // GART VA (the CP fetches + writes through VMID0).
    let fence_gpu_addr = gart_va(cp_fence_buf.dma_addr);
    let mut stream: Vec<u32> = Vec::new();
    stream.extend_from_slice(&pm4::nop(1));
    stream.extend_from_slice(&pm4::write_data_mem(fence_gpu_addr, &[0xCAFE_F00D]));
    stream.extend_from_slice(&pm4::release_mem(0x14, fence_gpu_addr, 1));
    ops.dma_write(&gfx, 0, &stream);

    // Build + (when discovery is up) RUN a real SDMA job (amdgpu's VRAM-clear/scanout
    // path). Scratch + fence are addressed by GART VA, matching the CP.
    let sdma_stream = sdma::constant_fill_with_fence(
        gart_va(sdma_scratch.dma_addr),
        0xA5A5_A5A5,
        4096,
        gart_va(sdma_fence_buf.dma_addr),
        1,
    );
    ops.dma_write(&sdma_ring, 0, &sdma_stream);
    ops.log(&format!(
        "[amdgpu] stage 6 SDMA ring: {} dwords (CONSTANT_FILL 0xA5A5A5A5 + FENCE)",
        sdma_stream.len()
    ));
    probe_gfx_alive(
        ops,
        "C:post-MES-queue-alloc+SDMA-ring (pre-SDMA-reg-program)",
    );
    // Program the SDMA0 QUEUE0 RB registers, advance WPTR to submit, and POLL the
    // fence the engine posts on completion — gated on discovery-resolved offsets
    // (None on QEMU/pre-discovery → build-only, no guessed MMIO). On iron the
    // engine executes the fill into the scratch buffer and posts the fence, so the
    // bootlog proves the SDMA engine ACTUALLY RAN, not just that it was submitted.
    // A timeout is non-fatal here (the GFX ring bring-up does not depend on this
    // SDMA proof) — but on iron it's the signal the engine is wedged.
    // SDMA SUBMIT DEFERRED to after the GFX power-up (below). On this APU the SDMA
    // engine shares the GFX power domain, so submitting HERE (pre-first-light) hit a
    // power-gated engine — boot 224734: "fence not posted" printed BEFORE "FIRST
    // LIGHT". The ring/stream/fence buffers are built above; the actual submit runs
    // once GFX is powered, right after wait_for_gfx_awake.

    // Program the CP ring buffer registers (gc11 CP_RB0_*).
    let Some(log2_size) = gc11::ring_buf_log2(RING_BYTES as u32) else {
        ops.log("[amdgpu] stage 6 ring size not a power of two");
        return false;
    };
    // GFXOFF / clock-gating ungate (gfx11): a gated GFX block silently DROPS CP
    // register writes — the suspected stage-6 readback-mismatch cause on iron.
    // Ask the PMFW to stop re-gating GFX (DisallowGfxOff), then hold the RLC in
    // safe mode so the CP registers are writable. Both are no-ops until their iron
    // offsets / message-ids are confirmed (default `None`), so this never writes
    // guessed MMIO pre-iron and the existing path is unchanged on QEMU.
    //
    // GFXOFF-IDLE GUARD (Step-0 finding 2026-06-29): on a first-light boot GFX is UP
    // here (probe C = 0x1f). The DisallowGfxOff + SMU-metrics steps below return None
    // on iron and burn ~2.5s of mailbox timeouts — a dead window long enough for GFX to
    // re-enter GFXOFF (probe ladder: up@C → DOWN@D with ZERO GFX register writes between
    // = pure idle-gating). amdgpu does NOT do these mid-ring-setup (it sets the metrics
    // table during SMU init). So when GFX is already up, SKIP the window to keep the
    // sequence continuous (GFX can't idle-gate during a wait that doesn't happen).
    let gfx_up_here = if let Some(gg) = ops.gfx_regs() {
        ops.reg_read(gg.gfx_imu_reset_ctrl) & 0x1f == 0x1f
    } else {
        false
    };
    if gfx_up_here {
        ops.log("[amdgpu] stage 6 SMU DisallowGfxOff+metrics window SKIPPED — GFX already UP (first light); avoiding the ~2.5s idle SMU-timeout window that lets GFX re-enter GFXOFF (Step-0: up@C, DOWN@D, no GFX writes between)");
    } else if let (Some(mb), Some(msg)) = (ops.smu_mailbox(), ops.gfx_off_disable_msg()) {
        // ~1 s budget (was 1000 bare reads = microseconds) so the PMFW actually has
        // time to ack — DisallowGfxOff PREVENTS future GFXOFF entry (it does NOT wake
        // an already-gated block — boot 183253 proved the block stayed GATED).
        match smu_send_msg(ops, &mb, msg, 0, 1_000_000) {
            Some(SMU_RESP_OK) => ops.log("[amdgpu] stage 6 SMU DisallowGfxOff acked"),
            other => ops.log(&format!(
                "[amdgpu] stage 6 SMU DisallowGfxOff resp {other:?}"
            )),
        }
        // SMU METRICS TABLE (smu_v13_0_4) — amdgpu's cold-init trace sets this up BEFORE
        // the MES enable and RaeenOS never did; the MES stalls one instruction into boot
        // (INSTR_PNTR @entry+1), a likely poll of the GFX power/clock state the SMU
        // publishes here. Give the PMFW a DRAM table address (SetDriverDramAddrHigh/Low)
        // then trigger a metrics transfer (TransferTableSmu2Dram). The buffer is plain
        // DMA memory the SMU writes via the IOMMU (guest-physical == IOVA in the VFIO
        // guest). Best-effort: log each response, never blocks the rest of bring-up.
        if let Some(metrics) = ops.dma_alloc(dev.handle, 4096) {
            let addr = metrics.dma_addr;
            let rh = smu_send_msg(
                ops,
                &mb,
                crate::regs::PPSMC_MSG_SET_DRIVER_DRAM_ADDR_HIGH,
                (addr >> 32) as u32,
                500_000,
            );
            let rl = smu_send_msg(
                ops,
                &mb,
                crate::regs::PPSMC_MSG_SET_DRIVER_DRAM_ADDR_LOW,
                (addr & 0xffff_ffff) as u32,
                500_000,
            );
            let rt = smu_send_msg(
                ops,
                &mb,
                crate::regs::PPSMC_MSG_TRANSFER_TABLE_SMU2DRAM,
                0,
                500_000,
            );
            ops.log(&format!(
                "[amdgpu] stage 6 SMU metrics table @ {addr:#x}: SetDramAddrHi->{rh:?} Lo->{rl:?} TransferSmu2Dram->{rt:?} (the pre-MES SMU step RaeenOS skipped)"
            ));
            // DmaBuf is a Copy handle — the backing memory is daemon-managed (not freed
            // on drop), so the SMU keeps a valid table to write into after this scope.
        }
    }
    probe_gfx_alive(
        ops,
        "D:post-SDMA-reg+DisallowGfxOff+SMU-metrics (pre-RLC-safe-mode/CP)",
    );
    // (Removed: the SMU `EnableGfxImu` "wake". Iron boot 185829 returned `resp None`
    // — no PMFW ack — and GFX stayed GATED, so it does NOT wake a PSP-load Phoenix
    // and it risks wedging the mailbox. The real model: on a PSP-load APU the PSP
    // autoloads the GFX firmware (IMU/RLC/CP ucode) and the IMU brings GFX out of
    // reset; the driver does NOT cold-start GFX, and the GOP framebuffer is the
    // DISPLAY block (DCN) lighting up WITHOUT the GFX/compute engine. The GFX-up
    // probe below reads the authoritative status registers instead of guessing
    // power-management messages.)
    let in_safe_mode = ops.rlc_safe_mode().is_some();
    if !rlc_enter_safe_mode(ops, 1000) {
        ops.log("[amdgpu] stage 6 RLC safe-mode enter TIMEOUT (RLC firmware not running yet?)");
    } else if in_safe_mode {
        ops.log("[amdgpu] stage 6 RLC safe mode entered — GFX held writable for CP programming");
    }

    // RS64 CP startup (gfx_v11_0_config_gfx_rs64): set the PFP/ME/MEC program-
    // counter starts (from the fw headers) + reset the pipes so the RS64 cores
    // execute the PSP-loaded ucode. THE step that makes the CP ring registers go
    // live — without it CP_ME_CNTL halt-clear alone leaves the cores stopped and
    // the ring regs dead (readback 0, as seen on boot 150355). Gated on BOTH the
    // resolved offsets AND the parsed start addresses (no guessed PC into the CP).
    if let (Some(rs), Some(starts)) = (ops.rs64_cp_regs(), ops.rs64_ucode_starts()) {
        ops.log(&format!(
            "[amdgpu] stage 6 config_gfx_rs64: RS64 CP start (pfp={:#x} me={:#x} mec={:#x})",
            starts.pfp, starts.me, starts.mec
        ));
        config_gfx_rs64(ops, &rs, &starts);
        ops.log("[amdgpu] stage 6 config_gfx_rs64 done — RS64 PFP/ME/MEC started");
    } else {
        ops.log("[amdgpu] stage 6 config_gfx_rs64 SKIPPED — CP is F32 (v1 ucode) on this ASIC, RS64 N/A; or offsets unresolved (QEMU). Phoenix=F32: PSP loads ucode, CP enable + ring is the path");
    }

    // (MES microengine load + start happens AFTER the GART build below — it needs the
    // GART to map the direct-loaded MES ucode/data buffers. See "RUNG 1 COMPLETION".)

    // CP ring base = the gfx ring's bus address. The GFXHUB VMID0 GART is built
    // LATER (init_gart_identity, after first light) as an IDENTITY map, so a bus
    // address IS its GPU VA — no rewriting needed. Building the GART HERE was the
    // root bug: GFX is still in reset pre-first-light, so the gfxhub register writes
    // were silently dropped (iron boot 032117: "GART built" then every gfxhub VM reg
    // read back 0, and "SDMA fence not posted — next suspect: VM addressing").
    let ring_addr = gart_va(gfx.dma_addr);

    // CP ring register offsets: discovery-resolved SOC15 (gfx11-correct) when
    // the daemon supplies them, else the gc11 LEGACY constants (QEMU/pre-
    // discovery — unchanged behavior). The legacy values address the wrong gfx11
    // registers on iron, so the SOC15 path is the CP-ring readback-mismatch fix.
    let g = ops.gfx_regs();
    let r_base = g.map_or(gc11::MM_CP_RB0_BASE, |x| x.cp_rb0_base);
    let r_base_hi = g.map_or(gc11::MM_CP_RB0_BASE_HI, |x| x.cp_rb0_base_hi);
    let r_cntl = g.map_or(gc11::MM_CP_RB0_CNTL, |x| x.cp_rb0_cntl);
    let r_rptr = g.map_or(gc11::MM_CP_RB0_RPTR, |x| x.cp_rb0_rptr);
    let r_wptr = g.map_or(gc11::MM_CP_RB0_WPTR, |x| x.cp_rb0_wptr);
    ops.log(if g.is_some() {
        "[amdgpu] stage 6 CP regs: SOC15 (discovery-resolved gfx11 offsets)"
    } else {
        "[amdgpu] stage 6 CP regs: gc11 legacy fallback (no discovery — QEMU)"
    });
    probe_gfx_alive(
        ops,
        "E:post-RLC-safe-mode+config_rs64+CP-reg-resolve (the existing GFX-up probe is next)",
    );

    // ── THE fork-resolver: is GFX actually up? ───────────────────────────────
    // On a PSP-load APU (Athena/Phoenix) the PSP autoloads the GFX firmware and the
    // IMU brings GFX out of reset; the driver then does wait_for_rlc_autoload_complete
    // → SMU → rlc_resume → cp_resume. We had been jumping straight to the CP ring, so
    // every CP_RB0_* write was dropped (readback 0) with no way to tell WHY. Two pure
    // reads settle it decisively:
    //   RLC_RLCS_BOOTLOAD_STATUS bit31 — PSP autoload finished (GFX firmware loaded)
    //   GFX_IMU_GFX_RESET_CTRL & 0x1f  — IMU released GFX from reset (writes will stick)
    // If both hold, GFX is UP and our remaining work is purely rlc_resume + the CP
    // ring (implementable). If not, the firmware did NOT bring GFX up and this ASIC
    // needs the DIRECT-load path (driver loads ucode + setup/start IMU) — a much
    // bigger lift. Either way the next flash tells us exactly which. Reads only, so
    // always safe; gated on discovery (SOC15 offsets, never QEMU/legacy).
    if let Some(gg) = g {
        // GFX power-up (program_rlc_ram + start_imu + RLC autoload wait) already ran at the
        // TOP of init_rings, BEFORE the MES setup (Linux gfx_v11_0_hw_init order). Here we
        // only probe the resulting state.
        let boot = ops.reg_read(gg.rlc_bootload_status);
        let rst = ops.reg_read(gg.gfx_imu_reset_ctrl);
        let autoload = boot & crate::regs::RLC_BOOTLOAD_COMPLETE_MASK != 0;
        let reset_done = rst & crate::regs::GFX_IMU_GFX_RESET_DONE_MASK
            == crate::regs::GFX_IMU_GFX_RESET_DONE_MASK;
        ops.log(&format!(
            "[amdgpu] stage 6 GFX-up probe: RLC_BOOTLOAD_STATUS={boot:#010x} (autoload_complete={autoload}); GFX_IMU_GFX_RESET_CTRL={rst:#010x} (&0x1f reset_done={reset_done})"
        ));
        // The RELIABLE IMU-alive indicator (2026-06-27 working-driver umr comparison):
        // GFX_IMU_GFX_RESET_CTRL and RLC_BOOTLOAD_STATUS BOTH read 0 on a working-but-
        // idle (GFXOFF-gated) GPU, so they are NOT trustworthy "GFX up" signals. The
        // working GFX_IMU_CORE_STATUS reads 0x03 = CBUSY(bit0)=1 + PWAIT_MODE(bit1)=1
        // (the IMU core running, in power-wait). CORE_STATUS is exactly one dword above
        // CORE_CTRL (umr: 0xe0b6 -> 0xe0b7), so read core_ctrl + 4. CBUSY=1 here means
        // the IMU is ALIVE even when the reset/bootload regs read 0 (gated) — the real
        // question is GFXOFF gating of the CP, not a dead IMU.
        let core_status = ops.reg_read(gg.gfx_imu_core_ctrl.wrapping_add(4));
        let imu_cbusy = core_status & 0x1 != 0;
        ops.log(&format!(
            "[amdgpu] stage 6 IMU CORE_STATUS={core_status:#010x} (CBUSY={imu_cbusy} = IMU {}) — the reliable alive signal vs the gated reset/bootload regs above",
            if imu_cbusy { "RUNNING" } else { "not running" }
        ));
        // LOCALIZER (boot 031738): the IMU I-RAM window reads 0xffffffff despite
        // resolving inside BAR5. Sweep known GC-seg1 regs to find where reads turn
        // DEAD — non-IMU regs (GRBM_SEC_CNTL/GCMC_VM_MARC) bracketing the IMU block
        // decode ⇒ the IMU SRAM is power-gated; all-dead-above-a-byte ⇒ rmmio
        // sub-aperture (indirect access). Anchored off the resolved CORE_CTRL byte.
        // The seg1 decode-boundary sweep reads HIGH GC-seg1 registers
        // (GFX_IMU_RLC_BOOTLOADER_HI / I_RAM at dword 0x5f81+). On a GATED GFX domain
        // those reads WEDGE the MMIO bus — they do NOT return 0xffffffff, they HANG —
        // which stalled the whole bring-up at probe #7 before the VERDICT on every warm
        // boot (the netlog always died right after the GCMC probe). So run this
        // diagnostic ONLY when GFX is UP (registers alive). On the DOWN branch we skip
        // it and go straight to the warm-recovery MODE2 reset + EnableGfxImu power-up;
        // the high-reg reads there happen only AFTER the rails are raised.
        if crate::regs::gfx_is_up(boot, rst) {
            probe_seg1_decode_boundary(ops, gg.gfx_imu_core_ctrl);
            ops.log("[amdgpu] stage 6 VERDICT: GFX is UP (PSP autoload done + out of reset) — CP/GFX writes should STICK; remaining work is rlc_resume + CP ring program");
        } else {
            ops.log("[amdgpu] stage 6 VERDICT: GFX is DOWN (PSP did NOT bring GFX up) — attempting the IMU-core wake (imu_v11_0_start: release GFX_IMU_CORE_CTRL.CRESET)");
            // WARM-GPU RECOVERY NOTE (amdgpu smu_v13_0_4_mode2_reset path — DISABLED).
            // A MODE2 SMU reset (PPSMC_MSG_GfxDeviceDriverReset, kept in regs.rs for a
            // future precondition'd path) is the obvious scrub for a dirty GFX domain,
            // but iron warm-boot proved this PMFW REJECTS it from a gated/never-up state:
            // it returned resp 0xFF (PPSMC_Result_Failed) and the failed reset then left
            // the SMU mailbox unresponsive, WEDGING the very next mailbox access. amdgpu
            // only issues MODE2 on an already-running GPU during recovery (after
            // PrepareMp1ForUnload + feature-disable), not at cold/warm bring-up. So the
            // warm-recovery here is EnableGfxImu directly — the IMU power-up that un-gates
            // the domain — which is exactly the cold-up path below, now REACHABLE on a
            // warm boot thanks to the seg1-probe gating (it no longer wedges pre-VERDICT).
            // COLD GFX POWER-UP (imu_v11_0_start APU path: amdgpu_dpm_set_gfx_power_up_by_imu).
            // The seg1 sweep (boot 034417) proved the high GC registers — incl. the IMU
            // I-RAM/bootloader AND general GC regs (GRBM_SEC_CNTL/GCMC) — read
            // 0xffffffff: the GFX power domain is DOWN (no PSP ran), so the I-RAM load
            // below writes to dead registers. The DIRECT path powers GFX via SMU
            // EnableGfxImu(GFXOFF_ENABLE=1) sent ASYNC (no ack — boot 185829's "resp
            // None" was that async semantics, not a failure). Send it, give the PMFW
            // time to raise the GFX rails, then RE-PROBE the decode boundary: if the
            // high regs come ALIVE the power-up worked and the load lands; if still DEAD
            // EnableGfxImu can't cold-start GFX and we need the PSP firmware-load path.
            // Gated on the daemon-confirmed mailbox + message id (never a guessed send).
            if let (Some(mb), Some(msg)) = (ops.smu_mailbox(), ops.enable_gfx_imu_msg()) {
                // PSP-LOAD PATH (Athena oracle 2026-06-23, smu_v13_0_set_gfx_power_up_by_imu):
                // on AMDGPU_FW_LOAD_PSP amdgpu sends EnableGfxImu *SYNCHRONOUSLY*
                // (smu_cmn_send_smc_msg_with_param, arg=ENABLE_IMU_ARG_GFXOFF_ENABLE=1) and
                // WAITS for the PMFW ack — the SMU DOES post a response on this path. The
                // ack is the PMFW confirming the GFX power rails are up. The old async send
                // (boot 185829 "resp None") never polled for that ack, so RaeenOS raced past
                // the power-up and re-probed a still-gated GFX (GFX_RESET_CTRL=0x10). Only
                // the DIRECT-load path is fire-and-forget. Wait up to 1s (the GFX rails take
                // real time to rise). Live SMU mailbox confirmed working (GetSmuVersion OK,
                // DisallowGfxOff acked), so a None here is a genuine power-up failure.
                // iron 2026-06-27 (cold-vfio, TCP-streamed serial): EnableGfxImu sent
                // SYNC WEDGES the entire APU/host — it was the LAST serial line before
                // the box died (no PMFW ack within the 1s poll, then garbage syscalls +
                // a full host hang requiring a power-cycle). async was already disproven
                // (boot 185829 "resp None"); the sync form is catastrophic. On this
                // Phoenix the GFX cold-start cannot be driven by this SMU message at all
                // (GFX comes up DOWN; the IMU I-RAM window reads 0xffffffff), so the real
                // path is the PSP RLC autoload, not EnableGfxImu. Gated OFF so the
                // bring-up DEGRADES GRACEFULLY (probe GFX, find it down, exit) instead of
                // hanging the host — making the GPU loop iterable without a power-cycle.
                // EnableGfxImu IS the IMU-start trigger: it's literally
                // `amdgpu_dpm_set_gfx_power_up_by_imu` (the PMFW powers up the GFX
                // domain + starts the IMU executing). It MUST be sent for the IMU to go
                // CBUSY=1 — gating it off entirely left the IMU at CORE_STATUS=0x02
                // (PWAIT_MODE set, CBUSY=0 = powered but never executing; iron
                // 2026-06-27). The catch: the SYNC response-poll WEDGES the APU (the 1s
                // of MMIO reads on the SMU response register during/after the GFX power-up
                // hangs the whole host — iron capture). The ASYNC send (write the mailbox
                // registers, NEVER read the response) is safe (boot 185829 never wedged)
                // and still triggers the power-up. The earlier "async failed" verdict was
                // a MISDIAGNOSIS — it checked GFX_IMU_GFX_RESET_CTRL, which reads 0 even on
                // a working GPU; the reliable signal is GFX_IMU_CORE_STATUS.CBUSY
                // (working=1). So: send ASYNC, let the GFX rails rise (~200 ms), then
                // re-read CORE_STATUS.CBUSY — does the IMU now execute?
                ops.log(&format!(
                    "[amdgpu] stage 6 cold GFX power-up: SMU EnableGfxImu({msg:#x}) arg=1 ASYNC (the IMU-start trigger; the SYNC response-poll wedges, so we never read the response)"
                ));
                smu_send_msg_async(ops, &mb, msg, 1, 50_000);
                ENABLE_GFX_IMU_SENT.store(true, AtomicOrdering::Relaxed);
                // Hold GFX awake for the CP/SDMA programming (EnableGfxImu's arg=1
                // re-arms GFXOFF). ASYNC too — a SYNC poll right after the no-ack
                // EnableGfxImu is the same wedge class.
                if let Some(msg2) = ops.gfx_off_disable_msg() {
                    smu_send_msg_async(ops, &mb, msg2, 0, 50_000);
                    ops.log("[amdgpu] stage 6 SMU DisallowGfxOff re-sent ASYNC post-EnableGfxImu (hold GFX awake)");
                }
                // Let the PMFW raise the GFX rails + start the IMU executing.
                for _ in 0..200 {
                    ops.delay_us(1000);
                }
                // The decisive re-read: did the IMU start? CORE_STATUS = CORE_CTRL + 4.
                let post_cs = ops.reg_read(gg.gfx_imu_core_ctrl.wrapping_add(4));
                let post_cbusy = post_cs & 0x1 != 0;
                ops.log(&format!(
                    "[amdgpu] stage 6 post-EnableGfxImu IMU CORE_STATUS={post_cs:#010x} (CBUSY={post_cbusy} = IMU {})",
                    if post_cbusy { "STARTED — executing ucode (first light path)" } else { "STILL not executing" }
                ));
                // Poll a high (currently-dead) reg until it decodes, ~40 ms budget — the
                // SYNC ack should already mean GFX is up, but the rails settle slightly after.
                for _ in 0..40 {
                    if ops.reg_read(gg.gfx_imu_i_ram_data) != 0xFFFF_FFFF {
                        break;
                    }
                    ops.delay_us(1000);
                }
                ops.log(
                    "[amdgpu] stage 6 post-EnableGfxImu re-probe (did the GFX domain come alive?):",
                );
                probe_seg1_decode_boundary(ops, gg.gfx_imu_core_ctrl);
                // GFX should now be powered: RE-POLL the RLC autoload (the earlier poll timed
                // out because GFX was still gated). amdgpu's PSP path does EnableGfxImu THEN
                // wait_for_rlc_autoload_complete, so the bootload can only finish now.
                {
                    let mut boot2 = 0u32;
                    for _ in 0..200 {
                        boot2 = ops.reg_read(gg.rlc_bootload_status);
                        if boot2 & crate::regs::RLC_BOOTLOAD_COMPLETE_MASK != 0 {
                            break;
                        }
                        ops.delay_us(10_000);
                    }
                    let done = boot2 & crate::regs::RLC_BOOTLOAD_COMPLETE_MASK != 0;
                    ops.log(&format!(
                        "[amdgpu] stage 6 post-power-up RLC autoload re-poll: RLC_BOOTLOAD_STATUS={boot2:#010x} complete={done} {}",
                        if done { "=> FIRST LIGHT (GFX is UP)" } else { "(still gated — power-up ack did not bring GFX up)" }
                    ));
                }
            } else {
                ops.log("[amdgpu] stage 6 cold GFX power-up SKIPPED — SMU mailbox / EnableGfxImu id unavailable (QEMU)");
            }
            // The direct-load fallback below is ONLY for when the PSP path did NOT light
            // GFX. If the RLC autoload completed (first light, boot 213619), skip it — on
            // the PSP path the IMU I-RAM window stays gated (expected) and these steps just
            // emit misleading "all-ones / still GATED" noise. QEMU never completes the
            // autoload, so the fallback still runs there (no regression).
            if ops.reg_read(gg.rlc_bootload_status) & crate::regs::RLC_BOOTLOAD_COMPLETE_MASK != 0 {
                ops.log("[amdgpu] stage 6 PSP path lit GFX (RLC BOOTLOAD_COMPLETE) — skipping the direct-load fallback (IMU I-RAM load / CRESET wake unused on the PSP path)");
            } else {
                // DIRECT-load FALLBACK step 0: build the RLC autoload buffer (every GFX
                // engine's ucode) + point the IMU bootloader at it — boot 002046 proved the
                // IMU ucode load + CRESET alone can't wake GFX without this buffer.
                load_rlc_autoload_buffer(ops, dev, &gg);
                // DIRECT-load step 1: stream the IMU ucode into IMU SRAM BEFORE releasing
                // the core. Boot 233600 proved CRESET-release alone times out because the
                // IMU had no ucode (PSP left GFX cold). gc_11_0_1_imu.bin is already loaded.
                match ops.request_firmware_bytes("amdgpu/gc_11_0_1_imu.bin") {
                    Some(blob) => {
                        load_imu_microcode(
                            ops,
                            gg.gfx_imu_i_ram_addr,
                            gg.gfx_imu_i_ram_data,
                            gg.gfx_imu_d_ram_addr,
                            gg.gfx_imu_d_ram_data,
                            &blob,
                        );
                        // LOCALIZER (boot 024544): read the I-RAM back to prove the IMU
                        // ucode landed. ALL-ZERO => the GFX_IMU block is gated (writes
                        // dropped) and the real blocker is GFX-clock ungating; LANDED =>
                        // the IMU has its program and the blocker is downstream (the
                        // autoload-buffer MC address).
                        verify_imu_iram(ops, gg.gfx_imu_i_ram_addr, gg.gfx_imu_i_ram_data, &blob);
                    }
                    None => ops.log(
                        "[amdgpu] stage 6 IMU load SKIPPED — gc_11_0_1_imu.bin bytes unavailable",
                    ),
                }
                // DIRECT-load step 3: configure the IMU (imu_v11_0_setup) before start.
                setup_imu(ops, &gg);
                // LOCALIZER: confirm GFX_IMU_CORE_CTRL is writable before we rely on the
                // CRESET clear. If this readback doesn't change after try_imu_core_start
                // clears bit0, the control block is gated too (writes dropped).
                let core_before = ops.reg_read(gg.gfx_imu_core_ctrl);
                // DOWN-branch wake (imu_v11_0_start): the ucode is already PSP-loaded, so
                // the wake is just releasing the IMU core — clear GFX_IMU_CORE_CTRL.CRESET
                // (bit0), then poll GFX_IMU_GFX_RESET_CTRL & 0x1f == 0x1f. This is the
                // documented IMU start (NOT the EnableGfxImu SMU message, which PSP-load
                // never sends — gfx_v11_0/imu_v11_0 line 181). No-op if already running;
                // never touches the DCN display. Gated on discovery.
                let woke =
                    try_imu_core_start(ops, gg.gfx_imu_core_ctrl, gg.gfx_imu_reset_ctrl, 100_000);
                // LOCALIZER: did the CRESET clear actually land? If core_after == core_before
                // the GFX_IMU control block dropped the write (gated); if bit0 cleared the
                // IMU got the go signal but isn't bootstrapping (autoload-buffer blocker).
                let core_after = ops.reg_read(gg.gfx_imu_core_ctrl);
                ops.log(&format!(
                "[amdgpu] stage 6 GFX_IMU_CORE_CTRL: before={core_before:#010x} after-clear={core_after:#010x} (creset_cleared={})",
                core_before & crate::regs::GFX_IMU_CORE_CTRL_CRESET != 0
                    && core_after & crate::regs::GFX_IMU_CORE_CTRL_CRESET == 0
            ));
                if woke {
                    ops.log("[amdgpu] stage 6 IMU-core wake SUCCEEDED — GFX now out of reset (CP/GFX writes should STICK)");
                } else {
                    let rst2 = ops.reg_read(gg.gfx_imu_reset_ctrl);
                    ops.log(&format!(
                    "[amdgpu] stage 6 IMU-core wake TIMEOUT — GFX still in reset (GFX_IMU_GFX_RESET_CTRL={rst2:#010x} &0x1f != 0x1f); see I-RAM readback + CORE_CTRL above to localize (gated vs autoload-buffer)"
                ));
                }
            } // end direct-load fallback (else of the RLC BOOTLOAD_COMPLETE first-light skip)
        }
        // GFXOFF DISABLE — THE REAL FIX (gfx_v11_0_rlc_smu_handshake_cntl(enable=false)).
        // GFXOFF is the SMU<->RLC handshake, gated by RLC_PG_CNTL bit 23
        // (SMU_HANDSHAKE_DISABLE): bit23=0 => the RLC waits for SMU handshake acks and
        // GFXOFF is ENABLED; bit23=1 => the RLC issues NO message to the SMU, no
        // handshake, GFXOFF is DISABLED (amdgpu's own comment). The prior code wrote 0
        // here, which CLEARS bit23 — enabling the handshake and re-gating GFX (and
        // undoing the earlier DisallowGfxOff). Iron 2026-06-27 PROVED the consequence:
        // the MES fw-version heartbeat (CP_MES_GP3_LO) read 0 on BOTH pipes — the
        // microengine never executed despite CP_MES_CNTL ACTIVE, because the whole GFX
        // domain was power-gated (the SAME reason the CP never consumed its ring). SET
        // bit 23 (also clears every GFX_*_PG_ENABLE) right after FIRST LIGHT so the RLC
        // never gates GFX — and since GFXOFF needs ongoing handshaking, cutting it can
        // also drop GFX out of the gate it is already in.
        // HAILMARY 2026-06-30: bit23=0 (handshake ON) — see the post-first-light note above.
        let pg_before = ops.reg_read(gg.rlc_pg_cntl);
        ops.reg_write(gg.rlc_pg_cntl, 0);
        let pg_after = ops.reg_read(gg.rlc_pg_cntl);
        ops.log(&format!(
            "[amdgpu] stage 6 RLC_PG_CNTL HAILMARY bit23=0 handshake-ON: {pg_before:#010x} -> {pg_after:#010x}"
        ));
        // SMU QUIESCE (2026-07-02): the DisallowGfxOff re-send + 800 MHz gfxclk floor
        // (SetHardMinGfxClk/SetSoftMinGfxclk) that used to live here are DELETED. The
        // working driver's cold mmiotrace sends none of them — no GFXOFF or gfxclk-DPM
        // message exists anywhere in its init stream (GFXOFF is boot-disallowed on
        // Phoenix; AllowGfxOff 0x19 only fires ~150 ms AFTER the MES is up). Ours were
        // actively harmful: SetHardMinGfxClk never acked on iron, leaving an in-flight
        // PMFW command that the next send stomped — undefined PMFW state entering the
        // MES set_hw_resources window (the 0x7654 pipe0 clock-stop). The mailbox must
        // be IDLE from here until after the MES acks, exactly like the oracle's.
        // PSP-load rlc_resume's enable_srm step (gfx_v11_0_rlc_enable_srm): turn on
        // the RLC Save/Restore Machine. A known-correct amdgpu write to an RLC reg
        // (NOT the CP and NOT the live DCN display), gated on discovery — a harmless
        // dropped no-op while GFX is gated, the right step once it is up.
        let srm = ops.reg_read(gg.rlc_srm_cntl);
        ops.reg_write(
            gg.rlc_srm_cntl,
            srm | crate::regs::RLC_SRM_CNTL_ENABLE | crate::regs::RLC_SRM_CNTL_AUTO_INCR_ADDR,
        );
        let srm_after = ops.reg_read(gg.rlc_srm_cntl);
        ops.log(&format!(
            "[amdgpu] stage 6 RLC enable_srm: RLC_SRM_CNTL {srm:#010x} -> {srm_after:#010x}"
        ));
        // DOMAIN-WIDE DIAGNOSTIC (post-FIRST-LIGHT): all 3 GFX microengines (CP/SDMA/MES)
        // fail to execute, so the gate is the GFX domain itself, not any per-engine step.
        // Re-read the two authoritative domain-state regs HERE (the GFX-up probe only read
        // them PRE-power-up). GFX_IMU_GFX_RESET_CTRL & 0x1f == 0x1f ⇒ GFX is OUT OF RESET
        // (microengines can run); != 0x1f ⇒ still in reset (the common starve). The IMU
        // CORE_CTRL.CRESET (bit0) tells whether the IMU core is released. If reset≠done
        // here despite RLC FIRST LIGHT, the cold-start gate is the IMU/reset domain — NOT
        // the queue/ucode/gating layers (all ruled out 2026-06-27).
        let rst = ops.reg_read(gg.gfx_imu_reset_ctrl);
        let imu_core = ops.reg_read(gg.gfx_imu_core_ctrl);
        ops.log(&format!(
            "[amdgpu] stage 6 DOMAIN-STATE post-first-light: GFX_IMU_GFX_RESET_CTRL={rst:#010x} (&0x1f={:#x}, reset_done={}), GFX_IMU_CORE_CTRL={imu_core:#010x} (CRESET bit0={})",
            rst & 0x1f,
            rst & 0x1f == 0x1f,
            imu_core & 1
        ));
    }

    // CP enable (gfx_v11_0_cp_gfx_enable). On Phoenix the PFP/ME/MEC microcode is
    // PSP-loaded (stage 5), so the driver's CP step here is enable/resume, NOT a
    // ucode upload. First READ CP_ME_CNTL to record what the GOP/PSP left running
    // (iron ground-truth that confirms the halt-bit mask), then release the CP
    // from halt — gated, so it stays a NO-OP until the gfx11 RS64 halt mask is
    // confirmed (never a guessed write to the CP that drives the live display).
    // Only on the SOC15 path: the gc11 legacy CP_ME_CNTL offset addresses the
    // wrong gfx11 register, so we don't touch it without discovery.
    if let Some(gg) = g {
        let me_cntl = ops.reg_read(gg.cp_me_cntl);
        ops.log(&format!(
            "[amdgpu] stage 6 CP_ME_CNTL={me_cntl:#010x} (GOP/PSP CP enable state; gfx11 ucode is PSP-loaded)"
        ));
        // UNHALT the CP FIRST — this is the AUTHORITATIVE gfx_v11_0_cp_resume order
        // (amdgpu source: cp_gfx_enable(adev, true) is called BEFORE cp_gfx_resume,
        // i.e. the CP is RUNNING while the ring is programmed; the mdelay+double
        // CP_RB0_CNTL write inside cp_gfx_resume lets the running CP pick up the new
        // ring). The earlier "halt first" was backwards.
        if cp_gfx_enable(ops, gg.cp_me_cntl, true) {
            let after = ops.reg_read(gg.cp_me_cntl);
            ops.log(&format!(
                "[amdgpu] stage 6 CP unhalted (before ring program, amdgpu order): CP_ME_CNTL {me_cntl:#010x} -> {after:#010x}"
            ));
        }
    }

    // Wake GFX before touching the CP ring registers. By stage 6 the idle GFX has
    // entered GFXOFF (power-gated) — the boot-170417 readback-0 root cause: writes
    // dropped, reads 0, even with GART built. Poll a live GFX reg (CP_RB0_WPTR, left
    // non-zero by the GOP) until it reads back, which both detects + nudges the wake.
    if let Some(gg) = g {
        let awake = wait_for_gfx_awake(ops, gg.cp_rb0_wptr, 200_000);
        ops.log(if awake {
            "[amdgpu] stage 6 GFX awake (CP_RB0_WPTR readable) — programming ring"
        } else {
            "[amdgpu] stage 6 GFX still GATED after wake-wait (CP_RB0_WPTR=0) — ring writes will be dropped (need a stronger GFXOFF exit)"
        });
    }

    // Build the GFXHUB VMID0 GART NOW — after first light, the one window in which
    // the gfxhub registers accept writes (the Athena cold trace shows amdgpu programs
    // the VM here, immediately after it reads RLC_BOOTLOAD_STATUS = 0xc000001f). The
    // APERTURE map (at GFX_GART_APERTURE_BASE) covers the page span over every buffer
    // the GFX + SDMA engines touch, and each engine reaches its ring/scratch/fence at
    // `gart_va(phys)` (= phys + delta) — the high VA the gfxhub actually translates.
    // `lo` MUST be the same page-aligned `gart_lo` the gart_va mapper used.
    // lo/hi were computed once (gart_lo/gart_hi) over the full buffer span, including
    // the MES ucode/data buffers, so the MES reaches its code through VMID0 too.
    if !init_gart_identity(ops, dev, gart_lo, gart_hi) {
        ops.log("[amdgpu] GART (identity) SKIPPED — gfxhub offsets unresolved (QEMU/pre-discovery); no GMC register touched");
    }

    // RUNG 1 COMPLETION — DIRECT-LOAD + START THE MES, now that the GART maps the MES
    // ucode/data buffers (init_gart above). load = point CP_MES_IC_BASE/MDBASE at the
    // GART VAs + prime the I-cache (mes_v11_0_load_microcode); enable = PC + activate
    // (mes_v11_0_enable). Without the IC base the MES has no code and faults on enable
    // (iron: CP_MES_CNTL=0). Read CP_MES_CNTL back: 0x0c000000 (PIPE0+1 active) = MES
    // ALIVE. Gated on the load regs + the direct-loaded buffers (None on QEMU).
    if let (Some(lr), Some(mr), Some((ub, db, p0)), Some((_, p1))) = (
        ops.mes_load_regs(),
        ops.mes_enable_regs(),
        mes_load.as_ref(),
        ops.mes_uc_starts(),
    ) {
        // IC_BASE/MDBASE are PHYSICAL addresses, NOT GART VAs. The MES instruction fetch
        // must work BEFORE the MES sets up any queue VM, so it reads the ucode from
        // physical memory directly (umr oracle on the working amdgpu 2026-06-27: MES
        // IC_BASE=0x4_59120000, sitting next to the gfxhub page-table base 0x4_5fd00000
        // in PHYSICAL space — NOT inside the 0x7fff_xxxx gfxhub GART aperture where the
        // ring lives). Passing the GART VA here made the MES fetch garbage → GP3=0.
        let ucode_phys = ub.dma_addr;
        let data_phys = db.dma_addr;
        let kiq_present = mes_kiq_load.is_some();

        // PSP-LOAD CHECK: read CP_MES_IC_BASE (me=3/pipe0) BEFORE we touch it. The
        // WORKING amdgpu on this APU is AMDGPU_FW_LOAD_PSP — it does enable-ONLY (the
        // PSP loads the MES ucode + sets IC_BASE=0x4_59120000); only the DIRECT-load
        // "backdoor" path writes IC_BASE. If the PSP set it here, our direct-load
        // OVERWRITES the PSP's correct (primed) ucode pointer with our own extracted
        // copy — a divergence from the working driver and a candidate for GP3=0.
        ops.reg_write(lr.grbm_gfx_cntl, 3 << 2); // me=3, pipe=0
        let psp_ic_lo = ops.reg_read(lr.ic_base_lo);
        let psp_ic_hi = ops.reg_read(lr.ic_base_hi);
        ops.reg_write(lr.grbm_gfx_cntl, 0);
        // H2 (2026-06-27 iron): the PSP does NOT load the MES on this Phoenix ASIC — the
        // MES LOAD_IP_FW is rejected (0xffff0006) and H1 removed it from the PSP autoload
        // batch entirely. So a non-zero CP_MES_IC_BASE here is STALE (GOP/garbage, e.g. the
        // 0xd824:0x79d55000 read on iron), NOT a valid PSP-primed pointer. Trusting it
        // (enable-ONLY) runs the MES microengine from a garbage I-cache base → it parks at
        // entry+1 (INSTR_PNTR 0x1401, heartbeat=0). ALWAYS direct-load: point IC_BASE at
        // our extracted ucode. (amdgpu's enable-ONLY is correct only when the PSP actually
        // loaded the MES — which it does NOT for this ASIC+firmware.)
        let psp_set_ic = (psp_ic_lo != 0) || (psp_ic_hi != 0);
        // STEP 1 (2026-06-29): the PSP DID load the MES + set IC_BASE — iron readback is
        // 0x4:0x59120000 = the EXACT amdgpu value on this APU. The old "PSP rejects MES,
        // always direct-load" hardcode predates the NIGHT-5 fw-type fix (33/34/81/82) that
        // made LOAD_IP_FW accept the MES with no rejects. A non-zero IC_BASE here = the
        // PSP's VALID primed pointer → ENABLE-ONLY: trust it, do NOT overwrite it with our
        // own direct-load (which made the MES park at INSTR_PNTR 0x7204, a divergence from
        // the working driver). This is amdgpu's AMDGPU_FW_LOAD_PSP path.
        let psp_loaded = psp_set_ic;
        ops.log(&format!(
            "[amdgpu] stage 6 MES IC_BASE pre-load: {psp_ic_hi:#x}:{psp_ic_lo:#010x} (psp_loaded={psp_loaded} => {})",
            if psp_loaded { "ENABLE-ONLY, trust the PSP IC_BASE" } else { "direct-load IC_BASE (PSP did not set it)" }
        ));

        if psp_loaded {
            // ENABLE-ONLY — match the working driver's PSP-load path: the PSP already
            // loaded + primed the MES ucode and set IC_BASE, so do NOT overwrite it.
            ops.log("[amdgpu] stage 6 MES PSP-loaded → ENABLE-ONLY (no IC_BASE overwrite, matching amdgpu's AMDGPU_FW_LOAD_PSP path)");
        } else {
            // DIRECT-load "backdoor" path (PSP did NOT set IC_BASE): write our extracted
            // ucode/data pointers + prime. Load pipe 0 (SCHED) WITHOUT priming — amdgpu
            // primes only on the LAST pipe loaded, after both IC bases are set.
            ops.log(&format!(
                "[amdgpu] stage 6 MES load (rung 1) pipe0 SCHED: IC_BASE={ucode_phys:#x} MDBASE={data_phys:#x} (PHYSICAL) pc={p0:#x} (prime={})",
                !kiq_present
            ));
            for (reg, val) in crate::mes::build_mes_load_sequence(
                &lr,
                0,
                ucode_phys,
                data_phys,
                *p0,
                !kiq_present,
            ) {
                ops.reg_write(reg, val);
            }
            // Load pipe 1 (KIQ) WITH priming (primes the shared I-cache once both pipes'
            // IC bases are programmed) — this is what mes_v11_0_kiq_hw_init does.
            if let Some((kub, kdb, p1v)) = &mes_kiq_load {
                let kiq_ucode_phys = kub.dma_addr; // PHYSICAL, not GART VA (see pipe0 note)
                let kiq_data_phys = kdb.dma_addr;
                ops.log(&format!(
                    "[amdgpu] stage 6 MES load (rung 1) pipe1 KIQ: IC_BASE={kiq_ucode_phys:#x} MDBASE={kiq_data_phys:#x} (PHYSICAL) pc={p1v:#x} (prime=true)"
                ));
                for (reg, val) in crate::mes::build_mes_load_sequence(
                    &lr,
                    1,
                    kiq_ucode_phys,
                    kiq_data_phys,
                    *p1v,
                    true,
                ) {
                    ops.reg_write(reg, val);
                }
            }
        }
        // init_csb (gfx_v11_0_init_csb) — program the RLC Clear-State Buffer descriptor
        // BEFORE the MES enable, exactly as amdgpu does (oracle trace 2026-06-27: it
        // writes RLC_CSIB_ADDR_LO/HI + LENGTH right before CP_MES_PRGRM_CNTR_START). The
        // MES stalls one instruction into boot (INSTR_PNTR parked at entry+1) waiting on
        // the RLC, and this is the one pre-enable step RaeenOS skipped. Point CSIB at our
        // GART-mapped buffer + the working-driver LENGTH 0x3c0. Gated on discovery (g) +
        // the buffer; a harmless no-op on QEMU.
        if let (Some(gg), Some(csb)) = (g.as_ref(), csb_buf.as_ref()) {
            let csb_va = gart_va(csb.dma_addr);
            ops.reg_write(gg.rlc_csib_addr_lo, csb_va as u32);
            ops.reg_write(gg.rlc_csib_addr_hi, (csb_va >> 32) as u32);
            ops.reg_write(gg.rlc_csib_length, 0x3c0);
            ops.log(&format!(
                "[amdgpu] stage 6 init_csb: RLC_CSIB_ADDR={csb_va:#x} LENGTH=0x3c0 (the pre-MES-enable RLC step RaeenOS skipped; MES stalls @entry+1 without it)"
            ));
        }
        // REPLAY amdgpu's deterministic pre-MES GFX config that RaeenOS skips (cold-init
        // wreg trace 2026-06-27): per-VMID SH_MEM setup + RLC/CP config registers. The
        // MES stalls one instruction into boot polling for GFX state these establish.
        // GC seg1 wreg-index = ip_base+reg (same in RaeenOS via discovery); reg_write
        // takes the MMIO byte offset = index<<2. Gated on discovery (g).
        if g.is_some() {
            // RLC + CP config (exact values amdgpu writes before the MES).
            ops.reg_write(0xce0c << 2, 0x00f0_0188); // RLC_RLCS register
            ops.reg_write(0xa944 << 2, 0x0000_0008);
            ops.reg_write(0xa960 << 2, 0x5548_0100);
            ops.reg_write(0xc200 << 2, 0x4000_0000); // CP config sequence
            ops.reg_write(0xc200 << 2, 0x4000_0100);
            ops.reg_write(0xc200 << 2, 0xe000_0000);
            // per-VMID SH_MEM: GRBM_GFX_CNTL(0xa900)=vmid<<4, SH_MEM_CONFIG(0xa9e4)=0xc00c,
            // SH_MEM_BASES(0xa9e3)= 0x20001000 (vmid 1-7) / 0x10002 (vmid 8-15).
            for vmid in 1u32..16 {
                ops.reg_write(0xa900 << 2, vmid << 4);
                ops.reg_write(0xa9e4 << 2, 0x0000_c00c);
                ops.reg_write(
                    0xa9e3 << 2,
                    if vmid < 8 { 0x2000_1000 } else { 0x0001_0002 },
                );
            }
            ops.reg_write(0xa900 << 2, 0); // restore GRBM selection
                                           // CP_MES setup block (H3, cold-init trace 2026-06-27): the 10 CP_MES config
                                           // registers the working amdgpu writes that RaeenOS skipped ENTIRELY (RaeenOS
                                           // only wrote CNTL/PC/IC_BASE/MDBASE). The MES microengine reads these (pipe /
                                           // doorbell-range / bounds / interrupt config) at boot — without them it parks
                                           // at entry+1 (INSTR_PNTR 0x1401, heartbeat=0, HEADER_DUMP=reset). Exact values
                                           // + trace order from the live working driver (GC seg1, gc_base[1]=0 here, so the
                                           // trace's resolved dword index == the raw offset, same as the RLC/CP block above).
            ops.reg_write(0x2808 << 2, 0x003e_01e7);
            ops.reg_write(0x2809 << 2, 0x0000_0000);
            ops.reg_write(0x282b << 2, 0x0009_abff);
            ops.reg_write(0x282c << 2, 0x0000_0000);
            ops.reg_write(0x2825 << 2, 0x0006_0000);
            ops.reg_write(0x281c << 2, 0x0008_0e01);
            ops.reg_write(0x281d << 2, 0x0000_0003);
            ops.reg_write(0x281e << 2, 0x8013_0009);
            ops.reg_write(0x282e << 2, 0xffff_ffff);
            ops.reg_write(0x2824 << 2, 0x3fff_fffc);
            // WINDOW DIFF (2026-07-01, cold_init_named-20260624 comprehensive pass): every
            // GC write amdgpu makes between EnableGfxImu and the set_hw_resources doorbell
            // (43.60..43.9102) that RaeenOS still wrote NOWHERE (name+offset grep). All are
            // pre-set_hw_res PRECONDITIONS in the working sequence; exact trace values.
            ops.reg_write(0x282f << 2, 0x0000_000f); // GCVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_HI32
            ops.reg_write(0x2830 << 2, 0x0000_0000); // GCVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_LO32
            ops.reg_write(0x2831 << 2, 0x0000_0000); // GCVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_HI32
            ops.reg_write(0x2832 << 2, 0x0000_0000); // GCVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_LO32
            ops.reg_write(0x2833 << 2, 0x0000_0000); // GCVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_HI32
            ops.reg_write(0x2834 << 2, 0x0000_0001); // GCVM_L2_CNTL4
            ops.reg_write(0x283a << 2, 0x0000_3fe0); // GCVM_L2_CNTL5
            ops.reg_write(0x307f << 2, 0x0040_8000); // CP_DEBUG
            ops.reg_write(0x31b3 << 2, 0x0000_c200); // CPF_GCR_CNTL — the CP FETCHER's
                                                     // global-cache-request config; the fetch path every MES/CP memory op takes.
            ops.reg_write(0xa802 << 2, 0x0000_0000); // CP_MEC_CNTL — un-halt the MEC pipes
                                                     // (amdgpu releases MEC right before the MES enable; ours stayed halted).
                                                     // GDS zeroing (gfx_v11_0 init): amdgpu zeroes every VMID's GDS base/size +
                                                     // GWS/OA allocation right before the RLC_CSIB/MES block — POST/GOP leftovers
                                                     // in the GDS can poison engine state. BASE/SIZE pairs stride 2 from 0x3300,
                                                     // GWS at 0x3320+n, OA at 0x3330+n (GC seg0, trace-verified layout).
            for vmid in 0u32..16 {
                ops.reg_write((0x3300 + vmid * 2) << 2, 0); // GDS_VMIDn_BASE
                ops.reg_write((0x3301 + vmid * 2) << 2, 0); // GDS_VMIDn_SIZE
                ops.reg_write((0x3320 + vmid) << 2, 0); // GDS_GWS_VMIDn
                ops.reg_write((0x3330 + vmid) << 2, 0); // GDS_OA_VMIDn
            }
            // ORACLE DIFF (2026-06-30): the GC registers the live amdgpu writes during init
            // that RaeenOS wrote NOWHERE — found by diffing our register set against amdgpu's
            // full-init wreg trace (docs/gpu-oracle/full_init_named-20260630). Exact values +
            // absolute dword offsets (gc_base baked in) from the live driver. The leading
            // candidates for the SCHED-pipe-halts-after-the-handler-runs/no-ack symptom:
            //   - missing CP pipe interrupt routing (the event/ACK the MES pipe waits on), and
            //   - missing L1/TCP cache config (the path the MES fence-writeback traverses).
            // [cat3] CP global + MEC/MES-pipe interrupt enables (CP_INT_CNTL_RING0 final value;
            //        all four ME1 pipes 0x04000000):
            ops.reg_write(0x306a << 2, 0x8400_0000); // CP_INT_CNTL_RING0
            ops.reg_write(0x3085 << 2, 0x0400_0000); // CP_ME1_PIPE0_INT_CNTL
            ops.reg_write(0x3086 << 2, 0x0400_0000); // CP_ME1_PIPE1_INT_CNTL
            ops.reg_write(0x3087 << 2, 0x0400_0000); // CP_ME1_PIPE2_INT_CNTL
            ops.reg_write(0x3088 << 2, 0x0400_0000); // CP_ME1_PIPE3_INT_CNTL
            ops.reg_write(0x2000 << 2, 0x0000_00ff); // GRBM_CNTL (read/write timeout)
                                                     // [cat2] L1 / texture / aux cache config (the MES fence-writeback coherency path):
            ops.reg_write(0xb9a2 << 2, 0x239c_0020); // TCP_CNTL
            ops.reg_write(0xb9a3 << 2, 0x0000_000a); // TCP_CNTL2
            ops.reg_write(0x2542 << 2, 0x0103_0000); // TA_CNTL_AUX
                                                     // [cat4] GFX engine config:
            ops.reg_write(0x2285 << 2, 0x0088_0007); // PA_CL_ENHANCE
            ops.reg_write(0x31d2 << 2, 0x0000_0008); // SPI_GDBG_PER_VMID_CNTL
                                                     // [cat1] sub-block clock gating amdgpu sets that RaeenOS skipped:
            ops.reg_write(0x12bc << 2, 0x0040_0000); // SDMA0_RLC_CGCG_CTRL
            ops.reg_write(0xf087 << 2, 0x0000_0010); // CGTT_GS_NGG_CLK_CTRL
            ops.log("[amdgpu] stage 6 replayed amdgpu pre-MES GFX config + ORACLE-DIFF additions (CP int-routing, TCP/TA cache, PA_CL/SPI_GDBG, SDMA/GS clock-gating) — the init RaeenOS skipped");
        }
        for (reg, val) in crate::mes::build_mes_enable_sequence(&mr, *p0, p1) {
            ops.reg_write(reg, val);
        }
        ops.delay_us(500); // mes_v11_0_enable settle (udelay(500)) before the readback
        let cntl = ops.reg_read(mr.cp_mes_cntl);
        let p0_active = cntl & (1 << 0x1a) != 0;
        ops.log(&format!(
            "[amdgpu] stage 6 MES CP_MES_CNTL readback={cntl:#010x} (PIPE0_ACTIVE={p0_active}) — MES engine {}",
            if p0_active { "ALIVE" } else { "did not activate" }
        ));
        // DECISIVE liveness test: CP_MES_GP3_LO holds the fw VERSION the MES microengine
        // writes once it boots its ucode (mes_v11_0_get_fw_version, grbm me=3/pipe). A
        // non-zero value PROVES the engine is executing — ACTIVE bits alone do not. Read
        // both pipes: pipe0=scheduler (mes_2), pipe1=KIQ (mes1).
        ops.reg_write(mr.grbm_gfx_cntl, 3 << 2); // me=3, pipe=0
        let sched_ver = ops.reg_read(mr.cp_mes_gp3_lo);
        ops.reg_write(mr.grbm_gfx_cntl, (3 << 2) | 1); // me=3, pipe=1
        let kiq_ver = ops.reg_read(mr.cp_mes_gp3_lo);
        ops.reg_write(mr.grbm_gfx_cntl, 0);
        ops.log(&format!(
            "[amdgpu] stage 6 MES fw-version heartbeat: sched(pipe0)={sched_ver:#x} kiq(pipe1)={kiq_ver:#x} (NON-ZERO => microengine is EXECUTING ucode; 0 => not running despite ACTIVE)"
        ));
        // MES EXECUTION PROBE — localize WHY GP3=0. CP_MES_INSTR_PNTR (GC seg1 0x2813)
        // is the MES program counter: read it TWICE (with grbm me=3/pipe0). If it sits
        // at the entry PC (uc_start>>2) the microengine never started fetching; if it
        // ADVANCES between the two reads it IS executing but stalls before writing GP3
        // (then the gate is a handshake/dependency, not a dead engine). HEADER_DUMP
        // (0x280d) + GP0/GP1 may carry a fault/status code. All linearly offset from
        // GP3 (each reg = 4 bytes), so derive from mr.cp_mes_gp3_lo — no new plumbing.
        let gp3 = mr.cp_mes_gp3_lo;
        let instr_pntr = gp3.wrapping_sub((0x2849 - 0x2813) * 4); // CP_MES_INSTR_PNTR
        let header_dump = gp3.wrapping_sub((0x2849 - 0x280d) * 4); // CP_MES_HEADER_DUMP
        let gp0 = gp3.wrapping_sub((0x2849 - 0x2843) * 4); // CP_MES_GP0_LO
        let gp1 = gp3.wrapping_sub((0x2849 - 0x2845) * 4); // CP_MES_GP1_LO
        ops.reg_write(mr.grbm_gfx_cntl, 3 << 2); // me=3, pipe0
        let ip1 = ops.reg_read(instr_pntr);
        ops.delay_us(100);
        let ip2 = ops.reg_read(instr_pntr);
        let hdr = ops.reg_read(header_dump);
        let g0 = ops.reg_read(gp0);
        let g1 = ops.reg_read(gp1);
        // pipe1 (KIQ): SAME resolved INSTR_PNTR offset, GRBM-selected to pipe1. The KIQ
        // microengine must be in its service loop (running PC, like pipe0's 0x7204) to
        // ever fetch its ring. If it sits at entry+1 (0x1401) it booted (wrote the fw
        // version → non-zero heartbeat) but never entered the loop → the KIQ-fetch gate.
        ops.reg_write(mr.grbm_gfx_cntl, (3 << 2) | 1);
        let ip1_kiq = ops.reg_read(instr_pntr);
        ops.delay_us(100);
        let ip2_kiq = ops.reg_read(instr_pntr);
        ops.reg_write(mr.grbm_gfx_cntl, 0);
        ops.log(&format!(
            "[amdgpu] stage 6 MES exec probe: pipe0 INSTR_PNTR {ip1:#x}->{ip2:#x}, pipe1/KIQ {ip1_kiq:#x}->{ip2_kiq:#x} (entry PC={:#x}; pipe1 at a running PC like pipe0 => KIQ engine LOOPING [gate is doorbell-wake/ring-read]; pipe1 stuck@entry+1 0x1401 => never started), HEADER_DUMP={hdr:#x} GP0={g0:#x} GP1={g1:#x}",
            (*p0 >> 2) as u32
        ));
    } else {
        ops.log("[amdgpu] stage 6 MES load+enable SKIPPED — offsets/ucode unresolved (QEMU) or direct-load buffers unavailable");
    }

    // ENABLE THE DOORBELL APERTURE (nbio_v4_3_enable_doorbell_aperture) — set
    // RCC_DOORBELL_APER_EN.BIF_DOORBELL_APER_EN (bit 0) so BAR2 doorbell writes ROUTE
    // to the engines. The GOP firmware sets up only DISPLAY (no doorbells), so this is
    // likely OFF — and without it NO doorbell reaches the MES command ring OR the gfx
    // CP (the shared reason both ignored their doorbells). Must precede every doorbell
    // ring below. Read-modify-write; log the before/after so a 0→1 confirms the fix.
    if let Some(aper) = ops.doorbell_aper_en_reg() {
        let before = ops.reg_read(aper);
        ops.reg_write(aper, before | 1);
        let after = ops.reg_read(aper);
        ops.log(&format!(
            "[amdgpu] stage 6 doorbell aperture: RCC_DOORBELL_APER_EN {before:#010x} -> {after:#010x} (bit0 EN must be 1 for doorbells to route)"
        ));
    } else {
        ops.log("[amdgpu] stage 6 doorbell aperture enable SKIPPED — NBIO reg unresolved (QEMU)");
    }

    // PROGRAM THE MEC/MES DOORBELL RANGE (gfx_v11_0_cp_set_doorbell_range) — the
    // compute/MES-class doorbell range the MEC/MES monitors to WAKE on a doorbell ring.
    // The gfx CP_RB range [0x458,0x7f8] (set later in the gfx ring setup) does NOT cover
    // the MES SCHED (byte 0x58) + KIQ (byte 0x60) doorbells. Iron 2026-06-28 (umr on the
    // working amdgpu): CP_MEC_DOORBELL_RANGE = [0x0, 0x450]; RaeenOS OMITTED it, so the KIQ
    // doorbell HIT latched in the HQD but the MES microengine was never woken (KIQ rptr=0,
    // SCHED ring never mapped). Must precede the KIQ doorbell ring below.
    if let Some((mec_db_lo, mec_db_up)) = ops.cp_mec_doorbell_range_regs() {
        ops.reg_write(mec_db_lo, 0x0);
        ops.reg_write(mec_db_up, 0x450);
        ops.log("[amdgpu] stage 6 CP_MEC_DOORBELL_RANGE = [0x0, 0x450] (covers MES 0x58 + KIQ 0x60 doorbells so a ring WAKES the MES)");
    } else {
        ops.log("[amdgpu] stage 6 CP_MEC_DOORBELL_RANGE SKIPPED — GC reg unresolved (QEMU)");
    }

    // ── RUNGS 2-4: tell the ALIVE MES to SCHEDULE the gfx queue → the CP fetches.
    // Set up the MES command ring (MQD + queue_init), submit set_hw_resources, then
    // build the gfx MQD + submit map_legacy_queue. All addresses are GART VAs (the MES
    // + CP reach them through VMID0). Gated on the queue buffers + HQD regs + ip bases.
    if let (Some(q), Some(hqd), Some((gc_base, mmhub_base, osssys_base))) =
        (mes_queue.as_ref(), ops.mes_hqd_regs(), ops.mes_ip_bases())
    {
        // ── disable GFX clock gating (amdgpu gfx_v11_0_update_*_clock_gating(false)) ──
        // The MES SCHED pipe halts mid-op0 at a 2-byte-MISALIGNED PC (INSTR 0x7656 vs the
        // 4-aligned real instr 0x7654, mcause=0) = a CLOCK-STOP mid-instruction-FETCH,
        // while the fast KIQ pipe survives — the signature of medium-grain clock gating
        // gating the slower SCHED pipe between its longer set_hw_resources processing.
        // RaeenOS never touches clock gating; the PSP-loaded RLC fw can leave MGCG/CGCG ON.
        // Disable like amdgpu's enable==false path: SET all RLC_CGTT_MGCG_OVERRIDE override
        // bits (force every clock domain ON) + clear CGCG_EN/CGLS_EN. Regs are GC seg1
        // (gc_base[1]): MGCG_OVERRIDE=0x4c48, CGCG_CGLS_CTRL=0x4c49, _3D=0x4cc5 (umr seg).
        // Override masks (gc_11_0_0_sh_mask): RLC_CGTT_SCLK 0x02, GFXIP_MGCG 0x04,
        // GFXIP_CGCG 0x08, GFXIP_CGLS 0x10, GRBM_CGTT_SCLK 0x20, GFXIP_GFX3D_CG 0x80.
        let mgcg_override = gc_base[1].wrapping_add(0x4c48) << 2;
        let cgcg_ctrl = gc_base[1].wrapping_add(0x4c49) << 2;
        let cgcg_ctrl_3d = gc_base[1].wrapping_add(0x4cc5) << 2;
        let mg = ops.reg_read(mgcg_override);
        // HALT-DUMP (iron 2026-06-30) PROVED pipe0 clock-STOPS mid-set_hw_resources (PC
        // frozen 16/16 @0x7656, mcause=mstatus=0, no trap taken). 0xBE only overrode the
        // GFX-domain gates; the WORKING driver's MGCG_OVERRIDE is 0x607e7 (cold trace) —
        // bits 0,6,8,9,10,13,14 cover sub-blocks (RLC SCLK / FGCG / CP-SPI / perfmon) that
        // 0xBE missed, one of which is the MES pipe's clock. Force the superset.
        ops.reg_write(mgcg_override, mg | 0x0006_07ff); // disable ALL medium-grain clock gates
        let cg = ops.reg_read(cgcg_ctrl);
        ops.reg_write(cgcg_ctrl, cg & !0x3); // clear CGCG_EN|CGLS_EN
        let cg3 = ops.reg_read(cgcg_ctrl_3d);
        ops.reg_write(cgcg_ctrl_3d, cg3 & !0x3);
        let mg_after = ops.reg_read(mgcg_override);
        ops.log(&format!(
            "[amdgpu] GFX clock-gating DISABLED: MGCG_OVERRIDE {mg:#x}->{mg_after:#x} CGCG {cg:#x},{cg3:#x} (fix MES SCHED pipe clock-stop)"
        ));

        // ── GFX-DROP TRACKER (owner reframe 2026-06-29) ── The SCHED pipe halts mid-FETCH
        // (INSTR 0x7654->0x7656, mcause=0) while the KIQ runs — the signature of GFX losing
        // power/clock OUT FROM UNDER pipe0 during stage 6. bringup.rs already saw "reset_ctrl
        // 0x1f->0x10 gated back". Read GFX_IMU_GFX_RESET_CTRL (GC seg1 0x40bc; &0x1f==0x1f =
        // GFX up, 0x10 = GATED) at stage-6 START, right BEFORE set_hw_resources, and AFTER the
        // stall — to pin WHERE the drop happens so the hold-awake can be re-asserted there.
        let reset_ctrl_reg = gc_base[1].wrapping_add(0x40bc) << 2;
        let rc0 = ops.reg_read(reset_ctrl_reg);
        ops.log(&format!(
            "[amdgpu] GFX-CHK stage6-start: RESET_CTRL={rc0:#x} (0x..1f=up, 0x..10=GATED)"
        ));

        // ── program_invalidation (amdgpu gfxhub_v3_0/mmhub_v3_0_program_invalidation) ──
        // ROOT CAUSE (iron INV17 probe 2026-06-29): the MES set_hw_resources handler
        // issues a full-flush TLB invalidate on its dedicated engine ENG17 of BOTH hubs
        // (GCVM + MMVM ENG17 REQ=0x2f80000) and spins forever on the ACK (=0) at INSTR
        // 0x7656. RaeenOS's build_gart_enable_sequence set up CONTEXT0 + ENG0 only, never
        // the per-engine ADDR_RANGE amdgpu programs for ALL 18 engines — left at 0, an
        // invalidate on ENG17 covers no range and never completes. Write 0xffffffff/0x1f
        // (full coverage) for ENG0..17 on both hubs, BEFORE the MES runs set_hw_resources.
        // ADDR_RANGE is benign (no L2/context/translation change) so it can't disturb the
        // live DCN scanout. GCVM (GC seg0): ENG0_ADDR_RANGE_LO32=0x16cf/HI32=0x16d0,
        // stride 2 dwords/eng. MMVM (MMHUB seg1): ENG0_ADDR_RANGE_LO32=0x787/HI32=0x788,
        // stride 2 (offsets from gc_11_0_0 / mmhub_3_0_0 _offset.h; seg verified vs the
        // working umr dump's ENG17 absolute offsets 0x291c / 0x1a774).
        for i in 0u32..18 {
            ops.reg_write((gc_base[0].wrapping_add(0x16cf + i * 2)) << 2, 0xffff_ffff);
            ops.reg_write((gc_base[0].wrapping_add(0x16d0 + i * 2)) << 2, 0x1f);
            ops.reg_write(
                (mmhub_base[1].wrapping_add(0x787 + i * 2)) << 2,
                0xffff_ffff,
            );
            ops.reg_write((mmhub_base[1].wrapping_add(0x788 + i * 2)) << 2, 0x1f);
        }
        ops.log("[amdgpu] program_invalidation: GCVM+MMVM ENG0..17 ADDR_RANGE=full (fix MES ENG17 invalidate stall)");

        // ── enable MMHUB L2 cache so the MES's MMVM ENG17 invalidate can complete ──
        // Iron INV17 + L2_CNTL probe (f3d0c66) localized the set_hw_resources 0x7656 stall:
        // the MES does its GFXHUB (GCVM) invalidate fine, then issues the MMHUB (MMVM) one
        // and spins forever on the ACK — because MMHUB's L2 cache is OFF. Readback proved
        // it: MMVM_L2_CNTL=0x80602 (bit0 ENABLE_L2_CACHE clear) vs GCVM_L2_CNTL=0x80e01
        // (on). RaeenOS brings up GFXHUB but never touches MMHUB. RMW just set bit0 —
        // preserving firmware's MMHUB config bits (1/9/10/19 = the DCN aperture/fragment
        // setup) so the live display scanout is undisturbed — then kick an L2 invalidate so
        // the enable takes effect. Deliberately NOT amdgpu's full init_cache_regs (which
        // would overwrite firmware's MMHUB config and risk the running DCN). MMVM_L2_CNTL=
        // 0x700, MMVM_L2_CNTL2=0x701 (MMHUB seg1, mmhub_3_0_0_offset.h).
        let mmvm_l2_cntl = mmhub_base[1].wrapping_add(0x700) << 2;
        let mmvm_l2_cntl2 = mmhub_base[1].wrapping_add(0x701) << 2;
        let cur_l2 = ops.reg_read(mmvm_l2_cntl);
        ops.reg_write(mmvm_l2_cntl, cur_l2 | 1); // ENABLE_L2_CACHE (bit0)
        ops.reg_write(mmvm_l2_cntl2, (1 << 0) | (1 << 1)); // INVALIDATE_ALL_L1_TLBS|INVALIDATE_L2_CACHE
        ops.log(&format!(
            "[amdgpu] MMHUB L2 enable: MMVM_L2_CNTL {cur_l2:#x} -> {:#x} (lets MMVM ENG17 invalidate ack)",
            cur_l2 | 1
        ));

        // ── INV17-BEFORE (disambiguation 2026-06-29) ── 3 GMC fixes (ADDR_RANGE, MMHUB L2,
        // CONTEXT1-15) left ENG17 REQ=0x2f80000/ACK=0 rock-stable. Read ENG17 REQ/ACK on both
        // hubs HERE — BEFORE the MES has processed set_hw_resources (its SCHED ring isn't even
        // submitted yet this far up the block). If REQ already reads 0x2f80000 now, then the
        // post-submit 0x2f80000 is a firmware/reset CONSTANT, NOT a live MES invalidate —
        // i.e. the 0x7656 stall is something else and we've been chasing a red herring. If
        // REQ reads 0/reset here and 0x2f80000 only AFTER, the MES genuinely issued it.
        let b_gcvm_req = ops.reg_read(0x291c << 2);
        let b_gcvm_ack = ops.reg_read(0x292e << 2);
        let b_mmvm_req = ops.reg_read(0x1a774 << 2);
        let b_mmvm_ack = ops.reg_read(0x1a786 << 2);
        ops.log(&format!(
            "[amdgpu] INV17-BEFORE (no MES yet) GCVM r/a={b_gcvm_req:#x}/{b_gcvm_ack:#x} MMVM r/a={b_mmvm_req:#x}/{b_mmvm_ack:#x} (==0x2f80000 already => REQ is a CONSTANT, invalidate=red herring)"
        ));

        // ── setup_vmid_config: enable CONTEXT1..15 on both hubs (amdgpu gfxhub_v3_0/
        // mmhub_v3_0_setup_vmid_config) ── Iron (290bbac) proved L2-enable + ADDR_RANGE
        // alone don't make the MES's ENG17 invalidate ack. set_hw_resources runs with
        // vmid_mask_gfxhub/mmhub=0xFF00 (VMIDs 8-15), so the MES invalidates those VMIDs'
        // TLBs — but RaeenOS only ever enabled CONTEXT0 (VMID0); CONTEXT1-15 sit at reset
        // (disabled), so an invalidate over the masked VMIDs never completes. amdgpu's
        // gart_enable enables all 16 contexts (cold_mmio trace). Enable CONTEXT1..15 flat
        // (CNTL=0x1fffe01 = ENABLE | depth0 | all-fault-enable, same as CONTEXT0) with a
        // full page-table range [0, max], on BOTH hubs, BEFORE the MES runs. Offsets:
        // GFXHUB CONTEXT0_CNTL=0x1688 (RaeenOS-proven) ctx_distance 1, START_LO=0x1713/
        // END_LO=0x1733 ctx_addr_distance 2; MMHUB CONTEXT0_CNTL=0x740 / START_LO(0)=0x7cb /
        // END_LO(0)=0x7eb (mmhub_3_0_0, seg1 base mmhub_base[1] — between the readback-
        // confirmed 0x700 L2_CNTL and 0x763 ENG0_REQ anchors). A flat enabled context lets
        // its invalidate flush+ack; no PTB walk happens during an invalidate, and nothing
        // accesses VMIDs 8-15 during bring-up, so this can't fault the live DCN.
        for i in 1u32..=15 {
            ops.reg_write((gc_base[0].wrapping_add(0x1688 + i)) << 2, 0x1fffe01);
            ops.reg_write((gc_base[0].wrapping_add(0x1713 + i * 2)) << 2, 0);
            ops.reg_write((gc_base[0].wrapping_add(0x1714 + i * 2)) << 2, 0);
            ops.reg_write((gc_base[0].wrapping_add(0x1733 + i * 2)) << 2, 0xffff_ffff);
            ops.reg_write((gc_base[0].wrapping_add(0x1734 + i * 2)) << 2, 0xf);
            ops.reg_write((mmhub_base[1].wrapping_add(0x740 + i)) << 2, 0x1fffe01);
            ops.reg_write((mmhub_base[1].wrapping_add(0x7cb + i * 2)) << 2, 0);
            ops.reg_write((mmhub_base[1].wrapping_add(0x7cc + i * 2)) << 2, 0);
            ops.reg_write(
                (mmhub_base[1].wrapping_add(0x7eb + i * 2)) << 2,
                0xffff_ffff,
            );
            ops.reg_write((mmhub_base[1].wrapping_add(0x7ec + i * 2)) << 2, 0xf);
        }
        ops.log("[amdgpu] setup_vmid_config: enabled GCVM+MMVM CONTEXT1..15 flat (lets MES invalidate VMIDs 8-15 complete)");

        const MES_DOORBELL_BYTE: u32 = 0x58; // SCHED ring: mes_ring0(0x0B)<<1=0x16, *4
        const MES_DOORBELL_IDX: u32 = 0x16;
        const KIQ_DOORBELL_BYTE: u32 = 0x60; // KIQ ring: mes_ring1(0x0C)<<1=0x18, *4
        const KIQ_DOORBELL_IDX: u32 = 0x18;
        // The fence buffer holds the small writebacks at distinct dwords: SCHED ring
        // rptr@0, wptr@4, api-completion fence@64, query-status-fence@128; KIQ ring
        // rptr@256, wptr@260.
        let mes_ring_va = gart_va(q.cmd_ring.dma_addr);
        let mes_mqd_va = gart_va(q.mqd.dma_addr);
        let mes_eop_va = gart_va(q.eop.dma_addr);
        let mes_rptr_va = gart_va(q.fence.dma_addr);
        let mes_wptr_va = gart_va(q.fence.dma_addr + 16);
        let api_fence_va = gart_va(q.fence.dma_addr + 256);
        let query_status_va = gart_va(q.fence.dma_addr + 512);
        let sch_ctx_va = gart_va(q.sch_ctx.dma_addr);
        let kiq_ring_va = gart_va(q.kiq_ring.dma_addr);
        let kiq_mqd_va = gart_va(q.kiq_mqd.dma_addr);
        let kiq_eop_va = gart_va(q.kiq_eop.dma_addr);
        let kiq_rptr_va = gart_va(q.fence.dma_addr + 1024);
        let kiq_wptr_va = gart_va(q.fence.dma_addr + 1040);

        // THE CORRECTED MES BRING-UP ORDER (mes_v11_0_kiq_hw_init + _hw_init):
        // the SCHED ring (pipe0) is NEVER brought up by direct CP_HQD writes — only the
        // KIQ (pipe1) is. The KIQ then maps the SCHED ring via a PACKET3_MAP_QUEUES.
        // My earlier code direct-wrote the SCHED HQD + rang its doorbell (the doorbell
        // HIT the HQD, but the MES microengine never serviced a queue it wasn't told
        // about by the KIQ). This is that missing KIQ layer.

        // 0. kiq_setting: tell the RLC the KIQ is at me=3 / pipe=1 / queue=0. Until
        // 2026-07-01 REG_RLC_CP_SCHEDULERS resolved to SEGMENT 0 (a dead address), so the
        // real register never saw this write and the RLC — which clock-manages the MES
        // pipes — had no live scheduler on record: the prime suspect for the pipe0
        // clock-stop mid-set_hw_resources (INSTR 0x7656, mcause=0). The working driver
        // reads 0x3038 here and writes 0x30e8 (low byte 0xe8 = me3/pipe1/q0 + 0x80 enable).
        let prev_sched = ops.reg_read(hqd.rlc_cp_schedulers);
        ops.reg_write(
            hqd.rlc_cp_schedulers,
            crate::mes::kiq_setting_value(prev_sched, 3, 1, 0),
        );
        let sched_after = ops.reg_read(hqd.rlc_cp_schedulers);
        ops.log(&format!(
            "[amdgpu] stage 6 kiq_setting: RLC_CP_SCHEDULERS(seg1) {prev_sched:#010x} -> {sched_after:#010x} (expect low byte 0xe8; working driver 0x3038 -> 0x30e8)"
        ));

        // 1. KIQ ring (pipe1): MQD → write → queue_init_register(pipe=1). Direct CP_HQD
        //    bring-up — the KIQ is the bootstrap queue that maps everything else.
        let kiq_mqd = crate::mes::build_mes_mqd(
            kiq_mqd_va,
            kiq_ring_va,
            kiq_rptr_va,
            kiq_wptr_va,
            kiq_eop_va,
            4096,
            KIQ_DOORBELL_IDX,
        );
        ops.dma_write(&q.kiq_mqd, 0, &kiq_mqd);
        for (reg, val) in crate::mes::build_mes_queue_init_register(&hqd, &kiq_mqd, 1) {
            ops.reg_write(reg, val);
        }
        ops.log("[amdgpu] stage 6 MES KIQ ring (pipe1) up via direct CP_HQD writes");

        // 2. SCHED ring (pipe0): build the MQD IN MEMORY ONLY. The KIQ maps it next —
        //    NO direct CP_HQD writes for the SCHED ring (that was the bug).
        let mes_mqd = crate::mes::build_mes_mqd(
            mes_mqd_va,
            mes_ring_va,
            mes_rptr_va,
            mes_wptr_va,
            mes_eop_va,
            64 * 1024,
            MES_DOORBELL_IDX,
        );
        ops.dma_write(&q.mqd, 0, &mes_mqd);

        // 3. KIQ → MAP_QUEUES(SCHED ring): push to the KIQ ring + ring the KIQ doorbell.
        //    The KIQ (MES pipe1) consumes it and activates the SCHED ring (MES pipe0).
        // The working amdgpu's mes_kiq ring (iron hexdump 2026-06-28) carries ONLY the
        // MAP_QUEUES packet — no SET_RESOURCES. So push just the map; the earlier
        // SET_RESOURCES experiment was reverted once the ring dump disproved it.
        // KIQ RING-TEST replica (iron mmiotrace + working-ring hexdump 2026-06-28): amdgpu's
        // KIQ submission is MAP_QUEUES *followed by a WRITE_DATA* that stores 0xdeadbeef to
        // register dword 0xc040 (a CP_MES scratch), and amdgpu primes that scratch with
        // 0xcafedead via MMIO right before the doorbell, then polls it. The MES proves it
        // DRAINED the ring by overwriting the scratch. We replicate it both as the likely
        // commit the MES needs AND as a DECISIVE probe: if the scratch reads 0xdeadbeef after
        // the doorbell, the MES IS processing RaeenOS's KIQ ring (and SCHED-not-active is a
        // separate issue); if it stays 0xcafedead, the MES never drained the ring at all.
        const MES_SCRATCH_DWORD: u32 = 0xc040;
        let scratch_reg = MES_SCRATCH_DWORD << 2; // byte offset for reg_read/write
        ops.reg_write(scratch_reg, 0xcafe_dead);
        let mut kiq_wptr = 0u32;
        let mut kiq_map =
            crate::mes::build_kiq_map_queues_mes(MES_DOORBELL_IDX, mes_mqd_va, mes_wptr_va, 0, 0);
        // WRITE_DATA(reg 0xc040 = 0xdeadbeef): PACKET3_WRITE_DATA(0x37) count 3, control
        // 0x00010000 (DST_SEL=mem-mapped reg), addr_lo=0xc040, addr_hi=0, data=0xdeadbeef —
        // byte-identical to the working KIQ ring.
        kiq_map.extend_from_slice(&[
            0xc003_3700,
            0x0001_0000,
            0x0000_c040,
            0x0000_0000,
            0xdead_beef,
        ]);
        // PAD the submission to 256 dwords (type-2 NOPs) so the doorbell rings 0x100 like
        // amdgpu (every KIQ submission is 256-dword padded; the 2nd is 0x200). (4KB KIQ
        // ring = 1024 dwords, so 256 fits.)
        while kiq_map.len() < 256 {
            kiq_map.push(0x8000_0000);
        }
        mes_ring_push(
            ops,
            &q.kiq_ring,
            &mut kiq_wptr,
            &kiq_map,
            &q.fence,
            260,
            KIQ_DOORBELL_BYTE,
        );
        // DRAIN BARRIER (Athena ftrace mes_v11_0_hw_init, 2026-06-30): the working driver,
        // after gfx11_kiq_map_queues, runs amdgpu_ring_test_helper -> POLLS this scratch reg
        // until the CP executes the WRITE_DATA. Because the scratch write trails MAP_QUEUES in
        // the ring, its completion PROVES the map drained and the SCHED queue is loaded into
        // hardware. We previously waited a fixed 300us with no proof, so the MES could start
        // op0 on a half-loaded queue context and halt mid-handler (INSTR 0x7654, mcause=0).
        // GATE on the scratch here — only proceed to set_hw_resources once the KIQ has drained.
        let mut kiq_drained = false;
        for _ in 0..2000 {
            if ops.reg_read(scratch_reg) == 0xdead_beef {
                kiq_drained = true;
                break;
            }
            ops.delay_us(1);
        }
        let scratch_final = ops.reg_read(scratch_reg);
        ops.log(&format!(
            "[amdgpu] stage 6 KIQ drain barrier: scratch={scratch_final:#x} drained={kiq_drained} (0xdeadbeef => MAP_QUEUES consumed + SCHED queue live; proceeding to set_hw_resources)"
        ));

        // 4. set_hw_resources → submit on the SCHED ring, poll the ack.
        let mut wptr = 0u32;
        // Values BYTE-DIFFED against the WORKING amdgpu SET_HW_RSRC packet (Athena live
        // `umr -RS mes_3.0.0`, 2026-06-29): every field matches EXCEPT sdma_hqd_mask[1].
        // Working = [0xfc, 0x0] (Phoenix has ONE SDMA engine — dmesg shows only sdma0);
        // RaeenOS sent [0xfc, 0xfc], telling the MES there's a SECOND SDMA engine. The
        // set_hw_resources handler then tries to set up the phantom SDMA1's HQDs (which
        // don't exist) and HALTS pipe0 mid-handler (INSTR 0x7656, mcause=0 = no fault, a
        // bad value not a bus fault) — exactly the halt the firmware-disasm localized.
        // gds_size=0x1000, compute_hqd_mask=0x0c×4, gfx_hqd_mask [0x2,0] all confirmed match.
        let hwres = crate::mes::MesHwResources {
            vmid_mask_mmhub: 0xFF00,
            vmid_mask_gfxhub: 0xFF00,
            gds_size: 0x1000,
            // (Engine-readiness RULED OUT 2026-06-29: zeroing all hqd_masks did NOT stop
            // the mid-op0 halt — so it's NOT the MES reaching into a dead MEC/SDMA/gfx
            // engine; the halt is in the EARLY, mask-independent part of op0. Working
            // values restored.)
            compute_hqd_mask: [0x0c, 0x0c, 0x0c, 0x0c, 0, 0, 0, 0],
            gfx_hqd_mask: [0x2, 0],
            sdma_hqd_mask: [0xfc, 0x0],
            sch_ctx_va,
            query_fence_va: query_status_va,
            gc_base,
            mmhub_base,
            osssys_base,
            api_fence_addr: api_fence_va,
            api_fence_value: 1,
        };
        // Diagnostic: our gc_base (IP register bases) vs the live working amdgpu's. If
        // these differ, the MES reads set_hw_resources but can't reach the GC registers
        // to apply it -> aborts the frame without acking. Working amdgpu (Athena ring
        // dump 2026-06-29): gc_base[0..5]=0x1260,0xa000,0x2402c00,0x2000029,0x10205.
        ops.log(&format!(
            "[amdgpu] stage 6 gc_base=[{:#x},{:#x},{:#x},{:#x},{:#x}] mmhub0={:#x} (work=1260,a000,2402c00,2000029,10205)",
            gc_base[0], gc_base[1], gc_base[2], gc_base[3], gc_base[4], mmhub_base[0]
        ));
        ops.dma_write(&q.fence, 64, &[0]); // zero the api fence (dword 64)
                                           // Fence slots (q.fence dwords): op0 api_fence@64, op0 QUERY@96, op1 QUERY@100,
                                           // cleaner_shader_fence@104.
        let op0_query_va = gart_va(q.fence.dma_addr + 384); // dword 96
        let op1_query_va = gart_va(q.fence.dma_addr + 400); // dword 100
        let cleaner_va = gart_va(q.fence.dma_addr + 416); // dword 104
        let pkt = crate::mes::build_mes_set_hw_resources(&hwres);
        let pkt1 = crate::mes::build_mes_set_hw_resources_1(api_fence_va, 1, cleaner_va);
        ops.dma_write(&q.fence, 64, &[0]); // zero op0 api fence
        ops.dma_write(&q.fence, 96, &[0]); // zero op0 QUERY fence
        ops.dma_write(&q.fence, 100, &[0]); // zero op1 QUERY fence
        let rc1 = ops.reg_read(reset_ctrl_reg);
        ops.log(&format!(
            "[amdgpu] GFX-CHK pre-set_hw_res (just before doorbell): RESET_CTRL={rc1:#x}"
        ));
        // SEPARATE submits (Athena ftrace mes_v11_0_hw_init, 2026-06-30): the working driver
        // submits set_hw_resources ALONE, polls its completion, THEN set_hw_resources_1 ALONE
        // — each its own doorbell ring + fence poll (mes_v11_0_submit_pkt_and_poll_completion,
        // which appends a QUERY_SCHEDULER_STATUS and waits on its fence). We previously batched
        // op0+QUERY+op1+QUERY behind ONE doorbell. mes_submit_and_poll replicates the per-
        // packet append-QUERY-and-poll protocol; call it once per op, advancing the same wptr.
        let acked0 = mes_submit_and_poll(
            ops,
            &q.cmd_ring,
            &mut wptr,
            &pkt,
            &q.fence,
            4,
            MES_DOORBELL_BYTE,
            &q.fence,
            96,
            op0_query_va,
            1,
            500,
        );
        let acked1 = if acked0 {
            mes_submit_and_poll(
                ops,
                &q.cmd_ring,
                &mut wptr,
                &pkt1,
                &q.fence,
                4,
                MES_DOORBELL_BYTE,
                &q.fence,
                100,
                op1_query_va,
                1,
                500,
            )
        } else {
            false
        };
        ops.log(&format!(
            "[amdgpu] stage 6 MES set_hw_res SEPARATE submits → op0={} op1={}",
            if acked0 { "ACKed" } else { "NO ack" },
            if acked1 { "ACKed" } else { "NO ack/skipped" }
        ));
        let rc2 = ops.reg_read(reset_ctrl_reg);
        // NOTE: this reads the reset state, not the GFXOFF clock state — and on iron it
        // stays 0x1f (GFX UP) THROUGH the halt. Compute the verdict from the value so the
        // line can't be misread as "gated" (an earlier free-text parenthetical was).
        let gfx_up = rc2 & 0x1f == 0x1f;
        ops.log(&format!(
            "[amdgpu] GFX-CHK post-set_hw_res (after the stall): RESET_CTRL={rc2:#x} => GFX {} ((&0x1f)==0x1f means UP; ==0x10 would mean reset-gated — iron: stays UP, so the SCHED halt is NOT whole-GFX GFXOFF)",
            if gfx_up { "UP" } else { "GATED" }
        ));
        // FENCE-CHASE DIAGNOSTIC (2026-06-29): the MES READ the SCHED ring (HQD_RPTR=0x100)
        // but wrote no ack fence. Bisect WHY: the MES writes its global scheduler-context
        // pointer into sch_ctx when it PROCESSES set_hw_resources. The mem-write probe
        // proved the GPU can write memory via VMID0, so —
        //   sch_ctx NON-ZERO  => MES PROCESSED set_hw_resources; the missing ack is a
        //                        fence-write/address issue (chase api_fence encoding).
        //   sch_ctx ALL-ZERO  => MES read the ring but ABORTED before processing (the real
        //                        wall — the SCHED queue isn't actually being scheduled by
        //                        the KIQ MAP_QUEUES; chase the queue-map / pipe0 activation).
        let mut sc = [0u32; 2];
        ops.dma_read(&q.sch_ctx, 0, &mut sc);
        let mut apif = [0u32; 1];
        ops.dma_read(&q.fence, 64, &mut apif);
        let mut qsf = [0u32; 1];
        ops.dma_read(&q.fence, 128, &mut qsf);
        // Short (klog caps at 159B). sch_ctx!=0 => MES wrote its scheduler ctx = PROCESSED
        // set_hw_resources; all-0 => MES read the ring (rptr) but ABORTED before processing
        // (SCHED pipe0 not truly scheduled). Iron 2026-06-29: sc=0,0 = NOT processed.
        ops.log(&format!(
            "[amdgpu] stage 6 set_hw_res POST-SCAN: sch_ctx={:#x}:{:#x} api_fence={:#x} query_fence={:#x} (sc!=0 => MES processed; 0 => aborted)",
            sc[1], sc[0], apif[0], qsf[0]
        ));
        // MMHUB-INVALIDATE PROBE (2026-06-29): the working MES SCHED ring (Athena umr) polls
        // BOTH hubs' VM-invalidate engines via WAIT_REG_MEM — gfx1101.GCVM_INVALIDATE_ENG17
        // (REQ dword 0x291c / ACK 0x292e) and mmhub301.MMVM_INVALIDATE_ENG17 (REQ 0x1a774 /
        // ACK 0x1a786). RaeenOS configures GFXHUB (GCVM) fully but does ZERO MMHUB (MMVM) VM
        // init. If the set_hw_resources handler issues an MMHUB TLB invalidate, it writes
        // MMVM_ENG17_REQ then spins on MMVM_ENG17_ACK — which never sets because MMHUB's
        // invalidate engine isn't enabled. Read all four (absolute SOC15 dwords from the
        // working umr dump << 2 = MMIO byte offset; RaeenOS's gc/mmhub bases match the
        // working driver, so the same offsets address the same regs). MMVM req!=0 & ack=0
        // (while GCVM acks) => the MES is stuck on an MMHUB invalidate = MMHUB VM not init.
        let gcvm17_req = ops.reg_read(0x291c << 2);
        let gcvm17_ack = ops.reg_read(0x292e << 2);
        let mmvm17_req = ops.reg_read(0x1a774 << 2);
        let mmvm17_ack = ops.reg_read(0x1a786 << 2);
        ops.log(&format!(
            "[amdgpu] INV17 GCVM r/a={gcvm17_req:#x}/{gcvm17_ack:#x} MMVM r/a={mmvm17_req:#x}/{mmvm17_ack:#x} (after program_invalidation: want ack!=0; ack still 0 => hub L2/VM not up)"
        ));
        // L2 state: ENABLE_L2_CACHE = bit0. GCVM_L2_CNTL (GC seg0 0x15bc) — RaeenOS enables
        // it. MMVM_L2_CNTL (MMHUB seg1 0x700) — RaeenOS never touches it; if firmware left
        // MMHUB L2 enabled (DCN scans out through it) bit0=1 and program_invalidation alone
        // should let MMVM ENG17 ack; bit0=0 => MMHUB needs an L2-enable before its invalidate
        // can complete (do it WITHOUT reprogramming apertures, to not disturb the display).
        let gcvm_l2 = ops.reg_read(gc_base[0].wrapping_add(0x15bc) << 2);
        let mmvm_l2 = ops.reg_read(mmhub_base[1].wrapping_add(0x700) << 2);
        ops.log(&format!(
            "[amdgpu] L2_CNTL GCVM={gcvm_l2:#x} MMVM={mmvm_l2:#x} (bit0=ENABLE_L2_CACHE; MMVM bit0=0 => MMHUB L2 off = MMVM invalidate can't ack)"
        ));
        // PIPE0-STATE PROBE (fence chase 2026-06-29): GRBM-select each MES pipe + read its
        // GP/INSTR registers (offsets from cp_mes_gp3_lo @0x2849: GP0_HI=-20, GP2_HI=-4,
        // GP3_LO=0, INSTR_PNTR @0x2813 = -216 dwords*4). The WORKING amdgpu (live umr) has
        // GP0=0x7fffffff:0xf800e409, GP2=0xffffffff:0xf017fdf0 (valid MES-local pointers),
        // GP3=0x01025088 (fw version), INSTR=0x7204. Hypothesis: pipe0(SCHED) reads but
        // never processed set_hw_resources, so its scheduler pointers (GP0/GP2 high bits)
        // are 0 while pipe1(KIQ, which executes) has them. Compare the two.
        if let (Some(mr), Some(lr)) = (ops.mes_enable_regs(), ops.mes_load_regs()) {
            let gp3 = mr.cp_mes_gp3_lo;
            let cntl = lr.grbm_gfx_cntl;
            ops.reg_write(cntl, 3 << 2); // me=3 pipe=0 (SCHED)
            let p0_gp0hi = ops.reg_read(gp3.wrapping_sub(20));
            let p0_gp2hi = ops.reg_read(gp3.wrapping_sub(4));
            let p0_gp3 = ops.reg_read(gp3);
            let p0_instr = ops.reg_read(gp3.wrapping_sub(216));
            // MES RISC-V trap CSRs on the STALLED SCHED pipe (offsets from Athena umr db
            // gc_11_0_0.reg, GC seg1: MCAUSE_LO=0x281a, MEPC_LO=0x2818, MBADADDR_LO=0x281c;
            // derive from gp3@0x2849 by byte delta). The invalidate was a red herring — this
            // is the smoking gun: if the MES FAULTED inside set_hw_resources, mcause!=0 and
            // mbadaddr = the exact bad address (5=load fault, 7=store fault, 2=illegal instr).
            // The working idle MES reads all 0 (no fault); a tight poll-loop also reads 0.
            let p0_mcause = ops.reg_read(gp3.wrapping_sub((0x2849 - 0x281a) * 4));
            let p0_mepc = ops.reg_read(gp3.wrapping_sub((0x2849 - 0x2818) * 4));
            let p0_mbad_lo = ops.reg_read(gp3.wrapping_sub((0x2849 - 0x281c) * 4));
            let p0_mbad_hi = ops.reg_read(gp3.wrapping_sub((0x2849 - 0x281d) * 4));
            // #5 (owner fix list): SCHED pipe0 HQD/queue state vs the WORKING KIQ pipe1.
            // KIQ runs => doorbell/GART/MES-exec are fine; the contrast is pipe0's queue.
            // cp_hqd_active=1 = queue live; pq_base = ring>>8; pq_control = QUEUE_SIZE+flags.
            let p0_active = ops.reg_read(hqd.cp_hqd_active);
            let p0_pqbase = ops.reg_read(hqd.cp_hqd_pq_base);
            let p0_pqctl = ops.reg_read(hqd.cp_hqd_pq_control);
            ops.reg_write(cntl, (3 << 2) | 1); // me=3 pipe=1 (KIQ)
            let p1_gp0hi = ops.reg_read(gp3.wrapping_sub(20));
            let p1_gp2hi = ops.reg_read(gp3.wrapping_sub(4));
            let p1_gp3 = ops.reg_read(gp3);
            let p1_active = ops.reg_read(hqd.cp_hqd_active);
            let p1_pqbase = ops.reg_read(hqd.cp_hqd_pq_base);
            let p1_pqctl = ops.reg_read(hqd.cp_hqd_pq_control);
            ops.reg_write(cntl, 0);
            ops.log(&format!(
                "[amdgpu] PIPE0(SCHED) GP0_HI={p0_gp0hi:#x} GP2_HI={p0_gp2hi:#x} GP3={p0_gp3:#x} INSTR={p0_instr:#x} (work 7fffffff,ffffffff,1025088,7204)"
            ));
            ops.log(&format!(
                "[amdgpu] MES-TRAP(SCHED) mcause={p0_mcause:#x} mepc={p0_mepc:#x} mbadaddr={p0_mbad_hi:#x}:{p0_mbad_lo:#x} (mcause!=0 => MES FAULTED in set_hw_resources; mbadaddr=bad addr)"
            ));
            ops.log(&format!(
                "[amdgpu] PIPE1(KIQ) GP0_HI={p1_gp0hi:#x} GP2_HI={p1_gp2hi:#x} GP3={p1_gp3:#x} (KIQ executes — contrast; pipe0 HI=0 => SCHED scheduler never init)"
            ));
            ops.log(&format!(
                "[amdgpu] HQD-DIFF SCHED active={p0_active:#x} pqbase={p0_pqbase:#x} pqctl={p0_pqctl:#x} || KIQ active={p1_active:#x} pqbase={p1_pqbase:#x} pqctl={p1_pqctl:#x} (SCHED active=0/pqbase=0 vs KIQ ok => SCHED queue never truly mapped by KIQ MAP_QUEUES)"
            ));
            // MES DATA-RAM probe (#5/#1 follow-up): the SCHED pipe is healthy in every
            // observable way (active queue, correct IMEM, GFX up, no fault) but halts in the
            // handler — the remaining variable is the DATA it reads. The handler derefs the
            // MES local-data pointer *(0xf0100168) (loaded by the PSP into the MES DATA RAM /
            // DC_BASE VRAM). Verify the data RAM is set up like the working driver
            // (DC_BASE=0x4:0x593b0000, MDBOUND=0x7ffff) and read the pointer via the MES
            // data-memory index port (CP_MES_DM_INDEX_ADDR 0x5c00 / _DATA 0x5c01, GC seg1).
            // A wrong DC_BASE/MDBOUND or a null/garbage ptr@0x168 => the PSP MES-data load is
            // wrong => the handler reads bad data and hangs.
            let dc_lo = ops.reg_read(gc_base[1].wrapping_add(0x5854) << 2);
            let dc_hi = ops.reg_read(gc_base[1].wrapping_add(0x5855) << 2);
            let mdbound = ops.reg_read(gc_base[1].wrapping_add(0x585d) << 2);
            let dm_addr = gc_base[1].wrapping_add(0x5c00) << 2;
            let dm_data = gc_base[1].wrapping_add(0x5c01) << 2;
            ops.reg_write(dm_addr, 0x168); // DMEM byte offset of the handler's pointer
            let dmem_168 = ops.reg_read(dm_data);
            ops.reg_write(dm_addr, 0x16c);
            let dmem_16c = ops.reg_read(dm_data);
            ops.log(&format!(
                "[amdgpu] MES-DMEM DC_BASE={dc_hi:#x}:{dc_lo:#x} MDBOUND={mdbound:#x} ptr@0x168={dmem_16c:#x}:{dmem_168:#x} (work DC_BASE=0x4:0x593b0000 MDBOUND=0x7ffff; mismatch/null ptr => PSP MES-data load wrong)"
            ));

            // ── SURGICAL HALT DUMP (owner request 2026-06-30) ──────────────────────────
            // The capstone disasm proved 0x7654 (=andi a3,a3,0) is pure STACK arithmetic
            // (no MMIO near the halt; the two jals before it returned) and the iron probe
            // showed GFX_IMU_GFX_RESET_CTRL=0x1f (GFX powered). So the open question is
            // narrow: is pipe0's microengine CLOCK-STOPPED (PC frozen, not retiring) vs
            // hung-but-clocked, and what did the firmware stash in its GP scratch that the
            // set_hw_resources handler is blocked on. Re-select pipe0 (SCHED) and dump:
            //  (a) INSTR_PNTR sampled 16x in a tight loop. 0x7654 is straight-line (NOT a
            //      loop, per disasm) -> if all 16 reads are identical, the core is not
            //      retiring instructions = clock-stop or a hard pipeline stall (NOT a poll
            //      loop). If the PC moves, the earlier single-shot reads mis-sampled and
            //      the core is actually alive.
            //  (b) MSTATUS (0x2816: MIE=bit3 global-int-enable, MPP=bits11:12 privilege)
            //      + MTVEC (0x2801) — a non-default MSTATUS/MTVEC narrows a wait/trap state.
            //  (c) the FULL CP_MES_GP0..GP5 scratch (we previously logged only GP0_HI/
            //      GP2_HI/GP3). The working idle MES has GP0=0x7fffffff:0xf800e409,
            //      GP2=0xffffffff:0xf017fdf0 (MES-local pointers) — a pointer in OUR pipe0
            //      GP scratch that differs / points at an unmapped resource is the lead.
            // Offsets: GC seg1 byte-deltas off GP3_LO@0x2849 (cp-mes-trap-csr-offsets.txt):
            //   INSTR_PNTR(0x2813)=-216, MSTATUS(0x2816)=-204, MTVEC(0x2801)=-288;
            //   GP pairs are contiguous lo/hi (CONFIRMED: GP0_HI@-20,GP2_HI@-4 matched the
            //   working values), so GP0_LO=-24 .. GP5_HI=+20.
            // NOTE: the absolute clock-cycle proof (RISC-V minstret/mcycle read twice)
            // needs those CSRs' MMIO offsets from Athena's umr db (grep CP_MES.*CYCLE in
            // /usr/share/umr/database/ip/gc_11_0_0.reg) — not derivable from the 6 known
            // points; add as a one-liner once captured. The PC-freeze test below is the
            // strongest clock signal obtainable without it.
            let p0_instr_pntr = gp3.wrapping_sub(216);
            ops.reg_write(cntl, 3 << 2); // me=3 pipe=0 (SCHED) — re-select after the DMEM reads
            let mut ips = [0u32; 16];
            for s in ips.iter_mut() {
                *s = ops.reg_read(p0_instr_pntr);
            }
            let ip_frozen = ips.iter().all(|&v| v == ips[0]);
            let ip_min = ips.iter().copied().min().unwrap_or(0);
            let ip_max = ips.iter().copied().max().unwrap_or(0);
            let d_mstatus = ops.reg_read(gp3.wrapping_sub(204));
            let d_mtvec = ops.reg_read(gp3.wrapping_sub(288));
            let g0_lo = ops.reg_read(gp3.wrapping_sub(24));
            let g0_hi = ops.reg_read(gp3.wrapping_sub(20));
            let g1_lo = ops.reg_read(gp3.wrapping_sub(16));
            let g1_hi = ops.reg_read(gp3.wrapping_sub(12));
            let g2_lo = ops.reg_read(gp3.wrapping_sub(8));
            let g2_hi = ops.reg_read(gp3.wrapping_sub(4));
            let g4_lo = ops.reg_read(gp3.wrapping_add(8));
            let g4_hi = ops.reg_read(gp3.wrapping_add(12));
            let g5_lo = ops.reg_read(gp3.wrapping_add(16));
            let g5_hi = ops.reg_read(gp3.wrapping_add(20));
            ops.reg_write(cntl, 0); // restore default GRBM selection
            ops.log(&format!(
                "[amdgpu] HALT-DUMP INSTR_PNTR x16: first={:#x} frozen={ip_frozen} (min={ip_min:#x} max={ip_max:#x}) — frozen on straight-line 0x7654 => core NOT retiring (clock-stop/hard-stall, NOT a poll loop); moving => core alive, single-shot mis-sampled",
                ips[0]
            ));
            ops.log(&format!(
                "[amdgpu] HALT-DUMP MSTATUS={d_mstatus:#x} (MIE=bit3 MPP=bits11:12) MTVEC={d_mtvec:#x} (working idle reads MSTATUS with M-mode running, MTVEC = the trap base)"
            ));
            ops.log(&format!(
                "[amdgpu] HALT-DUMP GP-scratch GP0={g0_hi:#x}:{g0_lo:#x} GP1={g1_hi:#x}:{g1_lo:#x} GP2={g2_hi:#x}:{g2_lo:#x} GP4={g4_hi:#x}:{g4_lo:#x} GP5={g5_hi:#x}:{g5_lo:#x} (work GP0=7fffffff:f800e409 GP2=ffffffff:f017fdf0; a differing/unmapped pointer = the blocked resource)"
            ));
            // PMFW LIVENESS AT THE HALT (2026-07-02): with the mailbox quiesced (no
            // in-flight command can exist here any more), a bounded GetSmuVersion asks
            // "is the PMFW itself responsive at the moment pipe0 froze?". None => the
            // PMFW is wedged/busy — strong evidence the pipe0 clock-stop is a hung
            // PMFW service (it owns the MES pipe clocks). Some(_) => PMFW fine, the
            // halt lives elsewhere (MES-internal / pipe state).
            if let Some(mb) = ops.smu_mailbox() {
                let alive = smu_send_msg(ops, &mb, 0x02, 0, 200_000); // GetSmuVersion
                ops.log(&format!(
                    "[amdgpu] HALT-DUMP PMFW liveness: GetSmuVersion->{alive:?} (None => PMFW wedged at the halt; Some => PMFW responsive, halt is MES/pipe-internal)"
                ));
            }
        }

        // 3. gfx MQD → write; 4. map_legacy_queue → submit (SCHEDULE the gfx ring).
        let gfx_mqd_va = gart_va(q.gfx_mqd.dma_addr);
        let gfx_ring_va = gart_va(gfx.dma_addr);
        let gfx_rptr_va = gart_va(gfx.dma_addr + RING_BYTES as u64 - 8);
        let gfx_wptr_va = gart_va(gfx.dma_addr + RING_BYTES as u64 - 16);
        let gfx_mqd = crate::mes::build_gfx_mqd(
            gfx_mqd_va,
            gfx_ring_va,
            gfx_rptr_va,
            gfx_wptr_va,
            64 * 1024,
            0x116,
        );
        ops.dma_write(&q.gfx_mqd, 0, &gfx_mqd);
        ops.dma_write(&q.fence, 64, &[0]);
        let mappkt = crate::mes::build_mes_map_legacy_queue(0x116, gfx_mqd_va, gfx_wptr_va, 0, 0);
        let mapped = mes_submit_and_poll(
            ops,
            &q.cmd_ring,
            &mut wptr,
            &mappkt,
            &q.fence,
            4,
            MES_DOORBELL_BYTE,
            &q.fence,
            64,
            api_fence_va,
            1,
            500, // bounded (see set_hw_resources poll note)
        );
        ops.log(&format!(
            "[amdgpu] stage 6 MES map_legacy_queue(gfx) submitted → {}",
            if mapped {
                "ACKed — gfx queue SCHEDULED (CP should fetch)"
            } else {
                "NO ack"
            }
        ));

        // DIAGNOSTIC: localize where the chain breaks. Read BOTH MES pipes:
        //  - KIQ HQD (me=3, pipe=1): proves the KIQ bootstrap queue is active. If its
        //    rptr (kiq_rptr@dw256) advanced, the KIQ DRAINED the MAP_QUEUES packet.
        //  - SCHED HQD (me=3, pipe=0): if ACTIVE=1 here AND the KIQ drained, the KIQ
        //    successfully mapped the SCHED ring. sched rptr@dw0 advancing => MES pipe0
        //    drained set_hw_resources/map_legacy.
        let mut sched_rptr = [0u32; 1];
        let mut kiq_rptr = [0u32; 1];
        ops.dma_read(&q.fence, 0, &mut sched_rptr);
        ops.dma_read(&q.fence, 256, &mut kiq_rptr);
        // KIQ pipe (me=3, pipe=1).
        // HQD register byte-offsets from CP_HQD_ACTIVE @0x1fab (resolved offsets are
        // byte-addressed; (Δdword)*4): CP_HQD_PQ_RPTR @0x1fb3 = +0x20, WPTR_POLL_ADDR_LO
        // @0x1fb6 = +0x2c, WPTR_POLL_ADDR_HI @0x1fb7 = +0x30, CP_HQD_PQ_WPTR_LO @0x1fbc =
        // +0x44. All reads, zero risk.
        const OFF_RPTR: u32 = 0x20;
        const OFF_POLL_LO: u32 = 0x2c;
        const OFF_POLL_HI: u32 = 0x30;
        const OFF_WPTR_LO: u32 = 0x44;
        // KIQ pipe (me=3, pipe=1) — the CONTROL: the KIQ DRAINS, so its HQD_WPTR MUST be
        // non-zero. If it reads 0 too, the WPTR offset is wrong and the SCHED 0 is bogus.
        ops.reg_write(hqd.grbm_gfx_cntl, (3 << 2) | 1);
        let kiq_active = ops.reg_read(hqd.cp_hqd_active);
        let kiq_db = ops.reg_read(hqd.cp_hqd_pq_doorbell_control);
        let kiq_hqd_wptr = ops.reg_read(hqd.cp_hqd_active.wrapping_add(OFF_WPTR_LO));
        let kiq_hqd_rptr = ops.reg_read(hqd.cp_hqd_active.wrapping_add(OFF_RPTR));
        // SCHED pipe (me=3, pipe=0). Read its live wptr/rptr + the wptr-POLL-ADDRESS the
        // MES will poll: if POLL_ADDR != mes_wptr_va (q.fence+16, where we write), the KIQ
        // MAP_QUEUES failed to transfer it from the MQD (packet bug); if POLL_ADDR matches
        // but HQD_WPTR stays 0, the MES isn't SERVICING pipe0 (scheduler/run gate).
        ops.reg_write(hqd.grbm_gfx_cntl, 3 << 2);
        let sched_active = ops.reg_read(hqd.cp_hqd_active);
        let sched_db = ops.reg_read(hqd.cp_hqd_pq_doorbell_control);
        let sched_hqd_wptr = ops.reg_read(hqd.cp_hqd_active.wrapping_add(OFF_WPTR_LO));
        let sched_hqd_rptr = ops.reg_read(hqd.cp_hqd_active.wrapping_add(OFF_RPTR));
        let sched_poll_lo = ops.reg_read(hqd.cp_hqd_active.wrapping_add(OFF_POLL_LO));
        let sched_poll_hi = ops.reg_read(hqd.cp_hqd_active.wrapping_add(OFF_POLL_HI));
        ops.reg_write(hqd.grbm_gfx_cntl, 0);
        ops.log(&format!(
            "[amdgpu] stage 6 KIQ diag (pipe1) CONTROL: ACTIVE={kiq_active:#x}, DOORBELL={kiq_db:#010x} (off 0x60), rptr_report={:#x} (mem), HQD_WPTR={kiq_hqd_wptr:#x} (MUST be >0 — KIQ drains; 0 => WPTR offset wrong), HQD_RPTR={kiq_hqd_rptr:#x}",
            kiq_rptr[0]
        ));
        ops.log(&format!(
            "[amdgpu] stage 6 SCHED diag (pipe0): ACTIVE={sched_active:#x}, DOORBELL={sched_db:#010x} (off 0x58), rptr_report={:#x} (mem), HQD_WPTR={sched_hqd_wptr:#x}, HQD_RPTR={sched_hqd_rptr:#x}, POLL_ADDR={sched_poll_hi:#x}:{sched_poll_lo:#010x} (want mes_wptr_va={:#x}; mismatch => MAP_QUEUES didn't transfer it, match+WPTR0 => MES not servicing pipe0)",
            sched_rptr[0], mes_wptr_va
        ));
    }

    // DIAGNOSTIC: re-read the gfxhub VM registers right after the build to PROVE the
    // writes stuck (vs were dropped by gating). Pre-build they all read 0; if they now
    // read back the enable values (CONTEXT0_CNTL=0x1fffe01, L1_TLB=0x1859, a non-zero
    // PAGE_TABLE_BASE), VMID0 is genuinely programmed and any SDMA failure is downstream
    // (engine kick / VM translation), not a dropped write. Reads only — always safe.
    if let Some(vm) = ops.gmc_vm_regs() {
        ops.log("[amdgpu] stage 6 gfxhub VM readback AFTER GART build (want CONTEXT0_CNTL=0x1fffe01, L1_TLB=0x1859, PAGE_TABLE_BASE!=0):");
        log_gmc_vm_state(ops, &vm);
    }

    // SDMA submit — NOW that GFX is powered (first light) and VMID0 is mapped, the
    // SDMA engine (same power domain on this APU) can drain the ring. The ring/stream/
    // fence were built above; program_sdma_ring re-clears SDMA0_F32_CNTL.HALT + re-rings
    // the queue, and the engine posts the fence on the CONSTANT_FILL's completion =
    // rung-1 command submission proven on iron. The ring/scratch/fence bus addresses
    // resolve through the identity GART just built; gated on discovery (None on QEMU).
    if let Some(sr) = ops.sdma_regs() {
        // HALT + assert BOTH thread resets BEFORE loading the ucode — exactly the
        // sdma_v6_0_enable(false) that PRECEDES sdma_v6_0_load_microcode. The GOP
        // leaves the engine running (HALT=0, TH1 enabled), so the BROADCAST load
        // below would otherwise stream the RS64 instruction RAM out from under a
        // live engine — corrupting the image so it never executes (iron: ring armed,
        // threads enabled, yet RB_RPTR=0). program_sdma_ring then releases the resets
        // and enables both threads to boot the CLEAN image. RMW preserves priorities.
        let f32_pre = ops.reg_read(sr.f32_cntl);
        ops.reg_write(
            sr.f32_cntl,
            f32_pre
                | sdma::SDMA_F32_CNTL_HALT
                | sdma::SDMA_F32_CNTL_TH0_RESET
                | sdma::SDMA_F32_CNTL_TH1_RESET,
        );
        // DIRECT-LOAD the SDMA RS64 microcode FIRST — this Phoenix PSP rejects SDMA via
        // LOAD_IP_FW, so the F32 engine had no firmware and never ran (boots 170257/
        // 183116: RB_RPTR=0 across every kick). Without this, all the ring/doorbell/VM
        // plumbing below has no engine to drive. Gated on the firmware blob being present.
        match ops.request_firmware_bytes("amdgpu/sdma_6_0_1.bin") {
            Some(blob) => {
                let ok = load_sdma_microcode(ops, &sr, &blob);
                ops.log(if ok {
                    "[amdgpu] stage 6 SDMA RS64 ucode DIRECT-loaded (TH0@0 + TH1@0x8000 via BROADCAST_UCODE) — engine now has firmware"
                } else {
                    "[amdgpu] stage 6 SDMA ucode direct-load FAILED — sdma_6_0_1.bin header malformed (engine will stay dead)"
                });
            }
            None => ops.log(
                "[amdgpu] stage 6 SDMA ucode direct-load SKIPPED — sdma_6_0_1.bin bytes unavailable",
            ),
        }
        // The F32 firmware polls the write pointer from a GART-mapped memory dword
        // (Athena umr: RB_CNTL.F32_WPTR_POLL_ENABLE=1) — NOT the RB_WPTR register.
        // Park it in the fence buffer at dword 0x20 (byte 0x80), clear of the fence
        // at dword 0; the identity GART maps it VA==PA so the engine can read it.
        const SDMA_WPTR_POLL_DW: usize = 0x20;
        let done = sdma_submit_and_wait(
            ops,
            &sr,
            gart_va(sdma_ring.dma_addr),
            RING_BYTES as u32,
            sdma_stream.len() as u32,
            &sdma_fence_buf,
            1,
            100_000,
            &sdma_fence_buf,
            SDMA_WPTR_POLL_DW,
            gart_delta,
        );
        ops.log(if done {
            "[amdgpu] stage 6 SDMA fill COMPLETE — F32 polled the WPTR, drained the ring + posted the fence (RUNG 1: command submission works)"
        } else {
            "[amdgpu] stage 6 SDMA submitted via F32 WPTR-poll; fence STILL not posted — see the queue readback below (RB_RPTR>0 => ran but fence wrong; ==0 => engine didn't consume the ring)"
        });
        // DIAGNOSTIC: read the SDMA queue pointers to LOCALIZE a non-posting fence.
        // RB_BASE stuck (== ring>>8) => the SDMA register block is writable (not gated).
        // RB_RPTR advanced past 0 => the engine FETCHED + ran the ring through VMID0 (so
        // the VM works and only the fence write is wrong); RB_RPTR==0 => the engine never
        // consumed the ring (kick/doorbell or VM-fetch fault). amdgpu kicks via a doorbell
        // with RB_WPTR=0; we advance RB_WPTR directly — this readback says whether that
        // register kick is honoured on this autoloaded gfx11 SDMA.
        let rb_base = ops.reg_read(sr.rb_base);
        let rb_rptr = ops.reg_read(sr.rb_rptr);
        let rb_wptr = ops.reg_read(sr.rb_wptr);
        let rb_cntl = ops.reg_read(sr.rb_cntl);
        let f32_cntl = ops.reg_read(sr.f32_cntl);
        let utcl1 = ops.reg_read(sr.utcl1_cntl);
        let mut pollmem = [0u32; 1];
        ops.dma_read(&sdma_fence_buf, SDMA_WPTR_POLL_DW, &mut pollmem);
        // Split across two SHORT lines so neither is truncated by the serial ring
        // (the combined line lost F32_CNTL's last digit on iron). Together these
        // localize an RB_RPTR==0 stall: RB_CNTL.bit0 (RB_ENABLE) + bit11 (F32_WPTR_
        // POLL) say whether the queue is actually armed; F32_CNTL.bit0 (HALT) /
        // 0xffffffff (seg1 gated) say whether the engine is running. RB_RPTR>0 =>
        // the engine fetched + ran the ring (only the fence write would be wrong).
        ops.log(&format!(
            "[amdgpu] stage 6 SDMA readback A: RB_BASE={rb_base:#010x} (want {:#010x}) RB_RPTR={rb_rptr:#010x} RB_WPTR={rb_wptr:#010x}",
            (gart_va(sdma_ring.dma_addr) >> 8) as u32
        ));
        ops.log(&format!(
            "[amdgpu] stage 6 SDMA readback B: RB_CNTL={rb_cntl:#010x} (bit0=RB_ENABLE bit11=F32_WPTR_POLL) F32_CNTL={f32_cntl:#010x} (bit0=HALT, 0xffffffff=gated)"
        ));
        // Readback C — localize the engine-didn't-fetch case: UTCL1_CNTL proves the
        // translation-cache write landed (want low bits 0x609 = RESP_MODE 3 + REDO_DELAY
        // 9); WPTR_poll_mem proves the submit actually wrote the write pointer into the
        // GART-mapped dword the F32 firmware polls (want stream_len<<2). If both are
        // correct yet RB_RPTR==0, the engine is not booting its ucode (kick/boot issue).
        ops.log(&format!(
            "[amdgpu] stage 6 SDMA readback C: UTCL1_CNTL={utcl1:#010x} (want low 0x609) WPTR_poll_mem={:#x} (want {:#x})",
            pollmem[0],
            (sdma_stream.len() as u32) << 2
        ));

        // ── SDMA FILL-VERIFY + LINEAR COPY (owner request 2026-07-01) ──────────────
        // The fence proved the engine RAN; now prove the BYTES moved. Only meaningful
        // once the fill completed (engine confirmed live).
        if done {
            // (1) FILL-VERIFY: the CONSTANT_FILL wrote 0xA5A5A5A5 across sdma_scratch —
            // read it back to prove the fill's data (not just its fence) landed.
            let mut fv = [0u32; 1];
            ops.dma_read(&sdma_scratch, 0, &mut fv);
            ops.log(&format!(
                "[amdgpu] stage 6 SDMA FILL-VERIFY: scratch[0]={:#010x} (want 0xa5a5a5a5 => the CONSTANT_FILL bytes actually landed in memory)",
                fv[0]
            ));
            // (2) LINEAR COPY: write a distinct pattern into src (scratch dword 0), have
            // the SDMA engine copy 256 B src(off 0) -> dst(off 2048), then read dst back.
            // dst held 0xA5A5A5A5 from the fill, so dst==pattern proves REAL data movement
            // by the DMA engine (not the CPU) — the primitive amdgpu_copy_buffer uses.
            const COPY_BYTES: u32 = 256;
            const DST_OFF_BYTES: u64 = 2048;
            const DST_OFF_DW: usize = 512; // 2048 / 4
            const PATTERN: u32 = 0xC0FF_EE00;
            let pat: Vec<u32> = (0..(COPY_BYTES as usize / 4))
                .map(|i| PATTERN | i as u32)
                .collect();
            ops.dma_write(&sdma_scratch, 0, &pat);
            let src_va = gart_va(sdma_scratch.dma_addr);
            let dst_va = gart_va(sdma_scratch.dma_addr + DST_OFF_BYTES);
            let copy_stream = sdma::linear_copy_with_fence(
                src_va,
                dst_va,
                COPY_BYTES,
                gart_va(sdma_fence_buf.dma_addr),
                2, // fence value 2 (the fill left dword0 = 1)
            );
            ops.dma_write(&sdma_ring, 0, &copy_stream);
            let copied = sdma_submit_and_wait(
                ops,
                &sr,
                gart_va(sdma_ring.dma_addr),
                RING_BYTES as u32,
                copy_stream.len() as u32,
                &sdma_fence_buf,
                2,
                100_000,
                &sdma_fence_buf,
                SDMA_WPTR_POLL_DW,
                gart_delta,
            );
            let mut dv = [0u32; 2];
            ops.dma_read(&sdma_scratch, DST_OFF_DW, &mut dv);
            let data_ok = dv[0] == PATTERN && dv[1] == (PATTERN | 1);
            ops.log(&format!(
                "[amdgpu] stage 6 SDMA LINEAR-COPY {}: fence_posted={copied} dst[0]={:#010x} dst[1]={:#010x} (want {:#010x},{:#010x}) — {}",
                if copied && data_ok { "COMPLETE" } else { "FAILED" },
                dv[0],
                dv[1],
                PATTERN,
                PATTERN | 1,
                if copied && data_ok {
                    "the SDMA engine moved real data src->dst on bare metal (RUNG 1: DMA copy works)"
                } else {
                    "copy fence or data mismatch — see values"
                }
            ));
        }
    }

    // gfx_v11_0_cp_gfx_resume: CP_RB0_BASE is the ring address in 256-BYTE units
    // (>>8). `ring_addr` is the gfx ring's bus address, which the identity VMID0 GART
    // (init_gart_identity, above) maps VA==PA — so the CP fetches the ring at that
    // same address through VMID0. CNTL packs RB_BUFSZ[5:0] | RB_BLKSZ[13:8](=bufsz-2).
    // The completion regs (VMID,
    // RPTR/WPTR writeback, and the load-bearing CP_RB_ACTIVE=1) only fire when
    // discovery resolved them (gated).
    // gfx_v11_0_cp_gfx_resume — the AUTHORITATIVE amdgpu order (verified vs the kernel
    // source 2026-06-27). The CP is already UNHALTED (above), per amdgpu. Sequence:
    //   WPTR_DELAY=0 -> VMID=0 -> CNTL -> WPTR=0 -> RPTR_ADDR -> WPTR_POLL_ADDR ->
    //   mdelay(1) -> CNTL AGAIN -> BASE -> RB_ACTIVE -> doorbell.
    // The DOUBLE CP_RB0_CNTL write with the mdelay between is LOAD-BEARING: the
    // running CP latches the ring on the 2nd write. Our old code wrote CNTL once and
    // the PFP stayed stuck (RPTR=0). We also no longer write CP_RB0_RPTR (amdgpu
    // doesn't — the CP owns it) and give rptr/wptr SEPARATE writeback dwords.
    let rb_addr = ring_addr >> 8;
    let bufsz = log2_size & 0x3f;
    let cntl_val = bufsz | (bufsz.saturating_sub(2) << 8);
    let rr = ops.cp_gfx_ring_regs();
    // CP_RB_WPTR_DELAY (seg0 0x0f61) = CP_RB0_RPTR (0x0f60) + 1 dword.
    ops.reg_write(r_rptr.wrapping_add(4), 0);
    if let Some(rr) = rr {
        ops.reg_write(rr.rb_vmid, 0);
    }
    ops.reg_write(r_cntl, cntl_val);
    ops.reg_write(r_wptr, 0); // init WPTR=0 (empty ring; clears the GOP's stale WPTR)
    if let Some(rr) = rr {
        // RPTR writeback dword (gfx + RING_BYTES - 8), as a GART VA (the CP writes
        // it through VMID0 — same aperture as the ring base).
        let rptr_wb = gart_va(gfx.dma_addr.wrapping_add(RING_BYTES as u64).wrapping_sub(8));
        ops.reg_write(rr.rb0_rptr_addr, (rptr_wb & 0xFFFF_FFFF) as u32);
        ops.reg_write(rr.rb0_rptr_addr_hi, (rptr_wb >> 32) as u32);
        // WPTR-poll dword — a SEPARATE dword (gfx + RING_BYTES - 16); amdgpu uses
        // distinct rptr_gpu_addr / wptr_gpu_addr (ours collided on one dword).
        let wptr_wb = gart_va(
            gfx.dma_addr
                .wrapping_add(RING_BYTES as u64)
                .wrapping_sub(16),
        );
        ops.reg_write(rr.rb_wptr_poll_addr_lo, (wptr_wb & 0xFFFF_FFFF) as u32);
        ops.reg_write(rr.rb_wptr_poll_addr_hi, (wptr_wb >> 32) as u32);
    }
    ops.delay_us(1000); // mdelay(1) — let the ring settle before the 2nd CNTL write
    ops.reg_write(r_cntl, cntl_val); // CP_RB0_CNTL written AGAIN (the load-bearing write)
    ops.reg_write(r_base, (rb_addr & 0xFFFF_FFFF) as u32);
    ops.reg_write(r_base_hi, (rb_addr >> 32) as u32);
    if let Some(rr) = rr {
        ops.reg_write(rr.rb_active, 1); // ACTIVATE the ring
                                        // gfx_v11_0_cp_gfx_start CP-init: MAX_CONTEXT = max_hw_contexts-1 (=7 on
                                        // gfx11) + DEVICE_ID = 1. DEVICE_ID is the kick that makes the loaded,
                                        // unhalted CP actually begin processing — without it the PFP stays busy
                                        // but never fetches (RPTR=0). amdgpu writes these before the first submit.
        const CP_MAX_CONTEXT_GFX11: u32 = 8 - 1;
        ops.reg_write(rr.max_context, CP_MAX_CONTEXT_GFX11);
        ops.reg_write(rr.device_id, 1);
        // gfx_v11_0_cp_gfx_set_doorbell — calibrated to the LIVE working amdgpu
        // (umr 2026-06-27): CP_RB_DOORBELL_CONTROL reads 0xc0000458 = DOORBELL_OFFSET
        // field[26:2] (raw bits 0x458 => doorbell_index 0x116) | DOORBELL_EN(bit30)
        // | the runtime DOORBELL_HIT(bit31) status. amdgpu WRITES only OFFSET|EN
        // (= 0x40000458); bit31 is hardware status, not written. RANGE_LOWER=0x458,
        // RANGE_UPPER=0x7f8. Our old value (EN only, offset 0, ring at byte 0) meant
        // the CP — which reads WPTR from the doorbell once EN is set — monitored a
        // slot we never wrote, so it never saw our WPTR and RPTR stayed 0. The
        // gfx_ring0 doorbell BYTE offset = index 0x116 * 4 = 0x458.
        const CP_GFX_RING0_DOORBELL_OFF: u32 = 0x458;
        const CP_DOORBELL_EN: u32 = 1 << 30;
        ops.reg_write(
            rr.doorbell_control,
            CP_GFX_RING0_DOORBELL_OFF | CP_DOORBELL_EN,
        );
        ops.reg_write(rr.doorbell_range_lower, 0x458);
        ops.reg_write(rr.doorbell_range_upper, 0x7f8);
        // Zero the RPTR writeback so a post-kick non-zero read is the CP's own write.
        ops.dma_write(&gfx, RING_BYTES / 4 - 2, &[0]);
        // SUBMIT: advance WPTR (the CP reads it from the doorbell once EN is set) and
        // RING the gfx_ring0 doorbell at byte 0x458 — the slot the CP now monitors.
        // value = dword write-pointer (stream length).
        ops.reg_write(r_wptr, stream.len() as u32);
        ops.ring_doorbell(CP_GFX_RING0_DOORBELL_OFF, stream.len() as u64);
    }

    // Verify the ring base + wptr read back (catches a wrong-offset register map
    // OR a gated GFX block that silently drops writes). Log the WRITTEN-vs-READ
    // values per register so a bootlog disambiguates the cause off-target.
    let want_base_lo = (rb_addr & 0xFFFF_FFFF) as u32;
    let want_base_hi = (rb_addr >> 32) as u32;
    let want_wptr = stream.len() as u32;
    let base_lo = ops.reg_read(r_base);
    let base_hi = ops.reg_read(r_base_hi);
    let cntl = ops.reg_read(r_cntl);
    let wptr = ops.reg_read(r_wptr);
    let active = rr.map(|r| ops.reg_read(r.rb_active));
    let base_ok = base_lo == want_base_lo && base_hi == want_base_hi;
    let wptr_ok = wptr == want_wptr;

    // THE decisive proof of GFX command execution (post-first-light): poll
    // CP_RB0_RPTR after the WPTR kick. The CP fetches packets from RB_BASE
    // (RPTR..WPTR) through VMID0 and advances RPTR as it consumes them. RPTR
    // advancing toward WPTR means the GFX command processor RAN our ring — the
    // first GPU-executed graphics command on RaeenOS. A short, dedicated line (the
    // combined readback above truncates at the 160-byte klog buffer, hiding this).
    let mut exec_rptr = ops.reg_read(r_rptr);
    for _ in 0..200 {
        if exec_rptr >= want_wptr && want_wptr != 0 {
            break;
        }
        ops.delay_us(1000);
        exec_rptr = ops.reg_read(r_rptr);
    }
    let cp_ran = want_wptr != 0 && exec_rptr >= want_wptr;
    ops.log(&format!(
        "[amdgpu] stage 6 CP EXEC: wrote WPTR={want_wptr} RPTR={exec_rptr:#x} -> CP {} (GFX {})",
        if cp_ran {
            "DRAINED the ring"
        } else {
            "RPTR did not reach WPTR"
        },
        if cp_ran {
            "EXECUTES COMMANDS — first GPU command!"
        } else {
            "ring not consumed yet"
        }
    ));
    // Localize WHY the CP didn't drain (working driver: CP_STAT=0 idle, CP_ME_CNTL=0).
    // CP_STAT is seg0 0x0f40 = 0x80 bytes below CP_RB0_RPTR (0x0f60) — anchor off
    // r_rptr. CP_ME_CNTL re-read: still 0 => GFX awake + CP just not fetching (core
    // issue); 0xffffffff/non-0 => GFXOFF re-gated the domain after the unhalt.
    let cp_stat = ops.reg_read(r_rptr.wrapping_sub(0x80));
    let me_cntl_now = g.map(|x| ops.reg_read(x.cp_me_cntl));
    ops.log(&format!(
        "[amdgpu] stage 6 CP STATE: CP_STAT={cp_stat:#010x} ({}) CP_ME_CNTL_now={me_cntl_now:#x?} (working: CP_STAT=0 idle, CP_ME_CNTL=0; 0xffffffff => GFXOFF re-gated)",
        if cp_stat == 0 {
            "idle — not fetching"
        } else if cp_stat == 0xFFFF_FFFF {
            "GATED (0xffffffff)"
        } else {
            "busy"
        }
    ));
    // DECISIVE: the CP reports its read pointer to the WRITEBACK memory
    // (rb0_rptr_addr = gfx + RING_BYTES - 8), which amdgpu reads instead of the
    // CP_RB0_RPTR REGISTER (the register often does NOT update). So the CP may have
    // DRAINED our ring (executed the command) while the register-RPTR probe above
    // still reads 0. Read the wb dword: if it advanced past 0, the GPU ran our
    // first command. (We zero it before the kick so any non-zero is the CP's write.)
    let mut wb_rptr = [0u32; 1];
    ops.dma_read(&gfx, RING_BYTES / 4 - 2, &mut wb_rptr);
    ops.log(&format!(
        "[amdgpu] stage 6 CP EXEC (writeback): wb_RPTR={:#x} -> CP {} (the register-RPTR may stay 0 even when the CP ran)",
        wb_rptr[0],
        if wb_rptr[0] != 0 {
            "DRAINED the ring — FIRST GPU COMMAND EXECUTED"
        } else {
            "did not write the RPTR writeback"
        }
    ));

    // Always exit safe mode before returning (let the RLC resume GFXOFF mgmt).
    rlc_exit_safe_mode(ops);

    if !base_ok || !wptr_ok {
        ops.log(&format!(
            "[amdgpu] stage 6 CP ring readback MISMATCH: BASE wrote {want_base_hi:#010x}:{want_base_lo:#010x} read {base_hi:#010x}:{base_lo:#010x} | CNTL wrote {cntl_val:#x} read {cntl:#010x} | WPTR wrote {want_wptr} read {wptr:#x} | RB_ACTIVE {active:?}"
        ));
        // Read it like this on the next iron flash:
        //  * readbacks == the firmware/GOP's pre-existing values (unchanged) ⇒
        //    writes were IGNORED ⇒ GFX is clock-gated / GFXOFF ⇒ the RLC/SMU
        //    ungate above must be wired with confirmed offsets (it's a no-op
        //    until then) — that's the candidate fix;
        //  * readbacks are garbage ⇒ the CP_RB0_* offsets are wrong for gfx11
        //    (these are GCN-era — gfx11 reworked the CP register map);
        //  * partial match ⇒ a specific offset is wrong.
        ops.log(
            "[amdgpu] stage 6 hint: if reads == GOP values, GFX is gated (wire RLC/SMU ungate offsets); if garbage, gfx11 CP offsets wrong",
        );
        return false;
    }
    ops.log(&format!(
        "[amdgpu] stage 6 GFX ring: {} PM4 dwords, CP_RB0 base+cntl+wptr programmed + verified (RB_ACTIVE {active:?})",
        stream.len()
    ));
    // INHERIT-GART check: confirm the started CP can actually reach this ring
    // through the firmware's existing system aperture (no GART build needed). If
    // the fence below posts, this verdict + config_gfx_rs64 + the activated ring
    // are why; if it doesn't, an OUT-OF-APERTURE verdict says we need the §4.1 build.
    if let Some(vm) = ops.gmc_vm_regs() {
        gfx_ring_reachable_via_aperture(ops, &vm, gart_va(gfx.dma_addr));
    }
    // Poll the RELEASE_MEM fence the CP posts when it drains the PM4 stream — the
    // GFX-engine twin of the SDMA fence-poll. The WPTR write above rang the
    // doorbell; this confirms the CP actually EXECUTED the submitted stream on
    // iron (vs being merely programmed). Bounded + non-fatal: a timeout off-iron
    // is expected (no engine), but on iron it means a wedged CP.
    let mut got = [0u32; 1];
    let mut cp_done = false;
    for _ in 0..100_000 {
        ops.dma_read(&cp_fence_buf, 0, &mut got);
        if got[0] == 1 {
            cp_done = true;
            break;
        }
    }
    ops.log(if cp_done {
        "[amdgpu] stage 6 GFX CP executed — RELEASE_MEM fence posted"
    } else {
        "[amdgpu] stage 6 GFX CP submitted; fence not posted (no engine off-iron, or wedged)"
    });
    true
}

/// The submit -> complete loop at the heart of GPU usage: write a PM4 `stream`
/// (whose tail is a `RELEASE_MEM` that writes `fence_value` to `fence`) into the
/// GFX ring, ring the doorbell by advancing WPTR (`wptr_reg`), then poll the
/// fence memory until the GPU posts `fence_value`. Returns true on completion,
/// false on timeout — a wedged GPU that never signals must surface, not hang.
pub fn submit_and_wait_fence<O: GpuOps>(
    ops: &mut O,
    gfx_ring: &DmaBuf,
    wptr_reg: u32,
    fence: &DmaBuf,
    fence_value: u32,
    stream: &[u32],
    max_polls: u32,
) -> bool {
    ops.dma_write(gfx_ring, 0, stream);
    ops.reg_write(wptr_reg, stream.len() as u32); // doorbell: advance the CP WPTR
    let mut got = [0u32; 1];
    for _ in 0..max_polls {
        ops.dma_read(fence, 0, &mut got);
        if got[0] == fence_value {
            return true;
        }
    }
    false
}

/// READ-ONLY DCN scanout probe — the native display path's FIRST CONTACT, and the one
/// step that must run BEFORE the GFX bring-up. The DCN is a SEPARATE power domain the
/// firmware already lit for the GOP framebuffer, so these registers read LIVE on a WARM
/// boot — whereas the GFX power-up (init_rings) WEDGES on a warm GPU and would never let
/// a later display stage run. A sane OTG0 H_TOTAL (~w+blanking, not 0 / 0xffffffff)
/// proves the panel timing is active and that the HUBP0 primary surface address read
/// back is the firmware's CURRENT scanout buffer — the value the next step will REPLACE
/// to point the panel at an amdgpu buffer. Read-only: cannot blank the display. Gated on
/// discovery (QEMU has no DMU block → skipped).
pub fn probe_dcn_scanout<O: GpuOps>(ops: &mut O) {
    if let Some(dcn) = ops.dcn_scanout_regs() {
        let h_total = ops.reg_read(dcn.otg_h_total);
        let surf_lo = ops.reg_read(dcn.primary_surface_addr_lo);
        let surf_hi = ops.reg_read(dcn.primary_surface_addr_hi);
        let cfg = ops.reg_read(dcn.surface_config);
        let live = h_total != 0 && h_total != 0xffff_ffff;
        ops.log(&format!(
            "[amdgpu] DCN PROBE: OTG0_H_TOTAL={h_total:#x} (display {}); HUBP0 surface=0x{surf_hi:x}_{surf_lo:08x} config={cfg:#x}",
            if live { "ACTIVE — DCN reachable on this (warm) boot, page-flip path open" } else { "INACTIVE/gated" }
        ));
    } else {
        ops.log("[amdgpu] DCN PROBE skipped (DMU block unresolved — QEMU or no discovery)");
    }
}

/// PAGE FLIP — the "amdgpu displays graphics" proof, warm-testable. Fills a scratch VRAM
/// region with a solid color via the proven MM_INDEX `vram_write`, then writes HUBP0's
/// primary surface address to point the DCN at that buffer. If the DCN latches the new
/// address on the next vblank, the PANEL turns that color — i.e. amdgpu is driving the
/// scanout, not the firmware GOP. Runs on a WARM boot (the DCN is firmware-lit). The
/// register write is netlog-verifiable (readback); the panel color needs an eye on the
/// monitor. The scratch buffer is real VRAM we just filled, so a wrong aperture guess
/// shows garbage (recoverable on the auto-return), not a fault. Gated on discovery.
pub fn try_page_flip<O: GpuOps>(ops: &mut O) {
    let Some(dcn) = ops.dcn_scanout_regs() else {
        ops.log("[amdgpu] PAGE-FLIP skipped (DMU block unresolved)");
        return;
    };
    const SCRATCH_OFF: u64 = 0x0600_0000; // 96 MiB in (the proven-safe self-test region)
                                          // STEP 1 (netlog-provable, FAST): point HUBP0 at the scratch buffer and read it back.
                                          // This is the decisive proof that amdgpu can WRITE the DCN surface register — it lands
                                          // in the log regardless of the (slow, indirect) VRAM fill below. The firmware surface
                                          // (hi=0x80, lo=0x0) is the VRAM aperture base, so scratch's DCN address is
                                          // 0x80_00000000 + SCRATCH_OFF. Scratch is valid VRAM, so a wrong-aperture guess shows
                                          // garbage, never a fault. (The address may be double-buffered/latched on vblank, so a
                                          // readback that differs is not necessarily a failed write.)
                                          // CRC VERIFICATION SETUP (eyes-free display proof). The OTG computes a hardware CRC of
                                          // the scanned-out frame within a window. Read it BEFORE the flip (the live console) and
                                          // AFTER (our static buffer): a stable AFTER value that DIFFERS from BEFORE = the panel
                                          // content changed = amdgpu's flip reached the glass. CRC regs are fixed dword offsets
                                          // from OTG0_H_TOTAL (0x1b2a): CNTL +0x3e, WINDOWA_X +0x3f, WINDOWA_Y +0x40, DATA_RG
                                          // +0x43, DATA_B +0x44 (×4 for byte deltas). Window = a top box (x 256..1280, y 64..512).
    let crc_cntl = dcn.otg_h_total + 0x3e * 4;
    let crc_win_x = dcn.otg_h_total + 0x3f * 4;
    let crc_win_y = dcn.otg_h_total + 0x40 * 4;
    let crc_rg = dcn.otg_h_total + 0x43 * 4;
    let crc_b = dcn.otg_h_total + 0x44 * 4;
    ops.reg_write(crc_win_x, (1280u32 << 16) | 256); // X_END<<16 | X_START
    ops.reg_write(crc_win_y, (512u32 << 16) | 64); // Y_END<<16 | Y_START
    ops.reg_write(crc_cntl, (1u32 << 4) | 1); // OTG_CRC_CONT_EN | OTG_CRC_EN, SELECT=0
    ops.delay_us(80_000); // a few frames for the CRC to compute over the console
    let before_rg = ops.reg_read(crc_rg);
    let before_b = ops.reg_read(crc_b);
    ops.delay_us(40_000);
    let before_rg2 = ops.reg_read(crc_rg);
    ops.log(&format!(
        "[amdgpu] PAGE-FLIP CRC before-flip: RG=0x{before_rg:08x}(again 0x{before_rg2:08x}) B=0x{before_b:08x} [console, stable={}]",
        before_rg == before_rg2
    ));
    let new_hi = 0x80u32;
    let new_lo = SCRATCH_OFF as u32; // 0x06000000
    ops.reg_write(dcn.primary_surface_addr_hi, new_hi);
    ops.reg_write(dcn.primary_surface_addr_lo, new_lo);
    let rb_lo = ops.reg_read(dcn.primary_surface_addr_lo);
    let rb_hi = ops.reg_read(dcn.primary_surface_addr_hi);
    ops.log(&format!(
        "[amdgpu] PAGE-FLIP: HUBP0 surface <- 0x{new_hi:x}_{new_lo:08x}; readback=0x{rb_hi:x}_{rb_lo:08x} (write {})",
        if rb_lo == new_lo && rb_hi == new_hi {
            "STUCK — amdgpu controls the DCN scanout register"
        } else {
            "differs — likely double-buffered (latches on vblank)"
        }
    ));
    // CRC AFTER the flip: read the OTG checksum again. The DCN should now scan our static
    // scratch buffer, so a STABLE value DIFFERENT from before = the panel content CHANGED =
    // amdgpu's flip reached the glass (eyes-free). Runs BEFORE the slow fill so it lands.
    ops.delay_us(80_000);
    let after_rg = ops.reg_read(crc_rg);
    let after_b = ops.reg_read(crc_b);
    ops.delay_us(40_000);
    let after_rg2 = ops.reg_read(crc_rg);
    let stable = after_rg == after_rg2;
    let changed = after_rg != before_rg || after_b != before_b;
    ops.log(&format!(
        "[amdgpu] PAGE-FLIP CRC after-flip: RG=0x{after_rg:08x}(again 0x{after_rg2:08x}) B=0x{after_b:08x} => {}",
        if stable && changed {
            "STABLE + CHANGED vs before => DISPLAY VERIFIED — amdgpu's buffer is on the panel"
        } else if !changed {
            "UNCHANGED vs before — the flip did not reach the panel"
        } else {
            "varying — inconclusive (console still live, or CRC source/window wrong)"
        }
    ));
    // HUBP0 INUSE=0 suggests it is IDLE (not the pipe driving the panel). Read INUSE +
    // PRIMARY across all 4 HUBP instances (stride 0xDC dwords = 0x370 bytes) to find the
    // ACTIVE scanout pipe, so a follow-up flip targets the right one. Read-only diagnostic.
    const HUBP_STRIDE_BYTES: u32 = 0x370;
    for i in 0..4u32 {
        let lo = ops.reg_read(dcn.surface_inuse_lo + i * HUBP_STRIDE_BYTES);
        let hi = ops.reg_read(dcn.surface_inuse_hi + i * HUBP_STRIDE_BYTES);
        let pri_lo = ops.reg_read(dcn.primary_surface_addr_lo + i * HUBP_STRIDE_BYTES);
        ops.log(&format!(
            "[amdgpu] PAGE-FLIP scan: HUBP{i} INUSE=0x{hi:x}_{lo:08x} PRIMARY_lo=0x{pri_lo:08x}{}",
            if lo != 0 || hi != 0 {
                " <- ACTIVE scanout pipe"
            } else {
                ""
            }
        ));
    }
    // STEP 2 (visual, best-effort): fill the scratch buffer with magenta. The indirect
    // MM_INDEX path is ~2 MMIO writes/dword, so keep it to 1 MiB (a wide top band) — the
    // earlier 16 MiB fill ran past the boot window. A magenta band = amdgpu drove the pixels.
    const COLOR: u32 = 0x00FF_00FF; // magenta, XRGB8888 (0x00RRGGBB) — distinctive
                                    // 8 MiB covers a full 1080p XRGB8888 frame (and most of 1440p) — a near-full-screen
                                    // magenta so the owner's glance is an unambiguous yes/no. The 1 MiB version completed
                                    // cleanly on iron, so this scales the same proven path; if it ever wedges, the
                                    // surface-reg proof above already logged. Progress logged each 2 MiB.
                                    // Display is already VERIFIED (CRC + owner's eyes saw magenta), so keep the fill TINY.
                                    // The 8 MiB fill ate the ENTIRE boot window via the slow MM_INDEX path and starved
                                    // init_rings (the GFX/games bring-up) — it never ran on the cold boot. A 256 KiB fill
                                    // (~seconds) leaves the rest of the boot for the GFX path.
    const FILL_DWORDS: usize = 64 * 1024; // 256 KiB
    ops.log("[amdgpu] PAGE-FLIP: filling 256 KiB scratch with magenta (kept small so init_rings runs)...");
    let chunk = alloc::vec![COLOR; 32 * 1024];
    let mut done = 0usize;
    while done < FILL_DWORDS {
        let n = (FILL_DWORDS - done).min(chunk.len());
        ops.vram_write(SCRATCH_OFF + (done as u64) * 4, &chunk[..n]);
        done += n;
    }
    ops.log("[amdgpu] PAGE-FLIP: fill done");
}

/// Stage 7 — `amdgpu_dm_init`: pick a mode, allocate the scanout FB, commit.
pub fn init_display<O: GpuOps>(ops: &mut O, dev: &Device) -> bool {
    let (w, h) = (1920u32, 1080u32);
    let ok = ops.commit_scanout(w, h, w * 4, dev.vram_base);
    if ok {
        ops.log("[amdgpu] stage 7 DC modeset committed");
    } else {
        ops.log("[amdgpu] stage 7 DC modeset FAILED");
    }
    ok
}

/// Run the whole `amdgpu_device_init` sequence over `ops`. Probes `bdfs` in
/// order; if no AMD GPU is found, returns a report with `device_present=false`
/// (the expected QEMU outcome — not an error).
/// Read-only register-identity probe — the safe FIRST CONTACT with a real GPU.
/// Reads a handful of CONFIRMED GFX11 status/ring registers (`gc11` offsets)
/// through the BAR5 register aperture and logs them. This proves the daemon can
/// talk to the GPU's registers on iron — the foundational milestone before any
/// register WRITE — and dumps the firmware-initialized state (is GFX busy? is
/// the CP ring already programmed by the UEFI GOP driver?) as ground truth for
/// the offsets the later stages still need. BAR5 register reads are the normal,
/// safe access space; unlike the BAR0 VRAM aperture, they don't hard-hang the
/// fabric before GMC is up. NEVER writes — so a wrong offset reads garbage, it
/// can't corrupt GPU state.
pub fn probe_registers<O: GpuOps>(ops: &mut O) {
    let grbm = ops.reg_read(gc11::MM_GRBM_STATUS);
    let gui_active = grbm & gc11::GRBM_STATUS_GUI_ACTIVE != 0;
    let rb_cntl = ops.reg_read(gc11::MM_CP_RB0_CNTL);
    let rb_base = ops.reg_read(gc11::MM_CP_RB0_BASE);
    let rb_base_hi = ops.reg_read(gc11::MM_CP_RB0_BASE_HI);
    let rb_rptr = ops.reg_read(gc11::MM_CP_RB0_RPTR);
    let rb_wptr = ops.reg_read(gc11::MM_CP_RB0_WPTR);
    let me_cntl = ops.reg_read(gc11::MM_CP_ME_CNTL);
    ops.log(&format!(
        "[amdgpu] reg probe (BAR5): GRBM_STATUS={grbm:#010x} gui_active={gui_active} CP_RB0_CNTL={rb_cntl:#010x} CP_RB0_BASE={rb_base_hi:#010x}:{rb_base:#010x} RPTR={rb_rptr:#x} WPTR={rb_wptr:#x} CP_ME_CNTL={me_cntl:#010x}"
    ));
    // Aperture-liveness is decided across ALL probed registers, not GRBM_STATUS
    // alone: an idle GFX block legitimately reads GRBM_STATUS=0 (GUI_ACTIVE clear,
    // no error bits), so GRBM_STATUS==0 is NOT proof of a dead aperture. If ANY
    // register reads a non-zero, non-all-ones value, the BAR5 aperture is
    // responding. On Athena the UEFI GOP leaves the CP ring programmed
    // (CP_RB0_WPTR/BASE_HI non-zero), which is exactly that proof.
    let regs = [
        grbm, rb_cntl, rb_base, rb_base_hi, rb_rptr, rb_wptr, me_cntl,
    ];
    let live = regs.iter().any(|&r| r != 0 && r != 0xFFFF_FFFF);
    if live {
        ops.log(&format!(
            "[amdgpu] reg probe: register aperture LIVE (GFX {}; GOP left CP ring {}programmed)",
            if gui_active { "BUSY" } else { "idle" },
            if rb_base | rb_base_hi != 0 { "" } else { "un" }
        ));
    } else {
        ops.log("[amdgpu] reg probe: ALL registers 0x0/0xFFFFFFFF — BAR5 aperture not responding (bad BAR map / wrong offsets)");
    }

    // DWORD-vs-BYTE addressing test (read-only, safe). The CP_RB0 offsets
    // (`0x3040` & `0x3041` only 1 apart) MUST be DWORD indices, but `reg_read`
    // uses a byte pointer (`mmio.add(off)`), so CP regs are read at byte `off`
    // instead of byte `off*4` — the suspected stage-6 mismatch root cause. Read
    // each key register at BOTH `off` and `off<<2`: the interpretation that hits
    // the live register (the GOP left the ring programmed) is the correct one.
    // GRBM_STATUS=0x8010 already looks byte-correct, so the units may be mixed.
    for (name, off) in [
        ("GRBM_STATUS", gc11::MM_GRBM_STATUS),
        ("CP_RB0_BASE", gc11::MM_CP_RB0_BASE),
        ("CP_RB0_CNTL", gc11::MM_CP_RB0_CNTL),
        ("CP_RB0_WPTR", gc11::MM_CP_RB0_WPTR),
    ] {
        let as_byte = ops.reg_read(off);
        let as_dword = ops.reg_read(off << 2);
        ops.log(&format!(
            "[amdgpu] reg-addr-test {name}: byte[{off:#x}]={as_byte:#010x}  dword[{:#x}]={as_dword:#010x}",
            off << 2
        ));
    }
}

/// Resolve IP-discovery blocks from the firmware file `amdgpu/ip_discovery.bin`
/// — amdgpu's `amdgpu_discovery_read_binary_from_file` path, and the SAFE
/// alternative to reading the discovery blob from VRAM (the `MM_INDEX` read that
/// has wedged CPU 0 on every Athena boot so far). The blob is silicon-identical
/// for every Phoenix1 (`1002:15bf`), so a vendored copy resolves every SOC15
/// register offset with NO MMIO VRAM access at all.
///
/// `None` if the file is absent or fails the signature/parse gate
/// ([`discovery::parse_checked`]) — the caller then falls back to the VRAM read,
/// exactly as before, so this is purely additive. To populate the file: capture
/// the blob once from a Linux box running on any 780M (sysfs
/// `/sys/kernel/debug/dri/N/amdgpu_discovery`, or `umr`), or from one Athena boot
/// that persists the VRAM read, and vendor it at `firmware/amdgpu/ip_discovery.bin`.
pub fn discovery_from_firmware<O: GpuOps>(ops: &mut O) -> Option<Vec<discovery::IpBlock>> {
    let blob = ops.request_firmware_bytes("amdgpu/ip_discovery.bin")?;
    match discovery::parse_checked(&blob) {
        Some(blocks) => {
            ops.log(&format!(
                "[amdgpu] IP discovery from firmware file: {} blocks — SOC15 offsets ACTIVE (no VRAM read)",
                blocks.len()
            ));
            Some(blocks)
        }
        None => {
            ops.log("[amdgpu] ip_discovery.bin present but signature/parse invalid — falling back to VRAM read");
            None
        }
    }
}

pub fn bringup<O: GpuOps>(ops: &mut O, bdfs: &[(u8, u8, u8)]) -> BringupReport {
    let mut report = BringupReport::default();
    let Some(mut dev) = probe(ops, bdfs) else {
        ops.log("[amdgpu] no AMD GPU found at any probe BDF (expected on QEMU)");
        return report;
    };
    report.device_present = true;
    // Safe read-only first contact: prove we can talk to the register aperture
    // and dump the firmware-initialized state before any stage writes a reg.
    probe_registers(ops);
    report.vbios = read_vbios(ops, &mut dev);
    report.gmc = init_gmc(ops, &mut dev);
    report.vram_mib = (dev.vram_size / (1024 * 1024)) as u32;
    report.ih = init_ih(ops, &dev);
    report.smu = init_smu(ops, &dev);
    // PSP path increment 1 (read-only): is the PSP secure-OS reachable? On a PSP-load
    // APU the PSP is the ONLY thing that can cold-start GFX (boot 041507), so prove
    // the MP0 mailbox channel before building the ring/TMR/firmware-load on top of it.
    psp_sign_of_life(ops);
    report.rings = init_rings(ops, &dev);
    report.display = init_display(ops, &dev);
    if report.all_ok() {
        ops.log(&format!(
            "[amdgpu] amdgpu_device_init complete — GPU online ({:04x}:{:04x})",
            dev.vendor, dev.device
        ));
    }
    report
}

// ── In-crate host test: a minimal mock GpuOps that proves the sequence ────────
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    /// Minimal mock: a register map, present-firmware set, and a synthetic ROM.
    struct MockOps {
        regs: BTreeMap<u32, u32>,
        present_fw: bool,
        rom: Option<Vec<u8>>,
        next_dma: u64,
        scanout_committed: bool,
        device_id: u16,
        memsize_mb: Option<u32>,
        /// Number of `GRBM_STATUS` reads that still report GUI_ACTIVE (busy)
        /// before the pipe models idle — drives the wait-for-idle handshake.
        gfx_busy_reads: u32,
        /// SMU mailbox offsets the mock honors; `Some` enables the PMFW model.
        smu_mailbox: Option<SmuMailbox>,
        /// Whether the modeled PMFW answers a message (false models a hung SMU).
        smu_responds: bool,
        /// GPU-visible DMA memory, keyed by absolute address (dma_addr + byte off).
        dma_mem: BTreeMap<u64, u32>,
        /// `(wptr_reg, fence_addr, fence_value)`: when the doorbell (`wptr_reg`)
        /// is written, the modeled GPU posts `fence_value` to `fence_addr`.
        gpu_completes_fence: Option<(u32, u64, u32)>,
        /// Records the last `ring_doorbell(byte_offset, value)` so tests can assert
        /// the SDMA queue was woken via the doorbell aperture.
        last_doorbell: Option<(u32, u64)>,
        /// RLC_SAFE_MODE offset the mock honors; `Some` models a working RLC that
        /// clears CMD on entry (so `rlc_enter_safe_mode` succeeds).
        rlc_safe_mode: Option<RlcSafeMode>,
        /// A synthetic `amdgpu/ip_discovery.bin` returned by `request_firmware_bytes`;
        /// `None` models the file being absent (the default on iron today).
        fw_discovery: Option<Vec<u8>>,
        /// The CP_ME_CNTL halt mask the mock honors; `Some` enables the CP
        /// enable/halt path (models the confirmed gfx11 RS64 halt bits).
        cp_me_cntl_halt_mask: Option<u32>,
        /// CP gfx-ring completion regs; `Some` makes stage 6 program + ACTIVATE
        /// the ring (write `CP_RB_ACTIVE = 1`).
        cp_gfx_ring_regs: Option<CpGfxRingRegs>,
        /// gfxhub GART-build regs; `Some` makes init_gart build + apply GART.
        gfxhub_gart_regs: Option<crate::gart::GfxhubGartRegs>,
        /// GPU VRAM, keyed by byte offset (BAR0 window). Backs vram_write/read/zero.
        vram: BTreeMap<u64, u32>,
        /// Reacting-PSP model for `psp_submit_gpcom`: when `Some(ring_wptr_reg)`, a
        /// write to that reg makes the mock read the just-written ring frame, post the
        /// fence, and write a success response (status 0 + `psp_tmr_reply`) into the
        /// command buffer — exactly what the real sOS does for a GPCOM command.
        psp_wptr_reg: Option<u32>,
        psp_vram_base: u64,
        psp_ring_size_dw: u32,
        psp_tmr_reply: u32,
        /// Gated firmware-staging inputs for `psp_load_gfx_firmware`.
        vram_mc_base_val: Option<u64>,
        psp_toc_val: Option<(u64, u32)>,
        psp_tmr_val: Option<u64>,
    }
    impl MockOps {
        fn new(device_id: u16, present_fw: bool, rom: Option<Vec<u8>>) -> Self {
            Self {
                regs: BTreeMap::new(),
                present_fw,
                rom,
                next_dma: 0x1_0000_0000,
                scanout_committed: false,
                device_id,
                memsize_mb: None,
                gfx_busy_reads: 0,
                smu_mailbox: None,
                smu_responds: false,
                dma_mem: BTreeMap::new(),
                gpu_completes_fence: None,
                last_doorbell: None,
                rlc_safe_mode: None,
                fw_discovery: None,
                cp_me_cntl_halt_mask: None,
                cp_gfx_ring_regs: None,
                gfxhub_gart_regs: None,
                vram: BTreeMap::new(),
                psp_wptr_reg: None,
                psp_vram_base: 0,
                psp_ring_size_dw: 0,
                psp_tmr_reply: 0,
                vram_mc_base_val: None,
                psp_toc_val: None,
                psp_tmr_val: None,
            }
        }
    }
    impl GpuOps for MockOps {
        fn pci_enable(&mut self, _b: u8, _d: u8, _f: u8) -> Option<u64> {
            Some(0x42)
        }
        fn config_read_dword(&mut self, _h: u64, off: u16) -> u32 {
            if off == 0 {
                (AMD_VENDOR as u32) | ((self.device_id as u32) << 16)
            } else {
                0
            }
        }
        fn map_register_bar(&mut self, _h: u64, _bar: u8) -> bool {
            true
        }
        fn reg_read(&mut self, off: u32) -> u32 {
            // Model a GFX pipe that is busy for `gfx_busy_reads` polls, then idle.
            if off == gc11::MM_GRBM_STATUS && self.gfx_busy_reads > 0 {
                self.gfx_busy_reads -= 1;
                return gc11::GRBM_STATUS_GUI_ACTIVE;
            }
            *self.regs.get(&off).unwrap_or(&0)
        }
        fn reg_write(&mut self, off: u32, val: u32) {
            self.regs.insert(off, val);
            // Model the PMFW answering OK when the message id is written.
            if let Some(mb) = self.smu_mailbox {
                if off == mb.msg_reg && self.smu_responds {
                    self.regs.insert(mb.resp_reg, SMU_RESP_OK);
                }
            }
            // Model the GPU draining the ring + posting the fence on the doorbell.
            if let Some((wptr_reg, fence_addr, fence_value)) = self.gpu_completes_fence {
                if off == wptr_reg {
                    self.dma_mem.insert(fence_addr, fence_value);
                }
            }
            // Model a working RLC: on a safe-mode request (CMD set), the RLC
            // clears CMD once it has entered, which the enter-poll observes.
            if let Some(rlc) = self.rlc_safe_mode {
                if off == rlc.reg && val & gc11::RLC_SAFE_MODE_CMD != 0 {
                    self.regs.insert(rlc.reg, val & !gc11::RLC_SAFE_MODE_CMD);
                }
            }
            // Reacting PSP: a write to the ring write-pointer means a new frame was
            // posted. Read it back, then behave like the sOS — post the fence and
            // write a success response into the command buffer.
            if let Some(wptr_reg) = self.psp_wptr_reg {
                if off == wptr_reg && self.psp_ring_size_dw != 0 {
                    let base = self.psp_vram_base;
                    let new_wptr = val;
                    let old_wptr = (new_wptr + self.psp_ring_size_dw - PSP_RB_FRAME_DWORDS)
                        % self.psp_ring_size_dw;
                    let slot = psp_ring_frame_offset(old_wptr, self.psp_ring_size_dw) as u64;
                    let frame_off = PSP_RING_VRAM_OFFSET + slot;
                    let rd = |m: &BTreeMap<u64, u32>, o: u64| *m.get(&o).unwrap_or(&0);
                    let cmd_mc = rd(&self.vram, frame_off) as u64
                        | ((rd(&self.vram, frame_off + 4) as u64) << 32);
                    let fence_mc = rd(&self.vram, frame_off + 12) as u64
                        | ((rd(&self.vram, frame_off + 16) as u64) << 32);
                    let fence_value = rd(&self.vram, frame_off + 20);
                    let cmd_off = cmd_mc - base;
                    let cmd_id = rd(&self.vram, cmd_off + 8); // psp_gfx_cmd_resp.cmd_id
                                                              // Write the response (status 0) and, for LOAD_TOC, the TMR size.
                    self.vram.insert(cmd_off + 864, 0); // resp.status @ +864
                    if cmd_id == GFX_CMD_ID_LOAD_TOC {
                        self.vram.insert(cmd_off + 880, self.psp_tmr_reply); // resp.tmr_size
                    }
                    // Post the completion fence.
                    self.vram.insert(fence_mc - base, fence_value);
                }
            }
        }
        fn read_vbios_rom(&mut self, _h: u64, _max: usize) -> Option<Vec<u8>> {
            self.rom.clone()
        }
        fn dma_alloc(&mut self, _h: u64, size: usize) -> Option<DmaBuf> {
            let dma_addr = self.next_dma;
            self.next_dma += size as u64;
            Some(DmaBuf {
                dma_addr,
                size,
                id: dma_addr,
            })
        }
        fn dma_write(&mut self, buf: &DmaBuf, off: usize, data: &[u32]) {
            for (i, &d) in data.iter().enumerate() {
                self.dma_mem
                    .insert(buf.dma_addr + ((off + i) * 4) as u64, d);
            }
        }
        fn ring_doorbell(&mut self, byte_offset: u32, value: u64) {
            self.last_doorbell = Some((byte_offset, value));
        }
        fn dma_read(&mut self, buf: &DmaBuf, off: usize, out: &mut [u32]) {
            for (i, slot) in out.iter_mut().enumerate() {
                let addr = buf.dma_addr + ((off + i) * 4) as u64;
                *slot = *self.dma_mem.get(&addr).unwrap_or(&0);
            }
        }
        fn request_firmware(&mut self, _name: &str) -> bool {
            self.present_fw
        }
        fn request_firmware_bytes(&mut self, name: &str) -> Option<Vec<u8>> {
            if name == "amdgpu/ip_discovery.bin" {
                self.fw_discovery.clone()
            } else {
                None
            }
        }
        fn config_memsize_mb(&mut self) -> Option<u32> {
            self.memsize_mb
        }
        fn smu_mailbox(&mut self) -> Option<SmuMailbox> {
            self.smu_mailbox
        }
        fn rlc_safe_mode(&mut self) -> Option<RlcSafeMode> {
            self.rlc_safe_mode
        }
        fn cp_me_cntl_halt_mask(&mut self) -> Option<u32> {
            self.cp_me_cntl_halt_mask
        }
        fn cp_gfx_ring_regs(&mut self) -> Option<CpGfxRingRegs> {
            self.cp_gfx_ring_regs
        }
        fn gfxhub_gart_regs(&mut self) -> Option<crate::gart::GfxhubGartRegs> {
            self.gfxhub_gart_regs
        }
        fn commit_scanout(&mut self, _w: u32, _h: u32, _p: u32, _a: u64) -> bool {
            self.scanout_committed = true;
            true
        }
        fn vram_zero(&mut self, offset: u64, len: usize) {
            for i in 0..(len / 4) {
                self.vram.insert(offset + (i * 4) as u64, 0);
            }
        }
        fn vram_write(&mut self, offset: u64, data: &[u32]) {
            for (i, &d) in data.iter().enumerate() {
                self.vram.insert(offset + (i * 4) as u64, d);
            }
        }
        fn vram_read(&mut self, offset: u64, out: &mut [u32]) {
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = *self.vram.get(&(offset + (i * 4) as u64)).unwrap_or(&0);
            }
        }
        fn vram_mc_base(&mut self) -> Option<u64> {
            self.vram_mc_base_val
        }
        fn psp_toc_blob(&mut self) -> Option<(u64, u32)> {
            self.psp_toc_val
        }
        fn psp_tmr_base(&mut self) -> Option<u64> {
            self.psp_tmr_val
        }
        fn log(&mut self, _msg: &str) {}
    }

    #[test]
    fn bringup_full_sequence_on_present_gpu() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        // Enable the CP gfx-ring completion path so the activation (CP_RB_ACTIVE=1)
        // is exercised end-to-end through init_rings.
        ops.cp_gfx_ring_regs = Some(CpGfxRingRegs {
            rb_active: 0x9000,
            rb_vmid: 0x9004,
            rb0_rptr_addr: 0x9008,
            rb0_rptr_addr_hi: 0x900c,
            rb_wptr_poll_addr_lo: 0x9010,
            rb_wptr_poll_addr_hi: 0x9014,
            doorbell_control: 0x9018,
            doorbell_range_lower: 0x901c,
            doorbell_range_upper: 0x9020,
            max_context: 0x9024,
            device_id: 0x9028,
        });
        let report = bringup(&mut ops, &[(0, 1, 0)]);
        assert!(report.device_present);
        assert!(report.all_ok(), "every IP block should init: {report:?}");
        assert!(ops.scanout_committed);
        assert_eq!(report.vram_mib, 512); // CONFIG_MEMSIZE unavailable -> default
                                          // CP_RB0_BASE is the ring's GART VA in 256-BYTE units (>>8). The gfx ring
                                          // is the lowest-addressed buffer (gart_lo), so its GART VA == the aperture
                                          // base itself (CP_RB0_BASE_HI=0x7f, matching the working amdgpu driver).
        let base_lo = *ops.regs.get(&gc11::MM_CP_RB0_BASE).unwrap();
        let base_hi = *ops.regs.get(&gc11::MM_CP_RB0_BASE_HI).unwrap();
        assert_eq!(
            (base_lo as u64) | ((base_hi as u64) << 32),
            GFX_GART_APERTURE_BASE >> 8
        );
        // The ring was ACTIVATED (the previously-missing CP_RB_ACTIVE write).
        assert_eq!(*ops.regs.get(&0x9000).unwrap(), 1, "CP_RB_ACTIVE must be 1");
        // CP-init (cp_gfx_start): DEVICE_ID=1 (the start kick) + MAX_CONTEXT=7.
        assert_eq!(*ops.regs.get(&0x9028).unwrap(), 1, "CP_DEVICE_ID must be 1");
        assert_eq!(
            *ops.regs.get(&0x9024).unwrap(),
            7,
            "CP_MAX_CONTEXT must be 7"
        );
    }

    #[test]
    fn gfx_ring_reachable_via_aperture_checks_range() {
        let vm = GmcVmRegs {
            fb_location_base: 0x10,
            fb_location_top: 0x14,
            agp_base: 0x18,
            sys_aperture_low: 0x1c,
            sys_aperture_high: 0x20,
            mx_l1_tlb_cntl: 0x24,
            context0_cntl: 0x28,
            context0_ptb_lo32: 0x2c,
            context0_ptb_hi32: 0x30,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        // Firmware aperture covers units [0x1000 .. 0x2000] (256 KiB each).
        ops.regs.insert(vm.sys_aperture_low, 0x1000);
        ops.regs.insert(vm.sys_aperture_high, 0x2000);
        assert!(gfx_ring_reachable_via_aperture(
            &mut ops,
            &vm,
            0x1500u64 << 18
        )); // inside
        assert!(!gfx_ring_reachable_via_aperture(
            &mut ops,
            &vm,
            0x3000u64 << 18
        )); // above
            // A zeroed aperture (firmware never set it) must NOT report a false reach.
        let mut ops2 = MockOps::new(RADEON_760M, true, None);
        assert!(!gfx_ring_reachable_via_aperture(
            &mut ops2,
            &vm,
            0x1500u64 << 18
        ));
    }

    #[test]
    fn init_gart_builds_table_and_enables_vmid0() {
        let g = crate::gart::GfxhubGartRegs {
            l2_cntl: 0xa00,
            l2_cntl2: 0xa04,
            l2_cntl3: 0xa08,
            fb_location_base: 0xa0c,
            fb_location_top: 0xa10,
            agp_base: 0xa14,
            agp_bot: 0xa18,
            agp_top: 0xa1c,
            sys_aperture_low: 0xa20,
            sys_aperture_high: 0xa24,
            sys_default_lsb: 0xa28,
            sys_default_msb: 0xa2c,
            mx_l1_tlb_cntl: 0xa30,
            context0_cntl: 0xa34,
            context0_ptb_lo32: 0xa38,
            context0_ptb_hi32: 0xa3c,
            context0_start_lo32: 0xa40,
            context0_start_hi32: 0xa44,
            context0_end_lo32: 0xa48,
            context0_end_hi32: 0xa4c,
            invalidate_eng0_req: 0xa50,
            invalidate_eng0_ack: 0xa54,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.gfxhub_gart_regs = Some(g);
        let dev = Device {
            handle: 1,
            vendor: AMD_VENDOR,
            device: RADEON_760M,
            vram_base: 0,
            vram_size: 0,
            bootup_sclk_mhz: 0,
            bootup_mclk_mhz: 0,
        };
        let ring_dma = 0x1234_5000u64; // page-aligned
        let va = init_gart(&mut ops, &dev, ring_dma, 8192).unwrap();
        assert_eq!(va, 0x0080_0000, "ring uses the non-zero GART VA base");
        // VMID0 (CONTEXT0) enabled.
        assert_ne!(*ops.regs.get(&g.context0_cntl).unwrap() & 1, 0);
        // Page-table base programmed to the allocated table (mock's first dma_alloc),
        // with bit 0 = PDB VALID.
        let tbl_addr = 0x1_0000_0000u64;
        assert_eq!(
            *ops.regs.get(&g.context0_ptb_lo32).unwrap(),
            ((tbl_addr & 0xFFFF_FFFF) as u32) | 1
        );
        // First PTE maps the ring's first page + VALID (written to the table buffer).
        let pte_lo = *ops.dma_mem.get(&tbl_addr).unwrap();
        assert_eq!(pte_lo & 0xFFFFF000, 0x1234_5000, "PTE addr = ring page");
        assert_ne!(pte_lo & 1, 0, "PTE VALID");
    }

    #[test]
    fn init_gart_identity_maps_aperture_over_span() {
        let g = crate::gart::GfxhubGartRegs {
            l2_cntl: 0xa00,
            l2_cntl2: 0xa04,
            l2_cntl3: 0xa08,
            fb_location_base: 0xa0c,
            fb_location_top: 0xa10,
            agp_base: 0xa14,
            agp_bot: 0xa18,
            agp_top: 0xa1c,
            sys_aperture_low: 0xa20,
            sys_aperture_high: 0xa24,
            sys_default_lsb: 0xa28,
            sys_default_msb: 0xa2c,
            mx_l1_tlb_cntl: 0xa30,
            context0_cntl: 0xa34,
            context0_ptb_lo32: 0xa38,
            context0_ptb_hi32: 0xa3c,
            context0_start_lo32: 0xa40,
            context0_start_hi32: 0xa44,
            context0_end_lo32: 0xa48,
            context0_end_hi32: 0xa4c,
            invalidate_eng0_req: 0xa50,
            invalidate_eng0_ack: 0xa54,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.gfxhub_gart_regs = Some(g);
        // Athena's FB: base 0x80_0000_0000 (MMHUB FB_LOCATION<<24), 2 GiB.
        ops.vram_mc_base_val = Some(0x80_0000_0000);
        let dev = Device {
            handle: 1,
            vendor: AMD_VENDOR,
            device: RADEON_760M,
            vram_base: 0,
            vram_size: 2048 * 1024 * 1024,
            bootup_sclk_mhz: 0,
            bootup_mclk_mhz: 0,
        };
        // A buffer span like the real bring-up (ring + scratch + fence), unaligned lo.
        let lo = 0x3dc0_8123u64;
        let hi = 0x3dc0_a456u64;
        assert!(init_gart_identity(&mut ops, &dev, lo, hi));
        // VMID0 (CONTEXT0) enabled with the calibrated fault bits, TLB at 0x1859.
        assert_eq!(*ops.regs.get(&g.context0_cntl).unwrap(), 0x1fffe01);
        assert_eq!(*ops.regs.get(&g.mx_l1_tlb_cntl).unwrap(), 0x1859);
        // System aperture = the VRAM FB range (fb_start/end >> 18) — the Athena live
        // umr values that let the GPU reach the framebuffer (display path).
        assert_eq!(*ops.regs.get(&g.sys_aperture_low).unwrap(), 0x200000);
        assert_eq!(*ops.regs.get(&g.sys_aperture_high).unwrap(), 0x201fff);
        // CONTEXT0 window starts at the GART APERTURE base (>>12 page units),
        // matching amdgpu's live START_LO=0xfff00000 / START_HI=0x7 — NOT the
        // physical lo (the gfxhub only translates this high region).
        let ap_page = GFX_GART_APERTURE_BASE >> 12;
        assert_eq!(
            *ops.regs.get(&g.context0_start_lo32).unwrap(),
            ap_page as u32
        );
        assert_eq!(
            *ops.regs.get(&g.context0_start_hi32).unwrap(),
            (ap_page >> 32) as u32
        );
        // The aperture window maps to physical [lo,hi): PTE 0 -> physical page `lo`.
        let tbl_addr = 0x1_0000_0000u64; // mock's first dma_alloc
        let pte0 = *ops.dma_mem.get(&tbl_addr).unwrap();
        assert_eq!(
            (pte0 as u64) & 0xFFFFF000,
            (lo & !0xfffu64) & 0xFFFFF000,
            "PTE0 -> physical page lo"
        );
        assert_ne!(pte0 & 1, 0, "PTE0 VALID");
        // No gfxhub regs -> returns false, touches nothing (QEMU path).
        let mut ops2 = MockOps::new(RADEON_760M, true, None);
        assert!(!init_gart_identity(&mut ops2, &dev, lo, hi));
        assert!(ops2.regs.get(&g.context0_cntl).is_none());
    }

    #[test]
    fn wait_for_gfx_awake_detects_gated_vs_awake() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        // Gated: probe reg reads 0 -> exhausts the budget, reports not-awake.
        assert!(!wait_for_gfx_awake(&mut ops, 0x500, 3000));
        // Awake: probe reg non-zero (a GOP-left value) -> awake immediately.
        ops.regs.insert(0x500, 0x3ffa208);
        assert!(wait_for_gfx_awake(&mut ops, 0x500, 3000));
    }

    #[test]
    fn imu_core_start_releases_reset_then_detects_up() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        let core = 0x600;
        let reset = 0x604;
        // Core held in reset (CRESET=1); reset-ctrl absent -> reads 0 -> GFX DOWN.
        ops.regs.insert(core, 0x1);
        // Times out (GFX never comes out of reset in this mock) -> false ...
        assert!(!try_imu_core_start(&mut ops, core, reset, 3000));
        // ... but it MUST have released the IMU core (CRESET cleared) regardless.
        assert_eq!(
            *ops.regs.get(&core).unwrap() & crate::regs::GFX_IMU_CORE_CTRL_CRESET,
            0,
            "IMU core must be released (CRESET=0)"
        );
        // Model GFX out of reset (low 5 bits set) -> the poll sees it -> true.
        ops.regs
            .insert(reset, crate::regs::GFX_IMU_GFX_RESET_DONE_MASK);
        assert!(try_imu_core_start(&mut ops, core, reset, 3000));
        // A partial reset-done (one bit short) must NOT read as up.
        ops.regs
            .insert(reset, crate::regs::GFX_IMU_GFX_RESET_DONE_MASK - 1);
        assert!(!try_imu_core_start(&mut ops, core, reset, 3000));
    }

    #[test]
    fn load_imu_microcode_streams_then_sets_fw_version() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        // Minimal valid gc_*_imu.bin: 48-byte header, 8 B I-RAM (2 dwords), 4 B D-RAM.
        let mut blob = [0u8; 48 + 8 + 4].to_vec();
        blob[16..20].copy_from_slice(&0xABCDu32.to_le_bytes()); // ucode_version
        blob[24..28].copy_from_slice(&48u32.to_le_bytes()); // ucode_array_offset_bytes
        blob[32..36].copy_from_slice(&8u32.to_le_bytes()); // imu_iram size
        blob[40..44].copy_from_slice(&4u32.to_le_bytes()); // imu_dram size
        blob[48..52].copy_from_slice(&0x1111_1111u32.to_le_bytes()); // iram dw0
        blob[52..56].copy_from_slice(&0x2222_2222u32.to_le_bytes()); // iram dw1 (last)
        blob[56..60].copy_from_slice(&0x3333_3333u32.to_le_bytes()); // dram dw0 (last)
        let (ia, id, da, dd) = (0x100u32, 0x104u32, 0x200u32, 0x204u32);
        assert!(load_imu_microcode(&mut ops, ia, id, da, dd, &blob));
        // Each ADDR ends at fw_version; each DATA holds the LAST dword streamed.
        assert_eq!(
            *ops.regs.get(&ia).unwrap(),
            0xABCD,
            "I-RAM addr = fw_version"
        );
        assert_eq!(
            *ops.regs.get(&da).unwrap(),
            0xABCD,
            "D-RAM addr = fw_version"
        );
        assert_eq!(
            *ops.regs.get(&id).unwrap(),
            0x2222_2222,
            "I-RAM data = last iram dword"
        );
        assert_eq!(
            *ops.regs.get(&dd).unwrap(),
            0x3333_3333,
            "D-RAM data = last dram dword"
        );
        // A short/garbage blob -> parse None -> false, and NO register writes.
        let mut ops2 = MockOps::new(RADEON_760M, true, None);
        assert!(!load_imu_microcode(&mut ops2, ia, id, da, dd, &[0u8; 8]));
        assert!(ops2.regs.is_empty(), "no writes on a bad blob");
    }

    #[test]
    fn imu_iram_verdict_classifies_each_iron_case() {
        let expect = [0x1111_1111u32, 0x2222_2222, 0x3333_3333];
        // Readback == source: the GFX_IMU block accepts writes.
        assert!(imu_iram_verdict(&expect, &expect).starts_with("LANDED"));
        // All-zero: the block is gated (the boot-024544 "writes dropped" case).
        assert!(imu_iram_verdict(&[0, 0, 0], &expect).starts_with("ALL-ZERO"));
        // All-ones: dead/unmapped aperture.
        assert!(
            imu_iram_verdict(&[0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF], &expect)
                .starts_with("ALL-ONES")
        );
        // Partial/garbage: addressing or auto-increment wrong.
        assert!(imu_iram_verdict(&[0x1111_1111, 0xDEAD, 0], &expect).starts_with("MISMATCH"));
        // Degenerate empty slice must not panic and reads as EMPTY.
        assert!(imu_iram_verdict(&[], &[]).starts_with("EMPTY"));
    }

    #[test]
    fn seg1_probe_byte_anchors_off_core_ctrl() {
        // Iron: GC seg1 base 0xa000, CORE_CTRL resolves to (0xa000+0x40b6)<<2.
        let core = (0xa000u32 + 0x40b6) << 2;
        // The sweep must reconstruct the byte address of any seg1 register.
        assert_eq!(
            seg1_probe_byte(core, 0x40b6),
            core,
            "CORE_CTRL maps to itself"
        );
        assert_eq!(
            seg1_probe_byte(core, 0x5f90),
            (0xa000 + 0x5f90) << 2,
            "I_RAM_ADDR"
        );
        assert_eq!(
            seg1_probe_byte(core, 0x5f91),
            (0xa000 + 0x5f91) << 2,
            "I_RAM_DATA"
        );
        assert_eq!(
            seg1_probe_byte(core, 0x4000),
            (0xa000 + 0x4000) << 2,
            "C2PMSG_0"
        );
        // Independent of the actual base — anchor math only needs the offset delta.
        let core2 = (0x2000u32 + 0x40b6) << 2;
        assert_eq!(seg1_probe_byte(core2, 0x5f90), (0x2000 + 0x5f90) << 2);
    }

    #[test]
    fn psp_ring_init_cmd_is_init_gpcom_for_km() {
        // KM(2)<<16 must equal GFX_CTRL_CMD_ID_INIT_GPCOM_RING (0x00020000), the
        // exact value psp_v13_0_ring_create writes to C2PMSG_64.
        assert_eq!(psp_ring_init_cmd(PSP_RING_TYPE_KM), 0x0002_0000);
        // The mailbox flag/error masks must split a completion word correctly.
        let resp_ok = PSP_MBOX_FLAG; // bit31 set, low16 = 0
        assert_ne!(resp_ok & PSP_MBOX_FLAG, 0);
        assert_eq!(resp_ok & PSP_MBOX_ERR_MASK, 0);
        let resp_err = PSP_MBOX_FLAG | 0x0007;
        assert_eq!(resp_err & PSP_MBOX_ERR_MASK, 7, "error code in low 16 bits");
    }

    #[test]
    fn psp_gpcom_cmd_buf_offsets_match_psp_gfx_if() {
        // psp_gfx_cmd_resp: cmd_id @ byte +8 (dword 2), union @ byte +28 (dword 7).
        // amdgpu memsets the buffer to 0 then sets ONLY cmd_id + the union, so dwords
        // 0/1 (buf_size/buf_version) and 3..7 (RBI resp fields) MUST stay 0 for GPCOM.
        let tmr = psp_cmd_setup_tmr(0x80_7800_0000, 0x0400_0000);
        assert_eq!(tmr[0], 0, "buf_size stays 0 (GPCOM)");
        assert_eq!(tmr[1], 0, "buf_version stays 0 (GPCOM, RBI-only field)");
        assert_eq!(tmr[2], GFX_CMD_ID_SETUP_TMR, "cmd_id @ +8");
        assert_eq!(tmr[3], 0, "resp_buf_addr_lo @ +12 = 0 (GPCOM)");
        assert_eq!(tmr[7], 0x7800_0000, "tmr buf_phy_addr_lo @ +28");
        assert_eq!(tmr[8], 0x0000_0080, "tmr buf_phy_addr_hi @ +32");
        assert_eq!(tmr[9], 0x0400_0000, "tmr buf_size @ +36");
        assert_eq!(tmr[10], 0, "tmr_flags @ +40 = 0 (no SR-IOV)");

        // LOAD_TOC: same header, only the 3-dword union.
        let toc = psp_cmd_load_toc(0x80_0033_9000, 0x1000);
        assert_eq!(toc[2], GFX_CMD_ID_LOAD_TOC);
        assert_eq!(toc[7], 0x0033_9000);
        assert_eq!(toc[8], 0x0000_0080);
        assert_eq!(toc[9], 0x1000);

        // LOAD_IP_FW: 4-dword union, fw_type @ +40 (IMU_I first powers up GFX).
        let fw = psp_cmd_load_ip_fw(0x7FFF_0000_2000, 0x8000, GFX_FW_TYPE_IMU_I);
        assert_eq!(fw[2], GFX_CMD_ID_LOAD_IP_FW);
        assert_eq!(fw[7], 0x0000_2000, "fw_phy_addr_lo @ +28");
        assert_eq!(fw[8], 0x7FFF, "fw_phy_addr_hi @ +32 (GART MC)");
        assert_eq!(fw[9], 0x8000, "fw_size @ +36");
        assert_eq!(fw[10], 68, "fw_type IMU_I @ +40");
    }

    #[test]
    fn psp_rb_frame_offsets_match_psp_gfx_if() {
        // psp_gfx_rb_frame (64 B): cmd_buf_addr_lo/hi @ +0/+4, cmd_buf_size @ +8,
        // fence_addr_lo/hi @ +12/+16, fence_value @ +20; SID/vmid/reserved tail = 0.
        let f = psp_rb_frame(0x80_0400_0000, 1024, 0x7FFF_0001_0000, 0x2a);
        assert_eq!(f.len(), 16, "frame is exactly 64 bytes");
        assert_eq!(f[0], 0x0400_0000, "cmd_buf_addr_lo @ +0");
        assert_eq!(f[1], 0x0000_0080, "cmd_buf_addr_hi @ +4");
        assert_eq!(f[2], 1024, "cmd_buf_size @ +8");
        assert_eq!(f[3], 0x0001_0000, "fence_addr_lo @ +12");
        assert_eq!(f[4], 0x7FFF, "fence_addr_hi @ +16");
        assert_eq!(f[5], 0x2a, "fence_value @ +20");
        assert_eq!(
            &f[6..],
            &[0u32; 10],
            "SID/vmid/frame_type/reserved tail = 0"
        );
    }

    #[test]
    fn psp_ring_wptr_math_matches_psp_ring_cmd_submit() {
        // ring_size 0x1000 B = 0x400 dwords; one frame = 16 dwords. wptr is in dwords.
        let rsz = 0x1000u32 / 4; // 0x400
                                 // First submit: wptr 0 -> slot 0, advance to 16.
        assert_eq!(psp_ring_frame_offset(0, rsz), 0);
        assert_eq!(psp_ring_advance_wptr(0, rsz), 16);
        // Second submit: wptr 16 -> byte offset (16/16)*64 = 64, advance to 32.
        assert_eq!(psp_ring_frame_offset(16, rsz), 64);
        assert_eq!(psp_ring_advance_wptr(16, rsz), 32);
        // Oracle live wptr 0x1b0 (432 dw) -> slot (432/16)*64 = 27*64 = 0x6c0.
        assert_eq!(psp_ring_frame_offset(0x1b0, rsz), 27 * 64);
        // Last slot before wrap: wptr 0x3f0 (1008 dw) -> advance wraps to 0.
        assert_eq!(psp_ring_advance_wptr(rsz - 16, rsz), 0);
        // A wptr that is a multiple of ring_size_dw wraps to slot 0 (the % == 0 case).
        assert_eq!(psp_ring_frame_offset(rsz, rsz), 0);
    }

    #[test]
    fn psp_submit_gpcom_round_trips_through_reacting_psp() {
        // End-to-end host proof of the submission mechanism: a mock that behaves like
        // the sOS (reads the posted ring frame, writes resp.status + tmr_size, posts
        // the fence) must make psp_submit_gpcom return the response. FAIL-able: a wrong
        // ring slot, wptr unit, frame layout, or resp offset breaks the round-trip.
        let vram_base = 0x80_0000_0000u64;
        let wptr_reg = 0x0005_8a0c; // arbitrary BAR5 offset; the mock keys regs by it
        let mut ops = MockOps::new(0x15bf, true, None);
        ops.psp_wptr_reg = Some(wptr_reg);
        ops.psp_vram_base = vram_base;
        ops.psp_ring_size_dw = PSP_RING_SIZE / 4; // 0x400
        ops.psp_tmr_reply = 0x0080_0000; // 8 MiB

        let psp = PspRegs {
            sol: 0x10,
            sos_fw_version: 0x14,
            bl_status: 0x18,
            vmbx_status: 0x1c,
            bl_arg: 0x20,
            ring_cmd: 0x24,
            ring_lo: 0x28,
            ring_hi: 0x2c,
            ring_size: 0x30,
            ring_wptr: wptr_reg,
        };
        let ring = PspRing {
            ring_mc: vram_base + PSP_RING_VRAM_OFFSET,
            ring_size: PSP_RING_SIZE,
        };

        // Submit LOAD_TOC (fence index 1): the reacting PSP replies status 0 + tmr_size.
        let toc = psp_cmd_load_toc(vram_base + 0x10_0000, 0x1000);
        let resp = psp_submit_gpcom(&mut ops, &psp, &ring, vram_base, &toc, 1);
        assert_eq!(
            resp,
            Some((0, 0x0080_0000)),
            "LOAD_TOC round-trip: status 0 + tmr_size from resp area"
        );
        assert_eq!(
            ops.reg_read(wptr_reg),
            16,
            "wptr advanced one frame (dwords)"
        );

        // Submit SETUP_TMR (fence index 2): advances to the next ring slot, completes.
        let tmr = psp_cmd_setup_tmr(vram_base + 0x78_0000, 0x0080_0000);
        let resp2 = psp_submit_gpcom(&mut ops, &psp, &ring, vram_base, &tmr, 2);
        assert_eq!(
            resp2.map(|(s, _)| s),
            Some(0),
            "SETUP_TMR round-trip: status 0"
        );
        assert_eq!(ops.reg_read(wptr_reg), 32, "wptr advanced to the 3rd slot");
    }

    /// Build a `PspRegs` with arbitrary distinct offsets for the orchestration tests.
    fn test_psp_regs(ring_wptr: u32) -> PspRegs {
        PspRegs {
            sol: 0x10,
            sos_fw_version: 0x14,
            bl_status: 0x18,
            vmbx_status: 0x1c,
            bl_arg: 0x20,
            ring_cmd: 0x24,
            ring_lo: 0x28,
            ring_hi: 0x2c,
            ring_size: 0x30,
            ring_wptr,
        }
    }

    #[test]
    fn psp_load_gfx_firmware_runs_the_full_handshake() {
        // LOAD_TOC -> SETUP_TMR -> AUTOLOAD_RLC over the reacting sOS, end to end.
        let vram_base = 0x80_0000_0000u64;
        let wptr_reg = 0x0005_8a0c;
        let mut ops = MockOps::new(0x15bf, true, None);
        ops.psp_wptr_reg = Some(wptr_reg);
        ops.psp_vram_base = vram_base;
        ops.psp_ring_size_dw = PSP_RING_SIZE / 4;
        ops.psp_tmr_reply = 0x0400_0000; // 64 MiB (oracle TMR size)
        ops.vram_mc_base_val = Some(vram_base);
        ops.psp_toc_val = Some((vram_base + 0x10_0000, 0x2000));
        ops.psp_tmr_val = Some(0x80_7800_0000);

        let psp = test_psp_regs(wptr_reg);
        let ring = PspRing {
            ring_mc: vram_base + PSP_RING_VRAM_OFFSET,
            ring_size: PSP_RING_SIZE,
        };
        // Two gfx ucodes, RLC_G last so AUTOLOAD_RLC fires after it.
        let pfp = [0xaau8; 32];
        let rlc = [0xbbu8; 48];
        let fw_blobs: &[(u32, &[u8])] = &[(GFX_FW_TYPE_CP_PFP, &pfp), (GFX_FW_TYPE_RLC_G, &rlc)];
        assert!(
            psp_load_gfx_firmware(&mut ops, &psp, &ring, fw_blobs),
            "full handshake accepted by the reacting sOS"
        );
        // Frames: LOAD_TOC, SETUP_TMR, LOAD_IP_FW(PFP), LOAD_IP_FW(RLC_G), AUTOLOAD_RLC
        // = 5 frames -> wptr advanced 5 * 16 = 80 dwords.
        assert_eq!(
            ops.reg_read(wptr_reg),
            80,
            "five frames submitted (2 fw + autoload)"
        );
    }

    #[test]
    fn psp_load_gfx_firmware_skips_without_staged_firmware() {
        // TOC not staged (psp_toc_blob None) -> skip cleanly, submit nothing.
        let mut ops = MockOps::new(0x15bf, true, None);
        ops.vram_mc_base_val = Some(0x80_0000_0000);
        let psp = test_psp_regs(0x40);
        let ring = PspRing {
            ring_mc: 0x80_0400_0000,
            ring_size: PSP_RING_SIZE,
        };
        assert!(
            !psp_load_gfx_firmware(&mut ops, &psp, &ring, &[]),
            "skips when no firmware is staged"
        );
        assert_eq!(ops.reg_read(0x40), 0, "no frame submitted (wptr untouched)");
    }

    #[test]
    fn psp_resp_parse_reads_status_and_tmr_size() {
        // psp_gfx_resp @ cmd buffer +864: status @ +864 (dword 216), tmr_size @ +880
        // (dword 220). A full GPCOM cmd buffer is 1024 B = 256 dwords.
        let mut buf = [0u32; 256];
        buf[216] = 0; // status OK
        buf[220] = 0x0080_0000; // 8 MiB TMR (example LOAD_TOC reply)
        assert_eq!(psp_resp_status_tmr(&buf), Some((0, 0x0080_0000)));
        // An error status surfaces verbatim.
        buf[216] = 0x0000_0007;
        assert_eq!(psp_resp_status_tmr(&buf), Some((7, 0x0080_0000)));
        // Too-short buffer => None (never read past the end).
        assert_eq!(psp_resp_status_tmr(&[0u32; 4]), None);
    }

    #[test]
    fn psp_sol_verdict_classifies_sign_of_life() {
        // A live sOS version (any non-0/non-0xffffffff value) = channel reachable.
        assert!(psp_sol_verdict(0x0080_0000).starts_with("ALIVE"));
        assert!(psp_sol_verdict(0x0000_0001).starts_with("ALIVE"));
        // 0xffffffff = the MP0 aperture doesn't decode (wrong seg/base).
        assert!(psp_sol_verdict(0xFFFF_FFFF).starts_with("DEAD"));
        // 0 = secure OS not exposing the register.
        assert!(psp_sol_verdict(0).starts_with("SILENT"));
    }

    #[test]
    fn rlc_safe_mode_enter_exit() {
        // Offset unconfirmed (None) → enter is a no-op SUCCESS (caller proceeds)
        // and writes NO register (never a guessed offset on iron).
        let mut ops = MockOps::new(RADEON_760M, true, None);
        assert!(
            rlc_enter_safe_mode(&mut ops, 10),
            "no-offset enter must succeed as a no-op"
        );
        assert!(
            ops.regs.is_empty(),
            "no RLC write when the offset is unconfirmed"
        );

        // Offset confirmed → the modeled RLC clears CMD on entry, so enter acks;
        // exit writes MESSAGE alone (CMD clear).
        let rlc_reg = 0x4000;
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.rlc_safe_mode = Some(RlcSafeMode { reg: rlc_reg });
        assert!(
            rlc_enter_safe_mode(&mut ops, 10),
            "RLC must ack safe-mode entry (CMD cleared)"
        );
        assert_eq!(
            *ops.regs.get(&rlc_reg).unwrap() & gc11::RLC_SAFE_MODE_CMD,
            0,
            "CMD must read clear after entry"
        );
        rlc_exit_safe_mode(&mut ops);
        assert_eq!(
            *ops.regs.get(&rlc_reg).unwrap(),
            gc11::RLC_SAFE_MODE_MESSAGE,
            "exit writes MESSAGE only (CMD=0)"
        );
    }

    #[test]
    fn discovery_from_firmware_uses_bundled_blob() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        // No ip_discovery.bin bundled -> None (caller falls back to the VRAM read).
        assert!(discovery_from_firmware(&mut ops).is_none());
        // A valid blob -> Some(blocks): the SOC15-offset source with no VRAM MMIO.
        ops.fw_discovery = Some(crate::discovery::synthetic_blob());
        let blocks = discovery_from_firmware(&mut ops).expect("valid blob -> blocks");
        assert_eq!(blocks.len(), 2);
        // A corrupt blob is rejected — stays gated, never adopts garbage bases.
        let mut corrupt = crate::discovery::synthetic_blob();
        corrupt[0] ^= 0xFF;
        ops.fw_discovery = Some(corrupt);
        assert!(discovery_from_firmware(&mut ops).is_none());
    }

    #[test]
    fn cp_gfx_enable_clears_halt_only_when_mask_known() {
        const CP_ME_CNTL: u32 = 0x6000;
        // The real gfx11 GFX-CP halt mask (ME_HALT|PFP_HALT), harvested from
        // gc_11_0_0_sh_mask.h + gfx_v11_0_cp_gfx_enable.
        const HALT_MASK: u32 = crate::gc11::CP_ME_CNTL_GFX11_HALT_MASK;

        // Mask unknown (None) -> no-op, returns false, writes NOTHING. This is the
        // iron-today behaviour: never a guessed write to the live CP register.
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.regs.insert(CP_ME_CNTL, HALT_MASK); // CP starts halted
        assert!(!cp_gfx_enable(&mut ops, CP_ME_CNTL, true));
        assert_eq!(*ops.regs.get(&CP_ME_CNTL).unwrap(), HALT_MASK); // untouched

        // Mask known -> enable clears ONLY the halt bits (RMW preserves the rest).
        ops.cp_me_cntl_halt_mask = Some(HALT_MASK);
        ops.regs.insert(CP_ME_CNTL, HALT_MASK | 0x1); // a non-halt bit also set
        assert!(cp_gfx_enable(&mut ops, CP_ME_CNTL, true));
        assert_eq!(*ops.regs.get(&CP_ME_CNTL).unwrap(), 0x1); // halt cleared, rest kept
                                                              // Disable sets the halt bits back without disturbing the other bit.
        assert!(cp_gfx_enable(&mut ops, CP_ME_CNTL, false));
        assert_eq!(*ops.regs.get(&CP_ME_CNTL).unwrap(), HALT_MASK | 0x1);
    }

    #[test]
    fn program_sdma_ring_programs_base_size_wptr() {
        let sdma = SdmaRegs {
            rb_cntl: 0x7000,
            rb_base: 0x7004,
            rb_base_hi: 0x7008,
            rb_rptr: 0x700c,
            rb_rptr_hi: 0x7010,
            rb_wptr: 0x7014,
            rb_wptr_hi: 0x7018,
            rb_wptr_poll_addr_hi: 0x7020,
            rb_wptr_poll_addr_lo: 0x7024,
            doorbell: 0x7028,
            doorbell_offset: 0x702c,
            broadcast_ucode_addr: 0x7030,
            broadcast_ucode_data: 0x7034,
            f32_cntl: 0x701c,
            utcl1_cntl: 0x7038,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        let ring_addr = 0x1_2345_6700u64; // 256-byte aligned
        let wptr_poll = ops.dma_alloc(0, 4096).unwrap();
        let poll_dw = 0x20usize;
        // The SDMA engine starts HALTED with TH0_ENABLE preserved-but-not-enough
        // (GOP state F32_CNTL=HALT | 0xc00: bit10 TH0_ENABLE + bit11, NO TH1_ENABLE).
        ops.regs
            .insert(sdma.f32_cntl, crate::sdma::SDMA_F32_CNTL_HALT | 0xc00);
        assert!(program_sdma_ring(
            &mut ops,
            &sdma,
            ring_addr,
            64 * 1024,
            9,
            &wptr_poll,
            poll_dw,
            0,
        ));
        // Engine STARTED: HALT (bit 0) cleared, both thread resets cleared, and
        // TH0_ENABLE (bit 10) + TH1_ENABLE (bit 14) set. From HALT|0xc00 -> 0x4c00
        // (0xc00 with TH1_ENABLE=0x4000 added; HALT removed).
        assert_eq!(*ops.regs.get(&sdma.f32_cntl).unwrap(), 0x4c00);
        // UTC L1 translation cache programmed (RESP_MODE=3 + REDO_DELAY=9) so the
        // engine can resolve its ring/WPTR/fence addresses through VMID0.
        assert_eq!(
            *ops.regs.get(&sdma.utcl1_cntl).unwrap(),
            crate::sdma::SDMA_UTCL1_CNTL_VALUE
        );
        // RB_BASE is the address >> 8 (256-byte shifted), NOT the raw low 32 bits.
        assert_eq!(
            *ops.regs.get(&sdma.rb_base).unwrap(),
            (ring_addr >> 8) as u32
        );
        assert_eq!(
            *ops.regs.get(&sdma.rb_base_hi).unwrap(),
            (ring_addr >> 40) as u32
        );
        // RB_CNTL: RB_ENABLE | (log2(16384 dwords)=14 << 1) | F32_WPTR_POLL_ENABLE | RB_PRIV.
        assert_eq!(
            *ops.regs.get(&sdma.rb_cntl).unwrap(),
            crate::sdma::SDMA_RB_CNTL_RB_ENABLE
                | (14 << crate::sdma::SDMA_RB_CNTL_RB_SIZE_SHIFT)
                | crate::sdma::SDMA_RB_CNTL_F32_WPTR_POLL_ENABLE
                | crate::sdma::SDMA_RB_CNTL_RB_PRIV
        );
        // The WPTR-poll address register points at the poll buffer dword.
        let poll_addr = wptr_poll.dma_addr + (poll_dw as u64) * 4;
        assert_eq!(
            *ops.regs.get(&sdma.rb_wptr_poll_addr_lo).unwrap(),
            (poll_addr & 0xFFFF_FFFF) as u32
        );
        assert_eq!(
            *ops.regs.get(&sdma.rb_wptr_poll_addr_hi).unwrap(),
            (poll_addr >> 32) as u32
        );
        // SUBMIT lands in the poll MEMORY (dwords << 2 = byte offset) AND the RB_WPTR
        // REGISTER directly (the iron fix — poll+doorbell alone left RB_WPTR=0 and the
        // F32 never drained the ring; the direct register write is what makes it run).
        assert_eq!(*ops.dma_mem.get(&poll_addr).unwrap(), 9 << 2);
        assert_eq!(*ops.regs.get(&sdma.rb_wptr).unwrap(), 9 << 2);
        assert_eq!(*ops.regs.get(&sdma.rb_wptr_hi).unwrap(), 0);
        assert_eq!(*ops.regs.get(&sdma.rb_rptr).unwrap(), 0);
        // Doorbell: queue routed to the aperture (ENABLE) at offset 0x800, and the
        // engine WOKEN via a doorbell ring carrying the WPTR-in-bytes (the live
        // amdgpu wake path; boot 170257 proved WPTR-poll alone left RB_RPTR=0).
        assert_eq!(*ops.regs.get(&sdma.doorbell_offset).unwrap(), 0x800);
        assert_eq!(
            *ops.regs.get(&sdma.doorbell).unwrap(),
            crate::sdma::SDMA_DOORBELL_ENABLE
        );
        assert_eq!(ops.last_doorbell, Some((0x800, (9u64) << 2)));
        // A non-power-of-two ring size is rejected (never program a bad RB_SIZE).
        assert!(!program_sdma_ring(
            &mut ops, &sdma, ring_addr, 3000, 9, &wptr_poll, poll_dw, 0
        ));
    }

    #[test]
    fn sdma_submit_and_wait_completes_and_times_out() {
        let sdma = SdmaRegs {
            rb_cntl: 0x7000,
            rb_base: 0x7004,
            rb_base_hi: 0x7008,
            rb_rptr: 0x700c,
            rb_rptr_hi: 0x7010,
            rb_wptr: 0x7014,
            rb_wptr_hi: 0x7018,
            rb_wptr_poll_addr_hi: 0x7020,
            rb_wptr_poll_addr_lo: 0x7024,
            doorbell: 0x7028,
            doorbell_offset: 0x702c,
            broadcast_ucode_addr: 0x7030,
            broadcast_ucode_data: 0x7034,
            f32_cntl: 0x701c,
            utcl1_cntl: 0x7038,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        let ring = ops.dma_alloc(0, 64 * 1024).unwrap();
        let fence = ops.dma_alloc(0, 4096).unwrap();
        let wptr_poll = ops.dma_alloc(0, 4096).unwrap();
        let poll_dw = 0x20usize;
        // Wedged engine: nothing posts the fence -> the wait must time out.
        assert!(!sdma_submit_and_wait(
            &mut ops,
            &sdma,
            ring.dma_addr,
            64 * 1024,
            9,
            &fence,
            1,
            4,
            &wptr_poll,
            poll_dw,
            0,
        ));
        // Arm: the program_sdma_ring RPTR/WPTR-register clear (still issued) makes the
        // mock post the fence — exercising the poll-success path. (The real submit is
        // the WPTR-poll memory write; the mock keys completion off a register write.)
        ops.gpu_completes_fence = Some((sdma.rb_wptr, fence.dma_addr, 1));
        assert!(sdma_submit_and_wait(
            &mut ops,
            &sdma,
            ring.dma_addr,
            64 * 1024,
            9,
            &fence,
            1,
            16,
            &wptr_poll,
            poll_dw,
            0,
        ));
        // A bad ring size fails fast (before any poll).
        assert!(!sdma_submit_and_wait(
            &mut ops,
            &sdma,
            ring.dma_addr,
            3000,
            9,
            &fence,
            1,
            16,
            &wptr_poll,
            poll_dw,
            0,
        ));
    }

    fn test_device() -> Device {
        Device {
            handle: 1,
            vendor: AMD_VENDOR,
            device: RADEON_760M,
            vram_base: 0,
            vram_size: 0,
            bootup_sclk_mhz: 0,
            bootup_mclk_mhz: 0,
        }
    }

    #[test]
    fn init_gmc_uses_config_memsize() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.memsize_mb = Some(2048); // Athena's real BIOS UMA carve-out
        let mut dev = test_device();
        assert!(init_gmc(&mut ops, &mut dev));
        assert_eq!(dev.vram_size, 2048 * 1024 * 1024);
    }

    #[test]
    fn init_gmc_falls_back_when_memsize_absent_or_implausible() {
        // None -> conservative default (the QEMU / pre-iron path).
        let mut ops = MockOps::new(RADEON_760M, true, None);
        let mut dev = test_device();
        assert!(init_gmc(&mut ops, &mut dev));
        assert_eq!(dev.vram_size, 512 * 1024 * 1024);
        // A garbage/out-of-range read also falls back (never programs nonsense).
        ops.memsize_mb = Some(0xFFFF_FFFF);
        let mut dev2 = test_device();
        assert!(init_gmc(&mut ops, &mut dev2));
        assert_eq!(dev2.vram_size, 512 * 1024 * 1024);
    }

    #[test]
    fn wait_for_gfx_idle_succeeds_after_busy_clears() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.gfx_busy_reads = 3; // busy for 3 polls, idle on the 4th
        assert!(wait_for_gfx_idle(&mut ops, 10));
        assert_eq!(ops.gfx_busy_reads, 0); // consumed exactly the busy polls
    }

    #[test]
    fn wait_for_gfx_idle_times_out_when_stuck() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.gfx_busy_reads = 1000; // never idle within the budget
        assert!(!wait_for_gfx_idle(&mut ops, 5)); // timeout -> false (surfaceable)
    }

    #[test]
    fn smu_send_msg_acks_when_pmfw_responds() {
        let mb = SmuMailbox {
            msg_reg: 0x100,
            arg_reg: 0x104,
            resp_reg: 0x108,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.smu_mailbox = Some(mb);
        ops.smu_responds = true;
        // The PMFW posts a boot/prior-command status — the mailbox reads READY
        // (iron: stage-5 pre-resp=0x1). The pre-check consumes it and sends.
        ops.regs.insert(mb.resp_reg, SMU_RESP_OK);
        assert_eq!(
            smu_send_msg(&mut ops, &mb, 0x02, 0xABCD, 10),
            Some(SMU_RESP_OK)
        );
        // The handshake wrote the argument then the message id.
        assert_eq!(*ops.regs.get(&mb.arg_reg).unwrap(), 0xABCD);
        assert_eq!(*ops.regs.get(&mb.msg_reg).unwrap(), 0x02);
    }

    #[test]
    fn smu_send_msg_times_out_when_pmfw_silent() {
        let mb = SmuMailbox {
            msg_reg: 0x100,
            arg_reg: 0x104,
            resp_reg: 0x108,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.smu_mailbox = Some(mb);
        ops.smu_responds = false; // a hung PMFW never answers
        ops.regs.insert(mb.resp_reg, SMU_RESP_OK); // ready at send time
        assert_eq!(smu_send_msg(&mut ops, &mb, 0x02, 0, 5), None);
        // The message WAS issued (mailbox was ready); only the ack is missing.
        assert_eq!(*ops.regs.get(&mb.msg_reg).unwrap(), 0x02);
    }

    #[test]
    fn smu_send_msg_never_stomps_an_in_flight_command() {
        let mb = SmuMailbox {
            msg_reg: 0x100,
            arg_reg: 0x104,
            resp_reg: 0x108,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        ops.smu_mailbox = Some(mb);
        ops.smu_responds = true;
        // resp_reg stays 0 = a prior command is STILL IN FLIGHT. Writing a new
        // message id on top of it is undefined PMFW behavior (the iron 2026-07-01
        // failure mode: a never-acking SetHardMinGfxClk got stomped by follow-up
        // sends, leaving the PMFW undefined for the whole MES window). The send
        // must ABORT: no message register write, `None` returned.
        assert_eq!(smu_send_msg(&mut ops, &mb, 0x1C, 800, 5), None);
        assert!(ops.regs.get(&mb.msg_reg).is_none());
        assert!(ops.regs.get(&mb.arg_reg).is_none());
    }

    #[test]
    fn parse_rs64_ucode_start_reads_v2_header() {
        // Build a minimal gfx_firmware_header_v2_0: version_major=2 @8,
        // ucode_start_addr_lo @0x34, _hi @0x38.
        let mut blob = alloc::vec![0u8; 0x40];
        blob[8] = 2; // header_version_major (u16 LE) = 2
        blob[0x34..0x38].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        blob[0x38..0x3c].copy_from_slice(&0x0000_00abu32.to_le_bytes());
        assert_eq!(parse_rs64_ucode_start(&blob), Some(0x0000_00ab_1234_5678));
        // A v1 (non-RS64) header is rejected — never a bogus program counter.
        let mut v1 = blob.clone();
        v1[8] = 1;
        assert_eq!(parse_rs64_ucode_start(&v1), None);
        // Too-short blob -> None.
        assert_eq!(parse_rs64_ucode_start(&[0u8; 4]), None);
    }

    #[test]
    fn rs64_pc_start_is_dword_address_with_hi_carry() {
        // Sub-4GiB entry point: pure >>2, hi reg = 0.
        assert_eq!(rs64_pc_start(0x1234_5678), (0x1234_5678u32 >> 2, 0));
        // hi carry: the low 2 bits of addr_hi ride into bit[31:30] of the low reg,
        // the rest goes to the hi reg (addr_hi >> 2). Matches gfx_v11_0_config_gfx_rs64.
        let (lo, hi) = rs64_pc_start(0x0000_000a_0000_0008);
        assert_eq!(lo, (0x8u32 >> 2) | (0xau32 << 30)); // (0xa & 0x3)=2 -> bit31..30
        assert_eq!(hi, 0xau32 >> 2);
    }

    #[test]
    fn config_gfx_rs64_sets_starts_and_pulses_resets() {
        let regs = Rs64CpRegs {
            grbm_gfx_cntl: 0x900,
            me_cntl: 0x803,
            pfp_start: 0xe44,
            pfp_start_hi: 0xe59,
            me_start: 0xe45,
            me_start_hi: 0xe79,
            mec_start: 0x2900,
            mec_start_hi: 0x2938,
            mec_cntl: 0x2904,
        };
        let starts = Rs64UcodeStarts {
            pfp: 0x1111,
            me: 0x2222,
            mec: 0x3333,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        config_gfx_rs64(&mut ops, &regs, &starts);
        // Each engine's program-counter START is the DWORD address (raw >> 2), NOT
        // the raw byte address — the gfx11 RS64 PC register is dword-granular.
        assert_eq!(*ops.regs.get(&regs.pfp_start).unwrap(), 0x1111 >> 2);
        assert_eq!(*ops.regs.get(&regs.me_start).unwrap(), 0x2222 >> 2);
        assert_eq!(*ops.regs.get(&regs.mec_start).unwrap(), 0x3333 >> 2);
        // hi halves are 0 for these sub-4GiB starts.
        assert_eq!(*ops.regs.get(&regs.pfp_start_hi).unwrap(), 0);
        assert_eq!(*ops.regs.get(&regs.me_start_hi).unwrap(), 0);
        // Pipe resets were PULSED back to clear (final CP_ME_CNTL has no reset bits).
        assert_eq!(
            *ops.regs.get(&regs.me_cntl).unwrap()
                & (CP_ME_CNTL_PFP_PIPE_RESET | CP_ME_CNTL_ME_PIPE_RESET),
            0
        );
        assert_eq!(
            *ops.regs.get(&regs.mec_cntl).unwrap() & CP_MEC_RS64_CNTL_PIPE_RESET,
            0
        );
        // GRBM selection restored to default (me0/pipe0/queue0/vmid0 = 0).
        assert_eq!(*ops.regs.get(&regs.grbm_gfx_cntl).unwrap(), 0);
    }

    #[test]
    fn smu_poll_response_times_out_then_reads_ready() {
        let mb = SmuMailbox {
            msg_reg: 0x100,
            arg_reg: 0x104,
            resp_reg: 0x108,
        };
        let mut ops = MockOps::new(RADEON_760M, true, None);
        // Empty response register -> the poll exhausts its budget and reports None
        // (a real, surfaceable timeout — the bug the readback diagnostic chases).
        assert_eq!(smu_poll_response(&mut ops, &mb, 3000), None);
        // Once the PMFW posts a status, the very next poll returns it.
        ops.regs.insert(mb.resp_reg, SMU_RESP_OK);
        assert_eq!(smu_poll_response(&mut ops, &mb, 3000), Some(SMU_RESP_OK));
    }

    fn test_ih_ring() -> IhRing {
        IhRing {
            rb_base: 0x300,
            rb_base_hi: 0x304,
            rb_rptr: 0x308,
            rb_wptr: 0x30c,
        }
    }

    #[test]
    fn ih_drain_consumes_pending_entries() {
        let ring = test_ih_ring();
        let mut ops = MockOps::new(RADEON_760M, true, None);
        let ring_bytes = 256 * 1024;
        // GPU posted 3 cookies: WPTR = 3 entries, RPTR still 0.
        ops.regs.insert(ring.rb_wptr, 3 * IH_ENTRY_BYTES);
        assert_eq!(ih_drain(&mut ops, &ring, ring_bytes), 3);
        // RPTR caught up to WPTR (ring drained), written back for the GPU.
        assert_eq!(*ops.regs.get(&ring.rb_rptr).unwrap(), 3 * IH_ENTRY_BYTES);
        // A second drain with no new cookies consumes nothing.
        assert_eq!(ih_drain(&mut ops, &ring, ring_bytes), 0);
    }

    #[test]
    fn ih_drain_wraps_at_ring_end() {
        let ring = test_ih_ring();
        let mut ops = MockOps::new(RADEON_760M, true, None);
        let ring_bytes = 4 * IH_ENTRY_BYTES; // tiny 4-entry ring
                                             // RPTR near the end, WPTR wrapped past 0: 3 -> 0 -> 1 = 2 entries.
        ops.regs.insert(ring.rb_rptr, 3 * IH_ENTRY_BYTES);
        ops.regs.insert(ring.rb_wptr, IH_ENTRY_BYTES);
        assert_eq!(ih_drain(&mut ops, &ring, ring_bytes), 2);
        assert_eq!(*ops.regs.get(&ring.rb_rptr).unwrap(), IH_ENTRY_BYTES);
    }

    #[test]
    fn submit_and_wait_fence_completes_on_doorbell() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        let gfx = ops.dma_alloc(0, 4096).unwrap();
        let fence = ops.dma_alloc(0, 4096).unwrap();
        let wptr_reg = gc11::MM_CP_RB0_WPTR;
        const FENCE_VAL: u32 = 0xCAFE_F00D;
        // The modeled GPU posts the fence when the doorbell is rung.
        ops.gpu_completes_fence = Some((wptr_reg, fence.dma_addr, FENCE_VAL));
        let stream = pm4::write_data_mem(fence.dma_addr, &[FENCE_VAL]);
        assert!(submit_and_wait_fence(
            &mut ops, &gfx, wptr_reg, &fence, FENCE_VAL, &stream, 10
        ));
        assert_eq!(*ops.regs.get(&wptr_reg).unwrap(), stream.len() as u32);
    }

    #[test]
    fn submit_and_wait_fence_times_out_when_wedged() {
        let mut ops = MockOps::new(RADEON_760M, true, None);
        let gfx = ops.dma_alloc(0, 4096).unwrap();
        let fence = ops.dma_alloc(0, 4096).unwrap();
        let wptr_reg = gc11::MM_CP_RB0_WPTR;
        // gpu_completes_fence stays None -> the GPU never posts the fence.
        let stream = pm4::nop(1);
        assert!(!submit_and_wait_fence(
            &mut ops, &gfx, wptr_reg, &fence, 0xDEAD, &stream, 5
        ));
    }

    #[test]
    fn no_device_is_not_a_failure() {
        struct Absent;
        impl GpuOps for Absent {
            fn pci_enable(&mut self, _b: u8, _d: u8, _f: u8) -> Option<u64> {
                None
            }
            fn config_read_dword(&mut self, _h: u64, _o: u16) -> u32 {
                0
            }
            fn map_register_bar(&mut self, _h: u64, _b: u8) -> bool {
                false
            }
            fn reg_read(&mut self, _o: u32) -> u32 {
                0
            }
            fn reg_write(&mut self, _o: u32, _v: u32) {}
            fn read_vbios_rom(&mut self, _h: u64, _m: usize) -> Option<Vec<u8>> {
                None
            }
            fn dma_alloc(&mut self, _h: u64, _s: usize) -> Option<DmaBuf> {
                None
            }
            fn dma_write(&mut self, _b: &DmaBuf, _o: usize, _d: &[u32]) {}
            fn request_firmware(&mut self, _n: &str) -> bool {
                false
            }
            fn commit_scanout(&mut self, _w: u32, _h: u32, _p: u32, _a: u64) -> bool {
                false
            }
            fn log(&mut self, _m: &str) {}
        }
        let report = bringup(&mut Absent, &[(0, 1, 0)]);
        assert!(!report.device_present);
        assert!(!report.all_ok());
    }
}
