//! amdgpud — AthenaOS userspace AMD GPU driver daemon.
//!
//! Runs the real amdgpu initialization pipeline (mirroring `amdgpu_device_init`
//! → `amdgpu_device_ip_init`) on top of the LinuxKPI host + ath_drm:
//!
//!   1. PCI enable + BAR map           (ath_linuxkpi: pci_enable, ioremap)
//!   2. VBIOS / ATOMBIOS read          (BAR / ROM)
//!   3. GMC / GPUVM (memory controller + GPU page tables)
//!   4. IH ring (interrupt handler)
//!   5. SMU / PSP power-up + firmware
//!   6. GFX + SDMA rings (drm_sched)
//!   7. DC (Display Core) modeset → first scanout (ath_drm KMS)
//!
//! On QEMU there is no Radeon, so PCI probe reports "no AMD GPU present" and the
//! daemon exits cleanly. On real hardware (Athena Radeon 760M, `c4:00.0`
//! `1002:15bf`) it walks every stage. This is the driver-port skeleton; each IP
//! block's register sequences are the remaining bring-up work (documented per
//! stage).

#![no_std]
#![no_main]

extern crate alloc;

use ath_abi::syscall as abi;
#[cfg(feature = "real_amdgpu_init")]
use ath_amdgpu::bringup::GpuOps;
use ath_drm::kms;
use core::panic::PanicInfo;

const _: () = assert!(ath_abi::ABI_VERSION == 4);

// Global allocator backed by the LinuxKPI heap (ath_linuxkpi::kmalloc/kfree).
// ath_drm + this daemon use alloc::{Vec, String, format}.
struct LkpiAllocator;

unsafe impl core::alloc::GlobalAlloc for LkpiAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        ath_linuxkpi::kmalloc(layout.size(), 0)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: core::alloc::Layout) {
        ath_linuxkpi::kfree(ptr);
    }
}

#[global_allocator]
static ALLOCATOR: LkpiAllocator = LkpiAllocator;

#[inline(always)]
unsafe fn sys_print(value: u64) {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_PRINT,
        in("rdi") value,
        out("rcx") _, out("r11") _,
    );
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_EXIT,
        in("rdi") code,
        options(noreturn),
    );
}

/// Self-promote amdgpud into the Game scheduling class (SYS_SETPRIORITY=21,
/// target=0=self, prio=1=Game). The GPU bring-up polls the SMU/IMU/RLC with
/// `msleep` (yield) loops; at the default Normal priority each yield only resumes
/// after the SCHED_BODY compositor's next frame on CPU0 — the one CPU that
/// schedules post-boot (the APs idle) — which stretched nominal ~2s polls into
/// ~90s and the whole bring-up to ~480s on iron (2026-06-28: ~225s of it stuck in
/// two yield-bound stalls). Game priority lets the polls resume promptly. The
/// kernel permits SELF-promotion to Game only (syscall.rs BUG M-2), so this must
/// run from inside the daemon. The daemon exits after bring-up, dropping it.
#[inline(always)]
unsafe fn sys_setpriority_self_game() {
    core::arch::asm!(
        "syscall",
        inout("rax") 21u64 => _, // SYS_SETPRIORITY (docs/SYSCALL_TABLE.md)
        in("rdi") 0u64,          // target = self
        in("rsi") 1u64,          // prio = 1 = Game
        out("rcx") _, out("r11") _,
    );
}

/// Self-demote amdgpud back to the Normal scheduling class (SYS_SETPRIORITY=21,
/// target=0=self, prio=0=Normal). Called BEFORE the live-GFX `init_rings` path:
/// once GFX is powered, that path can busy-poll a ring/fence that wedges CPU 0.
/// At Game priority a wedge starves the netlog-broadcast + 480s auto-return
/// threads (2026-06-29: a post-first-light hang stranded the box with zero
/// capture, needing a manual power cycle). At Normal priority the scheduler
/// round-robins, so the safe-progress broadcaster keeps capturing AND the
/// auto-return still fires — the box self-recovers and we see where it wedged.
/// Self-demotion is always permitted (syscall.rs only gates promoting OTHERS).
unsafe fn sys_setpriority_self_normal() {
    core::arch::asm!(
        "syscall",
        inout("rax") 21u64 => _, // SYS_SETPRIORITY (docs/SYSCALL_TABLE.md)
        in("rdi") 0u64,          // target = self
        in("rsi") 0u64,          // prio = 0 = Normal
        out("rcx") _, out("r11") _,
    );
}

fn klog(msg: &str) {
    let mut buf = [0u8; 160];
    let n = msg.len().min(159);
    buf[..n].copy_from_slice(&msg.as_bytes()[..n]);
    buf[n] = 0;
    ath_linuxkpi::athena_printk(buf.as_ptr());
}

/// SYS_NETLOG_FLUSH (296): broadcast the kernel bootlog ring over UDP NOW.
/// Synchronous on CPU 0 in the daemon's own context — it does NOT depend on the
/// safe-progress broadcaster / auto-return threads (which starve when this
/// daemon holds CPU 0 at Game priority). So a marker logged-then-flushed here
/// reaches the wire BEFORE the next (possibly CPU-0-wedging) stage runs.
#[inline(always)]
fn netlog_flush() {
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") abi::SYS_NETLOG_FLUSH => _,
            out("rcx") _, out("r11") _,
        );
    }
}

/// Diagnostic seam used by the upstream C bring-up at hardware ownership
/// boundaries. The vendored driver does not need to know syscall numbers.
#[cfg(feature = "real_amdgpu_init")]
#[no_mangle]
pub extern "C" fn rae_diag_netlog_flush() {
    // The RTL NIC can drop the tail of a 150+ datagram snapshot when it is
    // emitted as one burst. Retransmit after short scheduler yields; the
    // listener is last-write-wins by sequence number, so passes fill holes.
    for _ in 0..3 {
        netlog_flush();
        ath_linuxkpi::msleep(5);
    }
}

/// Fence a real-amdgpu bring-up phase onto the wire: print the sentinel (serial),
/// log the human label (into the ring), then broadcast the ring synchronously.
/// If the NEXT stage hard-hangs CPU 0, the netlog trail ends at exactly this
/// label — the "where did it wedge" answer, hang-proof by construction.
fn flush_marker(sentinel: u64, label: &str) {
    unsafe { sys_print(sentinel) };
    klog(label);
    netlog_flush();
}

/// Debug hook the vendored amdgpu C can call to dump pointers through the
/// reliable Rust `klog` (the C vscnprintf-varargs path faults on %lx). Used to
/// diagnose the soc21_common_early_init `nbio.funcs` bring-up wall.
#[cfg(feature = "real_amdgpu_init")]
#[no_mangle]
pub extern "C" fn rae_dbg_ptrs(a: u64, b: u64, c: u64) {
    klog(&alloc::format!(
        "[amdgpu] DBG a={:#x} b={:#x} c={:#x}",
        a,
        b,
        c
    ));
}

/// Phoenix1 / Radeon 760M (Athena) firmware set — the signed blobs the PSP/SMU
/// and command processors need. On Athena these are dropped into the initramfs
/// `firmware/amdgpu/` tree; the kernel serves them via request_firmware (142).
/// The canonical list lives in `ath_amdgpu::bringup::FW_PHOENIX` so the
/// preflight and the stage code can never drift apart.
const AMDGPU_FW_PHOENIX: &[&str] = ath_amdgpu::bringup::FW_PHOENIX;

/// Load one firmware blob through the LinuxKPI host (`request_firmware` → 142).
/// Returns true if present and mapped.
fn load_fw(name: &str) -> bool {
    match ath_linuxkpi::request_firmware_blob(name) {
        Some((_, sz)) if sz > 0 => {
            klog(&alloc::format!(
                "[amdgpu] firmware '{}' loaded ({} bytes)",
                name,
                sz
            ));
            true
        }
        _ => {
            klog(&alloc::format!("[amdgpu] firmware '{}' absent", name));
            false
        }
    }
}

/// Firmware preflight — runs regardless of GPU presence so the request_firmware
/// path is exercised even on QEMU (where there is no Radeon and the IP pipeline
/// is never reached). Proves the loader works against the packed self-test blob,
/// then reports how many Phoenix/780M amdgpu blobs are present (0 until the real
/// signed microcode is added to the initramfs `firmware/amdgpu/` tree on Athena).
/// Sentinels: 9050 start, 9050+present_count.
fn firmware_preflight() {
    unsafe { sys_print(9050) };
    let loader_ok = load_fw("athena-selftest.bin");
    let mut present = 0u32;
    for (index, name) in AMDGPU_FW_PHOENIX.iter().enumerate() {
        // Every preflight request is a synchronous netlog fence. The real C
        // driver has not started yet, so an iron stop here must be attributed to
        // the LinuxKPI firmware syscall itself rather than PSP/MES. In
        // particular, this distinguishes the request for the second Phoenix
        // blob (the PSP TA) from a successful TOC load.
        flush_marker(
            9050,
            &alloc::format!("[amdgpu] FW-PREFLIGHT {} ENTER '{}'", index, name),
        );
        if load_fw(name) {
            present += 1;
        }
        flush_marker(
            9050,
            &alloc::format!("[amdgpu] FW-PREFLIGHT {} RETURN '{}'", index, name),
        );
    }
    klog(&alloc::format!(
        "[amdgpu] firmware preflight: loader={} amdgpu_blobs={}/{} present",
        if loader_ok { "ok" } else { "FAIL" },
        present,
        AMDGPU_FW_PHOENIX.len()
    ));
    unsafe { sys_print(9050 + present as u64) };
}

/// LinuxKPI-backed [`GpuOps`] — the platform operations `amdgpu_device_init`
/// needs, routed to the real `ath_linuxkpi` shim (PCI claim, ioremap, MMIO,
/// IOMMU-sandboxed DMA, request_firmware) + `ath_drm::kms` for the modeset
/// commit. The bring-up STAGE SEQUENCE lives in `ath_amdgpu::bringup` and is
/// generic over this trait, so the identical code path is replayed against a
/// mock register file by `tools/linuxkpi_harness` (host, no QEMU/iron).
///
/// [`GpuOps`]: ath_amdgpu::bringup::GpuOps
struct LkpiGpuOps {
    /// Most-recently-claimed LinuxKPI device handle (for the supervisor).
    handle: u64,
    /// Base of the most-recently-mapped register BAR; `reg_read`/`reg_write`
    /// offset from here.
    mmio: *mut u8,
    /// Base of the doorbell BAR (BAR2, separate from the register BAR5); `ring_doorbell`
    /// does a 64-bit write at an offset from here to wake an engine. Mapped alongside
    /// BAR5; null until then (then `ring_doorbell` is a no-op, as on QEMU).
    doorbell_mmio: *mut u8,
    /// IP-discovery blocks parsed from the GPU's discovery table (top-of-VRAM).
    /// `None` until `read_discovery` succeeds — then the offset methods resolve
    /// authoritative SOC15 register offsets via `ath_amdgpu::regs`, instead of
    /// staying gated. None on QEMU / if the read fails → bring-up falls back.
    discovery_blocks: Option<alloc::vec::Vec<ath_amdgpu::discovery::IpBlock>>,
    /// PSP TOC blob staged in VRAM: `(GPU MC addr, byte size)`, set lazily by
    /// `psp_toc_blob` (load `psp_13_0_4_toc.bin` → MM_INDEX-write into VRAM). Cached so
    /// the firmware-load handshake stages it once.
    psp_toc_staged: Option<(u64, u32)>,
}

impl LkpiGpuOps {
    fn new() -> Self {
        Self {
            handle: 0,
            mmio: core::ptr::null_mut(),
            doorbell_mmio: core::ptr::null_mut(),
            discovery_blocks: None,
            psp_toc_staged: None,
        }
    }

    /// Read one VRAM dword via the MM_INDEX/MM_DATA indirect aperture (mirrors
    /// amdgpu_device_mm_access). BAR5 byte offsets: MM_INDEX=0x0, MM_DATA=0x4,
    /// MM_INDEX_HI=0x18. `self.mmio` must already point at BAR5.
    unsafe fn mm_read_dw(&self, p: u64, hi: &mut u32) -> u32 {
        let p_hi = (p >> 31) as u32;
        ath_linuxkpi::pci::writel((p as u32) | 0x8000_0000, self.mmio.add(0x0) as *mut u32);
        if p_hi != *hi {
            ath_linuxkpi::pci::writel(p_hi, self.mmio.add(0x18) as *mut u32);
            *hi = p_hi;
        }
        ath_linuxkpi::pci::readl(self.mmio.add(0x4) as *const u32)
    }

    /// Write one VRAM dword via the MM_INDEX/MM_DATA indirect aperture (the write
    /// sibling of [`mm_read_dw`]; the 0x80000000 FB bit selects VRAM). Used to stage the
    /// PSP command buffer / ring frames / firmware into VRAM without a BAR0 mapping.
    unsafe fn mm_write_dw(&self, p: u64, val: u32, hi: &mut u32) {
        let p_hi = (p >> 31) as u32;
        ath_linuxkpi::pci::writel((p as u32) | 0x8000_0000, self.mmio.add(0x0) as *mut u32);
        if p_hi != *hi {
            ath_linuxkpi::pci::writel(p_hi, self.mmio.add(0x18) as *mut u32);
            *hi = p_hi;
        }
        ath_linuxkpi::pci::writel(val, self.mmio.add(0x4) as *mut u32);
    }

    /// Build the ordered gfx firmware list for the PSP `LOAD_IP_FW` sequence, in the
    /// EXACT order the working amdgpu submits on the Athena (the last untested autoload
    /// variable — 2026-07-01). Captured psp_cmd_submit_buf trace
    /// (docs/gpu-oracle/MES-FWTYPE-FIX-2026-06-28.md, lines 16-34):
    ///   SDMA(71,72) → CP PFP/ME/MEC(2,1,4) → MES pipe0/KIQ(33,34,81,82) →
    ///   IMU(68,69) → RLC GPM/SRM(20,21) → RLC_IRAM/DRAM(26,48) → RLC_P(25) → RLC_G(8).
    /// RLC_G is LAST (the PSP starts RLC autoload right after it). Note IMU is loaded
    /// AFTER the engines here, NOT first — LOAD_IP_FW only STAGES each blob into its
    /// region; the GFX power-up runs at AUTOLOAD_RLC time, so amdgpu stages the engines
    /// before IMU. The GPM/SRM restore lists (2026-07-01 diff) loaded fine but did not by
    /// themselves clear the set_hw_resources halt; matching the full ORDER is the remaining
    /// autoload variable to eliminate. Reuses the same extractors as the DIRECT-load path;
    /// a blob that fails to load/parse is skipped (the per-blob LOAD_IP_FW result localizes
    /// any PSP rejection on iron).
    fn build_gfx_fw_blobs(&mut self) -> alloc::vec::Vec<(u32, alloc::vec::Vec<u8>)> {
        use ath_amdgpu::bringup as bu;
        use ath_amdgpu::bringup::GpuOps as _; // bring request_firmware_bytes into scope
        let mut out: alloc::vec::Vec<(u32, alloc::vec::Vec<u8>)> = alloc::vec::Vec::new();

        // 1. SDMA (SDMA_UCODE_TH0=71 / TH1=72) — amdgpu loads these FIRST (iron mmiotrace
        // 2026-06-28: ftype 71 sz=17408 = TH0/ctx, ftype 72 sz=16896 = TH1/ctl). extract_sdma_threads
        // slices ctx_jt_offset/ctl_jt_offset = 17408/16896 (host-KAT'd vs sdma_firmware_header_v2_0),
        // matching the trace. SDMA command execution is INDEPENDENT of the gfx/MES KIQ path.
        if let Some(b) = self.request_firmware_bytes("amdgpu/sdma_6_0_1.bin") {
            if let Some((th0, th1)) = ath_amdgpu::rlc_autoload::extract_sdma_threads(&b) {
                out.push((bu::GFX_FW_TYPE_SDMA_UCODE_TH0, th0.to_vec()));
                out.push((bu::GFX_FW_TYPE_SDMA_UCODE_TH1, th1.to_vec()));
            }
        }

        // 2. CP F32 PFP(2)/ME(1)/MEC(4) — single-section ucode behind the gfx fw header.
        for (name, ty) in [
            ("amdgpu/gc_11_0_1_pfp.bin", bu::GFX_FW_TYPE_CP_PFP),
            ("amdgpu/gc_11_0_1_me.bin", bu::GFX_FW_TYPE_CP_ME),
            ("amdgpu/gc_11_0_1_mec.bin", bu::GFX_FW_TYPE_CP_MEC),
        ] {
            if let Some(b) = self.request_firmware_bytes(name) {
                if let Some(uc) = ath_amdgpu::rlc_autoload::extract_common_ucode(&b) {
                    out.push((ty, uc.to_vec()));
                }
            }
        }

        // 3. MES — CORRECT fw types (2026-06-28 iron crack): 33/34 (pipe0, mes_2.bin) + 81/82
        // (KIQ, mes1.bin), captured from amdgpu's psp_cmd_submit_buf payload. The RS64_MES values
        // 76-79 were the WRONG type numbers for THIS PSP/firmware (0xffff0006 reject); with the
        // right types the PSP loads + sets up the MES so it comes up PSP-loaded like amdgpu.
        for (name, ucode_ty, data_ty) in [
            (
                "amdgpu/gc_11_0_1_mes_2.bin",
                bu::GFX_FW_TYPE_MES_PIPE0_UCODE,
                bu::GFX_FW_TYPE_MES_PIPE0_DATA,
            ),
            (
                "amdgpu/gc_11_0_1_mes1.bin",
                bu::GFX_FW_TYPE_MES_PIPE1_UCODE,
                bu::GFX_FW_TYPE_MES_PIPE1_DATA,
            ),
        ] {
            if let Some(b) = self.request_firmware_bytes(name) {
                if let Some((ucode, data)) = ath_amdgpu::rlc_autoload::extract_mes_ucode_data(&b) {
                    out.push((ucode_ty, ucode.to_vec()));
                    out.push((data_ty, data.to_vec()));
                }
            }
        }

        // 4. IMU I-RAM(68) + D-RAM(69) (gc_*_imu.bin) — the GFX power-up ucode. amdgpu stages
        // it here (AFTER the engines), not first: LOAD_IP_FW only copies the blob into its
        // region; the power-up runs at AUTOLOAD_RLC time.
        if let Some(imu) = self.request_firmware_bytes("amdgpu/gc_11_0_1_imu.bin") {
            if let Some(l) = ath_amdgpu::imu::parse_imu_ucode_layout(&imu) {
                if let Some(s) = imu.get(l.iram_offset..l.iram_offset + l.iram_size) {
                    out.push((bu::GFX_FW_TYPE_IMU_I, s.to_vec()));
                }
                if let Some(s) = imu.get(l.dram_offset..l.dram_offset + l.dram_size) {
                    out.push((bu::GFX_FW_TYPE_IMU_D, s.to_vec()));
                }
            }
        }

        // 5-8. RLC family, in amdgpu's order: GPM(20)/SRM(21) restore lists → RLX6 IRAM(26)/
        // DRAM_BOOT(48) → RLC_P(25) → RLC_G(8) LAST (the PSP starts the RLC autoload right
        // after RLC_G). The GPM/SRM restore lists (the RLC uses them to establish + restore the
        // GFX pipeline state) were OMITTED before 2026-07-01; loaded fine but did not alone clear
        // the set_hw_resources halt. RLC_G alone is not enough — the RLX6 program + RLC_P are
        // needed for the autoload to complete (BOOTLOAD_STATUS stayed 0 without them).
        let rlc_blob = self.request_firmware_bytes("amdgpu/gc_11_0_1_rlc.bin");
        if let Some(rlc) = rlc_blob.as_deref() {
            if let Some(gpm) = ath_amdgpu::rlc_autoload::extract_rlc_gpm(rlc) {
                out.push((bu::GFX_FW_TYPE_RLC_RESTORE_LIST_GPM_MEM, gpm.to_vec()));
            }
            if let Some(srm) = ath_amdgpu::rlc_autoload::extract_rlc_srm(rlc) {
                out.push((bu::GFX_FW_TYPE_RLC_RESTORE_LIST_SRM_MEM, srm.to_vec()));
            }
            if let Some((iram, dram)) = ath_amdgpu::rlc_autoload::extract_rlc_rlx6(rlc) {
                out.push((bu::GFX_FW_TYPE_RLC_IRAM, iram.to_vec()));
                out.push((bu::GFX_FW_TYPE_RLC_DRAM_BOOT, dram.to_vec()));
            }
            if let Some(rlcp) = ath_amdgpu::rlc_autoload::extract_rlcp(rlc) {
                out.push((bu::GFX_FW_TYPE_RLC_P, rlcp.to_vec()));
            }
            // RLC_G LAST — the PSP starts the RLC autoload right after it loads.
            if let Some(uc) = ath_amdgpu::rlc_autoload::extract_common_ucode(rlc) {
                out.push((bu::GFX_FW_TYPE_RLC_G, uc.to_vec()));
            }
        }
        out
    }

    /// Read the GPU IP-discovery table → register bases. BATCHED DIAGNOSTIC: logs
    /// a progress marker before each step (safe reads first, the risky indirect
    /// read LAST) so a single Athena boot pinpoints exactly how far it gets — if
    /// a line is the LAST one in the log, the next operation wedged. Now runs at
    /// the stage-1 BAR5 point (map_register_bar), the access the reg-probe does
    /// successfully — not mid-pci_enable, which wedged on 06-16T1719/1739/1745.
    fn read_discovery(&mut self, handle: u64) {
        // ── Instrumented FS-first discovery (iron-debug, d94ba16 follow-up) ──────
        // The d94ba16 boot loaded ip_discovery.bin (10240 B at user_virt) then
        // stalled before CKPT 1 with NO further output — static analysis refuted
        // every short-map / supervisor-only / parser-loop theory, so the suspect
        // is the daemon's FIRST read of the UC-mapped firmware page (faults or
        // returns aliased garbage on this AMD silicon). These self-persisting
        // checkpoints (log + 8s sleep, like `checkpoint`) bracket each sub-step so
        // the next bootlog ENDS at the exact failing line: syscall-return, the
        // first-dword UC read, the full copy, or the parse. On success/absent we
        // return; only a parse-INVALID blob falls through to the legacy VRAM read
        // (a known CPU-0 wedge) — and only AFTER DISC 5 has persisted.
        checkpoint(
            "[amdgpu] DISC 1: read_discovery entered — requesting ip_discovery.bin (raw ptr/sz)",
        );
        if let Some((ptr, sz)) = ath_linuxkpi::request_firmware_blob("amdgpu/ip_discovery.bin") {
            checkpoint(&alloc::format!(
                "[amdgpu] DISC 2: syscall returned ptr={:#x} sz={} — about to read first dword (UC test)",
                ptr as u64,
                sz
            ));
            if !ptr.is_null() && sz >= 4 {
                // Read ONLY the first dword first: isolates a UC-firmware-read
                // fault/garbage to a single access before the full memcpy.
                let first4 = unsafe { core::ptr::read_volatile(ptr as *const u32) };
                checkpoint(&alloc::format!(
                    "[amdgpu] DISC 3: first dword = {:#010x} (want {:#010x}) — copying full {} bytes",
                    first4,
                    ath_amdgpu::discovery::BINARY_SIGNATURE,
                    sz
                ));
                let blob = unsafe { core::slice::from_raw_parts(ptr, sz) }.to_vec();
                checkpoint(&alloc::format!(
                    "[amdgpu] DISC 4: full blob copied ({} bytes) — parsing",
                    blob.len()
                ));
                match ath_amdgpu::discovery::parse_checked(&blob) {
                    Some(blocks) => {
                        checkpoint(&alloc::format!(
                            "[amdgpu] DISC 5: parsed {} IP blocks — SOC15 offsets ACTIVE",
                            blocks.len()
                        ));
                        self.discovery_blocks = Some(blocks);
                        return;
                    }
                    None => {
                        checkpoint(
                            "[amdgpu] DISC 5: signature/parse INVALID — falling to VRAM read",
                        );
                    }
                }
            } else {
                checkpoint("[amdgpu] DISC 2b: null/short blob — falling to VRAM read");
            }
        } else {
            checkpoint("[amdgpu] DISC 2: ip_discovery.bin ABSENT — falling to VRAM read");
        }
        const RCC_CONFIG_MEMSIZE: usize = 0xde3 << 2; // 0x378c
        const TMR_OFFSET: u64 = 64 * 1024;
        const BLOB_MAX: usize = 16 * 1024;
        let sig = ath_amdgpu::discovery::BINARY_SIGNATURE;
        if self.mmio.is_null() {
            self.mmio = ath_linuxkpi::pci::ioremap(handle, 5);
        }
        if self.mmio.is_null() {
            klog("[amdgpu] discovery: BAR5 map failed — gated");
            return;
        }
        klog("[amdgpu] discovery[1/5]: BAR5 mapped (stage-1 point) — reading CONFIG_MEMSIZE");
        // (1) plain BAR5 register read — VRAM size.
        let memsize_mb =
            unsafe { ath_linuxkpi::pci::readl(self.mmio.add(RCC_CONFIG_MEMSIZE) as *const u32) };
        klog(&alloc::format!(
            "[amdgpu] discovery[2/5]: CONFIG_MEMSIZE raw={:#010x} ({} MiB)",
            memsize_mb,
            memsize_mb
        ));
        // (2) sample known registers — cross-check BAR5 IS the register aperture
        // (CP_WPTR[0x3048] should read the GOP-programmed ~0x03ffa208 the reg-probe saw).
        let grbm = unsafe { ath_linuxkpi::pci::readl(self.mmio.add(0x8010) as *const u32) };
        let wptr = unsafe { ath_linuxkpi::pci::readl(self.mmio.add(0x3048) as *const u32) };
        klog(&alloc::format!(
            "[amdgpu] discovery[3/5]: sample GRBM[0x8010]={:#010x} CP_WPTR[0x3048]={:#010x}",
            grbm,
            wptr
        ));
        let vram_size = (memsize_mb as u64) << 20;
        if memsize_mb == 0 || memsize_mb == 0xffff_ffff || vram_size <= TMR_OFFSET {
            klog("[amdgpu] discovery: CONFIG_MEMSIZE implausible — skip indirect read (gated)");
            return;
        }
        // OBSERVABILITY: prior boots went fully blind because the risky read
        // wedges CPU 0 BEFORE the first late-flush (~7s), starving the bootlog
        // capture thread (no "LATE FLUSH" markers appeared). Sleep here so CPU 0
        // is free for the late-flush to PERSIST [1/5]-[3/5] above; msleep blocks
        // the daemon (yields the CPU), so the 7s/14s flushes fire during it.
        // Then even if [4/5] wedges, the safe diagnostics are already on disk.
        klog("[amdgpu] discovery: sleeping 16s so the late-flush persists [1-3/5] before the risky read");
        ath_linuxkpi::msleep(16000);
        // (3) the RISKY step LAST: one MM_INDEX/MM_DATA round-trip. If the [4/5]
        // line below is the last one in the log, this indirect read wedged.
        let pos_base = vram_size - TMR_OFFSET;
        klog(&alloc::format!(
            "[amdgpu] discovery[4/5]: attempting MM_INDEX read of VRAM {:#x} (probe 1 dword)",
            pos_base
        ));
        let mut hi = u32::MAX;
        let probe = unsafe { self.mm_read_dw(pos_base, &mut hi) };
        klog(&alloc::format!(
            "[amdgpu] discovery[5/5]: VRAM[{:#x}]={:#010x} (want sig {:#010x})",
            pos_base,
            probe,
            sig
        ));
        if probe != sig {
            klog("[amdgpu] discovery: no signature via MM_INDEX at VRAM_top-64K — gated (try other offset)");
            return;
        }
        // signature matched — read + parse the full blob.
        let mut blob = alloc::vec![0u8; BLOB_MAX];
        unsafe {
            let dst = blob.as_mut_ptr() as *mut u32;
            for i in 0..(BLOB_MAX / 4) {
                *dst.add(i) = self.mm_read_dw(pos_base + (i as u64) * 4, &mut hi);
            }
        }
        let blocks = ath_amdgpu::discovery::parse(&blob);
        klog(&alloc::format!(
            "[amdgpu] discovery: parsed {} IP blocks — SOC15 offsets {}",
            blocks.len(),
            if blocks.is_empty() {
                "GATED (parse failed)"
            } else {
                "ACTIVE"
            }
        ));
        if !blocks.is_empty() {
            self.discovery_blocks = Some(blocks);
        }
    }

    /// Shared post-claim bookkeeping: stash the handle and register with the
    /// LinuxKPI supervisor NOW (before the long IP-init runs) so a crash
    /// mid-bring-up triggers a clean restart.
    fn claimed(&mut self, h: u64) -> Option<u64> {
        self.handle = h;
        let _ = ath_linuxkpi::lkpi_supervisor_register(h);
        ath_linuxkpi::lkpi_supervisor_heartbeat(h);
        // NOTE: do NOT touch BAR5 here. claimed() runs mid-pci_enable, and a
        // BAR5 access this early wedged the daemon on 06-16T1719/1739/1745
        // (no userspace output at all). Discovery is read in map_register_bar
        // instead — the stage-1 point where the reg-probe reads BAR5 fine.
        Some(h)
    }
}

impl ath_amdgpu::bringup::GpuOps for LkpiGpuOps {
    fn pci_enable(&mut self, bus: u8, dev: u8, func: u8) -> Option<u64> {
        let h = ath_linuxkpi::pci::pci_enable(bus, dev, func);
        if h >= 0xFFFF_FFFF_FFFF_F000 {
            return None;
        }
        self.claimed(h)
    }

    fn pci_enable_match(&mut self, class: u8, vendor: u16) -> Option<u64> {
        let h = ath_linuxkpi::pci_enable_match(class, vendor)?;
        self.claimed(h)
    }

    fn config_read_dword(&mut self, handle: u64, offset: u16) -> u32 {
        ath_linuxkpi::pci::read_config_dword(handle, offset)
    }

    fn map_register_bar(&mut self, handle: u64, bar: u8) -> bool {
        // amdgpu maps BAR5 for the MMIO register aperture (BAR0 is VRAM).
        let p = ath_linuxkpi::pci::ioremap(handle, bar);
        if p.is_null() {
            return false;
        }
        self.mmio = p;
        // Read IP discovery NOW — BAR5 is mapped and the device is fully enabled
        // (pci_enable already returned). This is exactly the point the stage-1
        // reg-probe reads BAR5 successfully; doing it earlier (in claimed(), mid
        // pci_enable) wedged the daemon. Runs before the offset-consuming stages
        // 4/5/6, so discovery_blocks is ready when they query the offsets.
        if bar == 5 {
            // Map the doorbell BAR (BAR2) too — separate from the register BAR5 —
            // so ring_doorbell can wake the SDMA/CP engines via a 64-bit write.
            // Null on failure → ring_doorbell becomes a safe no-op.
            self.doorbell_mmio = ath_linuxkpi::pci::ioremap(handle, 2);
            klog(if self.doorbell_mmio.is_null() {
                "[amdgpu] doorbell BAR (BAR2) map FAILED — doorbell rings will no-op"
            } else {
                "[amdgpu] doorbell BAR (BAR2) mapped"
            });
            self.read_discovery(handle);
        }
        true
    }

    fn reg_read(&mut self, off: u32) -> u32 {
        if self.mmio.is_null() {
            return 0;
        }
        let addr = unsafe { self.mmio.add(off as usize) } as *const u32;
        ath_linuxkpi::pci::readl(addr)
    }

    fn reg_write(&mut self, off: u32, val: u32) {
        if self.mmio.is_null() {
            return;
        }
        let addr = unsafe { self.mmio.add(off as usize) } as *mut u32;
        ath_linuxkpi::pci::writel(val, addr);
    }

    fn ring_doorbell(&mut self, byte_offset: u32, value: u64) {
        // 64-bit atomic write into the doorbell BAR (BAR2) — wakes the engine to
        // consume its ring (amdgpu WDOORBELL64). No-op if BAR2 isn't mapped.
        if self.doorbell_mmio.is_null() {
            return;
        }
        let addr = unsafe { self.doorbell_mmio.add(byte_offset as usize) } as *mut u64;
        ath_linuxkpi::pci::writeq(value, addr);
    }

    fn delay_us(&mut self, usec: u32) {
        // ath_linuxkpi only exposes ms-granular msleep; round up so a sub-ms
        // request still yields ~1ms. Fine for the SMU ~1s poll budget, and msleep
        // YIELDS CPU 0 (to the bootlog-flush thread) instead of busy-spinning it.
        ath_linuxkpi::msleep(((usec + 999) / 1000).max(1));
    }

    fn read_vbios_rom(&mut self, handle: u64, max_len: usize) -> Option<alloc::vec::Vec<u8>> {
        // VBIOS lives in the PCI expansion ROM (BAR6). On QEMU there is no ROM so
        // the map fails (None) and bring-up treats absent VBIOS as non-fatal.
        let rom_ptr = ath_linuxkpi::pci::ioremap(handle, 6);
        if rom_ptr.is_null() {
            return None;
        }
        let slice = unsafe { core::slice::from_raw_parts(rom_ptr, max_len) };
        Some(slice.to_vec())
    }

    fn dma_alloc(&mut self, handle: u64, size: usize) -> Option<ath_amdgpu::bringup::DmaBuf> {
        let a = ath_linuxkpi::dma::dma_alloc_coherent(handle, size);
        if a.is_null() {
            return None;
        }
        // `dma_addr` is the bus address programmed into the ring registers; `id`
        // carries the CPU virtual address so `dma_write` can fill the buffer.
        Some(ath_amdgpu::bringup::DmaBuf {
            dma_addr: a.dma_addr,
            size: a.size,
            id: a.cpu_addr as u64,
        })
    }

    fn dma_write(&mut self, buf: &ath_amdgpu::bringup::DmaBuf, offset_dw: usize, data: &[u32]) {
        if buf.id == 0 {
            return;
        }
        let dst = buf.id as *mut u32;
        for (i, w) in data.iter().enumerate() {
            unsafe { core::ptr::write_volatile(dst.add(offset_dw + i), *w) };
        }
    }

    fn dma_write_bytes(
        &mut self,
        buf: &ath_amdgpu::bringup::DmaBuf,
        byte_offset: usize,
        data: &[u8],
    ) {
        if buf.id == 0 || data.is_empty() {
            return;
        }
        // `buf.id` is the CPU virtual address of the coherent buffer; memcpy the
        // assembled autoload buffer into it. Bounded by the BO size the kernel gave.
        let dst = (buf.id as usize + byte_offset) as *mut u8;
        let n = data.len().min(buf.size.saturating_sub(byte_offset));
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), dst, n) };
    }

    fn dma_read(&mut self, buf: &ath_amdgpu::bringup::DmaBuf, offset_dw: usize, out: &mut [u32]) {
        // Symmetric with `dma_write`: `buf.id` is the CPU virtual address of the
        // coherent DMA buffer, so the daemon reads back exactly what the GPU
        // posted to memory (e.g. a RELEASE_MEM fence or a WPTR writeback).
        if buf.id == 0 {
            out.iter_mut().for_each(|x| *x = 0);
            return;
        }
        let src = buf.id as *const u32;
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = unsafe { core::ptr::read_volatile(src.add(offset_dw + i)) };
        }
    }

    fn request_firmware(&mut self, name: &str) -> bool {
        load_fw(name)
    }

    fn request_firmware_bytes(&mut self, name: &str) -> Option<alloc::vec::Vec<u8>> {
        let (ptr, sz) = ath_linuxkpi::request_firmware_blob(name)?;
        if ptr.is_null() || sz == 0 {
            return None;
        }
        // The host maps the blob read-only into this daemon at `ptr` for `sz`
        // bytes (syscall 142 contract); copy out so the parser owns its data.
        let slice = unsafe { core::slice::from_raw_parts(ptr, sz) };
        Some(slice.to_vec())
    }

    // ── Register offsets — resolved from IP discovery (ath_amdgpu::regs) ───────
    // Once `read_discovery` parsed the table, every offset is the authoritative
    // SOC15 value `(ip_base(HWID,seg) + reg) << 2`. Until then (QEMU / read
    // failed) `discovery_blocks` is None and these return None, so the bring-up
    // stages fall back / skip — never writing a guessed MMIO offset on iron.

    fn config_memsize_mb(&mut self) -> Option<u32> {
        // nbio get_memsize: read RCC_DEV0_EPF0_RCC_CONFIG_MEMSIZE (the value IS
        // the UMA carve-out in MB) at its discovery-resolved SOC15 offset.
        let reg = ath_amdgpu::regs::config_memsize_reg(self.discovery_blocks.as_ref()?)?;
        Some(self.reg_read(reg))
    }

    fn smu_mailbox(&mut self) -> Option<ath_amdgpu::bringup::SmuMailbox> {
        ath_amdgpu::regs::smu_mailbox(self.discovery_blocks.as_ref()?)
    }

    fn psp_regs(&mut self) -> Option<ath_amdgpu::bringup::PspRegs> {
        // PSP (MP0) mailbox + ring offsets — the firmware-load channel that
        // cold-starts GFX on this PSP-load APU. Discovery-gated (QEMU never pokes).
        ath_amdgpu::regs::psp_regs(self.discovery_blocks.as_ref()?)
    }

    fn vram_mc_base(&mut self) -> Option<u64> {
        // The GPU MC base of the VRAM/FB aperture on Phoenix = 0x8000000000
        // (= MMHUB MMMC_VM_FB_LOCATION_BASE 0x8000 << 24). Athena-Linux-oracle
        // verified: the live PSP ring sits at MC 0x8000339000. PSP buffers carry GPU
        // MC addresses, not CPU/bus. Discovery-gated so QEMU never runs the PSP ring.
        // (Hardening follow-up: read MMHUB FB_LOCATION_BASE live instead of the const.)
        self.discovery_blocks.as_ref()?;
        Some(0x80_0000_0000)
    }

    fn psp_tmr_base(&mut self) -> Option<u64> {
        // Athena-Linux-oracle: the PSP TMR sits at MC 0x8078000000 (dmesg "reserve
        // 0x4000000 from 0x8078000000 for PSP TMR"). Discovery-gated.
        self.discovery_blocks.as_ref()?;
        Some(0x80_7800_0000)
    }

    fn psp_toc_blob(&mut self) -> Option<(u64, u32)> {
        // Stage the firmware TOC (psp_13_0_4_toc.bin) into VRAM for LOAD_TOC: the PSP
        // reads it to size the TMR. Lazily loaded + MM_INDEX-written once, then cached.
        // VRAM offset 0x4003000 is past the ring/cmd/fence (0x400_0000..0x400_3000) and
        // below fw_pri (0x500_0000) — unused VRAM, can't corrupt anything.
        self.discovery_blocks.as_ref()?;
        if let Some(cached) = self.psp_toc_staged {
            return Some(cached);
        }
        const PSP_TOC_VRAM_OFFSET: u64 = 0x0400_3000;
        let bytes = self.request_firmware_bytes("amdgpu/psp_13_0_4_toc.bin")?;
        // LOAD_TOC wants the TOC UCODE (after the common firmware header), NOT the raw
        // blob: psp_init_toc_microcode sets start_addr = blob + ucode_array_offset[24],
        // size = ucode_size[20]. Staging the raw blob (incl. header) made the PSP
        // reject LOAD_TOC with status 0x11 (boot 175656). extract_common_ucode pulls
        // exactly [off@24 .. off+size@20].
        let ucode = ath_amdgpu::rlc_autoload::extract_common_ucode(&bytes)?;
        let mut dwords = alloc::vec::Vec::with_capacity(ucode.len().div_ceil(4));
        for chunk in ucode.chunks(4) {
            let mut d = [0u8; 4];
            d[..chunk.len()].copy_from_slice(chunk);
            dwords.push(u32::from_le_bytes(d));
        }
        self.vram_write(PSP_TOC_VRAM_OFFSET, &dwords);
        let staged = (0x80_0000_0000 + PSP_TOC_VRAM_OFFSET, ucode.len() as u32);
        self.psp_toc_staged = Some(staged);
        Some(staged)
    }

    fn vram_write(&mut self, offset: u64, data: &[u32]) {
        // Stage dwords into VRAM via the MM_INDEX/MM_DATA indirect aperture (no BAR0
        // map). `offset` is a VRAM-relative byte offset (the FB bit selects VRAM).
        if self.mmio.is_null() {
            return;
        }
        let mut hi = u32::MAX; // force the first MM_INDEX_HI write
        for (i, &d) in data.iter().enumerate() {
            unsafe { self.mm_write_dw(offset + (i as u64) * 4, d, &mut hi) };
        }
    }

    fn vram_read(&mut self, offset: u64, out: &mut [u32]) {
        if self.mmio.is_null() {
            out.iter_mut().for_each(|x| *x = 0);
            return;
        }
        let mut hi = u32::MAX;
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = unsafe { self.mm_read_dw(offset + (i as u64) * 4, &mut hi) };
        }
    }

    fn ih_ring(&mut self) -> Option<ath_amdgpu::bringup::IhRing> {
        ath_amdgpu::regs::ih_ring(self.discovery_blocks.as_ref()?)
    }

    fn rlc_safe_mode(&mut self) -> Option<ath_amdgpu::bringup::RlcSafeMode> {
        ath_amdgpu::regs::rlc_safe_mode(self.discovery_blocks.as_ref()?)
    }

    fn dcn_scanout_regs(&mut self) -> Option<ath_amdgpu::regs::DcnScanout> {
        // DMU-block (HWID 271, seg 2) HUBP0/OTG0 offsets — discovery-gated so QEMU (no
        // DMU block) never reads a fabricated offset. The DCN is firmware-lit, so these
        // resolve + read live on a warm boot.
        ath_amdgpu::regs::dcn_scanout(self.discovery_blocks.as_ref()?)
    }

    fn gfx_off_disable_msg(&mut self) -> Option<u32> {
        // Phoenix-APU SMU 13.0.4 message id (only once discovery confirms we are
        // on the real GPU, so QEMU never sends it).
        self.discovery_blocks.as_ref()?;
        Some(ath_amdgpu::regs::PPSMC_MSG_DISALLOW_GFXOFF)
    }

    fn enable_gfx_imu_msg(&mut self) -> Option<u32> {
        // PPSMC_MSG_EnableGfxImu (0x16) — powers up GFX via the IMU. Discovery-gated.
        self.discovery_blocks.as_ref()?;
        Some(ath_amdgpu::regs::PPSMC_MSG_ENABLE_GFX_IMU)
    }

    fn gfx_device_driver_reset_msg(&mut self) -> Option<u32> {
        // PPSMC_MSG_GfxDeviceDriverReset (0x11) — the SMU MODE2 GFX/SDMA scrub amdgpu
        // runs on a dirty (warm) load. Discovery-gated so QEMU never sends it; only
        // fired on the GFX-DOWN branch, so a cold boot never sends it either.
        self.discovery_blocks.as_ref()?;
        Some(ath_amdgpu::regs::PPSMC_MSG_GFX_DEVICE_DRIVER_RESET)
    }

    fn gfx_regs(&mut self) -> Option<ath_amdgpu::bringup::GfxRegs> {
        ath_amdgpu::regs::gfx_regs(self.discovery_blocks.as_ref()?)
    }

    fn cp_me_cntl_halt_mask(&mut self) -> Option<u32> {
        // gfx11 GFX-CP halt bits (ME_HALT|PFP_HALT), confirmed from
        // gc_11_0_0_sh_mask.h + gfx_v11_0_cp_gfx_enable. Gated on discovery (like
        // every offset method) so the CP unhalt only ever fires once the SOC15
        // CP_ME_CNTL offset is resolved — never a guessed write to the live CP.
        self.discovery_blocks.as_ref()?;
        Some(ath_amdgpu::gc11::CP_ME_CNTL_GFX11_HALT_MASK)
    }

    fn sdma_regs(&mut self) -> Option<ath_amdgpu::bringup::SdmaRegs> {
        // SDMA0 QUEUE0 ring offsets, discovery-resolved (SDMA regs are in the GC
        // block on gfx11). Gated on discovery → stage 6 only programs/submits the
        // SDMA ring once the offsets are authoritative; never a guessed write.
        ath_amdgpu::regs::sdma_regs(self.discovery_blocks.as_ref()?)
    }

    fn cp_gfx_ring_regs(&mut self) -> Option<ath_amdgpu::bringup::CpGfxRingRegs> {
        // CP gfx-ring completion regs (CP_RB_ACTIVE + writeback addrs), discovery-
        // resolved. Gated → stage 6 fully programs + ACTIVATES the ring only with
        // authoritative offsets; never a guessed write to the live CP.
        ath_amdgpu::regs::cp_gfx_ring_regs(self.discovery_blocks.as_ref()?)
    }

    fn rs64_cp_regs(&mut self) -> Option<ath_amdgpu::bringup::Rs64CpRegs> {
        // RS64 CP startup register offsets, discovery-resolved (gfx11 config_gfx_rs64).
        ath_amdgpu::regs::rs64_cp_regs(self.discovery_blocks.as_ref()?)
    }

    fn gmc_vm_regs(&mut self) -> Option<ath_amdgpu::bringup::GmcVmRegs> {
        // gfxhub GPUVM state regs (read-only), discovery-resolved — the GART-
        // inheritance diagnostic dumps the firmware's VM config from these.
        ath_amdgpu::regs::gmc_vm_regs(self.discovery_blocks.as_ref()?)
    }

    fn mes_enable_regs(&mut self) -> Option<ath_amdgpu::mes::MesEnableRegs> {
        // MES engine-enable regs, discovery-resolved (mes_v11_0_enable / rung 1).
        ath_amdgpu::regs::mes_enable_regs(self.discovery_blocks.as_ref()?)
    }

    fn mes_uc_starts(&mut self) -> Option<(u64, Option<u64>)> {
        // The MES microengine entry points from the autoloaded MES firmware headers:
        // mes_2.bin = scheduler (pipe 0, required), mes1.bin = KIQ (pipe 1, optional).
        let p0 = ath_amdgpu::mes::parse_mes_uc_start_addr(
            &self.request_firmware_bytes("amdgpu/gc_11_0_1_mes_2.bin")?,
        )?;
        let p1 = self
            .request_firmware_bytes("amdgpu/gc_11_0_1_mes1.bin")
            .and_then(|b| ath_amdgpu::mes::parse_mes_uc_start_addr(&b));
        Some((p0, p1))
    }

    fn mes_load_regs(&mut self) -> Option<ath_amdgpu::mes::MesLoadRegs> {
        // MES IC/MD-base regs, discovery-resolved (mes_v11_0_load_microcode).
        ath_amdgpu::regs::mes_load_regs(self.discovery_blocks.as_ref()?)
    }

    fn imu_fw_blob(&mut self) -> Option<alloc::vec::Vec<u8>> {
        // The raw gc_*_imu.bin — bringup streams its I-RAM/D-RAM ucode straight into the
        // IMU (imu_load_microcode), since the PSP leaves the IMU empty on this APU and an
        // empty IMU core can't bring GFX out of reset.
        self.request_firmware_bytes("amdgpu/gc_11_0_1_imu.bin")
    }

    fn mes_ucode_blobs(&mut self) -> Option<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> {
        // The MES scheduler (pipe 0) ucode + data, split out of mes_2.bin — AthenaOS
        // direct-loads them into GART-mapped buffers (the PSP copy isn't addressable).
        let blob = self.request_firmware_bytes("amdgpu/gc_11_0_1_mes_2.bin")?;
        let (ucode, data) = ath_amdgpu::rlc_autoload::extract_mes_ucode_data(&blob)?;
        Some((ucode.to_vec(), data.to_vec()))
    }

    fn mes_kiq_ucode_blobs(&mut self) -> Option<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> {
        // The MES KIQ (pipe 1) ucode + data, split out of mes1.bin — direct-loaded into
        // pipe 1's IC so the KIQ microengine can run (and map the SCHED ring).
        let blob = self.request_firmware_bytes("amdgpu/gc_11_0_1_mes1.bin")?;
        let (ucode, data) = ath_amdgpu::rlc_autoload::extract_mes_ucode_data(&blob)?;
        Some((ucode.to_vec(), data.to_vec()))
    }

    fn mes_hqd_regs(&mut self) -> Option<ath_amdgpu::mes::MesHqdRegs> {
        // CP_HQD regs (queue_init_register), discovery-resolved.
        ath_amdgpu::regs::mes_hqd_regs(self.discovery_blocks.as_ref()?)
    }

    fn doorbell_aper_en_reg(&mut self) -> Option<u32> {
        // regRCC_DEV0_EPF0_RCC_DOORBELL_APER_EN (NBIO seg 2), discovery-resolved.
        ath_amdgpu::regs::rcc_doorbell_aper_en(self.discovery_blocks.as_ref()?)
    }

    fn cp_mec_doorbell_range_regs(&mut self) -> Option<(u32, u32)> {
        // regCP_MEC_DOORBELL_RANGE_LOWER/UPPER (GC seg0), discovery-resolved — the MES/KIQ
        // doorbell-wake range AthenaOS had been missing.
        ath_amdgpu::regs::cp_mec_doorbell_range(self.discovery_blocks.as_ref()?)
    }

    fn mes_ip_bases(&mut self) -> Option<([u32; 8], [u32; 8], [u32; 8])> {
        // gc/mmhub/osssys IP register bases (8 segments) for set_hw_resources, from
        // IP discovery (reg_offset[HWIP][0][seg]). HWIDs from soc15_hw_ip.h.
        use ath_amdgpu::discovery::ip_base;
        use ath_amdgpu::regs::{GC_HWID, MMHUB_HWID, OSSSYS_HWID};
        let blocks = self.discovery_blocks.as_ref()?;
        let mut gc = [0u32; 8];
        let mut mm = [0u32; 8];
        let mut oss = [0u32; 8];
        for s in 0..8 {
            gc[s] = ip_base(blocks, GC_HWID, 0, s).unwrap_or(0);
            mm[s] = ip_base(blocks, MMHUB_HWID, 0, s).unwrap_or(0);
            oss[s] = ip_base(blocks, OSSSYS_HWID, 0, s).unwrap_or(0);
        }
        // Our discovery parse resolves only the first 3 GC/MMHUB/OSSSYS segments (iron
        // 2026-06-29: gc_base=[0x1260,0xa000,0x2402c00,0,0]), but the live working amdgpu
        // set_hw_resources ring carries the FULL segment set — and the MES reads our frame
        // yet aborts WITHOUT acking when the upper GC bases are 0 (it can't reach those
        // registers to apply the resources). Fill the gaps from the working-driver ring
        // values (Athena Phoenix1, debugfs amdgpu_ring_mes_3.0.0). Root fix is in the
        // discovery parser, which drops base_address segments past index 2.
        const WORK_GC: [u32; 8] = [0x1260, 0xa000, 0x2402c00, 0x2000029, 0x10205, 0, 0, 0];
        const WORK_MM: [u32; 8] = [0x13200, 0x1a000, 0x2408800, 0x60000ff, 0x4000d, 0, 0, 0];
        const WORK_OSS: [u32; 8] = [0x10a0, 0x240a000, 0x6000046, 0x6, 0, 0, 0, 0];
        for s in 0..8 {
            if gc[s] == 0 {
                gc[s] = WORK_GC[s];
            }
            if mm[s] == 0 {
                mm[s] = WORK_MM[s];
            }
            if oss[s] == 0 {
                oss[s] = WORK_OSS[s];
            }
        }
        Some((gc, mm, oss))
    }

    fn gfxhub_gart_regs(&mut self) -> Option<ath_amdgpu::gart::GfxhubGartRegs> {
        // gfxhub GART-build regs, discovery-resolved — init_gart builds + applies
        // GART (firmware left GFX GPUVM unconfigured, so we must build it).
        ath_amdgpu::regs::gfxhub_gart_regs(self.discovery_blocks.as_ref()?)
    }

    fn rs64_ucode_starts(&mut self) -> Option<ath_amdgpu::bringup::Rs64UcodeStarts> {
        // RS64 program-counter STARTs from the PFP/ME/MEC headers — BUT only if the
        // CP firmware is actually RS64. amdgpu picks RS64 vs F32 by the ucode header
        // version (gfx_v11_0.c: rs64_enable = amdgpu_ucode_hdr_version >= 2). The
        // Phoenix gc_11_0_1 CP ucodes ship as v1 (F32) in linux-firmware (confirmed
        // on iron, boot 162824), so Phoenix uses the F32 CP path and config_gfx_rs64
        // does NOT apply. Log the verdict, then None on F32 so the RS64 step is skipped.
        let pfp = self.request_firmware_bytes("amdgpu/gc_11_0_1_pfp.bin")?;
        let ver = if pfp.len() >= 10 {
            u16::from_le_bytes([pfp[8], pfp[9]])
        } else {
            0
        };
        if ver != 2 {
            klog(&alloc::format!(
                "[amdgpu] CP firmware: gc_11_0_1_pfp hdr v{ver} -> F32 CP path (NOT RS64); config_gfx_rs64 N/A for this ASIC"
            ));
            return None;
        }
        klog("[amdgpu] CP firmware: pfp hdr v2 -> RS64 CP path (config_gfx_rs64 applies)");
        let me = self.request_firmware_bytes("amdgpu/gc_11_0_1_me.bin")?;
        let mec = self.request_firmware_bytes("amdgpu/gc_11_0_1_mec.bin")?;
        Some(ath_amdgpu::bringup::Rs64UcodeStarts {
            pfp: ath_amdgpu::bringup::parse_rs64_ucode_start(&pfp)?,
            me: ath_amdgpu::bringup::parse_rs64_ucode_start(&me)?,
            mec: ath_amdgpu::bringup::parse_rs64_ucode_start(&mec)?,
        })
    }

    fn commit_scanout(&mut self, width: u32, height: u32, pitch: u32, gpu_addr: u64) -> bool {
        // amdgpu_dm builds a drm_atomic_state and commits; ath_drm forwards the
        // final mode + scanout buffer to the AthenaOS compositor.
        let mode = kms::DrmDisplayMode::new(width as u16, height as u16, 60);
        let fb = kms::DrmFramebuffer {
            fb_id: 1,
            width,
            height,
            pitch,
            format_fourcc: kms::DRM_FORMAT_XRGB8888,
            gpu_addr,
        };
        let state = kms::DrmAtomicState {
            crtc_id: 1,
            mode,
            fb,
        };
        kms::atomic_commit(&state);
        true
    }

    fn log(&mut self, msg: &str) {
        klog(msg);
    }
}

/// Iron-debug checkpoint: log `label`, then msleep briefly so the bootlog
/// persist + other threads interleave. ORIGINALLY 8s (to outlast a ~7s flush
/// interval in case a register write HARD-HANGS CPU 0) — but 4 Athena boots prove
/// the daemon never hard-wedges CPU 0 (the system stays alive: hid-diag + the
/// persist thread keep running). The 8s sleeps just stretched the full sequence to
/// ~90s, so the stick got pulled ~10s in, mid-sleep, losing everything past DISC 1
/// (boot 065153). The persist thread is independent and healthy, so a SHORT sleep
/// is enough. ROOT CAUSE of the 3 truncated boots (050943/065153/072405): the
/// late-flush thread only ran a BOUNDED ~10 flushes (~70s) then STOPPED, and the
/// daemon runs LATE in boot — so a slow (8s/3s) sequence ran out the flush window
/// before finishing (faster sleeps got further: DISC 1 -> DISC 2). FIX is paired
/// with widening that window (bootlog_persist: ~2.5s interval x 50 flushes).
/// 2026-06-22: boot 160558 STILL truncated at DISC 2 — the daemon runs LATE and a
/// 1s-per-checkpoint sequence (~15s) is long enough that an early pull catches
/// almost nothing. The stage-1..5 "where does it hang" debugging is DONE (firmware
/// read + SMU + discovery all confirmed working on iron), so the sleeps are now
/// pure capture-fragility. Cut to 250ms: the full DISC 1-5 + CKPT 0-6 + stage-6
/// (config_gfx_rs64 + ring + inherit check + fence) completes in ~3-4s, lands the
/// COMPLETE sequence in the ring fast, and any flush in the 125s window captures
/// it. Markers still print (ordering preserved); they just don't each stall 1s.
const CKPT_MS: u32 = 250;
fn checkpoint(label: &str) {
    klog(label);
    ath_linuxkpi::msleep(CKPT_MS);
}

// ── The REAL amdgpu init path (M5) ───────────────────────────────────────────
// Instead of the Rust reimpl (ath_amdgpu::bringup, which halts at 0x7654), call
// the COMPLETE upstream amdgpu_device_init compiled+linked from the real driver
// source. `rae_amdgpu_device_init` (linuxkpi-drm/bringup_entry.c) is linked in by
// build.rs from the FREESTANDING amdgpu object set; enabled by the
// `real_amdgpu_init` feature. See linuxkpi-drm/M5-BAREMETAL-PLAN.md.
#[cfg(feature = "real_amdgpu_init")]
extern "C" {
    fn rae_amdgpu_device_init(
        vendor: u16,
        device: u16,
        revision: u8,
        pci_bus: u8,
        pci_devfn: u8,
        bar0_phys: u64,
        bar0_size: u64,
        bar2_phys: u64,
        bar2_size: u64,
        bar5_phys: u64,
        bar5_size: u64,
    ) -> i32;
    fn rae_amdgpu_info_accel_working(working: *mut u32) -> i32;
    fn rae_amdgpu_render_open() -> *mut core::ffi::c_void;
    fn rae_amdgpu_render_ioctl(
        client: *mut core::ffi::c_void,
        cmd: u32,
        arg: *mut core::ffi::c_void,
    ) -> i32;
    fn rae_amdgpu_render_close(client: *mut core::ffi::c_void);
    fn rae_amdgpu_render_mmap_pages(
        client: *mut core::ffi::c_void,
        offset: u64,
        length: u64,
        pages_out: *mut u64,
        pages_cap: u32,
    ) -> i32;
}

#[cfg(feature = "real_amdgpu_init")]
#[repr(C)]
struct DrmAmdgpuInfo {
    return_pointer: u64,
    return_size: u32,
    query: u32,
    query_data: [u64; 2],
}

/// Exercise the full render-file lifecycle and generic DRM ioctl dispatcher,
/// not just the direct INFO handler.  Opening creates a per-file GPU VM; close
/// tears it down.  This is the daemon-side half of `/dev/dri/renderD128`.
#[cfg(feature = "real_amdgpu_init")]
fn render_client_probe() -> (i32, u32) {
    const DRM_IOCTL_AMDGPU_INFO: u32 = 0x4020_6445;
    const AMDGPU_INFO_ACCEL_WORKING: u32 = 0;

    flush_marker(
        9005,
        "[amdgpu] REAL-RENDER: opening upstream drm_file + per-client GPU VM",
    );
    let client = unsafe { rae_amdgpu_render_open() };
    let raw = client as usize;
    if client.is_null() || raw >= usize::MAX - 4095 {
        return (
            if client.is_null() {
                -12
            } else {
                client as isize as i32
            },
            0,
        );
    }

    let mut working = 0u32;
    let mut info = DrmAmdgpuInfo {
        return_pointer: (&mut working as *mut u32) as u64,
        return_size: core::mem::size_of::<u32>() as u32,
        query: AMDGPU_INFO_ACCEL_WORKING,
        query_data: [0; 2],
    };
    let result = unsafe {
        rae_amdgpu_render_ioctl(
            client,
            DRM_IOCTL_AMDGPU_INFO,
            (&mut info as *mut DrmAmdgpuInfo).cast(),
        )
    };
    flush_marker(
        9005,
        "[amdgpu] REAL-RENDER: ioctl returned; closing per-client GPU VM",
    );
    unsafe { rae_amdgpu_render_close(client) };
    (result, working)
}

#[cfg(feature = "real_amdgpu_init")]
#[inline(always)]
unsafe fn drm_service_register(device_handle: u64) -> u64 {
    let result: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") abi::SYS_DRM_SERVICE_REGISTER => result,
        in("rdi") device_handle,
        out("rcx") _, out("r11") _,
    );
    result
}

#[cfg(feature = "real_amdgpu_init")]
#[inline(always)]
unsafe fn drm_service_fetch(
    header: &mut ath_abi::drm_service::RequestHeader,
    payload: &mut [u8],
) -> u64 {
    let result: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") abi::SYS_DRM_SERVICE_FETCH => result,
        in("rdi") header as *mut _ as u64,
        in("rsi") payload.as_mut_ptr() as u64,
        in("rdx") payload.len() as u64,
        out("rcx") _, out("r11") _,
    );
    result
}

#[cfg(feature = "real_amdgpu_init")]
#[inline(always)]
unsafe fn drm_service_complete(request_id: u64, status: i32, payload: &[u8]) -> u64 {
    let result: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") abi::SYS_DRM_SERVICE_COMPLETE => result,
        in("rdi") request_id,
        in("rsi") status as u32 as u64,
        in("rdx") payload.as_ptr() as u64,
        in("r10") payload.len() as u64,
        out("rcx") _, out("r11") _,
    );
    result
}

/// Drain kernel-owned render-node requests into the retained upstream object
/// graph. All client memory arrived as a bounded copy. INFO's sole nested output
/// pointer is rewritten to the auxiliary bytes appended by the kernel; commands
/// needing richer marshalling never reach this loop until their kernel
/// marshaller exists.
#[cfg(feature = "real_amdgpu_init")]
fn render_service_loop(device_handle: u64) -> ! {
    use alloc::collections::BTreeMap;
    use ath_abi::drm_service as wire;

    let registered = unsafe { drm_service_register(device_handle) };
    if registered != 0 {
        klog(&alloc::format!(
            "[amdgpu] REAL-UAPI: render service registration failed ({registered:#x})"
        ));
        unsafe { sys_exit(1) };
    }
    flush_marker(
        9006,
        "[amdgpu] REAL-UAPI: /dev/dri/renderD128 broker registered",
    );

    let mut clients: BTreeMap<u64, usize> = BTreeMap::new();
    let mut payload = alloc::vec![0u8; wire::MAX_PAYLOAD];
    loop {
        let mut header = wire::RequestHeader::default();
        let fetched = unsafe { drm_service_fetch(&mut header, &mut payload) };
        if fetched == 0 {
            ath_linuxkpi::msleep(1);
            continue;
        }
        if fetched >= wire::ERR_BUSY {
            klog(&alloc::format!(
                "[amdgpu] REAL-UAPI: broker fetch failed ({fetched:#x})"
            ));
            ath_linuxkpi::msleep(10);
            continue;
        }
        let payload_len = (fetched - 1) as usize;
        if header.version != wire::VERSION || payload_len != header.payload_len as usize {
            klog("[amdgpu] REAL-UAPI: malformed broker request rejected");
            continue;
        }
        match header.op {
            wire::OP_OPEN => {
                let client = unsafe { rae_amdgpu_render_open() };
                let raw = client as usize;
                if !client.is_null() && raw < usize::MAX - 4095 {
                    clients.insert(header.client_id, raw);
                }
            }
            wire::OP_CLOSE => {
                if let Some(raw) = clients.remove(&header.client_id) {
                    unsafe { rae_amdgpu_render_close(raw as *mut core::ffi::c_void) };
                }
            }
            wire::OP_IOCTL => {
                let arg_len = header.arg_len as usize;
                let status = if arg_len > payload_len {
                    -22
                } else if let Some(raw) = clients.get(&header.client_id).copied() {
                    if header.flags & wire::FLAG_INFO_AUX != 0 {
                        if arg_len != 32 || payload_len < arg_len {
                            -22
                        } else {
                            let aux = if payload_len == arg_len {
                                0
                            } else {
                                (unsafe { payload.as_mut_ptr().add(arg_len) }) as u64
                            };
                            payload[0..8].copy_from_slice(&aux.to_le_bytes());
                            unsafe {
                                rae_amdgpu_render_ioctl(
                                    raw as *mut core::ffi::c_void,
                                    header.ioctl_cmd,
                                    payload.as_mut_ptr().cast(),
                                )
                            }
                        }
                    } else if header.flags & wire::FLAG_VERSION_AUX != 0 {
                        if arg_len != 64 || payload_len < 64 {
                            -22
                        } else {
                            let mut cursor = arg_len;
                            let mut valid = true;
                            for (len_off, ptr_off) in [(16usize, 24usize), (32, 40), (48, 56)] {
                                let len = u64::from_le_bytes(
                                    payload[len_off..len_off + 8].try_into().unwrap(),
                                ) as usize;
                                valid &= cursor
                                    .checked_add(len)
                                    .map_or(false, |end| end <= payload_len);
                                let local = if len == 0 {
                                    0
                                } else {
                                    (unsafe { payload.as_mut_ptr().add(cursor) }) as u64
                                };
                                payload[ptr_off..ptr_off + 8].copy_from_slice(&local.to_le_bytes());
                                cursor = cursor.saturating_add(len);
                            }
                            if !valid {
                                -22
                            } else {
                                unsafe {
                                    rae_amdgpu_render_ioctl(
                                        raw as *mut core::ffi::c_void,
                                        header.ioctl_cmd,
                                        payload.as_mut_ptr().cast(),
                                    )
                                }
                            }
                        }
                    } else if header.flags & wire::FLAG_BO_LIST_AUX != 0 {
                        if arg_len != 24 || payload_len < 24 {
                            -22
                        } else {
                            let count =
                                u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
                            let entry_size =
                                u32::from_le_bytes(payload[12..16].try_into().unwrap()) as usize;
                            let expected = count
                                .checked_mul(entry_size)
                                .and_then(|n| n.checked_add(arg_len));
                            if expected != Some(payload_len) {
                                -22
                            } else {
                                let local = if payload_len == arg_len {
                                    0
                                } else {
                                    (unsafe { payload.as_mut_ptr().add(arg_len) }) as u64
                                };
                                payload[16..24].copy_from_slice(&local.to_le_bytes());
                                unsafe {
                                    rae_amdgpu_render_ioctl(
                                        raw as *mut core::ffi::c_void,
                                        header.ioctl_cmd,
                                        payload.as_mut_ptr().cast(),
                                    )
                                }
                            }
                        }
                    } else if header.flags & wire::FLAG_CS_AUX != 0 {
                        if arg_len != 24 || payload_len < 24 {
                            -22
                        } else {
                            let count =
                                u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
                            let pointers_off = arg_len;
                            let headers_off =
                                match pointers_off.checked_add(count.saturating_mul(8)) {
                                    Some(offset) => offset,
                                    None => payload_len + 1,
                                };
                            let data_start = match headers_off.checked_add(count.saturating_mul(16))
                            {
                                Some(offset) => offset,
                                None => payload_len + 1,
                            };
                            if count == 0 || count > 64 || data_start > payload_len {
                                -22
                            } else {
                                let pointer_array =
                                    (unsafe { payload.as_mut_ptr().add(pointers_off) }) as u64;
                                payload[16..24].copy_from_slice(&pointer_array.to_le_bytes());
                                let mut cursor = data_start;
                                let mut valid = true;
                                for index in 0..count {
                                    let header_off = headers_off + index * 16;
                                    let header_ptr =
                                        (unsafe { payload.as_mut_ptr().add(header_off) }) as u64;
                                    payload[pointers_off + index * 8..pointers_off + index * 8 + 8]
                                        .copy_from_slice(&header_ptr.to_le_bytes());
                                    let dwords = u32::from_le_bytes(
                                        payload[header_off + 4..header_off + 8].try_into().unwrap(),
                                    ) as usize;
                                    let bytes = dwords.saturating_mul(4);
                                    let end = cursor.checked_add(bytes).unwrap_or(usize::MAX);
                                    if end > payload_len {
                                        valid = false;
                                        break;
                                    }
                                    let data_ptr = if bytes == 0 {
                                        0
                                    } else {
                                        (unsafe { payload.as_mut_ptr().add(cursor) }) as u64
                                    };
                                    payload[header_off + 8..header_off + 16]
                                        .copy_from_slice(&data_ptr.to_le_bytes());
                                    cursor = end;
                                }
                                if !valid || cursor != payload_len {
                                    -22
                                } else {
                                    unsafe {
                                        rae_amdgpu_render_ioctl(
                                            raw as *mut core::ffi::c_void,
                                            header.ioctl_cmd,
                                            payload.as_mut_ptr().cast(),
                                        )
                                    }
                                }
                            }
                        }
                    } else {
                        unsafe {
                            rae_amdgpu_render_ioctl(
                                raw as *mut core::ffi::c_void,
                                header.ioctl_cmd,
                                payload.as_mut_ptr().cast(),
                            )
                        }
                    }
                } else {
                    -19
                };
                let _ = unsafe {
                    drm_service_complete(header.request_id, status, &payload[..payload_len])
                };
            }
            wire::OP_MMAP => {
                let (status, response_len) = if payload_len != 16 {
                    (-22, 0usize)
                } else if let Some(raw) = clients.get(&header.client_id).copied() {
                    let offset = u64::from_le_bytes(payload[0..8].try_into().unwrap());
                    let length = u64::from_le_bytes(payload[8..16].try_into().unwrap());
                    let cap = (wire::MAX_PAYLOAD / core::mem::size_of::<u64>()) as u32;
                    let result = unsafe {
                        rae_amdgpu_render_mmap_pages(
                            raw as *mut core::ffi::c_void,
                            offset,
                            length,
                            payload.as_mut_ptr().cast::<u64>(),
                            cap,
                        )
                    };
                    if result < 0 {
                        (result, 0)
                    } else {
                        (0, result as usize * core::mem::size_of::<u64>())
                    }
                } else {
                    (-19, 0)
                };
                let _ = unsafe {
                    drm_service_complete(header.request_id, status, &payload[..response_len])
                };
            }
            _ => klog("[amdgpu] REAL-UAPI: unknown broker operation rejected"),
        }
    }
}

/// Read a PCI BAR's physical base + size via the standard sizing protocol
/// (write all-ones, read the mask, restore). `is64` handles a 64-bit BAR pair.
#[cfg(feature = "real_amdgpu_init")]
fn read_bar(handle: u64, off: u16, is64: bool) -> (u64, u64) {
    use ath_linuxkpi::pci::{read_config_dword as rd, write_config_dword as wr};
    let orig_lo = rd(handle, off);
    let lo = (orig_lo & !0xf) as u64;
    let orig_hi = if is64 { rd(handle, off + 4) } else { 0 };
    let phys = lo | ((orig_hi as u64) << 32);
    wr(handle, off, 0xffff_ffff);
    let m_lo = (rd(handle, off) & !0xf) as u64;
    let m_hi = if is64 {
        wr(handle, off + 4, 0xffff_ffff);
        rd(handle, off + 4) as u64
    } else {
        0xffff_ffff
    };
    wr(handle, off, orig_lo);
    if is64 {
        wr(handle, off + 4, orig_hi);
    }
    let mask = m_lo | (m_hi << 32);
    let size = if mask == 0 {
        0
    } else {
        (!mask).wrapping_add(1)
    };
    (phys, size)
}

/// Wire ath_linuxkpi's device access to the claimed GPU (device_map) and run the
/// real amdgpu_device_init. Reads the device id + BAR windows from PCI config.
/// Returns 0 on success.
#[cfg(feature = "real_amdgpu_init")]
fn run_real_amdgpu_init(handle: u64) -> i32 {
    use ath_linuxkpi::device_map::{lkpi_register_bar, lkpi_set_current_device};
    use ath_linuxkpi::pci::{read_config_dword as rd, write_config_dword as wr};
    let device = (rd(handle, 0x00) >> 16) as u16; // config 0x00 = vendor | device<<16
    let revision = (rd(handle, 0x08) & 0xff) as u8; // config 0x08 low byte = revision
                                                    // PCI spec 6.2.5.1: memory/IO decoding MUST be disabled while sizing a BAR
                                                    // (each read_bar writes all-ones then restores). The daemon already pci_enable'd
                                                    // the device, so decoding is on; leaving it on makes a VFIO host eagerly remap
                                                    // the transient all-ones BAR0 to 0xffffffff00000000 -> KVM_SET_USER_MEMORY_REGION
                                                    // abort. Bracket the sizing with decoding off, then restore PCI_COMMAND.
    let cmd = rd(handle, 0x04); // dword at 0x04: Command (low 16) | Status (high 16)
    wr(handle, 0x04, cmd & !0x3); // clear IO(0) + Memory(1) space enable
    let (b0p, b0s) = read_bar(handle, 0x10, true); // BAR0 VRAM (64-bit)
    let (b2p, b2s) = read_bar(handle, 0x18, true); // BAR2 doorbell (64-bit)
    let (b5p, b5s) = read_bar(handle, 0x24, false); // BAR5 registers (32-bit)
    wr(handle, 0x04, cmd); // restore decoding
    lkpi_set_current_device(handle);
    lkpi_register_bar(0, b0p, b0s);
    lkpi_register_bar(2, b2p, b2s);
    lkpi_register_bar(5, b5p, b5s);

    // RELOC-CHK: soc21_common_early_init faulted calling nbio.funcs->set_reg_remap
    // (jumped to 0x77). Verify at runtime whether the C static funcs table
    // `nbio_v4_3_funcs` (a .data.rel.ro table populated by R_X86_64_RELATIVE) is
    // correctly relocated in the daemon's own view: read the set_reg_remap slot
    // (struct offset 0x118 = u64 index 35) and compare to the real function
    // address. If they match -> the loader relocated it and the fault is in the C
    // access path; if the slot is 0/garbage -> the PIE reloc did not land here.
    unsafe {
        extern "C" {
            static nbio_v4_3_funcs: [u64; 36];
        }
        let tbl = core::ptr::addr_of!(nbio_v4_3_funcs) as u64;
        let slot = core::ptr::read_volatile(core::ptr::addr_of!(nbio_v4_3_funcs[35]));
        let get_rev = core::ptr::read_volatile(core::ptr::addr_of!(nbio_v4_3_funcs[0]));
        // Expected (base-0 link vaddrs from nm): set_reg_remap=0x12bc2c, get_rev_id=0x12b96f.
        // slot35==0x77 (the fault value) => the reloc did NOT land in the daemon's view.
        klog(&alloc::format!(
            "[amdgpu] RELOC-CHK nbio_v4_3_funcs@{:#x} slot35(set_reg_remap)={:#x} (want ~0x12bc2c) slot0(get_rev)={:#x} (want ~0x12b96f)",
            tbl, slot, get_rev
        ));
        rae_diag_netlog_flush();
    }

    // The claimed device's real location: amdgpu_acpi_vfct_bios matches the
    // VFCT VBIOS image against pdev->bus->number + slot/function, and the
    // class-match claim path never told the daemon where the GPU landed
    // (Athena/devbox: c4:00.0). Fall back to 0:0.0 only if the query fails —
    // the C init then still runs, and VFCT matching reports its own miss
    // instead of NULL-dereferencing pdev->bus.
    // Match pdev's BDF to the VFCT's NATIVE recorded location (Athena c4:00.0),
    // NOT the guest passthrough BDF: amdgpu_acpi_vfct_bios accepts the VBIOS image
    // only when pdev's bus/slot/func equal the image header's, and aligning pdev
    // to the ground-truth table matches whether amdgpu reads our served copy or
    // the original firmware mapping (writing the guest BDF into a served copy did
    // not take — cap3 wrote 0:3.0 but the match still missed). pdev's BDF is
    // provenance-only here; real MMIO/config route via the claimed handle + BARs,
    // so a non-guest bus number is safe. Fall back to the claimed BDF if the VFCT
    // has no readable image header.
    // Prefer the capability-backed claimed BDF. On Athena it is the same
    // c4:00.0 recorded in VFCT, and this avoids re-entering the firmware loader
    // at the final C-entry boundary. Keep VFCT as a fallback for hosts whose
    // class-match claim cannot report a location.
    let (bus, dev8, func) = ath_linuxkpi::device_bdf(handle)
        .or_else(ath_linuxkpi::vfct_native_bdf)
        .unwrap_or((0, 0, 0));
    let devfn = (dev8 << 3) | (func & 0x7);
    // Keep the served-copy patch aligned to the same value (belt-and-suspenders:
    // if amdgpu DOES read our copy, it now also carries this BDF).
    ath_linuxkpi::drm_bringup::set_vfct_bdf(bus, dev8, func);
    klog(&alloc::format!(
        "[amdgpu] VBIOS: pdev BDF set to claimed/native {:02x}:{:02x}.{} (devfn=0x{:x}) for amdgpu_acpi_vfct_bios match",
        bus, dev8, func, devfn
    ));
    unsafe {
        rae_amdgpu_device_init(
            0x1002, device, revision, bus, devfn, b0p, b0s, b2p, b2s, b5p, b5s,
        )
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Run the whole bring-up in the Game scheduling class. Its SMU/IMU/RLC poll
    // loops msleep-yield, and at Normal priority each yield waits a full SCHED_BODY
    // compositor frame to resume on the single post-boot scheduling CPU — which
    // ballooned the iron bring-up to ~480s (~225s of pure yield-wait, 2026-06-28).
    // Self-promote FIRST so every downstream poll resumes promptly.
    unsafe { sys_setpriority_self_game() };
    // Hang-proof trail (2026-07-06): fence each phase onto the wire synchronously
    // so a CPU-0 wedge in the REAL amdgpu init leaves the netlog ending at the
    // exact reached phase. 9000 = the daemon's _start ran at all (rules out a
    // pre-userspace / scheduler wedge as the cause of a blind hang).
    flush_marker(
        9000,
        "[amdgpu] amdgpud starting: amdgpu_device_init pipeline",
    );

    // Firmware preflight (runs even with no GPU) — proves the request_firmware
    // host path works and reports which Phoenix blobs are present.
    firmware_preflight();

    // STAGED bring-up with flush-checkpoints (iron diagnostic). The Athena boot
    // (logs/BOOTLOG.dump.txt) showed the bring-up hard-hangs CPU 0 on a register
    // write the moment discovery goes live — and since the late-flush also lives
    // on CPU 0, ALL bring-up output was lost (not even "starting" persisted past
    // the proof-of-life sleep). So run the stages INDIVIDUALLY (ath_amdgpu::
    // bringup exposes each as a pub fn) with a `checkpoint` (log + msleep-to-flush)
    // BEFORE each register-writing stage. The bootlog then ends at the exact stage
    // that stalls — telling us which write to fix. The probe is match-first (class
    // 0x03 + AMD — finds Athena's GPU at c4:00.0); the BDF list is the fallback.
    let mut ops = LkpiGpuOps::new();
    const BDFS: &[(u8, u8, u8)] = &[(0x00, 0x01, 0x00), (0x03, 0x00, 0x00)];

    // The real upstream path owns the accelerator from PCI claim through MES
    // initialization.  Do not run the legacy native-Rust probe around it: the
    // latter reclaims/inspects the already-owned device and its diagnostic
    // `Option<Device>` return path is neither part of nor safe after the C
    // driver's ownership transition.  We need only the capability-checked PCI
    // claim handle to wire the C entry seam.
    #[cfg(feature = "real_amdgpu_init")]
    {
        let Some(handle) = ops.pci_enable_match(0x03, 0x1002) else {
            klog("[amdgpu] upstream path: no AMD display device claim; exiting");
            unsafe { sys_print(9099) };
            unsafe { sys_exit(0) };
        };
        flush_marker(
            9002,
            "[amdgpu] REAL-INIT: upstream-only PCI claim; entering amdgpu_device_init (C)",
        );
        ath_linuxkpi::device::set_netlog_fence(true);
        unsafe { sys_setpriority_self_normal() };
        let r = run_real_amdgpu_init(handle);
        ath_linuxkpi::device::set_netlog_fence(false);
        let mut accel_working = 0u32;
        let info_r = if r == 0 {
            unsafe { rae_amdgpu_info_accel_working(&mut accel_working) }
        } else {
            -19 // -ENODEV: the retained adev is intentionally unavailable.
        };
        if r == 0 {
            flush_marker(
                9003,
                "[amdgpu] REAL-INIT: amdgpu_device_init RETURNED 0 — complete upstream init survived",
            );
        } else {
            flush_marker(
                9003,
                "[amdgpu] REAL-INIT: amdgpu_device_init returned nonzero",
            );
        }
        let (render_r, render_working) = if r == 0 && info_r == 0 && accel_working == 1 {
            render_client_probe()
        } else {
            (-19, 0)
        };
        let service_ready =
            r == 0 && info_r == 0 && accel_working == 1 && render_r == 0 && render_working == 1;
        if service_ready {
            flush_marker(
                9004,
                "[amdgpu] REAL-UAPI: direct + render-file INFO=1; retained adev and client VM are service-ready",
            );
        } else {
            flush_marker(
                9004,
                &alloc::format!(
                    "[amdgpu] REAL-UAPI: fail closed (init={}, info={}, accel={}, render={}, render_accel={})",
                    r,
                    info_r,
                    accel_working,
                    render_r,
                    render_working
                ),
            );
        }
        ath_linuxkpi::msleep(CKPT_MS);
        unsafe { sys_print(9098) };
        if service_ready {
            // The upstream adev, its rings, VM manager, IRQ/fence state, and BO
            // managers are daemon-owned. Stay resident so the render-node
            // broker can dispatch client ioctls into this exact object graph.
            render_service_loop(handle);
        }
        unsafe { sys_exit(1) };
    }

    checkpoint(
        "[amdgpu] CKPT 0: staged bring-up start — firmware preflight persisted, probing next",
    );
    let Some(mut dev) = ath_amdgpu::bringup::probe(&mut ops, BDFS) else {
        klog("[amdgpu] no AMD GPU found — amdgpud exiting (expected on QEMU)");
        unsafe { sys_print(9099) };
        unsafe { sys_exit(0) };
    };

    // probe() ran read_discovery (the firmware-file path). Report whether the
    // captured ip_discovery.bin made SOC15 offsets live — the whole point of this boot.
    if ops.discovery_blocks.is_some() {
        flush_marker(9001, "[amdgpu] CKPT 1: probe OK + DISCOVERY ACTIVE — SOC15 offsets LIVE (real register writes ahead)");
    } else {
        flush_marker(
            9001,
            "[amdgpu] CKPT 1: probe OK but discovery GATED — offsets fall back (blob not loaded?)",
        );
    }
    ath_linuxkpi::msleep(CKPT_MS);

    // M5: the REAL amdgpu_device_init path. When built with --features
    // real_amdgpu_init, wire ath_linuxkpi's device access to the claimed GPU and
    // run the COMPLETE upstream init (the whole point — see if it clears the 0x7654
    // halt the Rust reimpl below cannot). The `if cfg!()` wrapper keeps the Rust
    // staged path below reachable-per-compiler (no unreachable warning) while the
    // inner sys_exit terminates before it at runtime. Default: the block is skipped.
    if cfg!(feature = "real_amdgpu_init") {
        #[cfg(feature = "real_amdgpu_init")]
        {
            // 9002 = we reached the REAL init and are about to enter the
            // monolithic upstream amdgpu_device_init C call. Flushed
            // SYNCHRONOUSLY here: if the C init hard-hangs CPU 0, the netlog
            // trail ends at 9002 -> "reached real-init, hangs INSIDE the C init"
            // (distinguishes it from a pre-userspace / scheduler wedge, which
            // would never emit 9000). This is the first-order M1 question.
            flush_marker(
                9002,
                "[amdgpu] REAL-INIT: wiring device_map + entering amdgpu_device_init (C)",
            );
            // Arm the printk->netlog fence (M1.1): the C init is monolithic, so its
            // ONLY interleave point is the linuxkpi printk facade — have each of its
            // log lines broadcast the ring (throttled 40ms) so the netlog trail ends
            // at the EXACT line before a CPU-0 hard hang (the run-1 lucky single
            // broadcast only proved capture ENDS at CLKA #2, not that it HANGS there).
            ath_linuxkpi::device::set_netlog_fence(true);
            // Demote to Normal BEFORE the long C call so the safe-progress
            // broadcaster + ~480s auto-return threads keep scheduling on CPU 0 —
            // the box then self-recovers + keeps capturing even if the C init
            // wedges, instead of stranding it at Game priority (the 2026-07-06
            // blind-hang: no capture, no auto-return, manual power cycle). The
            // synchronous 9002 fence above already guarantees the entry marker
            // regardless; this restores self-recovery on top.
            unsafe { sys_setpriority_self_normal() };
            let r = run_real_amdgpu_init(ops.handle);
            // 9003 = the C init RETURNED (did not hang). r encodes success.
            if r == 0 {
                flush_marker(9003, "[amdgpu] REAL-INIT: amdgpu_device_init RETURNED 0 — complete upstream init survived");
            } else {
                flush_marker(9003, "[amdgpu] REAL-INIT: amdgpu_device_init returned nonzero (see the stage it stalled at)");
            }
            ath_linuxkpi::msleep(CKPT_MS);
            unsafe { sys_print(9098) };
            unsafe { sys_exit(if r == 0 { 0 } else { 1 }) };
        }
    }

    // Reads first (low hang risk; BAR5 reads proven safe on a prior boot).
    ath_amdgpu::bringup::probe_registers(&mut ops);
    ath_amdgpu::bringup::read_vbios(&mut ops, &mut dev);
    ath_amdgpu::bringup::init_gmc(&mut ops, &mut dev);
    checkpoint("[amdgpu] CKPT 2: reg-probe + VBIOS + GMC survived (reads OK) — NEXT init_ih (FIRST MMIO WRITES)");

    ath_amdgpu::bringup::init_ih(&mut ops, &dev);
    checkpoint("[amdgpu] CKPT 3: init_ih survived (IH ring writes OK) — NEXT init_smu (SMU mailbox writes)");

    ath_amdgpu::bringup::init_smu(&mut ops, &dev);
    checkpoint("[amdgpu] CKPT 4: init_smu survived (SMU mailbox OK) — NEXT init_rings (RLC/CP/SDMA writes)");

    // PSP path increment 1 (read-only): is the PSP secure-OS reachable over its MP0
    // mailbox? On this PSP-load APU the PSP is the only thing that can cold-start GFX
    // (boot 041507), so prove the channel before building ring/TMR/LOAD_IP_FW on it.
    ath_amdgpu::bringup::psp_sign_of_life(&mut ops);
    checkpoint("[amdgpu] CKPT 4b: PSP sign-of-life probed (MP0 mailbox) — NEXT ring create");

    // PSP path increment 2: create the GPCOM command ring (psp_v13_0_ring_create) —
    // a 4 KiB ring in VRAM, the channel the firmware-load commands (LOAD_TOC /
    // SETUP_TMR / LOAD_IP_FW) submit into. Pure C2PMSG register writes; the sOS
    // initializes the ring. Register values are Athena-oracle-confirmed.
    let _psp_ring = ath_amdgpu::bringup::psp_ring_create(&mut ops);
    checkpoint("[amdgpu] CKPT 4c: PSP ring-create attempted — NEXT init_rings");

    // VRAM write/read self-test (MM_INDEX/MM_DATA path) — PROVE CPU->VRAM writes work
    // on iron BEFORE the PSP firmware staging depends on vram_write. The memory flags
    // the indirect VRAM path as a CPU-0-wedge risk, and ring-create (which succeeded
    // on boot 163706) only writes C2PMSG regs — never VRAM content. Scratch is unused
    // VRAM (96 MiB in, past ring/cmd/fence/fw_pri), so a stray write can't corrupt
    // anything. FAIL-able: a value mismatch prints FAIL; a hang leaves "writing..." as
    // the last line (localizing the wedge). Only on the real GPU (ring-create OK).
    if let Some(ring) = _psp_ring {
        const VRAM_SELFTEST_OFF: u64 = 0x0600_0000; // 96 MiB into VRAM, unused
        let pat = [0xCAFE_BABEu32, 0x1234_5678, 0xDEAD_BEEF, 0xA5A5_5A5A];
        checkpoint("[amdgpu] VRAM self-test: writing 4 dwords via MM_INDEX @ VRAM+0x6000000...");
        ath_amdgpu::bringup::GpuOps::vram_write(&mut ops, VRAM_SELFTEST_OFF, &pat);
        let mut rb = [0u32; 4];
        ath_amdgpu::bringup::GpuOps::vram_read(&mut ops, VRAM_SELFTEST_OFF, &mut rb);
        let pass = rb == pat;
        checkpoint(&alloc::format!(
            "[amdgpu] VRAM self-test (MM_INDEX): read {:#x?} -> {}",
            rb,
            if pass {
                "PASS — CPU->VRAM writes work; PSP firmware staging unblocked"
            } else {
                "FAIL — indirect VRAM write unreliable; stage firmware via GART/sysmem instead"
            }
        ));

        // PSP fw-load increment 3a: LOAD_TOC + SETUP_TMR over the live GPCOM ring, with
        // an EMPTY fw_blobs list (no gfx ucodes yet — those are increment 3b, with the
        // intricate per-blob extraction). This proves the GPCOM submit round-trip
        // (cmd-buf -> ring frame -> C2PMSG_67 wptr -> fence -> resp) end-to-end against
        // the live PSP and that LOAD_TOC returns a real TMR size. Returns false ("no
        // RLC_G") by design — watch the LOAD_TOC/SETUP_TMR result lines for the proof.
        if pass {
            if let Some(psp) = ath_amdgpu::bringup::GpuOps::psp_regs(&mut ops) {
                // 3b: the full firmware-load handshake — LOAD_TOC -> SETUP_TMR ->
                // per-blob LOAD_IP_FW (IMU first) -> AUTOLOAD_RLC after RLC_G. This is
                // the swing at first light: the PSP authenticates each gfx ucode and the
                // IMU should power up the GFX domain (the seg1-probe below shows it).
                let owned = ops.build_gfx_fw_blobs();
                let fw_blobs: alloc::vec::Vec<(u32, &[u8])> =
                    owned.iter().map(|(t, b)| (*t, b.as_slice())).collect();
                checkpoint(&alloc::format!(
                    "[amdgpu] PSP fw-load 3b: LOAD_TOC -> SETUP_TMR -> LOAD_IP_FW x{} -> AUTOLOAD_RLC...",
                    fw_blobs.len()
                ));
                let r =
                    ath_amdgpu::bringup::psp_load_gfx_firmware(&mut ops, &psp, &ring, &fw_blobs);
                checkpoint(&alloc::format!(
                    "[amdgpu] PSP fw-load 3b done (returned {r}) — watch the seg1-probe for GFX DEAD->live (first light)"
                ));
            }
        }
    }

    // NATIVE DISPLAY PATH (warm-reachable) — probe the DCN scanout BEFORE init_rings. The
    // DCN is a separate, firmware-lit power domain that reads live on a warm boot, whereas
    // init_rings' GFX power-up WEDGES on a warm GPU, so anything sequenced after it never
    // runs on a warm boot. Read-only — cannot blank the panel.
    ath_amdgpu::bringup::probe_dcn_scanout(&mut ops);

    // PAGE FLIP — fill VRAM with magenta + point the DCN's HUBP0 surface address at it.
    // If the panel turns magenta, amdgpu is driving the scanout — the "displaying
    // graphics" proof. Warm-testable; runs before init_rings (the GFX power-up that
    // wedges on a warm GPU). Read-back is netlog-verified; the color needs eyes on screen.
    ath_amdgpu::bringup::try_page_flip(&mut ops);

    // ── MES-BYPASS DISPLAY PATH ───────────────────────────────────────────────
    // try_page_flip just proved (CRC + eyes) that we can point the real DCN's HUBP0
    // at a chosen VRAM buffer. Register that SAME buffer as the compositor's external
    // scanout so the AthenaOS DESKTOP is scanned out by the real display engine — a
    // working GPU display with ZERO reliance on the MES. DCN is a separate,
    // firmware-lit power domain that keeps scanning after this daemon exits, so the
    // desktop stays live.
    //
    // We DELIBERATELY SKIP init_rings (the CP/SDMA/MES set_hw_resources stage): its
    // MES `set_hw_resources` is the pipe0 clock-stop that hard-resets the SoC, and the
    // display path needs none of it. (Re-enable init_rings later for GFX/compute
    // submit once the MES environment issue is solved — tracked separately.)
    //
    // Scanout buffer physical = the APU VRAM carveout base + the page-flip scratch
    // offset. Athena's carveout base is FB_OFFSET(0x3e0)<<24 = 0x3E0000000 (oracle-
    // verified MMHUB MMMC_VM_FB_OFFSET). SCRATCH_OFF (0x0600_0000) matches the DCN
    // address try_page_flip programmed into HUBP0 (0x80_0600_0000).
    // (Hardening follow-up: read MMMC_VM_FB_OFFSET live instead of the const.)
    // Done with time-critical GPU register work; drop to Normal so the compositor +
    // persist/netlog threads round-robin freely once we register the scanout.
    unsafe { sys_setpriority_self_normal() };
    const VRAM_PHYS_BASE: u64 = 0x3_E000_0000; // Athena UMA carveout base (FB_OFFSET<<24)
    const SCRATCH_OFF: u64 = 0x0600_0000; // must match try_page_flip's SCRATCH_OFF
    const FB_W: u32 = 1920;
    const FB_H: u32 = 1080;
    let scanout_phys = VRAM_PHYS_BASE + SCRATCH_OFF; // 0x3_E600_0000
    let reg = unsafe {
        ath_linuxkpi::host::sys_register_scanout(dev.handle, scanout_phys, FB_W, FB_H, FB_W * 4)
    };
    klog(&alloc::format!(
        "[amdgpu] BYPASS: DCN desktop scanout register(phys={scanout_phys:#x} {FB_W}x{FB_H}) -> {}",
        if reg == 1 {
            "OK — the AthenaOS desktop is now scanned out by the real DCN (MES-free)"
        } else {
            "FAILED (ownership/bounds gate rejected — see [gpu]/[linuxkpi] lines)"
        }
    ));
    checkpoint(
        "[amdgpu] CKPT 5(bypass): DCN scanout registered — init_rings SKIPPED (MES-free display)",
    );

    if reg == 1 {
        klog(
            "[amdgpu] GPU display online (MES-bypass): real DCN scanning the compositor's desktop",
        );
        unsafe { sys_print(9900) };
        unsafe { sys_exit(0) };
    } else {
        klog("[amdgpu] bypass display registration failed (see CKPT markers)");
        unsafe { sys_print(9001) };
        unsafe { sys_exit(1) };
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        // Keep the panic path allocation- and formatter-free.  The normal
        // panic renderer itself can depend on the LinuxKPI heap that is under
        // investigation; an encoded source line survives through SYS_PRINT
        // and pinpoints the Rust boundary reached by upstream C init.
        let oom_size = ath_linuxkpi::mm::last_alloc_failure_size() as u64;
        if oom_size != 0 {
            sys_print(989_998);
            sys_print(oom_size);
        }
        let line = info
            .location()
            .map(|location| location.line() as u64)
            .unwrap_or(0);
        sys_print(990_000 + line);
        sys_print(9999);
        sys_exit(99);
    }
}
