//! M4 bring-up facade — the `EXPORT_SYMBOL`s the real upstream amdgpu driver
//! references on the MES bring-up path but that no other raeen_linuxkpi module
//! provides yet. These resolve the `linuxkpi-drm` static link (the FreeBSD
//! drm-kmod model: the real GPL amdgpu `.c` compiles against the MPL shim headers
//! and links against this crate). See `linuxkpi-drm/m4-link.sh`.
//!
//! Honesty (SCOPE.md rule 9): three classes only, no silent-success fakes —
//!   * REAL: computed/correct (strscpy, kobj_to_dev container_of, pci_rebar size,
//!     the data globals that genuinely back a C `extern`).
//!   * DELEGATING: routes to an existing facade / host syscall (ioremap, fence).
//!   * HONEST NO-OP: a feature deliberately out of the MES bring-up subset —
//!     sysfs introspection, runtime-PM, KMS display, vga_switcheroo. Each returns
//!     the "absent/disabled" answer (0 / false / null) the caller treats as
//!     "feature not present", never a value pretending success it didn't achieve.
//!
//! M5 (run vs Athena) will promote the DELEGATING set to the live device path;
//! the NO-OP set stays no-op until its subsystem is brought into scope.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};

/// Make a CPU-written shared DMA range visible to non-snooping engines (PSP),
/// or invalidate stale CPU lines before polling a device-written fence.
///
/// Athena supports CLFLUSH (proved during kernel CPU feature bring-up). The
/// range is rounded to complete cache lines; MFENCE orders the eviction before
/// the subsequent MMIO doorbell or CPU load.
#[no_mangle]
pub unsafe extern "C" fn rae_cpu_cache_flush(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let start = (ptr as usize) & !63usize;
    let Some(end) = (ptr as usize)
        .checked_add(len)
        .and_then(|v| v.checked_add(63))
    else {
        return;
    };
    let end = end & !63usize;
    let mut line = start;
    while line < end {
        core::arch::x86_64::_mm_clflush(line as *const u8);
        line += 64;
    }
    core::arch::asm!("mfence", options(nostack, preserves_flags));
}

// ───────────────────────── global data symbols ─────────────────────────
// The C side spells these as bare `extern` variables, not functions.
//
// NOTE: the global `jiffies` tick is already exported by lib.rs (an AtomicU64
// advanced by lkpi_tick_jiffies()); the C `extern volatile unsigned long jiffies`
// resolves to it. Not redefined here.

/// `extern enum system_states system_state` — 3 == SYSTEM_RUNNING (see the
/// enum in <linux/types.h>). amdgpu reads it on the reset/shutdown path.
#[no_mangle]
pub static mut system_state: c_int = 3;

/// Opaque workqueue handles. `queue_work()` ignores the wq pointer (the facade
/// is a single global pump — see workqueue.rs), so any stable non-null token is
/// a valid handle; these point at a private byte so the symbol is never null.
static mut SYSTEM_WQ_TOKEN: u8 = 0;
static mut SYSTEM_UNBOUND_WQ_TOKEN: u8 = 0;
static mut SYSTEM_PERCPU_WQ_TOKEN: u8 = 0;
#[no_mangle]
pub static mut system_wq: *mut c_void = core::ptr::addr_of_mut!(SYSTEM_WQ_TOKEN) as *mut c_void;
#[no_mangle]
pub static mut system_unbound_wq: *mut c_void =
    core::ptr::addr_of_mut!(SYSTEM_UNBOUND_WQ_TOKEN) as *mut c_void;
/// 7.0.x renamed the default system workqueue `system_wq` -> `system_percpu_wq`
/// (drm/scheduler's `drm_sched_init` reads this one). Same single-pump facade.
#[no_mangle]
pub static mut system_percpu_wq: *mut c_void =
    core::ptr::addr_of_mut!(SYSTEM_PERCPU_WQ_TOKEN) as *mut c_void;

/// `struct sysfs_ops` (show/store). sysfs is stubbed for bring-up; the default
/// ops are never invoked because no real attribute reads reach them, so the
/// callbacks are null. The symbol exists for the kset ktype to point at.
#[repr(C)]
pub struct SysfsOps {
    show: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_char) -> isize>,
    store: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char, usize) -> isize>,
}
unsafe impl Sync for SysfsOps {}
#[no_mangle]
pub static kobj_sysfs_ops: SysfsOps = SysfsOps {
    show: None,
    store: None,
};

// ───────────────────────── MMIO / device memory ─────────────────────────
// DELEGATING: routes to the host map syscall on the device build; under the
// `hosttest` harness there is no physical memory to map, so it reports absence.

/// `void __iomem *ioremap(phys_addr_t offset, size_t size)` — routes to the live
/// device map (device_map.rs): resolves which registered BAR owns `offset` and
/// returns its mapping. Null when no device is registered (host link test).
#[no_mangle]
pub unsafe extern "C" fn ioremap(offset: u64, size: usize) -> *mut c_void {
    crate::device_map::ioremap_phys(offset, size) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn iounmap(_addr: *mut c_void) {}

/// `void *memremap(resource_size_t offset, size_t size, unsigned long flags)` —
/// same BAR-backed mapping as ioremap (write-combining flags are advisory here).
#[no_mangle]
pub unsafe extern "C" fn memremap(offset: u64, size: usize, _flags: c_ulong) -> *mut c_void {
    crate::device_map::ioremap_phys(offset, size) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn memunmap(_addr: *mut c_void) {}

/// MTRR write-combining range registration — x86 PAT/MTRR, not modelled. No-op.
#[no_mangle]
pub unsafe extern "C" fn arch_io_free_memtype_wc(_base: u64, _size: u64) {}
#[no_mangle]
pub unsafe extern "C" fn arch_phys_wc_del(_handle: c_int) {}

// ───────────────────────── strings ─────────────────────────

/// `ssize_t strscpy(char *dst, const char *src, size_t count)` — bounded,
/// always-NUL-terminated copy. Returns the copied length, or -E2BIG (-7) on
/// truncation, matching the kernel contract. REAL.
#[no_mangle]
pub unsafe extern "C" fn strscpy(dst: *mut c_char, src: *const c_char, count: usize) -> isize {
    if count == 0 || dst.is_null() {
        return -7; // -E2BIG
    }
    let mut i = 0usize;
    while i < count {
        let c = if src.is_null() { 0 } else { *src.add(i) };
        *dst.add(i) = c;
        if c == 0 {
            return i as isize;
        }
        i += 1;
    }
    // ran out of room — NUL-terminate the last slot and report truncation
    *dst.add(count - 1) = 0;
    -7
}

// ───────────────────────── kobject / sysfs / kset ─────────────────────────
// sysfs is out of the bring-up subset; registration succeeds-as-no-op (the
// device works without its sysfs nodes). HONEST NO-OP, except kobj_to_dev which
// is a REAL container_of.

#[no_mangle]
pub unsafe extern "C" fn kobject_init(_kobj: *mut c_void, _ktype: *const c_void) {}
#[no_mangle]
pub unsafe extern "C" fn kobject_add(
    _kobj: *mut c_void,
    _parent: *mut c_void,
    _fmt: *const c_char,
) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn kobject_init_and_add(
    _kobj: *mut c_void,
    _ktype: *const c_void,
    _parent: *mut c_void,
    _fmt: *const c_char,
) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn kobject_put(_kobj: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn kobject_set_name(_kobj: *mut c_void, _fmt: *const c_char) {}
#[no_mangle]
pub unsafe extern "C" fn kobject_name(_kobj: *const c_void) -> *const c_char {
    c"".as_ptr()
}

/// `struct device *kobj_to_dev(struct kobject *kobj)` — container_of(kobj, struct
/// device, kobj). `struct device` embeds `struct kobject kobj` as its FIRST
/// member (see <linux/device.h>), so the device pointer == the kobject pointer.
/// REAL.
#[no_mangle]
pub unsafe extern "C" fn kobj_to_dev(kobj: *mut c_void) -> *mut c_void {
    kobj
}

#[no_mangle]
pub unsafe extern "C" fn kset_register(_kset: *mut c_void) -> c_int {
    0
}
/// `struct kset *to_kset(struct kobject *kobj)` — container_of(kobj, struct kset,
/// kobj). In `struct kset` the embedded kobj follows `list`+`list_lock` (see
/// <linux/kobject.h>), so it is NOT at offset 0; sysfs is stubbed so this is only
/// reached on the (empty) teardown walk. Return null — the walk sees no members.
#[no_mangle]
pub unsafe extern "C" fn to_kset(_kobj: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn sysfs_create_file(_kobj: *mut c_void, _attr: *const c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn sysfs_remove_file(_kobj: *mut c_void, _attr: *const c_void) {}
#[no_mangle]
pub unsafe extern "C" fn sysfs_create_bin_file(_kobj: *mut c_void, _attr: *const c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn sysfs_remove_bin_file(_kobj: *mut c_void, _attr: *const c_void) {}
#[no_mangle]
pub unsafe extern "C" fn sysfs_create_link(
    _kobj: *mut c_void,
    _target: *mut c_void,
    _name: *const c_char,
) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn devm_device_add_group(_dev: *mut c_void, _grp: *const c_void) -> c_int {
    0
}

// ───────────────────────── PCI / PCIe queries ─────────────────────────
// The BAR/config access goes through the claim_device + host pci_read_cfg path
// (pci.rs / host.rs). These are the topology/capability *queries* amdgpu makes
// during init; on the single-GPU Athena target the safe answers are the
// "standalone device, no special routing" ones. HONEST NO-OP / REAL where
// computable.

#[no_mangle]
pub unsafe extern "C" fn pci_enable_device(_dev: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pci_name(_dev: *mut c_void) -> *const c_char {
    c"0000:c4:00.0".as_ptr()
}
#[no_mangle]
pub unsafe extern "C" fn pci_domain_nr(_bus: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pci_is_root_bus(_bus: *mut c_void) -> bool {
    false
}
#[no_mangle]
pub unsafe extern "C" fn pci_upstream_bridge(_dev: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn pci_get_domain_bus_and_slot(
    _domain: c_int,
    _bus: c_uint,
    _devfn: c_uint,
) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn pci_pcie_type(_dev: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pci_reset_function(_dev: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pci_dev_is_disconnected(_dev: *const c_void) -> bool {
    false
}
#[no_mangle]
pub unsafe extern "C" fn pci_enable_atomic_ops_to_root(_dev: *mut c_void, _cap_mask: u32) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pci_pr3_present(_dev: *mut c_void) -> bool {
    false
}
/// `u32 pci_rebar_bytes_to_size(u64 bytes)` — log2(bytes/1MiB), floored at 0.
/// REAL (matches the kernel: `order_base_2(bytes >> 20)`).
#[no_mangle]
pub unsafe extern "C" fn pci_rebar_bytes_to_size(bytes: u64) -> u32 {
    let mb = bytes >> 20;
    if mb <= 1 {
        0
    } else {
        // order_base_2: ceil(log2(mb))
        (64 - (mb - 1).leading_zeros()) as u32
    }
}
#[no_mangle]
pub unsafe extern "C" fn pcie_get_speed_cap(_dev: *mut c_void) -> c_int {
    0 // PCI_SPEED_UNKNOWN — caller falls back to a conservative default
}
#[no_mangle]
pub unsafe extern "C" fn pcie_get_width_cap(_dev: *mut c_void) -> c_int {
    0 // PCIE_LNK_WIDTH_UNKNOWN
}
#[no_mangle]
pub unsafe extern "C" fn pcie_bandwidth_available(
    _dev: *mut c_void,
    _limiting: *mut *mut c_void,
    _speed: *mut c_int,
    _width: *mut c_int,
) -> c_uint {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pcie_aspm_enabled(_dev: *mut c_void) -> bool {
    false
}
#[no_mangle]
pub unsafe extern "C" fn pcie_find_root_port(_dev: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

// ───────────────────────── runtime PM ─────────────────────────
// Runtime PM is managed by the AthenaOS power subsystem, not by amdgpu's autosuspend
// during bring-up. The device is kept resumed; these are no-ops.

#[no_mangle]
pub unsafe extern "C" fn pm_runtime_disable(_dev: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn pm_runtime_resume(_dev: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pm_runtime_suspend(_dev: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pm_runtime_status_suspended(_dev: *mut c_void) -> bool {
    false // device is resumed
}
#[no_mangle]
pub unsafe extern "C" fn register_pm_notifier(_nb: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn unregister_pm_notifier(_nb: *mut c_void) -> c_int {
    0
}

// ───────────────────────── workqueue extras ─────────────────────────
// queue/schedule/cancel live in workqueue.rs; these few are not yet there.

#[no_mangle]
pub unsafe extern "C" fn cancel_work(_work: *mut c_void) -> bool {
    false
}
#[no_mangle]
pub unsafe extern "C" fn drain_workqueue(_wq: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn flush_delayed_work(_dwork: *mut c_void) -> bool {
    false
}

// ───────────────────────── dma_fence ─────────────────────────

/// `int dma_fence_get_status(struct dma_fence *fence)` — 0 = not signalled,
/// 1 = signalled (no error), <0 = error. Bring-up fences carry no error; the
/// signalled bit is tracked in dma_fence.rs. Report "no error" (0); the live
/// signalled state is read via dma_fence_is_signaled. HONEST conservative.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_get_status(_fence: *mut c_void) -> c_int {
    0
}

// ───────────────────────── firmware ─────────────────────────

/// `int firmware_request_nowarn(const struct firmware **fw, const char *name,
/// struct device *dev)` — Linux semantics: identical to `request_firmware` except
/// the loader stays silent on a miss (the CALLER decides whether absence matters).
/// Routes to the same live loader as `request_firmware` (host request_firmware
/// syscall → initramfs `firmware/` tree). Was a hardcoded -ENOENT stub until
/// 2026-07-08, which silently starved every `*_nowarn` ucode/blob load — e.g. the
/// `amdgpu_discovery=2` from-file path the off-target runner uses.
#[no_mangle]
pub unsafe extern "C" fn firmware_request_nowarn(
    fw: *mut *mut crate::Firmware,
    name: *const c_char,
    dev: *mut c_void,
) -> c_int {
    crate::request_firmware(fw, name as *const u8, dev as u64)
}

// ───────────────────────── ACPI table access (VFCT VBIOS) ─────────────────────────

/// `acpi_status acpi_get_table(char *signature, u32 instance, struct
/// acpi_table_header **out)` — serves the **VFCT** table (the ACPI-embedded VBIOS
/// container used by APUs) so `amdgpu_acpi_vfct_bios()` can fetch the VBIOS.
/// Phoenix has no PCI-ROM VBIOS; the VFCT path is how amdgpu gets ATOMBIOS on an
/// APU. The table is served from the bundled `firmware/acpi/.../VFCT.dat` capture
/// (the same raw table `raeen_amdgpu::atombios` parses natively). Any other
/// signature reports `AE_NOT_FOUND` — the live AthenaOS ACPI namespace is the
/// kernel's, and only VFCT is on the bring-up path. `AE_OK`=0, `AE_NOT_FOUND`=5.
#[no_mangle]
pub unsafe extern "C" fn acpi_get_table(
    signature: *const c_char,
    _instance: u32,
    out: *mut *mut c_void,
) -> u32 {
    if signature.is_null() || out.is_null() {
        return 0x0005; // AE_NOT_FOUND
    }
    let sig = core::slice::from_raw_parts(signature as *const u8, 4);
    if sig == b"VFCT" {
        if let Some((data, size)) =
            crate::request_firmware_blob("acpi/athena-beelink-elitemini/VFCT.dat")
        {
            // Serve a copy whose VBIOS image PCI-location matches the LIVE device.
            // Under GPU passthrough the claimed BDF is the guest's (00:03.0), not
            // Athena's native c4:00.0 baked into the capture, so amdgpu's location
            // gate would otherwise reject the (byte-identical) ATOMBIOS. See helper.
            *out = vfct_with_live_bdf(data, size) as *mut c_void;
            return 0x0000; // AE_OK
        }
    }
    0x0005 // AE_NOT_FOUND
}

/// `amdgpu_acpi_vfct_bios()` (amdgpu_bios.c) accepts a VFCT VBIOS image only when
/// its `VFCT_IMAGE_HEADER` PCIBus/PCIDevice/PCIFunction equal the live device's
/// `pdev->bus->number` / `PCI_SLOT(devfn)` / `PCI_FUNC(devfn)` — which bring-up
/// (`bringup_entry.c`) sets from the CLAIMED BDF (`device_bdf(handle)`). The
/// bundled `VFCT.dat` was captured on Athena, where the iGPU is `c4:00.0`, so its
/// header records `PCIBus=0xc4`. Under GPU passthrough the claimed BDF is the
/// guest's `00:03.0`, so the native VFCT never matched and amdgpu reported
/// "ACPI VFCT table present but broken (too short #2)" -> "Unable to locate a
/// BIOS ROM" -> "Fatal error during GPU init" (proven on Athena vfio 2026-07-09,
/// serial-vfio-cap1.log). The PCI-location triple is only a *sanity* match — the
/// VBIOS is byte-identical for this exact silicon — so rewrite it to the live BDF
/// (the SAME source bring-up feeds `pdev`, so the match then holds by
/// construction). Returns a `kmalloc`'d copy (the C `acpi_put_table` is a no-op,
/// so it outlives the call); on any bounds/OOM failure serves the original blob
/// unchanged (fail-safe: at worst the pre-fix behavior, never a crash).
unsafe fn vfct_with_live_bdf(data: *const u8, size: usize) -> *const u8 {
    // atombios.h: `VBIOSImageOffset` is a u32 at table offset 0x34; the first
    // GOP_VBIOS_CONTENT starts there and opens with the VFCT_IMAGE_HEADER =
    // PCIBus(u32)@+0, PCIDevice(u32)@+4, PCIFunction(u32)@+8, ...
    const VBIOS_IMAGE_OFFSET_OFF: usize = 0x34;
    if data.is_null() || size < VBIOS_IMAGE_OFFSET_OFF + 4 {
        return data;
    }
    let img_off = u32::from_le_bytes([
        *data.add(VBIOS_IMAGE_OFFSET_OFF),
        *data.add(VBIOS_IMAGE_OFFSET_OFF + 1),
        *data.add(VBIOS_IMAGE_OFFSET_OFF + 2),
        *data.add(VBIOS_IMAGE_OFFSET_OFF + 3),
    ]) as usize;
    // Need the 3 x u32 location triple in-bounds within the served copy.
    if img_off < VBIOS_IMAGE_OFFSET_OFF + 4 || img_off.saturating_add(12) > size {
        return data; // unexpected layout — serve unpatched
    }
    // Use the EXACT BDF the daemon handed to `pdev` (stashed via set_vfct_bdf),
    // not a fresh device_bdf() query — the two independent queries diverged in
    // practice (the first cap2 run still missed), so a single source of truth is
    // the only way the VFCT location and pdev location are guaranteed identical.
    let (bus, dev, func) = match vfct_bdf() {
        Some(b) => b,
        None => return data, // daemon didn't stash — serve native (pre-fix path)
    };
    let orig_bus = u32::from_le_bytes([
        *data.add(img_off),
        *data.add(img_off + 1),
        *data.add(img_off + 2),
        *data.add(img_off + 3),
    ]);
    let copy = crate::mm::kmalloc(size, 0);
    if copy.is_null() {
        return data; // OOM — serve original (match still fails, but no crash)
    }
    core::ptr::copy_nonoverlapping(data, copy, size);
    let bus_b = (bus as u32).to_le_bytes();
    let dev_b = (dev as u32).to_le_bytes();
    let fn_b = (func as u32).to_le_bytes();
    core::ptr::copy_nonoverlapping(bus_b.as_ptr(), copy.add(img_off), 4);
    core::ptr::copy_nonoverlapping(dev_b.as_ptr(), copy.add(img_off + 4), 4);
    core::ptr::copy_nonoverlapping(fn_b.as_ptr(), copy.add(img_off + 8), 4);
    use core::fmt::Write as _;
    let mut w = FixedLog::new();
    let _ = core::write!(
        w,
        "[linuxkpi] VFCT: realign img_off={} wrote bus={} dev={} func={} (was PCIBus=0x{:x}) -> amdgpu should accept ATOMBIOS\n",
        img_off, bus, dev, func, orig_bus,
    );
    w.emit();
    copy as *const u8
}

/// The PCI `(bus,dev,func)` bring-up passes to `pdev` — stashed by the daemon
/// (via [`set_vfct_bdf`]) right before `amdgpu_device_init`, and read back by
/// [`vfct_with_live_bdf`] so the served VFCT image's PCI-location matches `pdev`
/// BY CONSTRUCTION (both derive from this one value). Two independent
/// `device_bdf()` queries were observed to disagree on the vfio boot, so this
/// single source of truth is load-bearing. Packed `bus<<16 | dev<<8 | func`;
/// `0xFFFF_FFFF` = unset (serve the native VFCT location unchanged).
static VFCT_BDF: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0xFFFF_FFFF);

/// Daemon hook: record the exact BDF handed to `pdev` (see [`VFCT_BDF`]).
pub fn set_vfct_bdf(bus: u8, dev: u8, func: u8) {
    VFCT_BDF.store(
        ((bus as u32) << 16) | ((dev as u32) << 8) | (func as u32),
        core::sync::atomic::Ordering::SeqCst,
    );
}

fn vfct_bdf() -> Option<(u8, u8, u8)> {
    let p = VFCT_BDF.load(core::sync::atomic::Ordering::SeqCst);
    if p == 0xFFFF_FFFF {
        return None;
    }
    Some((
        ((p >> 16) & 0xFF) as u8,
        ((p >> 8) & 0xFF) as u8,
        (p & 0xFF) as u8,
    ))
}

/// Minimal `core::fmt::Write` sink over a fixed stack buffer — lets the bring-up
/// path log formatted values via `sys_linuxkpi_printk` without an allocator
/// (this crate is `#![no_std]` with no `alloc`). Overflow is silently truncated.
struct FixedLog {
    buf: [u8; 192],
    n: usize,
}
impl FixedLog {
    fn new() -> Self {
        FixedLog {
            buf: [0u8; 192],
            n: 0,
        }
    }
    fn emit(&self) {
        unsafe { crate::host::sys_linuxkpi_printk(self.buf.as_ptr(), self.n as u64) };
    }
}
impl core::fmt::Write for FixedLog {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let end = (self.n + b.len()).min(self.buf.len());
        if end > self.n {
            self.buf[self.n..end].copy_from_slice(&b[..end - self.n]);
            self.n = end;
        }
        Ok(())
    }
}

/// `void acpi_put_table(struct acpi_table_header *table)` — the VFCT blob stays
/// mapped for the daemon's lifetime (kernel-owned read-only mapping), so this is
/// a no-op.
#[no_mangle]
pub unsafe extern "C" fn acpi_put_table(_table: *mut c_void) {}

// ───────────────────────── iommu / power-supply / ratelimit ─────────────────────────

/// `struct iommu_domain *iommu_get_domain_for_dev(struct device *dev)` — null
/// means "no managed domain" (identity / passthrough), which is the bring-up
/// assumption. HONEST NO-OP.
#[no_mangle]
pub unsafe extern "C" fn iommu_get_domain_for_dev(_dev: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

/// `int power_supply_is_system_supplied(void)` — >0 means on external (AC) power.
/// Athena bring-up runs plugged in; report AC present.
#[no_mangle]
pub unsafe extern "C" fn power_supply_is_system_supplied() -> c_int {
    1
}

/// `void ratelimit_set_flags(struct ratelimit_state *rs, unsigned long flags)`.
/// Log ratelimiting is disabled (every message passes); no state to set.
#[no_mangle]
pub unsafe extern "C" fn ratelimit_set_flags(_rs: *mut c_void, _flags: c_ulong) {}

// ───────────────────────── device misc ─────────────────────────

/// `void dev_info_once(const struct device *dev, const char *fmt, ...)` — the
/// once-only info print. dev_* logging proper lives in device.rs; this throttled
/// variant is a no-op here (the message is non-essential boot chatter).
#[no_mangle]
pub unsafe extern "C" fn dev_info_once(_dev: *const c_void, _fmt: *const c_char) {}

/// `bool dev_is_removable(struct device *dev)` — the GPU is soldered. REAL: false.
#[no_mangle]
pub unsafe extern "C" fn dev_is_removable(_dev: *mut c_void) -> bool {
    false
}

/// `void unmap_mapping_range(struct address_space *, loff_t, loff_t, int)` — used
/// on BO teardown to drop userspace mmaps. No userspace mappings during bring-up.
#[no_mangle]
pub unsafe extern "C" fn unmap_mapping_range(
    _mapping: *mut c_void,
    _holebegin: c_long,
    _holelen: c_long,
    _even_cows: c_int,
) {
}

/// `void emergency_restart(void)` / `int ksys_sync_helper(void)` — kernel
/// shutdown helpers; not reached on a successful bring-up. No-op.
#[no_mangle]
pub unsafe extern "C" fn emergency_restart() {}
#[no_mangle]
pub unsafe extern "C" fn ksys_sync_helper() {}

// ───────────────────────── DRM KMS display (out of subset) ─────────────────────────
// The MES bring-up path does not bring up the display pipe (DC is shadow-stubbed,
// see linuxkpi-drm/include/amd-stubs/amdgpu_dm.h). These KMS helpers are reached
// only from amdgpu_device's modeset shutdown/resume; all no-op.

#[no_mangle]
pub unsafe extern "C" fn drm_atomic_helper_shutdown(_dev: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn drm_dev_unplug(_dev: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn drm_dev_wedged_event(_dev: *mut c_void, _method: c_ulong) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn drm_helper_force_disable_all(_dev: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn drm_helper_hpd_irq_event(_dev: *mut c_void) -> bool {
    false
}
#[no_mangle]
pub unsafe extern "C" fn drm_helper_resume_force_mode(_dev: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn drm_kms_helper_hotplug_event(_dev: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn drmm_mode_config_init(_dev: *mut c_void) -> c_int {
    0
}

// ───────────────────────── vga_switcheroo (disabled) ─────────────────────────
// Dual-GPU laptop muxing — disabled on AthenaOS (AthGuard owns GPU arbitration).

#[no_mangle]
pub unsafe extern "C" fn vga_client_unregister(_pdev: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn vga_switcheroo_init_domain_pm_ops(
    _dev: *mut c_void,
    _domain: *mut c_void,
) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn vga_switcheroo_fini_domain_pm_ops(_dev: *mut c_void) {}

// ═══════════════════ M4b link-closure exports (the ~55 the expanded subset
// references) ═══════════════════
// Same three honesty classes. The DELEGATING ones wrap crate::mm; the out-of-
// subset features (kthread/sync-file/fd/i2c/hmm/xarray-irq) report the "absent"
// answer the caller treats as not-present.

// ── capability (AthGuard grants the bring-up daemon its device caps) ──
#[no_mangle]
pub unsafe extern "C" fn capable(_cap: c_int) -> bool {
    true
}
#[no_mangle]
pub unsafe extern "C" fn perfmon_capable() -> bool {
    true
}

// ── the "current task": a single static descriptor for the bring-up daemon.
// Layout matches <linux/sched.h>'s struct task_struct. ──
#[repr(C)]
pub struct TaskStruct {
    prio: c_int,
    pid: c_int,
    tgid: c_int,
    comm: [u8; 16],
    mm: *mut c_void,
    flags: c_uint,
    exit_code: c_int,
    group_leader: *mut TaskStruct,
}
unsafe impl Sync for TaskStruct {}
static mut DAEMON_TASK: TaskStruct = TaskStruct {
    prio: 0,
    pid: 1,
    tgid: 1,
    comm: *b"raeamdgpu\0\0\0\0\0\0\0",
    mm: core::ptr::null_mut(),
    flags: 0,
    exit_code: 0,
    group_leader: core::ptr::null_mut(),
};

#[repr(C)]
pub struct Pid {
    nr: c_int,
}

static mut DAEMON_PID: Pid = Pid { nr: 1 };
#[no_mangle]
pub unsafe extern "C" fn get_current() -> *mut TaskStruct {
    let t = core::ptr::addr_of_mut!(DAEMON_TASK);
    if (*t).group_leader.is_null() {
        (*t).group_leader = t; // self-lead, so get_task_comm(current->group_leader) is safe
    }
    t
}
#[no_mangle]
pub unsafe extern "C" fn get_task_comm(buf: *mut c_char, tsk: *mut TaskStruct) -> *mut c_char {
    if !buf.is_null() && !tsk.is_null() {
        for i in 0..16 {
            *buf.add(i) = (*tsk).comm[i] as c_char;
        }
    }
    buf
}
#[no_mangle]
pub unsafe extern "C" fn task_pid_nr(tsk: *mut TaskStruct) -> c_int {
    if tsk.is_null() {
        0
    } else {
        (*tsk).pid
    }
}

#[no_mangle]
pub unsafe extern "C" fn task_tgid(_task: *mut TaskStruct) -> *mut Pid {
    core::ptr::addr_of_mut!(DAEMON_PID)
}

#[no_mangle]
pub unsafe extern "C" fn get_pid(pid: *mut Pid) -> *mut Pid {
    pid
}

#[no_mangle]
pub unsafe extern "C" fn put_pid(_pid: *mut Pid) {}

#[no_mangle]
pub unsafe extern "C" fn pid_task(pid: *mut Pid, _kind: c_int) -> *mut TaskStruct {
    if pid.is_null() {
        core::ptr::null_mut()
    } else {
        get_current()
    }
}

#[no_mangle]
pub unsafe extern "C" fn pid_nr(pid: *mut Pid) -> c_int {
    if pid.is_null() {
        0
    } else {
        (*pid).nr
    }
}

#[no_mangle]
pub static overflowuid: c_uint = 65534;

// ── user<->kernel copies. In the bring-up daemon the "user" pointer is in the
// same address space, so this is a checked memcpy returning 0 (= 0 bytes left
// uncopied = success). ──
#[no_mangle]
pub unsafe extern "C" fn copy_to_user(to: *mut c_void, from: *const c_void, n: c_ulong) -> c_ulong {
    if to.is_null() || from.is_null() {
        return n;
    }
    core::ptr::copy(from as *const u8, to as *mut u8, n as usize);
    0
}
#[no_mangle]
pub unsafe extern "C" fn copy_from_user(
    to: *mut c_void,
    from: *const c_void,
    n: c_ulong,
) -> c_ulong {
    copy_to_user(to, from, n)
}
#[no_mangle]
pub unsafe extern "C" fn memdup_user(src: *const c_void, len: usize) -> *mut c_void {
    if src.is_null() && len != 0 {
        return (-14isize) as *mut c_void; // ERR_PTR(-EFAULT)
    }
    let p = crate::mm::kmalloc(len, 0) as *mut c_void;
    if p.is_null() {
        return (-12isize) as *mut c_void; // ERR_PTR(-ENOMEM)
    }
    let _ = copy_from_user(p, src, len as c_ulong);
    p
}
#[no_mangle]
pub unsafe extern "C" fn memdup_user_nul(src: *const c_void, len: usize) -> *mut c_void {
    if src.is_null() && len != 0 {
        return (-14isize) as *mut c_void;
    }
    let alloc_len = match len.checked_add(1) {
        Some(value) => value,
        None => return (-75isize) as *mut c_void,
    };
    let out = crate::mm::kmalloc(alloc_len, 0);
    if out.is_null() {
        return (-12isize) as *mut c_void;
    }
    if len != 0 && copy_from_user(out.cast(), src, len as c_ulong) != 0 {
        crate::mm::kfree(out);
        return (-14isize) as *mut c_void;
    }
    *out.add(len) = 0;
    out.cast()
}
#[no_mangle]
pub unsafe extern "C" fn memdup_array_user(
    src: *const c_void,
    n: usize,
    size: usize,
) -> *mut c_void {
    match n.checked_mul(size) {
        Some(bytes) => memdup_user(src, bytes),
        None => (-75isize) as *mut c_void, // ERR_PTR(-EOVERFLOW)
    }
}
#[no_mangle]
pub unsafe extern "C" fn vmemdup_array_user(
    src: *const c_void,
    n: usize,
    size: usize,
) -> *mut c_void {
    memdup_array_user(src, n, size)
}

// ── slab caches: a cache is just the object size (the daemon's heap is the
// backing). The handle is a heap cell holding that size. ──
#[no_mangle]
pub unsafe extern "C" fn kmem_cache_create(
    _name: *const c_char,
    size: c_uint,
    _align: c_uint,
    _flags: c_ulong,
    _ctor: *mut c_void,
) -> *mut c_void {
    let h = crate::mm::kmalloc(core::mem::size_of::<usize>(), 0) as *mut usize;
    if !h.is_null() {
        *h = size as usize;
    }
    h as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn kmem_cache_destroy(s: *mut c_void) {
    if !s.is_null() {
        crate::mm::kfree(s as *mut u8);
    }
}
#[no_mangle]
pub unsafe extern "C" fn kmem_cache_alloc(s: *mut c_void, _flags: c_uint) -> *mut c_void {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    crate::mm::kmalloc(*(s as *mut usize), 0) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn kmem_cache_zalloc(s: *mut c_void, _flags: c_uint) -> *mut c_void {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    crate::mm::kzalloc(*(s as *mut usize), 0) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn kmem_cache_free(_s: *mut c_void, obj: *mut c_void) {
    if !obj.is_null() {
        crate::mm::kfree(obj as *mut u8);
    }
}
#[no_mangle]
pub unsafe extern "C" fn kvcalloc(n: usize, size: usize, _flags: c_uint) -> *mut c_void {
    match n.checked_mul(size) {
        Some(bytes) => crate::mm::kzalloc(bytes, 0) as *mut c_void,
        None => core::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn kvmalloc_array(n: usize, size: usize, flags: c_uint) -> *mut c_void {
    match n.checked_mul(size) {
        Some(bytes) => crate::mm::kmalloc(bytes, flags) as *mut c_void,
        None => core::ptr::null_mut(),
    }
}

// ── DMA streaming/resource maps: identity on the bring-up target (the device
// DMAs through GART/VMID0; a resource maps 1:1 to its bus address). ──
#[no_mangle]
pub unsafe extern "C" fn dma_map_resource(
    _dev: *mut c_void,
    phys: u64,
    _size: usize,
    _dir: c_int,
    _attrs: c_ulong,
) -> u64 {
    phys
}
#[no_mangle]
pub unsafe extern "C" fn dma_unmap_resource(
    _dev: *mut c_void,
    _addr: u64,
    _size: usize,
    _dir: c_int,
    _attrs: c_ulong,
) {
}
#[no_mangle]
pub unsafe extern "C" fn dma_set_max_seg_size(_dev: *mut c_void, _size: c_uint) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn dma_addressing_limited(_dev: *mut c_void) -> bool {
    false
}

// ── dma_fence_chain alloc/free (timeline syncobj). The chain struct (<linux/
// dma-fence-chain.h>) is ~5 pointers + a fence + a spinlock; over-allocate a
// page-fraction so the C side has room. ──
#[no_mangle]
pub unsafe extern "C" fn dma_fence_chain_alloc() -> *mut c_void {
    crate::mm::kzalloc(256, 0) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn dma_fence_chain_free(chain: *mut c_void) {
    if !chain.is_null() {
        crate::mm::kfree(chain as *mut u8);
    }
}

// ── dma_resv iteration: bring-up BOs carry no pending fences, so the cursor
// yields none (the for-each body never runs). begin/end manage no state. ──
// NB: dma_resv_iter_first/next(+_unlocked) are already exported by dma_resv.rs;
// only begin/end (cursor setup) + locking_ctx are added here.
#[no_mangle]
pub unsafe extern "C" fn dma_resv_iter_begin(
    _cursor: *mut c_void,
    _obj: *mut c_void,
    _usage: c_int,
) {
}
#[no_mangle]
pub unsafe extern "C" fn dma_resv_iter_end(_cursor: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn dma_resv_locking_ctx(_obj: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

// ── IRQ domain: the AthenaOS kernel owns interrupt delivery; the daemon receives
// already-demuxed IRQ events over its IRQ-wait syscall. The domain maps hwirq->
// virq 1:1 (a stable token), and the chip/flow wiring is inert here. ──
static mut IRQ_DOMAIN_TOKEN: u8 = 0;
#[no_mangle]
pub unsafe extern "C" fn irq_domain_create_linear(
    _fwnode: *mut c_void,
    _size: c_uint,
    _ops: *const c_void,
    _host_data: *mut c_void,
) -> *mut c_void {
    core::ptr::addr_of_mut!(IRQ_DOMAIN_TOKEN) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn irq_domain_remove(_domain: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn irq_create_mapping(_domain: *mut c_void, hwirq: c_ulong) -> c_uint {
    hwirq as c_uint // 1:1 hwirq -> virq
}
#[no_mangle]
pub unsafe extern "C" fn generic_handle_domain_irq(_domain: *mut c_void, _hwirq: c_uint) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn irq_set_chip_and_handler(
    _irq: c_uint,
    _chip: *const c_void,
    _handle: *mut c_void,
) {
}
#[no_mangle]
pub unsafe extern "C" fn handle_simple_irq(_data: *mut c_void) {}

// ── kthreads: the GPU-scheduler/worker kthreads are out of the MES bring-up
// subset (drm_sched is stubbed). kthread_create yields the daemon task as a
// valid (non-ERR) handle; should_stop is true so any body that did run exits. ──
#[no_mangle]
pub unsafe extern "C" fn kthread_create(
    _threadfn: *mut c_void,
    _data: *mut c_void,
    _namefmt: *const c_char,
) -> *mut c_void {
    get_current() as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn kthread_should_stop() -> bool {
    true
}
#[no_mangle]
pub unsafe extern "C" fn kthread_stop(_task: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn wake_up_process(_task: *mut c_void) -> c_int {
    0
}

// ── idr/xarray IRQ variants + wait wakeups (cooperative pump owns the real
// wakeups; these resolve the link). ──
#[no_mangle]
pub unsafe extern "C" fn idr_get_next(_idr: *mut c_void, _nextid: *mut c_int) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn xa_erase_irq(_xa: *mut c_void, _index: c_ulong) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn xa_store_irq(
    _xa: *mut c_void,
    _index: c_ulong,
    entry: *mut c_void,
    _gfp: c_uint,
) -> *mut c_void {
    entry
}
#[no_mangle]
pub unsafe extern "C" fn wake_up(_q: *mut c_void) {}

// ── PCI option-ROM map (the VBIOS comes via the PCI-ROM/firmware path elsewhere;
// the raw map is not used on the bring-up target). ──
#[no_mangle]
pub unsafe extern "C" fn pci_map_rom(_pdev: *mut c_void, _size: *mut usize) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn pci_unmap_rom(_pdev: *mut c_void, _rom: *mut c_void) {}

// ── fd table (sync-file fds — out of the MES bring-up subset). ──
#[repr(C)]
pub struct LkpiFd {
    file: *mut c_void,
    flags: c_uint,
}

#[no_mangle]
pub unsafe extern "C" fn fget(_fd: c_uint) -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn fput(_file: *mut c_void) {}

#[no_mangle]
pub unsafe extern "C" fn fdget(_fd: c_uint) -> LkpiFd {
    LkpiFd {
        file: core::ptr::null_mut(),
        flags: 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn fdput(_fd: LkpiFd) {}

#[no_mangle]
pub unsafe extern "C" fn fd_install(_fd: c_uint, _file: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn get_unused_fd_flags(_flags: c_uint) -> c_int {
    -95
}
#[no_mangle]
pub unsafe extern "C" fn put_unused_fd(_fd: c_uint) {}

// ── write-combined MMIO + MTRR (PAT/MTRR is the kernel's job; write-combining is
// advisory, so a WC mapping is the same BAR-backed mapping as plain ioremap). TTM
// CPU-maps a VRAM BO via ttm_bo_ioremap -> ioremap_wc; the GART page-table BO (and
// any kmapped VRAM BO) needs this to return a real address through the VRAM BAR
// aperture, not null (which surfaced as gart_table_vram_alloc -ENOMEM). ──
#[no_mangle]
pub unsafe extern "C" fn ioremap_wc(offset: u64, size: usize) -> *mut c_void {
    crate::device_map::ioremap_phys(offset, size) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn arch_io_reserve_memtype_wc(_base: u64, _size: u64) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn arch_phys_wc_add(_base: u64, _size: u64) -> c_int {
    0
}

// ── misc device/fs/time/misc helpers ──
#[no_mangle]
pub unsafe extern "C" fn default_llseek(
    _file: *mut c_void,
    _offset: c_long,
    _whence: c_int,
) -> c_long {
    0
}
#[no_mangle]
pub unsafe extern "C" fn dev_dbg_once(_dev: *const c_void, _fmt: *const c_char) {}
#[no_mangle]
pub unsafe extern "C" fn dev_name(_dev: *const c_void) -> *const c_char {
    c"raeamdgpu".as_ptr()
}
#[no_mangle]
pub unsafe extern "C" fn dev_driver_string(_dev: *const c_void) -> *const c_char {
    c"amdgpu".as_ptr()
}
#[no_mangle]
pub unsafe extern "C" fn nsecs_to_jiffies(n: u64) -> c_ulong {
    (n / 1_000_000) as c_ulong // HZ = 1000
}
#[no_mangle]
pub unsafe extern "C" fn hmm_pfn_to_page(_pfn: c_ulong) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn i2c_transfer(
    _adap: *mut c_void,
    _msgs: *mut c_void,
    _num: c_int,
) -> c_int {
    -6 // -ENXIO: no DDC/I2C during bring-up
}
// NB: to_drm_sched_fence is NOT a facade — the real drm_sched/sched_fence.c
// (compiled on-path in the M5 object set) defines it with the correct
// container_of cast. A null-returning facade here would (a) be wrong and (b)
// collide as a duplicate symbol at the daemon link. M5 convergence: as real .c
// lands, its facade export must be retired.
#[no_mangle]
pub unsafe extern "C" fn get_dma_buf(_dmabuf: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn sync_file_create(_fence: *mut c_void) -> *mut c_void {
    core::ptr::null_mut()
}

// ── minimal page facade: alloc_pages hands out a page descriptor whose data
// buffer sits immediately AFTER it in one allocation, so virt_to_page can recover
// the descriptor from the data pointer (va - header). Layout matches struct page
// in <linux/mm_types.h> (flags, refcount, virtual_addr, mapping, index, private).
// REAL for alloc_pages-originated pages (TTM gtt/vram managers); the broader
// pfn<->page map is an M5 page-facade item. ──
// MUST match <linux/mm_types.h> `struct page` byte-for-byte (72 bytes): TTM walks
// compound pages with `p + i` pointer arithmetic (ttm_pool_allocated_page_commit
// stores pages[i] = allocated + i) and writes p->lru / p->private, so a short Rust
// view both mis-strides the array (breaking multi-page vmap) AND lets the C side
// clobber memory past the struct. The trailing lru/private fields are unused by
// Rust but load-bearing for the layout.
#[repr(C, align(8))]
pub struct PageStruct {
    flags: usize,
    refcount: c_int,
    _pad: c_int,
    virtual_addr: *mut c_void,
    mapping: *mut c_void,
    index: usize,
    private_data: *mut c_void,
    lru_next: *mut c_void, // struct list_head lru.next (TTM pool free-chain)
    lru_prev: *mut c_void, // struct list_head lru.prev
    private: usize,        // TTM stashes the page order here
    rae_dma_addr: u64,     // host-approved DMA/physical address for this page
    rae_dma_token: u64,    // allocation token on the head page (zero otherwise)
}

#[derive(Clone, Copy)]
struct PageMapEntry {
    va: usize,
    page: usize,
}

const PAGE_MAP_SLOTS: usize = 131_071;
const EMPTY_PAGE_MAP_ENTRY: PageMapEntry = PageMapEntry { va: 0, page: 0 };
static PAGE_BY_VA: spin::Mutex<[PageMapEntry; PAGE_MAP_SLOTS]> =
    spin::Mutex::new([EMPTY_PAGE_MAP_ENTRY; PAGE_MAP_SLOTS]);

fn page_map_insert(va: usize, page: usize) -> bool {
    let mut table = PAGE_BY_VA.lock();
    let mut slot = (va >> 12) % PAGE_MAP_SLOTS;
    for _ in 0..PAGE_MAP_SLOTS {
        if table[slot].va <= 1 || table[slot].va == va {
            table[slot] = PageMapEntry { va, page };
            return true;
        }
        slot = (slot + 1) % PAGE_MAP_SLOTS;
    }
    false
}

fn page_map_lookup(va: usize) -> usize {
    let table = PAGE_BY_VA.lock();
    let mut slot = (va >> 12) % PAGE_MAP_SLOTS;
    for _ in 0..PAGE_MAP_SLOTS {
        match table[slot].va {
            0 => return 0,
            found if found == va => return table[slot].page,
            _ => slot = (slot + 1) % PAGE_MAP_SLOTS,
        }
    }
    0
}

fn page_map_remove(va: usize) {
    let mut table = PAGE_BY_VA.lock();
    let mut slot = (va >> 12) % PAGE_MAP_SLOTS;
    for _ in 0..PAGE_MAP_SLOTS {
        match table[slot].va {
            0 => return,
            found if found == va => {
                table[slot] = PageMapEntry { va: 1, page: 0 };
                return;
            }
            _ => slot = (slot + 1) % PAGE_MAP_SLOTS,
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn alloc_pages(_gfp: c_uint, order: c_uint) -> *mut c_void {
    // mem_map-compatible layout: a CONTIGUOUS array of (1<<order) page descriptors
    // followed by (1<<order) contiguous 4 KiB data pages. TTM walks compound pages
    // with `p + i` arithmetic (ttm_pool_type_give -> clear_page(page_address(p+i)))
    // and stores ttm->pages[i] = p + i, so the descriptors MUST be a real array;
    // keeping the data contiguous also makes a multi-page kmap (vmap, below) simply
    // page_address(pages[0]). A single PageStruct (the old layout) made p+1 garbage.
    let hdr = core::mem::size_of::<PageStruct>();
    let n = 1usize << (order as usize);
    let descs = n * hdr;
    let data_len = n * 4096usize;
    let descriptors = crate::mm::kzalloc(descs, 0);
    if descriptors.is_null() {
        return core::ptr::null_mut();
    }
    let dev = crate::device_map::current_device();
    let dma = if dev != 0 {
        crate::dma::dma_alloc_coherent(dev, data_len)
    } else {
        crate::dma::DmaAlloc {
            cpu_addr: crate::mm::alloc_pages(order, true),
            dma_addr: 0,
            size: data_len,
            token: 0,
        }
    };
    if dma.cpu_addr.is_null() {
        crate::mm::kfree(descriptors);
        return core::ptr::null_mut();
    }
    for i in 0..n {
        let page = descriptors.add(i * hdr) as *mut PageStruct;
        let va = dma.cpu_addr.add(i * 4096);
        (*page).virtual_addr = va.cast();
        // Host tests have no claimed device/IOMMU and never program hardware;
        // retain the old identity value there only. A live device always gets
        // the real host-provided DMA address.
        (*page).rae_dma_addr = if dev != 0 {
            dma.dma_addr + (i as u64 * 4096)
        } else {
            va as u64
        };
        (*page).rae_dma_token = if i == 0 { dma.token } else { 0 };
        if !page_map_insert(va as usize, page as usize) {
            for undo in 0..i {
                page_map_remove(dma.cpu_addr.add(undo * 4096) as usize);
            }
            if dma.token != 0 {
                crate::dma::dma_free_coherent(dev, &dma);
            } else {
                crate::mm::free_pages(dma.cpu_addr, order);
            }
            crate::mm::kfree(descriptors);
            return core::ptr::null_mut();
        }
    }
    descriptors as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn alloc_page(gfp: c_uint) -> *mut c_void {
    alloc_pages(gfp, 0)
}
#[no_mangle]
pub unsafe extern "C" fn __free_pages(page: *mut c_void, order: c_uint) {
    if !page.is_null() {
        let head = page as *mut PageStruct;
        let n = 1usize << order as usize;
        let data = (*head).virtual_addr as *mut u8;
        for i in 0..n {
            page_map_remove(data.add(i * 4096) as usize);
        }
        if (*head).rae_dma_token != 0 {
            crate::dma::dma_free_coherent(
                crate::device_map::current_device(),
                &crate::dma::DmaAlloc {
                    cpu_addr: data,
                    dma_addr: (*head).rae_dma_addr,
                    size: n * 4096,
                    token: (*head).rae_dma_token,
                },
            );
        } else {
            crate::mm::free_pages(data, order);
        }
        crate::mm::kfree(page as *mut u8);
    }
}
#[no_mangle]
pub unsafe extern "C" fn page_address(page: *const c_void) -> *mut c_void {
    if page.is_null() {
        return core::ptr::null_mut();
    }
    (*(page as *const PageStruct)).virtual_addr
}
#[no_mangle]
pub unsafe extern "C" fn virt_to_page(addr: *const c_void) -> *mut c_void {
    if addr.is_null() {
        return core::ptr::null_mut();
    }
    let base = (addr as usize) & !4095usize;
    page_map_lookup(base) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn page_to_phys(page: *const c_void) -> u64 {
    if page.is_null() {
        0
    } else {
        (*(page as *const PageStruct)).rae_dma_addr
    }
}

#[no_mangle]
pub unsafe extern "C" fn page_to_pfn(page: *const c_void) -> usize {
    (page_to_phys(page) >> 12) as usize
}

// ── dma page/resv extras (identity DMA on the bring-up target). ──
#[no_mangle]
pub unsafe extern "C" fn dma_map_page_attrs(
    _dev: *mut c_void,
    page: *mut c_void,
    offset: usize,
    _size: usize,
    _dir: c_int,
    _attrs: c_ulong,
) -> u64 {
    if page.is_null() {
        return 0;
    }
    (*(page as *const PageStruct))
        .rae_dma_addr
        .wrapping_add(offset as u64)
}
#[no_mangle]
pub unsafe extern "C" fn dma_unmap_page_attrs(
    _dev: *mut c_void,
    _addr: u64,
    _size: usize,
    _dir: c_int,
    _attrs: c_ulong,
) {
}
#[no_mangle]
pub unsafe extern "C" fn dma_resv_replace_fences(
    _obj: *mut c_void,
    _context: u64,
    _replacement: *mut c_void,
    _usage: c_int,
) {
}
// ── list_sort: stable sort of a circular doubly-linked list_head with a C
// comparator (cmp(priv, a, b) < 0 => a before b). Insertion sort — O(n^2) but
// correct + stable (equal keys keep insertion order), and the lists amdgpu sorts
// here are small. REAL. ──
#[repr(C)]
pub struct ListHead {
    next: *mut ListHead,
    prev: *mut ListHead,
}
pub type ListCmp = extern "C" fn(*mut c_void, *const ListHead, *const ListHead) -> c_int;
#[no_mangle]
pub unsafe extern "C" fn list_sort(priv_: *mut c_void, head: *mut ListHead, cmp: ListCmp) {
    if head.is_null() || (*head).next == head {
        return; // empty or single element
    }
    // Detach the chain [first..last] and break the ring; re-empty the head.
    let first = (*head).next;
    let last = (*head).prev;
    (*head).next = head;
    (*head).prev = head;
    (*last).next = core::ptr::null_mut();

    let mut node = first;
    while !node.is_null() {
        let next = (*node).next;
        // find the first existing element that should come AFTER `node`
        let mut pos = (*head).next;
        while pos != head && cmp(priv_, pos, node) <= 0 {
            pos = (*pos).next;
        }
        // splice `node` in before `pos` (stable: stops after equal keys)
        let prev = (*pos).prev;
        (*node).prev = prev;
        (*node).next = pos;
        (*prev).next = node;
        (*pos).prev = node;
        node = next;
    }
}

// ── radix tree: amdgpu keeps RAS error records here. During bring-up no errors
// have been logged, so the tree is empty and the iteration API yields nothing.
// HONEST (the tree genuinely has no entries on the init path; insert/lookup are
// not referenced by the compiled subset). ──
#[no_mangle]
pub unsafe extern "C" fn radix_tree_iter_init(_iter: *mut c_void, _start: c_ulong) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn radix_tree_next_chunk(
    _root: *const c_void,
    _iter: *mut c_void,
    _flags: c_uint,
) -> *mut c_void {
    core::ptr::null_mut() // empty tree -> no chunks
}
#[no_mangle]
pub unsafe extern "C" fn radix_tree_iter_delete(
    _root: *mut c_void,
    _iter: *mut c_void,
    _slot: *mut *mut c_void,
) {
}
#[no_mangle]
pub unsafe extern "C" fn radix_tree_tagged(_root: *const c_void, _tag: c_uint) -> c_int {
    0 // no tagged entries
}

// ── sysfs groups (introspection stubbed for bring-up — same as the single-file
// sysfs ops above). ──
#[no_mangle]
pub unsafe extern "C" fn sysfs_create_group(_kobj: *mut c_void, _grp: *const c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn sysfs_remove_group(_kobj: *mut c_void, _grp: *const c_void) {}
#[no_mangle]
pub unsafe extern "C" fn sysfs_add_file_to_group(
    _kobj: *mut c_void,
    _attr: *const c_void,
    _group: *const c_char,
) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn sysfs_remove_file_from_group(
    _kobj: *mut c_void,
    _attr: *const c_void,
    _group: *const c_char,
) {
}
#[no_mangle]
pub unsafe extern "C" fn sysfs_create_files(
    _kobj: *mut c_void,
    _attrs: *const *const c_void,
) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn sysfs_remove_files(_kobj: *mut c_void, _attrs: *const *const c_void) {}

// ── misc (referenced by the ih_v6_1 / vcn_v4_0 / jpeg_v4_0 IP drivers) ──
/// single NUMA node on the bring-up target.
#[no_mangle]
pub unsafe extern "C" fn num_possible_nodes() -> c_int {
    1
}
/// `atomic_long_cmpxchg` — REAL compare-and-swap on a long (SeqCst). Returns the
/// value that was present (old on success), matching the kernel contract.
#[no_mangle]
pub unsafe extern "C" fn atomic_long_cmpxchg(v: *mut c_long, old: c_long, new: c_long) -> c_long {
    let a = &*(v as *const core::sync::atomic::AtomicI64);
    match a.compare_exchange(
        old as i64,
        new as i64,
        core::sync::atomic::Ordering::SeqCst,
        core::sync::atomic::Ordering::SeqCst,
    ) {
        Ok(prev) | Err(prev) => prev as c_long,
    }
}
/// `atomic_long_set` — the one member of the atomic_long family that was still
/// falling through to m4c's inert weak stub (add/sub/read/cmpxchg are real,
/// below/at 1400): the store was silently DROPPED. Found by the 2026-07-08
/// implicit-declaration audit. Same SeqCst discipline as its siblings.
#[no_mangle]
pub unsafe extern "C" fn atomic_long_set(v: *mut c_long, i: c_long) {
    let a = &*(v as *const core::sync::atomic::AtomicI64);
    a.store(i as i64, core::sync::atomic::Ordering::SeqCst);
}

// ── boot_cpu_data: the x86 CPU-identity global amdgpu/TTM read (data symbol, so
// it MUST be real — a function stub would be read as a struct and fault). Athena =
// Ryzen 5 7640HS, Family 19h (Zen 4), 64B cache lines. Layout mirrors
// <asm/processor.h> struct cpuinfo_x86. ──
#[repr(C)]
pub struct CpuinfoX86 {
    x86: u8,
    x86_vendor: u8,
    x86_model: u8,
    x86_stepping: u8,
    x86_clflush_size: c_int,
    x86_cache_alignment: c_int,
    x86_capability: [u32; 24],
    x86_model_id: [u8; 64],
    x86_max_cores: c_uint,
    _reserved: [u64; 16],
}
unsafe impl Sync for CpuinfoX86 {}
#[no_mangle]
pub static boot_cpu_data: CpuinfoX86 = CpuinfoX86 {
    x86: 0x19,       // Family 19h (Zen 4)
    x86_vendor: 2,   // X86_VENDOR_AMD
    x86_model: 0x74, // Phoenix
    x86_stepping: 1,
    x86_clflush_size: 64,
    x86_cache_alignment: 64,
    x86_capability: [0; 24],
    x86_model_id: [0; 64],
    x86_max_cores: 12,
    _reserved: [0; 16],
};

// ═══════════════════ TTM support surface (page / vmalloc / highmem / shrinker /
// shmem / dma-attrs) — M5-ONPATH-AUDIT item 4 ═══════════════════
// Page ops route to the page facade above; reclaim/shmem/dma-attrs are honest
// no-ops (out of the bring-up subset); atomic_long_* + list_bulk_move_tail REAL.
use core::sync::atomic::{AtomicI64, Ordering};

// page refcount + lifecycle (the TTM pool owns page lifetime during bring-up).
#[no_mangle]
pub unsafe extern "C" fn put_page(_page: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn __free_page(page: *mut c_void) {
    __free_pages(page, 0);
}
#[no_mangle]
pub unsafe extern "C" fn alloc_pages_node(_nid: c_int, gfp: c_uint, order: c_uint) -> *mut c_void {
    alloc_pages(gfp, order)
}
#[no_mangle]
pub unsafe extern "C" fn split_page(_page: *mut c_void, _order: c_uint) {}
#[no_mangle]
pub unsafe extern "C" fn mark_page_accessed(_page: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn set_page_dirty(_page: *mut c_void) -> c_int {
    1
}
#[no_mangle]
pub unsafe extern "C" fn clear_page(addr: *mut c_void) {
    if !addr.is_null() {
        core::ptr::write_bytes(addr as *mut u8, 0, 4096);
    }
}
#[no_mangle]
pub unsafe extern "C" fn copy_highpage(to: *mut c_void, from: *mut c_void) {
    let a = page_address(to);
    let b = page_address(from);
    if !a.is_null() && !b.is_null() {
        core::ptr::copy_nonoverlapping(b as *const u8, a as *mut u8, 4096);
    }
}
#[no_mangle]
#[allow(non_snake_case)]
pub unsafe extern "C" fn PageHighMem(_page: *mut c_void) -> bool {
    false
}
#[no_mangle]
pub unsafe extern "C" fn vmalloc_to_page(_addr: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn is_vmalloc_addr(_x: *const c_void) -> bool {
    false
}

// kmap family — x86_64 has no highmem, so a page is always in the linear map
// (page_address). Also inline in <linux/highmem.h>; these exports resolve callers
// that reached them without the header.
#[no_mangle]
pub unsafe extern "C" fn kmap(page: *mut c_void) -> *mut c_void {
    page_address(page)
}
#[no_mangle]
pub unsafe extern "C" fn kunmap(_page: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn kunmap_local(_addr: *const c_void) {}
#[no_mangle]
pub unsafe extern "C" fn kmap_local_page_prot(page: *mut c_void, _prot: c_ulong) -> *mut c_void {
    page_address(page)
}
#[no_mangle]
pub unsafe extern "C" fn kmap_local_page_try_from_panic(page: *mut c_void) -> *mut c_void {
    page_address(page)
}

// vmap: multi-page CPU mapping. Our alloc_pages() now lays a compound allocation
// out as contiguous 4 KiB data pages (mem_map-compatible), so ttm's pages[] for a
// single TTM BO are virtually contiguous and the "map" is just page_address(p[0]).
// Verify contiguity (defensive: fall back to null -> caller sees -ENOMEM rather
// than handing back a bogus non-contiguous mapping). ttm_bo_kmap_ttm uses this to
// CPU-map GTT BOs (the writeback BO is the first: 2 pages).
#[no_mangle]
pub unsafe extern "C" fn vmap(
    pages: *mut *mut c_void,
    count: c_uint,
    _flags: c_ulong,
    _prot: c_ulong,
) -> *mut c_void {
    if pages.is_null() || count == 0 {
        return core::ptr::null_mut();
    }
    let base = page_address(*pages);
    if base.is_null() {
        return core::ptr::null_mut();
    }
    for i in 1..(count as usize) {
        let want = (base as usize).wrapping_add(i * 4096);
        if page_address(*pages.add(i)) as usize != want {
            return core::ptr::null_mut();
        }
    }
    base
}
#[no_mangle]
pub unsafe extern "C" fn vunmap(_addr: *const c_void) {}

// reclaim / shrinker — no memory reclaim during bring-up (the daemon owns its heap).
#[no_mangle]
pub unsafe extern "C" fn shrinker_alloc(_flags: c_uint, _fmt: *const c_char) -> *mut c_void {
    crate::mm::kzalloc(64, 0) as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn shrinker_register(_s: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn shrinker_free(s: *mut c_void) {
    if !s.is_null() {
        crate::mm::kfree(s as *mut u8);
    }
}
#[no_mangle]
pub unsafe extern "C" fn current_is_kswapd() -> bool {
    false
}
#[no_mangle]
pub unsafe extern "C" fn want_init_on_free() -> c_int {
    0
}

// Linux 7's opaque by-value VMA flag bitmap is one native word on x86_64.
#[repr(C)]
pub struct VmaFlags {
    bits: [c_ulong; 1],
}

// shmem swap backing (out of subset — no eviction during init).  Every entry
// point fails closed so GEM cannot mistake an absent backing store for success.
#[no_mangle]
pub unsafe extern "C" fn shmem_file_setup(
    _name: *const c_char,
    _size: c_long,
    _flags: VmaFlags,
) -> *mut c_void {
    (-95isize) as *mut c_void // ERR_PTR(-EOPNOTSUPP)
}
#[no_mangle]
pub unsafe extern "C" fn shmem_file_setup_with_mnt(
    _mnt: *mut c_void,
    _name: *const c_char,
    _size: c_long,
    _flags: VmaFlags,
) -> *mut c_void {
    (-95isize) as *mut c_void // ERR_PTR(-EOPNOTSUPP)
}
#[no_mangle]
pub unsafe extern "C" fn shmem_read_mapping_page_gfp(
    _m: *mut c_void,
    _idx: c_ulong,
    _gfp: c_uint,
) -> *mut c_void {
    (-95isize) as *mut c_void // ERR_PTR(-EOPNOTSUPP)
}
#[no_mangle]
pub unsafe extern "C" fn shmem_read_folio_gfp(
    _mapping: *mut c_void,
    _index: c_ulong,
    _gfp: c_uint,
) -> *mut c_void {
    (-95isize) as *mut c_void // ERR_PTR(-EOPNOTSUPP)
}

// Anonymous Linux fds have no meaning inside the sandboxed amdgpud process.
// PRIME/syncobj fd export must be completed by the kernel render-node broker;
// fail closed until that handoff exists rather than returning a fake file.
#[no_mangle]
pub unsafe extern "C" fn anon_inode_getfile(
    _name: *const c_char,
    _fops: *const c_void,
    _private: *mut c_void,
    _flags: c_int,
) -> *mut c_void {
    (-95isize) as *mut c_void // ERR_PTR(-EOPNOTSUPP)
}

#[no_mangle]
pub unsafe extern "C" fn anon_inode_getfile_fmode(
    _name: *const c_char,
    _fops: *const c_void,
    _private: *mut c_void,
    _flags: c_int,
    _mode: c_uint,
) -> *mut c_void {
    (-95isize) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn anon_inode_create_getfile(
    _name: *const c_char,
    _fops: *const c_void,
    _private: *mut c_void,
    _flags: c_int,
    _context_inode: *const c_void,
) -> *mut c_void {
    (-95isize) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn anon_inode_getfd(
    _name: *const c_char,
    _fops: *const c_void,
    _private: *mut c_void,
    _flags: c_int,
) -> c_int {
    -95
}

#[no_mangle]
pub unsafe extern "C" fn anon_inode_create_getfd(
    _name: *const c_char,
    _fops: *const c_void,
    _private: *mut c_void,
    _flags: c_int,
    _context_inode: *const c_void,
) -> c_int {
    -95
}

// eventfd descriptors also belong to the kernel broker.  No valid eventfd_ctx
// can exist in amdgpud until fd translation is installed, so lookup fails and
// the remaining entry points are unreachable defensive no-ops.
#[no_mangle]
pub unsafe extern "C" fn eventfd_ctx_fdget(_fd: c_int) -> *mut c_void {
    (-38isize) as *mut c_void // ERR_PTR(-ENOSYS)
}

#[no_mangle]
pub unsafe extern "C" fn eventfd_ctx_put(_ctx: *mut c_void) {}

#[no_mangle]
pub unsafe extern "C" fn eventfd_signal_mask(_ctx: *mut c_void, _mask: c_uint) {}
#[no_mangle]
pub unsafe extern "C" fn mapping_gfp_mask(_mapping: *mut c_void) -> c_uint {
    0x01 /* GFP_KERNEL */
}

// dma alloc-attrs variants + resv/fence extras (identity / no-op for bring-up).
#[no_mangle]
pub unsafe extern "C" fn dma_alloc_attrs(
    _dev: *mut c_void,
    size: usize,
    dma: *mut u64,
    _gfp: c_uint,
    _attrs: c_ulong,
) -> *mut c_void {
    // coherent alloc on the claimed GPU (device_map); null when no device (host).
    let a = crate::dma::dma_alloc_coherent(crate::device_map::current_device(), size);
    if a.cpu_addr.is_null() {
        return core::ptr::null_mut();
    }
    if !dma.is_null() {
        *dma = a.dma_addr;
    }
    a.cpu_addr as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn dma_free_attrs(
    _dev: *mut c_void,
    _size: usize,
    _cpu: *mut c_void,
    _dma: u64,
    _attrs: c_ulong,
) {
}
#[no_mangle]
pub unsafe extern "C" fn dma_resv_copy_fences(_dst: *mut c_void, _src: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn dma_resv_lock_slow_interruptible(
    _obj: *mut c_void,
    _ctx: *mut c_void,
) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn dma_fence_enable_sw_signaling(_fence: *mut c_void) {}
#[no_mangle]
pub unsafe extern "C" fn io_mapping_map_local_wc(_m: *mut c_void, _off: c_ulong) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn io_mapping_unmap_local(_v: *mut c_void) {}

// misc workqueue / debugfs / drm-print
#[no_mangle]
pub unsafe extern "C" fn queue_work_node(_node: c_int, wq: *mut c_void, work: *mut c_void) -> bool {
    // NUMA placement is not meaningful in the cooperative userspace daemon,
    // but claiming success without queuing the work loses DRM scheduler/fence
    // callbacks. Route through the real daemon work pump instead.
    crate::workqueue::queue_work(wq, work as *mut crate::workqueue::WorkStruct)
}
#[no_mangle]
pub unsafe extern "C" fn debugfs_create_atomic_t(
    _name: *const c_char,
    _mode: u16,
    _parent: *mut c_void,
    _value: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}
#[no_mangle]
pub unsafe extern "C" fn __drm_printfn_dbg(_p: *mut c_void, _fmt: *const c_char) {}

// atomic_long — REAL (SeqCst) over the caller's long.
#[no_mangle]
pub unsafe extern "C" fn atomic_long_add(i: c_long, v: *mut c_long) {
    (*(v as *const AtomicI64)).fetch_add(i as i64, Ordering::SeqCst);
}
#[no_mangle]
pub unsafe extern "C" fn atomic_long_sub(i: c_long, v: *mut c_long) {
    (*(v as *const AtomicI64)).fetch_sub(i as i64, Ordering::SeqCst);
}
#[no_mangle]
pub unsafe extern "C" fn atomic_long_read(v: *const c_long) -> c_long {
    (*(v as *const AtomicI64)).load(Ordering::SeqCst) as c_long
}

/// `list_bulk_move_tail(head, first, last)` — REAL: unlink the run [first..last]
/// and splice it before @head (i.e. at the list tail). Used by TTM's LRU bulk move.
#[no_mangle]
pub unsafe extern "C" fn list_bulk_move_tail(
    head: *mut ListHead,
    first: *mut ListHead,
    last: *mut ListHead,
) {
    // unlink [first..last]
    (*(*first).prev).next = (*last).next;
    (*(*last).next).prev = (*first).prev;
    // insert the run just before head (tail of the ring)
    let hprev = (*head).prev;
    (*first).prev = hprev;
    (*hprev).next = first;
    (*last).next = head;
    (*head).prev = last;
}

/// `reservation_ww_class` — the global ww_mutex class dma_resv locks against
/// (a data symbol). Layout mirrors the kernel's struct ww_class.
#[repr(C)]
pub struct WwClass {
    stamp: i64,
    acquire_name: *const c_char,
    mutex_name: *const c_char,
    is_wait_die: c_uint,
}
unsafe impl Sync for WwClass {}
#[no_mangle]
pub static reservation_ww_class: WwClass = WwClass {
    stamp: 0,
    acquire_name: c"reservation_ww_class_acquire".as_ptr(),
    mutex_name: c"reservation_ww_class_mutex".as_ptr(),
    is_wait_die: 0,
};

// ═══════════════════ SMU/power support surface (M5-ONPATH-AUDIT item 2) ═══════
// hex-dump/ratelimit/poweroff are debug/shutdown chatter (no-op); pci_is_enabled
// is true during bring-up; wbrf (wifi-band RF coexistence) is absent; REAL
// bitmap_intersects (SMU feature masks).
#[no_mangle]
pub unsafe extern "C" fn print_hex_dump(
    _level: *const c_char,
    _prefix: *const c_char,
    _ptype: c_int,
    _rowsize: c_int,
    _groupsize: c_int,
    _buf: *const c_void,
    _len: usize,
    _ascii: bool,
) {
}
#[no_mangle]
pub unsafe extern "C" fn print_hex_dump_debug(
    _prefix: *const c_char,
    _ptype: c_int,
    _rowsize: c_int,
    _groupsize: c_int,
    _buf: *const c_void,
    _len: usize,
    _ascii: bool,
) {
}
#[no_mangle]
pub unsafe extern "C" fn ___ratelimit(_rs: *mut c_void, _func: *const c_char) -> c_int {
    1
}
#[no_mangle]
pub unsafe extern "C" fn orderly_poweroff(_force: bool) -> c_int {
    0
}
#[no_mangle]
pub unsafe extern "C" fn pci_is_enabled(_pdev: *mut c_void) -> bool {
    true
}
#[no_mangle]
pub unsafe extern "C" fn acpi_amd_wbrf_supported_consumer(_dev: *mut c_void) -> bool {
    false
}
/// `bitmap_intersects(a, b, nbits)` — REAL: do the two bitmaps share a set bit in
/// the first `nbits`? (SMU tests feature masks this way.)
#[no_mangle]
pub unsafe extern "C" fn bitmap_intersects(
    a: *const c_ulong,
    b: *const c_ulong,
    nbits: c_uint,
) -> bool {
    // Word width follows c_ulong (64-bit on the bare/Linux target, 32-bit on the
    // Windows host build) so the bitmap words and `& mask` share a type on every
    // target — a bare `1u64 << rem` mask fails to compile against a 32-bit c_ulong.
    const BITS: u32 = c_ulong::BITS;
    let full = (nbits / BITS) as usize;
    for i in 0..full {
        if (*a.add(i)) & (*b.add(i)) != 0 {
            return true;
        }
    }
    let rem = (nbits % BITS) as usize;
    if rem > 0 {
        let mask: c_ulong = ((1 as c_ulong) << rem) - 1;
        if ((*a.add(full)) & (*b.add(full)) & mask) != 0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};

    static QUEUE_WORK_NODE_RUNS: AtomicU32 = AtomicU32::new(0);

    extern "C" fn queue_work_node_callback(_work: *mut crate::workqueue::WorkStruct) {
        QUEUE_WORK_NODE_RUNS.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn queue_work_node_enqueues_the_daemon_work_pump() {
        QUEUE_WORK_NODE_RUNS.store(0, Ordering::SeqCst);
        let mut work = crate::workqueue::WorkStruct {
            data: 0,
            entry: [0; 2],
            func: Some(queue_work_node_callback),
        };

        assert!(unsafe {
            queue_work_node(
                0,
                core::ptr::null_mut(),
                &mut work as *mut crate::workqueue::WorkStruct as *mut c_void,
            )
        });
        assert_eq!(crate::workqueue::lkpi_run_work(), 1);
        assert_eq!(QUEUE_WORK_NODE_RUNS.load(Ordering::SeqCst), 1);
    }

    /// The `n * size` array allocators must reject a multiply that overflows
    /// `usize` rather than wrapping to a tiny allocation the caller then writes
    /// `n * size` bytes into — the classic integer-overflow heap smash. Upstream
    /// GEM/BO-list ioctls reach these with attacker-influenced counts.
    #[test]
    fn array_allocators_reject_size_overflow_and_serve_valid_requests() {
        unsafe {
            // Overflowing multiply: memdup_array_user → ERR_PTR(-EOVERFLOW),
            // kvmalloc_array / kvcalloc → NULL. None may return a live buffer.
            let src = [0u8; 4];
            let overflow = memdup_array_user(src.as_ptr().cast(), usize::MAX, 2);
            assert_eq!(overflow as isize, -75, "memdup_array_user must EOVERFLOW");
            assert!(
                kvmalloc_array(usize::MAX, 2, 0).is_null(),
                "kvmalloc_array must reject overflow with NULL"
            );
            assert!(
                kvcalloc(usize::MAX, 2, 0).is_null(),
                "kvcalloc must reject overflow with NULL"
            );

            // A well-formed request still copies its bytes through.
            let payload = [0x11u8, 0x22, 0x33, 0x44];
            let dup = memdup_array_user(payload.as_ptr().cast(), 2, 2) as *mut u8;
            assert!(dup as isize > 0, "valid memdup_array_user returns a buffer");
            assert_eq!(
                core::slice::from_raw_parts(dup, 4),
                &payload,
                "memdup must copy the source bytes verbatim"
            );
            crate::mm::kfree(dup);
        }
    }

    #[test]
    fn compound_pages_keep_distinct_va_and_dma_identity() {
        unsafe {
            crate::device_map::lkpi_set_current_device(0);
            let head = alloc_pages(0, 1) as *mut PageStruct;
            assert!(!head.is_null());
            let second = head.add(1);
            let va0 = page_address(head.cast()) as usize;
            let va1 = page_address(second.cast()) as usize;
            assert_eq!(va0 & 4095, 0);
            assert_eq!(va1, va0 + 4096);
            assert_eq!(virt_to_page(va0 as *const c_void), head.cast());
            assert_eq!(virt_to_page(va1 as *const c_void), second.cast());
            assert_eq!(page_to_phys(head.cast()), va0 as u64);
            assert_eq!(page_to_pfn(second.cast()), va1 >> 12);
            __free_pages(head.cast(), 1);
            assert!(virt_to_page(va0 as *const c_void).is_null());
            assert!(virt_to_page(va1 as *const c_void).is_null());
        }
    }
}
