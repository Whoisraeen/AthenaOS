//! Raw syscall stubs into the AthenaOS LinuxKPI host (kernel `linuxkpi_host.rs`).
//!
//! **hosttest build (the `linuxkpi_harness`).** A raw `syscall` instruction with
//! a AthenaOS syscall number is harmless-but-meaningless on Linux (`-ENOSYS`) and
//! a hard fault on Windows — so on a non-Linux dev box the harness segfaults the
//! moment any facade reaches one (e.g. `ktime_get_ns` → `sys_linuxkpi_jiffies`).
//! Under `feature = "hosttest"` every site below is replaced by a benign stub:
//! device ops report "absent", and a monotonic millisecond clock backs the time
//! facades so the jiffies/ktime-derived tests still make progress. The bare-metal
//! build (`not(hosttest)`) keeps the real `syscall` instructions unchanged. No
//! real device I/O is exercised on the host — that is QEMU/iron's job.
//!
//! **hostrun build (tools/amdgpu_hostrun).** A third mode for *executing* the
//! real amdgpu C init graph off-target: the seam becomes a FUNCTIONAL Linux-host
//! implementation (mmap-backed `ioremap`/`map_phys`, file-backed
//! `request_firmware`, stderr `printk`, real monotonic clock) — see the
//! [`hostrun`] module. Inert (hosttest-equivalent) until the runner calls
//! [`hostrun_install`], so cargo feature unification with `hosttest` consumers
//! cannot change their behavior. `hostrun` takes precedence over `hosttest`.

use ath_abi::syscall as abi;

pub use abi::{
    SYS_LINUXKPI_DMA_ALLOC, SYS_LINUXKPI_DMA_FREE, SYS_LINUXKPI_IOREMAP, SYS_LINUXKPI_IOUNMAP,
    SYS_LINUXKPI_IRQ_WAIT, SYS_LINUXKPI_JIFFIES, SYS_LINUXKPI_MAP_PHYS, SYS_LINUXKPI_MSLEEP,
    SYS_LINUXKPI_PCI_ENABLE, SYS_LINUXKPI_PCI_READ_CFG, SYS_LINUXKPI_PCI_WRITE_CFG,
    SYS_LINUXKPI_PRINTK, SYS_LINUXKPI_REQUEST_FIRMWARE, SYS_LINUXKPI_REQUEST_IRQ,
    SYS_LINUXKPI_SUPERVISOR, SYS_LINUXKPI_VERSION, SYS_RAEGFX_REGISTER_SCANOUT,
};

const _: () = assert!(ath_abi::ABI_VERSION == 4);

/// hosttest only: a monotonic "jiffies" clock (1 kHz model — 1 jiffy = 1 ms) so
/// the time facades advance without a real syscall. `sys_linuxkpi_jiffies` reads
/// + ticks it; `sys_linuxkpi_msleep` advances it by the slept duration. Never
/// compiled into the bare-metal build.
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
static HOST_JIFFIES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Generic 1-argument syscall helper.
#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
unsafe fn syscall1(nr: u64, a0: u64) -> u64 {
    let v: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") nr => v,
        in("rdi") a0,
        out("rcx") _, out("r11") _,
    );
    v
}
/// hosttest: `pci_enable` reports "no device" (handle in the failure band);
/// `irq_wait` returns "no IRQ". Both keep callers on their absent/fallback path.
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
#[inline(always)]
unsafe fn syscall1(nr: u64, _a0: u64) -> u64 {
    if nr == SYS_LINUXKPI_PCI_ENABLE {
        u64::MAX // >= 0xFFFF_FFFF_FFFF_F000 → "no device"
    } else {
        0
    }
}

/// Generic 2-argument syscall helper.
#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
unsafe fn syscall2(nr: u64, a0: u64, a1: u64) -> u64 {
    let v: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") nr => v,
        in("rdi") a0,
        in("rsi") a1,
        out("rcx") _, out("r11") _,
    );
    v
}
/// hosttest: 0 for every 2-arg op. Crucially `ioremap` → 0 (NULL), so a caller
/// can never deref a bogus mapping; `read_cfg`/`supervisor`/`iounmap`/`dma_free`/
/// `request_irq` all read cleanly as "nothing there".
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
#[inline(always)]
unsafe fn syscall2(_nr: u64, _a0: u64, _a1: u64) -> u64 {
    0
}

/// Generic 3-argument syscall helper.
#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
unsafe fn syscall3(nr: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let v: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") nr => v,
        in("rdi") a0,
        in("rsi") a1,
        in("rdx") a2,
        out("rcx") _, out("r11") _,
    );
    v
}
/// hosttest: report failure (non-zero) for `dma_alloc`/`request_firmware`, so
/// `dma_alloc_coherent` returns NULL and `request_firmware_blob` returns `None`
/// rather than a fabricated success with an unwritten out-buffer.
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
#[inline(always)]
unsafe fn syscall3(_nr: u64, _a0: u64, _a1: u64, _a2: u64) -> u64 {
    u64::MAX // failure sentinel
}

/// Generic 4-argument syscall helper (4th arg in `r10`, the AthenaOS native ABI's
/// fourth register — `syscall` clobbers only `rcx`/`r11`, so `r10` survives).
#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
unsafe fn syscall4(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let v: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") nr => v,
        in("rdi") a0,
        in("rsi") a1,
        in("rdx") a2,
        in("r10") a3,
        out("rcx") _, out("r11") _,
    );
    v
}
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
#[inline(always)]
unsafe fn syscall4(_nr: u64, _a0: u64, _a1: u64, _a2: u64, _a3: u64) -> u64 {
    0 // host: no-op success (no real scanout to register)
}

// ── hostrun: route every helper through the functional dispatcher ───────────
#[cfg(feature = "hostrun")]
unsafe fn syscall1(nr: u64, a0: u64) -> u64 {
    hostrun::dispatch(nr, a0, 0, 0, 0)
}
#[cfg(feature = "hostrun")]
unsafe fn syscall2(nr: u64, a0: u64, a1: u64) -> u64 {
    hostrun::dispatch(nr, a0, a1, 0, 0)
}
#[cfg(feature = "hostrun")]
unsafe fn syscall3(nr: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    hostrun::dispatch(nr, a0, a1, a2, 0)
}
#[cfg(feature = "hostrun")]
unsafe fn syscall4(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    hostrun::dispatch(nr, a0, a1, a2, a3)
}

/// `pci_enable_device`: claim a PCI device by packed BDF, returns device handle.
#[inline(always)]
pub unsafe fn sys_pci_enable(packed_bdf: u64) -> u64 {
    syscall1(SYS_LINUXKPI_PCI_ENABLE, packed_bdf)
}

/// `ioremap`/`pci_iomap`: map BAR `bar_index` of device `dev_handle`, returns virt ptr.
#[inline(always)]
pub unsafe fn sys_ioremap(dev_handle: u64, bar_index: u64) -> u64 {
    syscall2(SYS_LINUXKPI_IOREMAP, dev_handle, bar_index)
}

#[inline(always)]
pub unsafe fn sys_iounmap(virt: u64, len: u64) -> u64 {
    syscall2(SYS_LINUXKPI_IOUNMAP, virt, len)
}

/// `SYS_LINUXKPI_MAP_PHYS`: map a NON-BAR reserved/carveout physical range (APU
/// UMA VRAM) into the daemon. Returns the user virt, or an error sentinel (high
/// bit set — the kernel returns the `E_*` codes, all >= 0x8000…). The `ioremap_wc`
/// path uses this when a VRAM BO's physical address is outside every PCI BAR.
#[inline(always)]
pub unsafe fn sys_map_phys(dev_handle: u64, phys: u64, size: u64) -> u64 {
    syscall3(SYS_LINUXKPI_MAP_PHYS, dev_handle, phys, size)
}

#[inline(always)]
pub unsafe fn sys_pci_read_cfg(dev_handle: u64, offset: u64) -> u64 {
    syscall2(SYS_LINUXKPI_PCI_READ_CFG, dev_handle, offset)
}

#[inline(always)]
pub unsafe fn sys_pci_write_cfg(dev_handle: u64, offset: u64, value: u64) -> u64 {
    syscall3(SYS_LINUXKPI_PCI_WRITE_CFG, dev_handle, offset, value)
}

/// `dma_alloc_coherent`: allocate IOMMU-sandboxed contiguous DMA. `out_ptr`
/// receives `[virt: u64, phys: u64, size: u64, token: u64]`. Returns 0 on success.
#[inline(always)]
pub unsafe fn sys_dma_alloc(dev_handle: u64, size: u64, out_ptr: u64) -> u64 {
    syscall3(SYS_LINUXKPI_DMA_ALLOC, dev_handle, size, out_ptr)
}

#[inline(always)]
pub unsafe fn sys_dma_free(dev_handle: u64, token: u64) -> u64 {
    syscall2(SYS_LINUXKPI_DMA_FREE, dev_handle, token)
}

/// `request_irq`: route device MSI-X `vector` to a doorbell, returns irq handle.
#[inline(always)]
pub unsafe fn sys_request_irq(dev_handle: u64, vector: u64) -> u64 {
    syscall2(SYS_LINUXKPI_REQUEST_IRQ, dev_handle, vector)
}

/// Block until the next IRQ doorbell fires; returns the vector.
#[inline(always)]
pub unsafe fn sys_irq_wait(irq_handle: u64) -> u64 {
    syscall1(SYS_LINUXKPI_IRQ_WAIT, irq_handle)
}

/// Supervisor: op (1=register, 2=heartbeat, 3=restart_count), arg=device handle.
#[inline(always)]
pub unsafe fn sys_supervisor(op: u64, arg: u64) -> u64 {
    syscall2(SYS_LINUXKPI_SUPERVISOR, op, arg)
}

/// Register this driver's display scanout framebuffer with the in-kernel
/// compositor (the amdgpu DCN path): the compositor then blits each frame into
/// `phys` and the display engine scans the same pages out. `dev_handle` from
/// `pci_enable`; `phys` MUST be a DMA region this device already owns (the kernel
/// rejects anything else). Returns 1 if the compositor attached it, else 0.
#[inline(always)]
pub unsafe fn sys_register_scanout(
    dev_handle: u64,
    phys: u64,
    width: u32,
    height: u32,
    stride: u32,
) -> u64 {
    let packed_dims = ((width as u64) << 32) | (height as u64);
    syscall4(
        SYS_RAEGFX_REGISTER_SCANOUT,
        dev_handle,
        phys,
        packed_dims,
        stride as u64,
    )
}

/// `request_firmware`: load firmware blob `name` (`name_ptr`/`name_len`) from the
/// initramfs `firmware/` tree. `out_ptr` receives `[user_virt: u64, size: u64]`
/// (16 bytes); the kernel maps the blob read-only into this daemon. Returns 0 on
/// success, or an `E_*` sentinel (>= 0xFFFF_FFFF_FFFF_FC00).
#[inline(always)]
pub unsafe fn sys_request_firmware(name_ptr: u64, name_len: u64, out_ptr: u64) -> u64 {
    syscall3(SYS_LINUXKPI_REQUEST_FIRMWARE, name_ptr, name_len, out_ptr)
}

#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_linuxkpi_version() -> u64 {
    let v: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_LINUXKPI_VERSION => v,
        out("rcx") _, out("r11") _,
    );
    v
}
/// hosttest: return the real ABI version constant so `self_test` still validates.
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_linuxkpi_version() -> u64 {
    0x524B_5049_0001
}
#[cfg(feature = "hostrun")]
#[inline(always)]
pub unsafe fn sys_linuxkpi_version() -> u64 {
    hostrun::dispatch(SYS_LINUXKPI_VERSION, 0, 0, 0, 0)
}

#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_linuxkpi_jiffies() -> u64 {
    let v: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_LINUXKPI_JIFFIES => v,
        out("rcx") _, out("r11") _,
    );
    v
}
/// hosttest: read + tick the monotonic host clock (strictly increasing across
/// calls, so `j1 >= j0` and ktime deltas hold).
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_linuxkpi_jiffies() -> u64 {
    HOST_JIFFIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}
#[cfg(feature = "hostrun")]
#[inline(always)]
pub unsafe fn sys_linuxkpi_jiffies() -> u64 {
    hostrun::dispatch(SYS_LINUXKPI_JIFFIES, 0, 0, 0, 0)
}

#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_linuxkpi_msleep(ms: u64) {
    core::arch::asm!(
        "syscall",
        in("rax") SYS_LINUXKPI_MSLEEP,
        in("rdi") ms,
        out("rcx") _, out("r11") _,
    );
}
/// hosttest: don't really sleep — advance the host clock by `ms` so a
/// sleep-then-read-jiffies sequence shows the expected elapsed time.
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_linuxkpi_msleep(ms: u64) {
    HOST_JIFFIES.fetch_add(ms, core::sync::atomic::Ordering::Relaxed);
}
#[cfg(feature = "hostrun")]
#[inline(always)]
pub unsafe fn sys_linuxkpi_msleep(ms: u64) {
    hostrun::dispatch(SYS_LINUXKPI_MSLEEP, ms, 0, 0, 0);
}

#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_linuxkpi_printk(buf: *const u8, len: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_LINUXKPI_PRINTK => r,
        in("rdi") buf,
        in("rsi") len,
        out("rcx") _, out("r11") _,
    );
    r
}
/// hosttest: report the bytes "accepted" (the harness uses its own `println!`
/// for output; this path is only reached if a driver calls printk directly).
#[cfg(all(feature = "hosttest", not(feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_linuxkpi_printk(_buf: *const u8, len: u64) -> u64 {
    len
}
#[cfg(feature = "hostrun")]
#[inline(always)]
pub unsafe fn sys_linuxkpi_printk(buf: *const u8, len: u64) -> u64 {
    hostrun::dispatch(SYS_LINUXKPI_PRINTK, buf as u64, len, 0, 0)
}

/// SYS_NETLOG_FLUSH (296): broadcast the kernel bootlog ring over UDP now. No
/// args. Drives the printk facade's diagnostic fence (see `device::set_netlog_fence`)
/// — pushes each real-amdgpu-init log line onto the wire so the netlog trail ends
/// at (within the throttle of) the exact line before a CPU-0 hard hang.
#[cfg(not(any(feature = "hosttest", feature = "hostrun")))]
#[inline(always)]
pub unsafe fn sys_netlog_flush() -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") abi::SYS_NETLOG_FLUSH => r,
        out("rcx") _, out("r11") _,
    );
    r
}
#[cfg(any(feature = "hosttest", feature = "hostrun"))]
#[inline(always)]
pub unsafe fn sys_netlog_flush() -> u64 {
    0
}

#[cfg(feature = "hostrun")]
pub use hostrun::{
    bar_host_va, hostrun_install, hostrun_read_intercept, hostrun_set_cfg_dword,
    hostrun_set_real_bar,
};

/// Functional Linux-host backing for the syscall seam — the off-target runner
/// mode. See the module docs at the top of this file.
#[cfg(feature = "hostrun")]
mod hostrun {
    use super::abi;
    use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

    // Raw libc externs — the runner binary is a std Linux executable, so these
    // resolve against glibc at the final link. Declared minimally (no libc crate:
    // this rlib stays no_std and dependency-free).
    #[repr(C)]
    struct Timespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    extern "C" {
        fn write(fd: i32, buf: *const u8, count: usize) -> isize;
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn open(path: *const u8, flags: i32) -> i32;
        fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
        fn close(fd: i32) -> i32;
        fn lseek(fd: i32, off: i64, whence: i32) -> i64;
        fn nanosleep(req: *const Timespec, rem: *mut Timespec) -> i32;
        fn clock_gettime(clk: i32, tp: *mut Timespec) -> i32;
    }
    const PROT_READ: i32 = 1;
    const PROT_WRITE: i32 = 2;
    const MAP_SHARED: i32 = 1;
    const MAP_PRIVATE: i32 = 2;
    const MAP_ANONYMOUS: i32 = 0x20;
    const MAP_FAILED: u64 = u64::MAX;
    const O_RDONLY: i32 = 0;
    const SEEK_SET: i32 = 0;
    const SEEK_END: i32 = 2;
    const CLOCK_MONOTONIC: i32 = 1;

    /// Armed by [`hostrun_install`]. Until then every op behaves exactly like the
    /// `hosttest` stubs, so a feature-unified `linuxkpi_harness` build is unaffected.
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    /// Fallback jiffies for the inactive (hosttest-equivalent) mode.
    static STUB_JIFFIES: AtomicU64 = AtomicU64::new(0);
    /// CLOCK_MONOTONIC ns at install time — jiffies epoch (1 kHz, like the kernel's).
    static EPOCH_NS: AtomicI64 = AtomicI64::new(0);
    /// Cached per-BAR host mapping (6 BARs), keyed off `device_map`'s sizes.
    static BAR_HOST_VA: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
    /// Fake PCI config space: 4 KiB as 1024 dwords; the runner seeds it from the
    /// real GPU's sysfs `config` (oracle values, not guesses).
    static CFG_SPACE: [AtomicU32; 1024] = [const { AtomicU32::new(0) }; 1024];
    /// `map_phys` identity table: (phys, size, host_va) — repeated maps of the same
    /// carveout range must return the SAME backing so contents persist, matching
    /// the kernel's behavior.
    const NPHYS: usize = 64;
    static PHYS_KEY: [AtomicU64; NPHYS] = [const { AtomicU64::new(0) }; NPHYS];
    static PHYS_LEN: [AtomicU64; NPHYS] = [const { AtomicU64::new(0) }; NPHYS];
    static PHYS_VA: [AtomicU64; NPHYS] = [const { AtomicU64::new(0) }; NPHYS];
    /// Firmware directory (NUL-free bytes + len), set by the runner.
    static FW_DIR: spinless::Cell<[u8; 256]> = spinless::Cell::new([0; 256]);
    static FW_DIR_LEN: AtomicU64 = AtomicU64::new(0);

    /// Single-writer, install-time-only cell (no spin dep in this crate). Safe
    /// because `hostrun_install` runs once before the C init, single-threaded.
    mod spinless {
        use core::cell::UnsafeCell;
        pub struct Cell<T>(UnsafeCell<T>);
        // SAFETY: written only during single-threaded install, read-only after.
        unsafe impl<T> Sync for Cell<T> {}
        impl<T> Cell<T> {
            pub const fn new(v: T) -> Self {
                Self(UnsafeCell::new(v))
            }
            pub fn get(&self) -> *mut T {
                self.0.get()
            }
        }
    }

    fn now_ns() -> i64 {
        let mut ts = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
        ts.tv_sec
            .wrapping_mul(1_000_000_000)
            .wrapping_add(ts.tv_nsec)
    }

    fn anon_map(size: u64) -> u64 {
        let sz = ((size + 0xFFF) & !0xFFF) as usize;
        let p = unsafe {
            mmap(
                core::ptr::null_mut(),
                sz,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        } as u64;
        if p == MAP_FAILED {
            0
        } else {
            p
        }
    }

    /// Arm the functional seam. `firmware_dir` is the directory that contains the
    /// `amdgpu/` blob tree (usually the repo's `firmware/`). Call once, before
    /// entering the C init, from a single thread.
    pub fn hostrun_install(firmware_dir: &str) {
        let bytes = firmware_dir.as_bytes();
        let n = bytes.len().min(255);
        unsafe {
            let buf = &mut *FW_DIR.get();
            buf[..n].copy_from_slice(&bytes[..n]);
        }
        FW_DIR_LEN.store(n as u64, Ordering::SeqCst);
        EPOCH_NS.store(now_ns(), Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
    }

    /// Seed one dword of the fake PCI config space (offset must be 4-aligned).
    pub fn hostrun_set_cfg_dword(offset: u16, value: u32) {
        CFG_SPACE[(offset as usize / 4).min(1023)].store(value, Ordering::SeqCst);
    }

    /// hosttest-equivalent behavior for the not-yet-installed seam (keeps the
    /// feature-unified harness build semantics unchanged).
    fn stub(nr: u64, a1: u64) -> u64 {
        if nr == abi::SYS_LINUXKPI_PCI_ENABLE
            || nr == abi::SYS_LINUXKPI_DMA_ALLOC
            || nr == abi::SYS_LINUXKPI_REQUEST_FIRMWARE
        {
            u64::MAX
        } else if nr == abi::SYS_LINUXKPI_PRINTK {
            a1
        } else if nr == abi::SYS_LINUXKPI_VERSION {
            0x524B_5049_0001
        } else if nr == abi::SYS_LINUXKPI_JIFFIES {
            STUB_JIFFIES.fetch_add(1, Ordering::Relaxed)
        } else {
            0
        }
    }

    fn firmware_load(name_ptr: u64, name_len: u64, out_ptr: u64) -> u64 {
        const E_FAIL: u64 = 0xFFFF_FFFF_FFFF_FC02; // >= E_* sentinel band
                                                   // path = FW_DIR ++ "/" ++ name ++ NUL
        let mut path = [0u8; 512];
        let dlen = FW_DIR_LEN.load(Ordering::SeqCst) as usize;
        let nlen = name_len as usize;
        if dlen + 1 + nlen + 1 > path.len() || name_ptr == 0 || out_ptr == 0 {
            return E_FAIL;
        }
        unsafe {
            let dir: &[u8; 256] = &*FW_DIR.get();
            path[..dlen].copy_from_slice(&dir[..dlen]);
            path[dlen] = b'/';
            core::ptr::copy_nonoverlapping(
                name_ptr as *const u8,
                path[dlen + 1..].as_mut_ptr(),
                nlen,
            );
        }
        let fd = unsafe { open(path.as_ptr(), O_RDONLY) };
        if fd < 0 {
            return E_FAIL;
        }
        let size = unsafe { lseek(fd, 0, SEEK_END) };
        if size <= 0 {
            unsafe { close(fd) };
            return E_FAIL;
        }
        unsafe { lseek(fd, 0, SEEK_SET) };
        let va = anon_map(size as u64);
        if va == 0 {
            unsafe { close(fd) };
            return E_FAIL;
        }
        let mut done = 0usize;
        while done < size as usize {
            let r = unsafe { read(fd, (va + done as u64) as *mut u8, size as usize - done) };
            if r <= 0 {
                unsafe { close(fd) };
                return E_FAIL;
            }
            done += r as usize;
        }
        unsafe { close(fd) };
        // kernel contract: out_ptr receives [user_virt: u64, size: u64]
        unsafe {
            *(out_ptr as *mut u64) = va;
            *((out_ptr + 8) as *mut u64) = size as u64;
        }
        0
    }

    fn map_phys(phys: u64, size: u64) -> u64 {
        // Same phys range -> same backing (contents persist across re-maps).
        for i in 0..NPHYS {
            let k = PHYS_KEY[i].load(Ordering::SeqCst);
            if k == phys && PHYS_LEN[i].load(Ordering::SeqCst) >= size {
                return PHYS_VA[i].load(Ordering::SeqCst);
            }
        }
        let va = anon_map(size);
        if va == 0 {
            return 0x8000_0000_0000_0001; // E_* band (high bit set)
        }
        for i in 0..NPHYS {
            if PHYS_KEY[i]
                .compare_exchange(0, phys, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                PHYS_LEN[i].store(size, Ordering::SeqCst);
                PHYS_VA[i].store(va, Ordering::SeqCst);
                return va;
            }
        }
        va // table full: still hand out a valid (unshared) mapping
    }

    pub fn dispatch(nr: u64, a0: u64, a1: u64, a2: u64, _a3: u64) -> u64 {
        if !ACTIVE.load(Ordering::Relaxed) {
            return stub(nr, a1);
        }
        match nr {
            n if n == abi::SYS_LINUXKPI_PCI_ENABLE => 1, // fake device handle
            n if n == abi::SYS_LINUXKPI_IOREMAP => {
                let bar = (a1 as usize).min(5);
                let cached = BAR_HOST_VA[bar].load(Ordering::SeqCst);
                if cached != 0 {
                    return cached;
                }
                let (_start, size) = crate::device_map::registered_bar(bar as u32);
                if size == 0 {
                    return u64::MAX; // unregistered BAR: same failure the kernel reports
                }
                let va = anon_map(size);
                if va == 0 {
                    return u64::MAX;
                }
                BAR_HOST_VA[bar].store(va, Ordering::SeqCst);
                va
            }
            n if n == abi::SYS_LINUXKPI_IOUNMAP => 0,
            n if n == abi::SYS_LINUXKPI_MAP_PHYS => map_phys(a1, a2),
            n if n == abi::SYS_LINUXKPI_PCI_READ_CFG => {
                CFG_SPACE[(a1 as usize / 4).min(1023)].load(Ordering::SeqCst) as u64
            }
            n if n == abi::SYS_LINUXKPI_PCI_WRITE_CFG => {
                CFG_SPACE[(a1 as usize / 4).min(1023)].store(a2 as u32, Ordering::SeqCst);
                0
            }
            n if n == abi::SYS_LINUXKPI_DMA_ALLOC => {
                let (size, out_ptr) = (a1, a2);
                let va = anon_map(size);
                if va == 0 || out_ptr == 0 {
                    return u64::MAX;
                }
                // kernel contract: [virt, phys, size, token]; host: phys == virt.
                unsafe {
                    *(out_ptr as *mut u64) = va;
                    *((out_ptr + 8) as *mut u64) = va;
                    *((out_ptr + 16) as *mut u64) = size;
                    *((out_ptr + 24) as *mut u64) = va;
                }
                0
            }
            n if n == abi::SYS_LINUXKPI_DMA_FREE => 0,
            n if n == abi::SYS_LINUXKPI_REQUEST_IRQ => 1,
            n if n == abi::SYS_LINUXKPI_IRQ_WAIT => {
                // No device to fire IRQs: sleep a tick so poll loops make time
                // progress instead of spinning.
                let ts = Timespec {
                    tv_sec: 0,
                    tv_nsec: 1_000_000,
                };
                unsafe { nanosleep(&ts, core::ptr::null_mut()) };
                0
            }
            n if n == abi::SYS_LINUXKPI_SUPERVISOR => 0,
            n if n == abi::SYS_RAEGFX_REGISTER_SCANOUT => 1,
            n if n == abi::SYS_LINUXKPI_REQUEST_FIRMWARE => firmware_load(a0, a1, a2),
            n if n == abi::SYS_LINUXKPI_VERSION => 0x524B_5049_0001,
            n if n == abi::SYS_LINUXKPI_JIFFIES => {
                let dt = now_ns().wrapping_sub(EPOCH_NS.load(Ordering::SeqCst));
                (dt / 1_000_000) as u64 // 1 kHz jiffies, like the kernel
            }
            n if n == abi::SYS_LINUXKPI_MSLEEP => {
                let ts = Timespec {
                    tv_sec: (a0 / 1000) as i64,
                    tv_nsec: ((a0 % 1000) * 1_000_000) as i64,
                };
                unsafe { nanosleep(&ts, core::ptr::null_mut()) };
                0
            }
            n if n == abi::SYS_LINUXKPI_PRINTK => {
                unsafe { write(2, a0 as *const u8, a1 as usize) };
                a1
            }
            n if n == abi::SYS_NETLOG_FLUSH => 0,
            _ => 0,
        }
    }

    /// Host virtual base of a BAR's fake mapping (0 if not yet mapped) — lets the
    /// runner prefill oracle register values before the C init reads them.
    pub fn bar_host_va(bar: usize) -> u64 {
        BAR_HOST_VA[bar.min(5)].load(Ordering::SeqCst)
    }

    // ── real-silicon READ-THROUGH (umr-class, read-only) ────────────────────
    // The dev box IS the reference GPU. With a real BAR registered, register
    // READS that land in the fake BAR window are forwarded to a PROT_READ mmap
    // of the live device's sysfs `resourceN` — the exact registers the live
    // amdgpu read during its own init, so umr-class safe. WRITES still land in
    // the fake buffer only (the live driver owns the hardware). Divergence to
    // expect: INDEX/DATA indirect pairs read whatever the LIVE driver's index
    // selects (garbage for us, harmless for it), and RMW registers read back
    // hardware state rather than our dropped writes. A diagnostic mode, not a
    // bring-up mode — the write-enabled test needs exclusive VFIO ownership.
    static REAL_BAR_VA: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
    static REAL_BAR_LEN: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

    /// Map the live device's `resourceN` read-only behind fake BAR `bar`.
    /// Needs root (sysfs resource files are 0600). Returns false on failure.
    pub fn hostrun_set_real_bar(bar: usize, sysfs_resource_path: &str) -> bool {
        if bar >= 6 {
            return false;
        }
        let mut path = [0u8; 256];
        let bytes = sysfs_resource_path.as_bytes();
        if bytes.len() >= path.len() {
            return false;
        }
        path[..bytes.len()].copy_from_slice(bytes);
        let fd = unsafe { open(path.as_ptr(), O_RDONLY) };
        if fd < 0 {
            return false;
        }
        let size = unsafe { lseek(fd, 0, SEEK_END) };
        if size <= 0 {
            unsafe { close(fd) };
            return false;
        }
        let va = unsafe {
            mmap(
                core::ptr::null_mut(),
                size as usize,
                PROT_READ,
                MAP_SHARED,
                fd,
                0,
            )
        } as u64;
        unsafe { close(fd) };
        if va == MAP_FAILED {
            return false;
        }
        REAL_BAR_LEN[bar].store(size as u64, Ordering::SeqCst);
        REAL_BAR_VA[bar].store(va, Ordering::SeqCst);
        true
    }

    /// If `addr` falls inside a fake BAR window that has a real read-through
    /// mapping, return the REAL register bytes at the same offset (`width` = 4
    /// or 8). Called from the shim's `readl`/`readq` under `hostrun`.
    pub fn hostrun_read_intercept(addr: u64, width: usize) -> Option<u64> {
        for bar in 0..6 {
            let real = REAL_BAR_VA[bar].load(Ordering::Relaxed);
            if real == 0 {
                continue;
            }
            let fake = BAR_HOST_VA[bar].load(Ordering::Relaxed);
            let len = REAL_BAR_LEN[bar].load(Ordering::Relaxed);
            if fake != 0 && addr >= fake && addr + width as u64 <= fake + len {
                let off = addr - fake;
                return Some(unsafe {
                    if width == 8 {
                        core::ptr::read_volatile((real + off) as *const u64)
                    } else {
                        core::ptr::read_volatile((real + off) as *const u32) as u64
                    }
                });
            }
        }
        None
    }
}
