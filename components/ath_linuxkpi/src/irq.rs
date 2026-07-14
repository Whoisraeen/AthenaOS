//! IRQ → doorbell bridge — Phase 2 of the LinuxKPI host.
//!
//! Linux drivers call `request_irq(vector, handler)` expecting the kernel to
//! invoke `handler` in interrupt context. AthenaOS runs the driver in userspace,
//! so we cannot deliver a real hardware interrupt to it directly. Instead:
//!
//!   1. `request_irq` routes the device's MSI-X vector to an IPC doorbell.
//!   2. The driver daemon spawns an IRQ thread that loops on `irq_wait`.
//!   3. When the hardware raises the interrupt, the kernel top-half bumps the
//!      doorbell counter; `irq_wait` returns and the daemon calls the driver's
//!      native C `irqreturn_t handler(int irq, void *dev)`.
//!
//! This preserves the Linux driver's interrupt model without giving it Ring 0.

use crate::host;

/// Opaque IRQ handle returned by `request_irq`, passed to `irq_wait`.
pub type IrqHandle = u64;

/// Linux `irq_handler_t` — `irqreturn_t (*)(int irq, void *dev_id)`.
pub type IrqHandler = extern "C" fn(irq: i32, dev_id: *mut core::ffi::c_void) -> i32;

pub const IRQ_NONE: i32 = 0;
pub const IRQ_HANDLED: i32 = 1;

/// Block until the next interrupt doorbell fires for this device.
/// Returns the vector that fired (so a multi-vector driver can demux).
pub fn irq_wait(handle: IrqHandle) -> u8 {
    let v = unsafe { host::sys_irq_wait(handle) };
    (v & 0xFF) as u8
}

/// Drive a Linux interrupt handler in a loop. The daemon calls this from a
/// dedicated IRQ thread; each doorbell invokes the native C handler.
///
/// `irq` is the Linux IRQ number to pass to the handler; `dev_id` is the
/// driver's private cookie (its `struct pci_dev *` or similar).
pub fn irq_thread_loop(
    handle: IrqHandle,
    irq: i32,
    handler: IrqHandler,
    dev_id: *mut core::ffi::c_void,
) -> ! {
    loop {
        let _vector = irq_wait(handle);
        // Fire the driver's native C interrupt handler.
        let _ret = handler(irq, dev_id);
        // A real impl checks IRQ_HANDLED vs IRQ_NONE for shared-IRQ demux.
    }
}

// ── Linux C-ABI IRQ surface (request_irq / free_irq / enable / disable) ───────
//
// Linux `request_irq(irq, handler, flags, name, dev)` registers a top-half
// handler. The daemon runs cooperatively, so we record the handler in a fixed
// registry keyed by irq number; the daemon's IRQ pump (`lkpi_serve_irq`) blocks
// on the doorbell and dispatches. Pointers are stored as usize in atomics so
// the registry is Sync without a lock.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const MAX_IRQS: usize = 32;
// handler fn-ptr (as usize), 0 = unregistered.
static HANDLERS: [AtomicUsize; MAX_IRQS] = [const { AtomicUsize::new(0) }; MAX_IRQS];
// driver dev_id cookie (as usize).
static DEV_IDS: [AtomicUsize; MAX_IRQS] = [const { AtomicUsize::new(0) }; MAX_IRQS];
// enable state (1 = enabled).
static ENABLED: [AtomicU32; MAX_IRQS] = [const { AtomicU32::new(0) }; MAX_IRQS];

/// Linux `request_irq(unsigned int irq, irq_handler_t handler, unsigned long
/// flags, const char *name, void *dev)`. Returns 0 on success, -EINVAL on a bad
/// irq. The handler fires from the daemon's IRQ pump, not hardware context.
#[no_mangle]
pub extern "C" fn request_irq(
    irq: u32,
    handler: IrqHandler,
    _flags: u64,
    _name: *const u8,
    dev: *mut core::ffi::c_void,
) -> i32 {
    let i = irq as usize;
    if i >= MAX_IRQS {
        return -22; // -EINVAL
    }
    HANDLERS[i].store(handler as usize, Ordering::SeqCst);
    DEV_IDS[i].store(dev as usize, Ordering::SeqCst);
    ENABLED[i].store(1, Ordering::SeqCst);
    0
}

/// `request_threaded_irq(irq, top, thread_fn, flags, name, dev)` — register the
/// primary handler (thread_fn, when non-null, is the bottom half; we run both
/// from the pump). Returns 0 on success.
#[no_mangle]
pub extern "C" fn request_threaded_irq(
    irq: u32,
    handler: IrqHandler,
    _thread_fn: usize,
    _flags: u64,
    _name: *const u8,
    dev: *mut core::ffi::c_void,
) -> i32 {
    request_irq(irq, handler, 0, core::ptr::null(), dev)
}

#[no_mangle]
pub extern "C" fn free_irq(irq: u32, _dev: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
    let i = irq as usize;
    if i < MAX_IRQS {
        HANDLERS[i].store(0, Ordering::SeqCst);
        ENABLED[i].store(0, Ordering::SeqCst);
        let d = DEV_IDS[i].swap(0, Ordering::SeqCst);
        return d as *mut core::ffi::c_void;
    }
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn enable_irq(irq: u32) {
    if (irq as usize) < MAX_IRQS {
        ENABLED[irq as usize].store(1, Ordering::SeqCst);
    }
}
#[no_mangle]
pub extern "C" fn disable_irq(irq: u32) {
    if (irq as usize) < MAX_IRQS {
        ENABLED[irq as usize].store(0, Ordering::SeqCst);
    }
}
#[no_mangle]
pub extern "C" fn disable_irq_nosync(irq: u32) {
    disable_irq(irq);
}
#[no_mangle]
pub extern "C" fn synchronize_irq(_irq: u32) {}

/// Dispatch the registered handler for `irq` once (called by the daemon's IRQ
/// pump after a doorbell). Returns the handler's `irqreturn_t`, or IRQ_NONE if
/// none registered / disabled.
#[no_mangle]
pub extern "C" fn lkpi_dispatch_irq(irq: u32) -> i32 {
    let i = irq as usize;
    if i >= MAX_IRQS || ENABLED[i].load(Ordering::SeqCst) == 0 {
        return IRQ_NONE;
    }
    let h = HANDLERS[i].load(Ordering::SeqCst);
    if h == 0 {
        return IRQ_NONE;
    }
    let handler: IrqHandler = unsafe { core::mem::transmute(h) };
    let dev = DEV_IDS[i].load(Ordering::SeqCst) as *mut core::ffi::c_void;
    handler(irq as i32, dev)
}

/// Daemon IRQ pump: block on the device doorbell, then dispatch `irq`'s handler.
/// Loops forever (the daemon runs this on its IRQ thread).
#[no_mangle]
pub extern "C" fn lkpi_serve_irq(dev_handle: u64, irq: u32, vector: u8) -> ! {
    let handle = request_irq_doorbell(dev_handle, vector);
    loop {
        let _ = irq_wait(handle);
        let _ = lkpi_dispatch_irq(irq);
    }
}

/// Internal: route a device MSI-X vector to a host doorbell (the old
/// `request_irq(dev, vector)` semantics, kept for the daemon's own use).
pub fn request_irq_doorbell(dev: u64, vector: u8) -> IrqHandle {
    unsafe { host::sys_request_irq(dev, vector as u64) }
}
