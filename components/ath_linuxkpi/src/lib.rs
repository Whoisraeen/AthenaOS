//! AthenaOS LinuxKPI — userspace shim exposing a Linux driver–compatible C ABI.
//!
//! Phase 1: memory (bump heap), jiffies, msleep, printk, spinlock stubs.
//! Phase 2: `ioremap`, PCI config, `request_irq` doorbells via host syscalls.
//! Phase 3: `dma_alloc_coherent` — IOMMU-sandboxed zero-copy DMA.
//! Phase 4: supervisor heartbeat for daemon-restart resilience.
//!
//! A Linux driver (`amdgpu`, `iwlwifi`, e1000e, …) is compiled as a userspace
//! daemon linked against this crate. The exported C-ABI symbols match what the
//! Linux kernel exports, so the driver source needs no modification — it just
//! links against `libath_linuxkpi` instead of `vmlinux`.

#![no_std]
// `device.rs` reads C varargs (`printk(fmt, ...)`) to interpolate %-specifiers.
#![feature(c_variadic)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// Panic handler for the STANDALONE C-linkable staticlib (build with
// `--features clib --crate-type staticlib`). The rlib consumed by the Rust
// daemons (amdgpud/i915d) must NOT define one — they provide their own — so this
// is feature-gated and OFF by default; existing rlib/hosttest builds are
// unaffected. `x86_64-unknown-none` is panic=abort, so no eh_personality.
#[cfg(feature = "clib")]
#[panic_handler]
fn rae_lkpi_panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

pub mod atomic;
pub mod bitmap;
pub mod delay;
pub mod device;
pub mod device_map;
pub mod dma;
pub mod dma_buf;
pub mod dma_fence;
pub mod dma_pool;
pub mod dma_resv;
pub mod dma_stream;
pub mod drm_bringup;
#[cfg(not(feature = "upstream_drm_core"))]
pub mod drm_exec;
pub mod host;
pub mod idr;
pub mod irq;
pub mod kalloc;
pub mod kfifo;
pub mod klist;
pub mod kstrtox;
pub mod kutil;
pub mod list;
pub mod llist;
mod log;
pub mod mm;
pub mod pci;
pub mod pci_ext;
pub mod pm;
pub mod printf;
pub mod rbtree;
pub mod rculist;
pub mod refcount;
pub mod scatterlist;
pub mod seqlock;
// `string` exports libc-shadow C symbols (memcpy/memset/strlen/...). On the
// bare-metal target those ARE libc. In the host test harness they collide with
// the platform CRT's symbols at link time, so the `hosttest` feature gates them
// out — the harness links the rest of the shim (atomics/mmio/alloc/sync) and
// tests it as real code (see tools/linuxkpi_harness). No internal module uses
// `string`, so gating it is safe. The `hostrun` runner keeps them IN: on Linux
// the executable's strong symbols preempt glibc's dynamic ones, and running the
// REAL shim string/printf code is the point of the off-target runner (they are
// wedge suspects).
#[cfg(any(not(feature = "hosttest"), feature = "hostrun"))]
pub mod string;
pub mod sync;
mod time;
pub mod workqueue;
pub mod ww_mutex;
pub mod xarray;

pub use host::{
    SYS_LINUXKPI_JIFFIES, SYS_LINUXKPI_MSLEEP, SYS_LINUXKPI_PRINTK, SYS_LINUXKPI_VERSION,
};

// ── C ABI exports (names match Linux driver expectations) ───────────────────

#[no_mangle]
pub extern "C" fn kmalloc(size: usize, flags: u32) -> *mut u8 {
    mm::kmalloc(size, flags)
}

#[no_mangle]
pub extern "C" fn kzalloc(size: usize, flags: u32) -> *mut u8 {
    mm::kzalloc(size, flags)
}

#[no_mangle]
pub extern "C" fn kfree(ptr: *mut u8) {
    mm::kfree(ptr);
}

#[no_mangle]
pub extern "C" fn get_jiffies_64() -> u64 {
    time::get_jiffies_64()
}

#[no_mangle]
pub extern "C" fn msleep(msecs: u32) {
    time::msleep(msecs);
}

#[no_mangle]
pub extern "C" fn athena_printk(msg: *const u8) -> i32 {
    if msg.is_null() {
        return -1;
    }
    let mut len = 0usize;
    while unsafe { *msg.add(len) } != 0 {
        len += 1;
        if len > 512 {
            break;
        }
    }
    let slice = unsafe { core::slice::from_raw_parts(msg, len) };
    log::athena_printk(slice)
}

/// `spin_lock` / `spin_unlock` — Linux's spinlock macros normally expand to
/// `_raw_spin_lock` (see `sync.rs`), but a driver referencing these named
/// symbols must still get real mutual exclusion. Forward to the SAME atomic
/// acquire/release; the previous body was a non-atomic read-then-write race.
#[no_mangle]
pub extern "C" fn spin_lock(lock: *mut u32) {
    sync::acquire(lock);
}

#[no_mangle]
pub extern "C" fn spin_unlock(lock: *mut u32) {
    sync::release(lock);
}

// ── Phase 2 C ABI: PCI + MMIO (names match Linux kernel exports) ─────────────

/// `pci_enable_device` — packed BDF in, opaque device handle out (0 = fail).
#[no_mangle]
pub extern "C" fn lkpi_pci_enable_device(bus: u8, dev: u8, func: u8) -> u64 {
    pci::pci_enable(bus, dev, func)
}

/// `ioremap` / `pci_iomap` — map BAR `bar` of device `dev`, returns register pointer.
#[no_mangle]
pub extern "C" fn lkpi_ioremap(dev: u64, bar: u8) -> *mut u8 {
    pci::ioremap(dev, bar)
}

#[no_mangle]
pub extern "C" fn lkpi_iounmap(virt: *mut u8, len: usize) {
    pci::iounmap(virt, len);
}

// NB: the C prototype is pci_read_config_dword(struct pci_dev *dev, int where,
// u32 *val) — amdgpu passes its pci_dev POINTER as the first arg, not the AthenaOS
// handle. When a device is registered (device_map), use that handle so config
// reads hit the real GPU; otherwise fall back to the passed value (mock/tests).
#[inline]
fn cfg_handle(dev: u64) -> u64 {
    let cur = device_map::current_device();
    if cur != 0 {
        cur
    } else {
        dev
    }
}

#[no_mangle]
pub extern "C" fn pci_read_config_dword(dev: u64, offset: u16, out: *mut u32) -> i32 {
    if out.is_null() {
        return -1;
    }
    unsafe { *out = pci::read_config_dword(cfg_handle(dev), offset) };
    0
}

#[no_mangle]
pub extern "C" fn pci_write_config_dword(dev: u64, offset: u16, value: u32) -> i32 {
    pci::write_config_dword(cfg_handle(dev), offset, value);
    0
}

// MMIO register accessors — Linux `readl`/`writel`/`readw`/`writew`/`readb`/`writeb`.
#[no_mangle]
pub extern "C" fn readl(addr: *const u32) -> u32 {
    // hostrun real-silicon read-through: register READS inside a fake BAR that
    // has a live sysfs `resourceN` mapping return the REAL hardware value
    // (umr-class read-only diagnostics; see host.rs `hostrun_read_intercept`).
    #[cfg(feature = "hostrun")]
    if let Some(v) = host::hostrun_read_intercept(addr as u64, 4) {
        return v as u32;
    }
    let val = pci::readl(addr);
    // Bring-up workaround: RCC_IOV_FUNC_IDENTIFIER (BAR5 dword 0x00c5 = byte 0x314)
    // misreads bit0=1 under VFIO passthrough of the PHYSICAL function, so amdgpu's
    // amdgpu_virt_init_detect_asic falsely flags IS_VF (SR-IOV virtual function) and
    // jumps into stubbed VF mailbox ops (crash). This IS a PF: log the raw value
    // once, then clear IS_VF (bit0) + ENABLE_IOV (bit31) so it presents as bare metal.
    if device_map::bar5_offset(addr as u64) == Some(0x314) {
        rcc_iov_log_once(val);
        return val & !0x8000_0001;
    }
    val
}

/// One-shot log of the raw RCC_IOV_FUNC_IDENTIFIER value (no-alloc hex).
fn rcc_iov_log_once(raw: u32) {
    use core::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if LOGGED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut buf = [0u8; 64];
    let mut n = 0;
    for &c in b"[linuxkpi] RCC_IOV_FUNC_IDENTIFIER raw=0x" {
        buf[n] = c;
        n += 1;
    }
    for i in (0..8).rev() {
        let nib = ((raw >> (i * 4)) & 0xf) as u8;
        buf[n] = if nib < 10 {
            b'0' + nib
        } else {
            b'a' + nib - 10
        };
        n += 1;
    }
    for &c in b" -> forcing PF\n" {
        buf[n] = c;
        n += 1;
    }
    crate::log::athena_printk(&buf[..n]);
}
#[no_mangle]
pub extern "C" fn writel(value: u32, addr: *mut u32) {
    pci::writel(value, addr)
}
#[no_mangle]
pub extern "C" fn readw(addr: *const u16) -> u16 {
    pci::readw(addr)
}
#[no_mangle]
pub extern "C" fn writew(value: u16, addr: *mut u16) {
    pci::writew(value, addr)
}
#[no_mangle]
pub extern "C" fn readb(addr: *const u8) -> u8 {
    pci::readb(addr)
}
#[no_mangle]
pub extern "C" fn writeb(value: u8, addr: *mut u8) {
    pci::writeb(value, addr)
}

// ── Phase 3 C ABI: zero-copy DMA ─────────────────────────────────────────────

/// `dma_alloc_coherent` — Linux signature returns CPU ptr, writes DMA addr to `dma_handle`.
/// `dev` is the device handle from `lkpi_pci_enable_device`.
#[no_mangle]
pub extern "C" fn dma_alloc_coherent(
    _dev: u64,
    size: usize,
    dma_handle: *mut u64,
    _gfp_flags: u32,
) -> *mut u8 {
    // amdgpu passes its `struct device *` here, not the AthenaOS device handle —
    // use the claimed GPU's handle (device_map) so the DMA lands on the real GPU.
    let a = dma::dma_alloc_coherent(device_map::current_device(), size);
    if a.is_null() {
        return core::ptr::null_mut();
    }
    if !dma_handle.is_null() {
        unsafe { *dma_handle = a.dma_addr };
    }
    // Stash the token in the word just below the buffer? No — Linux's free passes
    // cpu_addr+dma_addr+size, so we re-derive. For now the daemon keeps DmaAlloc.
    a.cpu_addr
}

/// `dma_free_coherent` — token-based free. The daemon passes the token it kept
/// from the `dma::dma_alloc_coherent` result (richer than the raw C signature).
#[no_mangle]
pub extern "C" fn lkpi_dma_free(dev: u64, token: u64) {
    let a = dma::DmaAlloc {
        cpu_addr: core::ptr::null_mut(),
        dma_addr: 0,
        size: 0,
        token,
    };
    dma::dma_free_coherent(dev, &a);
}

/// `dma_free_coherent(dev, size, cpu_addr, dma_handle)` — the standard C signature
/// the real driver calls (no daemon-held token). The coherent DMA arena is
/// reclaimed wholesale at daemon teardown (same model as `devm_*`), so this is a
/// deliberate per-call no-op, not a leak in the daemon lifetime. (Token-based
/// `lkpi_dma_free` is used when the daemon owns the allocation directly.)
#[no_mangle]
pub extern "C" fn dma_free_coherent(_dev: u64, _size: usize, _cpu_addr: *mut u8, _dma_handle: u64) {
}

/// `jiffies` — the kernel monotonic tick counter, read directly by drivers for
/// `time_after`/timeout math. Backed by an `AtomicU64` (same memory layout as the
/// `unsigned long` the C side reads). The daemon advances it via
/// [`lkpi_tick_jiffies`] in its poll loop (M5 wiring); 1 jiffy == 1 ms (host clock).
#[no_mangle]
pub static jiffies: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Advance `jiffies` to the host's current tick. Call from the daemon's main /
/// poll loop so driver timeout math (`time_after(jiffies, …)`) makes progress.
#[no_mangle]
pub extern "C" fn lkpi_tick_jiffies() {
    let now = unsafe { host::sys_linuxkpi_jiffies() };
    jiffies.store(now, core::sync::atomic::Ordering::Relaxed);
}

// ── Phase 2 C ABI: IRQ ───────────────────────────────────────────────────────

// `request_irq` (Linux signature) + `free_irq`/`enable_irq`/`disable_irq` live
// in `irq.rs` as C exports. The daemon's own doorbell routing uses
// `irq::request_irq_doorbell`.

#[no_mangle]
pub extern "C" fn lkpi_irq_wait(handle: u64) -> u8 {
    irq::irq_wait(handle)
}

// ── Phase 1 C ABI: mutex (maps to host futex/park; spin fallback) ────────────

/// `mutex_lock` / `mutex_unlock` — atomic acquire/release (CAS), yielding the
/// CPU on contention. The previous body was a non-atomic `read_volatile`-then-
/// `write_volatile`: two callers could both observe 0 and both "acquire" (a real
/// race). Now routed through the shared atomic primitive in `sync`.
#[no_mangle]
pub extern "C" fn mutex_lock(lock: *mut u32) {
    sync::acquire(lock);
}

#[no_mangle]
pub extern "C" fn mutex_unlock(lock: *mut u32) {
    sync::release(lock);
}

// ── Phase 4 C ABI: supervisor heartbeat ──────────────────────────────────────

/// Supervisor opcodes (mirror kernel `linuxkpi_host::SUP_*`).
const SUP_REGISTER: u64 = 1;
const SUP_HEARTBEAT: u64 = 2;
const SUP_DEVICE_BDF: u64 = 5;

/// Register this daemon with the supervisor so a crash triggers a clean restart.
#[no_mangle]
pub extern "C" fn lkpi_supervisor_register(dev: u64) -> u64 {
    unsafe { host::sys_supervisor(SUP_REGISTER, dev) }
}

/// Where did the claimed device land? Returns `(bus, dev, func)` for a device
/// handle. A daemon that claimed by class match (Linux-style id binding) never
/// learned the BDF, but the real amdgpu init needs `pdev->bus->number`: the
/// ACPI VFCT VBIOS image is matched against the GPU's own bus/slot/function
/// (`amdgpu_acpi_vfct_bios` — its NULL `pdev->bus` deref right after IP
/// discovery was found off-target 2026-07-08 by tools/amdgpu_hostrun).
pub fn device_bdf(dev: u64) -> Option<(u8, u8, u8)> {
    let packed = unsafe { host::sys_supervisor(SUP_DEVICE_BDF, dev) };
    if packed >= 0xFFFF_FFFF_FFFF_F000 {
        return None;
    }
    Some((
        ((packed >> 16) & 0xFF) as u8,
        ((packed >> 8) & 0xFF) as u8,
        (packed & 0xFF) as u8,
    ))
}

/// The PCI (bus,dev,func) recorded in the bundled VFCT VBIOS image header — the
/// GPU's *native* location on the machine the table was captured from (Athena:
/// `c4:00.0`). `amdgpu_acpi_vfct_bios()` only accepts the image whose location
/// equals `pdev`'s BDF, so bring-up sets `pdev` to THIS (not the guest's
/// passthrough BDF): matching `pdev` to the ground-truth table is immune to
/// whether amdgpu reads our served copy or the original firmware mapping. The
/// VBIOS is byte-identical for this exact silicon, so the location triple is
/// only a provenance tag. Reads `VBIOSImageOffset` (u32 @ table 0x34) then the
/// image header's PCIBus/PCIDevice/PCIFunction (u32 @ +0/+4/+8, low byte).
pub fn vfct_native_bdf() -> Option<(u8, u8, u8)> {
    let (data, size) = request_firmware_blob("acpi/athena-beelink-elitemini/VFCT.dat")?;
    if data.is_null() || size < 0x34 + 4 {
        return None;
    }
    let img_off = unsafe {
        u32::from_le_bytes([
            *data.add(0x34),
            *data.add(0x35),
            *data.add(0x36),
            *data.add(0x37),
        ])
    } as usize;
    if img_off < 0x38 || img_off.saturating_add(9) > size {
        return None;
    }
    let (bus, dev, func) = unsafe {
        (
            *data.add(img_off),
            *data.add(img_off + 4),
            *data.add(img_off + 8),
        )
    };
    Some((bus, dev, func))
}

/// Periodic liveness heartbeat — the supervisor uses absence of heartbeats to
/// detect a hung (not crashed) daemon.
#[no_mangle]
pub extern "C" fn lkpi_supervisor_heartbeat(dev: u64) {
    unsafe { host::sys_supervisor(SUP_HEARTBEAT, dev) };
}

// ── Firmware loading (request_firmware) ──────────────────────────────────────

/// Mirrors Linux `struct firmware` (the leading two fields drivers actually
/// read). Backed by a kernel-mapped, read-only blob.
#[repr(C)]
pub struct Firmware {
    pub size: usize,
    pub data: *const u8,
}

/// Rust-friendly loader: returns `(data_ptr, size)` for firmware `name`, or
/// `None` if absent. The blob is mapped read-only into this daemon by the
/// kernel (syscall 142) and stays live for the daemon's lifetime.
pub fn request_firmware_blob(name: &str) -> Option<(*const u8, usize)> {
    let mut out = [0u64; 2]; // [user_virt, size]
    let rc = unsafe {
        host::sys_request_firmware(
            name.as_ptr() as u64,
            name.len() as u64,
            out.as_mut_ptr() as u64,
        )
    };
    if rc != 0 || out[0] == 0 || out[1] == 0 {
        return None;
    }
    Some((out[0] as *const u8, out[1] as usize))
}

/// Linux C ABI `request_firmware(const struct firmware **fw, const char *name,
/// struct device *dev)`. Allocates a `Firmware` (via kmalloc), fills it, stores
/// it in `*fw_out`. Returns 0 on success, negative errno otherwise.
#[no_mangle]
pub extern "C" fn request_firmware(fw_out: *mut *mut Firmware, name: *const u8, _dev: u64) -> i32 {
    if fw_out.is_null() || name.is_null() {
        return -22; // -EINVAL
    }
    let mut len = 0usize;
    while unsafe { *name.add(len) } != 0 {
        len += 1;
        if len > 256 {
            break;
        }
    }
    let s = unsafe { core::slice::from_raw_parts(name, len) };
    let name_str = match core::str::from_utf8(s) {
        Ok(x) => x,
        Err(_) => return -22,
    };
    match request_firmware_blob(name_str) {
        Some((data, size)) => {
            let fw = mm::kmalloc(core::mem::size_of::<Firmware>(), 0) as *mut Firmware;
            if fw.is_null() {
                return -12; // -ENOMEM
            }
            unsafe {
                (*fw).size = size;
                (*fw).data = data;
                *fw_out = fw;
            }
            0
        }
        None => -2, // -ENOENT
    }
}

/// Linux C ABI `release_firmware`. Frees the descriptor; the underlying mapping
/// is reclaimed on daemon teardown.
#[no_mangle]
pub extern "C" fn release_firmware(fw: *mut Firmware) {
    if !fw.is_null() {
        mm::kfree(fw as *mut u8);
    }
}

/// Verify `request_firmware` end-to-end via syscall 142. Returns 1 if the named
/// blob loaded and its first byte is readable through the kernel mapping, else 0.
pub fn self_test_firmware(name: &str) -> u32 {
    match request_firmware_blob(name) {
        Some((data, size)) if size > 0 && !data.is_null() => {
            // Touch the mapping to prove it's live (not just a returned pointer).
            let _first = unsafe { core::ptr::read_volatile(data) };
            1
        }
        _ => 0,
    }
}

/// Phase 2: PCI claim (usdriver caps), ioremap into user VA, IRQ cap mint.
pub fn self_test_phase2() -> u32 {
    self_test_phase2_with_handle().0
}

/// Same as [`self_test_phase2`] but returns the LinuxKPI device handle for Phase 3 DMA.
pub fn self_test_phase2_with_handle() -> (u32, u64) {
    // Match-first: claim any network-class device (class 0x02) — present on
    // both QEMU (virtio-net/e1000) and Athena (RTL8125), so the PCI claim /
    // ioremap / IRQ / DMA paths get exercised on iron too, not just QEMU.
    // The fixed-BDF list remains as a fallback for configs with no NIC.
    const MATCH_NETWORK_CLASS: u8 = 0x02;
    const CANDIDATES: &[(u8, u8, u8)] = &[(0, 4, 0), (0, 3, 0), (0, 5, 0), (0, 6, 0)];
    let mut pass = 0u32;
    let mut handle = pci::pci_enable_match(MATCH_NETWORK_CLASS, 0);
    if handle >= 0xFFFF_FFFF_FFFF_F000 {
        for &(bus, dev, func) in CANDIDATES {
            handle = pci::pci_enable(bus, dev, func);
            if handle < 0xFFFF_FFFF_FFFF_F000 {
                break;
            }
        }
    }
    if handle < 0xFFFF_FFFF_FFFF_F000 {
        let vid = pci::read_config_dword(handle, 0);
        if vid != 0 && vid != 0xFFFF_FFFF {
            pass += 1;
        }
        let bar = pci::ioremap(handle, 0);
        if !bar.is_null() {
            pass += 1;
            let _ = pci::readl(bar as *const u32);
            pci::iounmap(bar, 4096);
        }
        let irq = irq::request_irq_doorbell(handle, 0);
        if irq != 0 && irq < 0xFFFF_FFFF_FFFF_F000 {
            pass += 1;
            // Inject doorbell then irq_wait (pending-IRQ path + supervisor smoketest).
            const SUP_TRIGGER_DEV_IRQ: u64 = 4;
            unsafe {
                let _ = host::sys_supervisor(SUP_TRIGGER_DEV_IRQ, handle);
            }
            let fired = irq::irq_wait(irq);
            if fired != 0 && fired < 0xFF {
                pass += 1;
            }
        }
        return (pass, handle);
    }
    (pass, 0)
}

/// Claim a device by class/vendor match (Linux id-table binding) — `None` if
/// no matching device or the claim was denied. GPU daemons use this instead of
/// fixed BDF lists.
pub fn pci_enable_match(class: u8, vendor: u16) -> Option<u64> {
    let h = pci::pci_enable_match(class, vendor);
    if h >= 0xFFFF_FFFF_FFFF_F000 {
        None
    } else {
        Some(h)
    }
}

/// Phase 3: IOMMU-sandboxed `dma_alloc_coherent` + free on an already-claimed device.
pub fn self_test_phase3_on(dev_handle: u64) -> u32 {
    if dev_handle == 0 || dev_handle >= 0xFFFF_FFFF_FFFF_F000 {
        return 0;
    }
    let mut pass = 0u32;
    let dma = dma::dma_alloc_coherent(dev_handle, 4096);
    if !dma.is_null() && dma.dma_addr != 0 && dma.size >= 4096 {
        pass += 1;
        unsafe {
            core::ptr::write_volatile(dma.cpu_addr, 0xDA);
        }
    }
    dma::dma_free_coherent(dev_handle, &dma);
    if !dma.is_null() {
        pass += 1;
    }
    pass
}

pub fn self_test_phase3() -> u32 {
    self_test_phase3_on(0)
}

/// Phase 4: supervisor register + heartbeat on a claimed device handle.
pub fn self_test_phase4_on(dev_handle: u64) -> u32 {
    if dev_handle == 0 || dev_handle >= 0xFFFF_FFFF_FFFF_F000 {
        return 0;
    }
    let mut pass = 0u32;
    let sup = unsafe { host::sys_supervisor(SUP_REGISTER, dev_handle) };
    if sup != 0 && sup < 0xFFFF_FFFF_FFFF_F000 {
        pass += 1;
    }
    unsafe {
        host::sys_supervisor(SUP_HEARTBEAT, dev_handle);
    }
    pass += 1;
    pass
}

const INTEL_VENDOR: u16 = 0x8086;

/// Intel display-class GPU probe (Path C i915d preview). Returns 1 if an Intel
/// display controller was claimed + BAR0 mapped, 0 if none (expected on QEMU).
pub fn self_test_intel_gpu() -> u32 {
    // Class 0x03 (display) + Intel vendor, resolved host-side — no BDF guessing.
    let handle = pci::pci_enable_match(0x03, INTEL_VENDOR);
    if handle >= 0xFFFF_FFFF_FFFF_F000 {
        return 0;
    }
    let id = pci::read_config_dword(handle, 0x00);
    if (id & 0xFFFF) as u16 != INTEL_VENDOR {
        return 0;
    }
    let bar = pci::ioremap(handle, 0);
    if bar.is_null() {
        return 0;
    }
    let _ = pci::readl(bar as *const u32);
    pci::iounmap(bar, 4096);
    1
}

/// Called from `hello_linuxkpi` to verify host ABI before driver ports land.
pub fn self_test() -> u32 {
    let mut pass = 0u32;
    let ver = unsafe { host::sys_linuxkpi_version() };
    if ver == 0x524B5049_0001 {
        pass += 1;
    }
    let j0 = time::get_jiffies_64();
    time::msleep(2);
    let j1 = time::get_jiffies_64();
    if j1 >= j0 {
        pass += 1;
    }
    let p = mm::kmalloc(32, 0);
    if !p.is_null() {
        unsafe {
            core::ptr::write_bytes(p, 0x5A, 32);
        }
        mm::kfree(p);
        pass += 1;
    }
    pass
}
