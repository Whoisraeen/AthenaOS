//! `dma_fence` — the dma-buf framework's async-completion primitive.
//!
//! A dma_fence represents "this async work will complete"; a GPU/codec driver
//! creates one per command submission and the rest of the system waits on it or
//! attaches callbacks. It is foundational dma-buf infrastructure (reused well
//! beyond DRM), so the shim implements the real semantics: a signaled flag bit,
//! an intrusive callback list run exactly once on signal, refcounted lifetime,
//! and bounded waits.
//!
//! **Layout is verified, not guessed.** Drivers EMBED `struct dma_fence` and
//! reach its fields by offset through inlines (`dma_fence_is_signaled` tests the
//! flag bit; `dma_fence_get/put` touch the embedded `kref`), so the layout must
//! match the target kernel byte-for-byte. The structs below were taken from the
//! WSL kernel's BTF (`pahole -C dma_fence /sys/kernel/btf/vmlinux`, Linux 6.6),
//! and the `const _: () = assert!(...)` offset checks at the bottom FAIL THE
//! BUILD if the layout ever drifts from those values. When targeting a different
//! kernel, re-run pahole and update both the structs and the asserts together.
//!
//! Single-threaded cooperative daemon: signalling runs callbacks inline and
//! waits poll the flag via the host millisecond sleep; the `dma_fence_ops`
//! `lock` is honored structurally but never contended.

use crate::host;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// `DMA_FENCE_FLAG_SIGNALED_BIT`.
const SIGNALED_BIT: usize = 0;

/// Linux `struct list_head` (intrusive, circular).
#[repr(C)]
pub struct ListHead {
    pub next: *mut ListHead,
    pub prev: *mut ListHead,
}

/// `dma_fence_func_t` — the callback fired when a fence signals.
pub type DmaFenceFunc = extern "C" fn(*mut DmaFence, *mut DmaFenceCb);

/// `struct dma_fence` (BTF: size 64; lock@0 ops@8 cb_list@16 context@32
/// seqno@40 flags@48 refcount@56 error@60). The `cb_list` union member is the
/// one drivers use after init; `timestamp`/`rcu` overlap it and are unused here.
#[repr(C)]
pub struct DmaFence {
    pub lock: *mut u8,           // spinlock_t *
    pub ops: *const DmaFenceOps, // const struct dma_fence_ops *
    pub cb_list: ListHead,       // union { list_head cb_list; ktime_t; rcu; }
    pub context: u64,
    pub seqno: u64,
    pub flags: usize,  // unsigned long
    pub refcount: i32, // struct kref { refcount_t { atomic_t { int } } }
    pub error: i32,
}

/// `struct dma_fence_ops` (BTF: size 80; bool@0 then the 9 vtable slots at
/// 8/16/24/32/40/48/56/64/72).
#[repr(C)]
pub struct DmaFenceOps {
    pub use_64bit_seqno: bool,
    pub get_driver_name: Option<extern "C" fn(*mut DmaFence) -> *const u8>,
    pub get_timeline_name: Option<extern "C" fn(*mut DmaFence) -> *const u8>,
    pub enable_signaling: Option<extern "C" fn(*mut DmaFence) -> bool>,
    pub signaled: Option<extern "C" fn(*mut DmaFence) -> bool>,
    pub wait: Option<extern "C" fn(*mut DmaFence, bool, i64) -> i64>,
    pub release: Option<extern "C" fn(*mut DmaFence)>,
    pub fence_value_str: Option<extern "C" fn(*mut DmaFence, *mut u8, i32)>,
    pub timeline_value_str: Option<extern "C" fn(*mut DmaFence, *mut u8, i32)>,
    pub set_deadline: Option<extern "C" fn(*mut DmaFence, i64)>,
}

/// `struct dma_fence_cb` (BTF: size 24; node@0 func@16). `node` is first, so a
/// `*DmaFenceCb` and its `&node` alias — `container_of` is identity.
#[repr(C)]
pub struct DmaFenceCb {
    pub node: ListHead,
    pub func: Option<DmaFenceFunc>,
}

// ── intrusive list helpers (inline in Linux; internal here) ──────────────────

// Reused by sibling dma-buf modules (`dma_buf` manages an attachment list).
#[inline]
pub(crate) unsafe fn list_init(h: *mut ListHead) {
    (*h).next = h;
    (*h).prev = h;
}
#[inline]
pub(crate) unsafe fn list_empty(h: *mut ListHead) -> bool {
    (*h).next == h
}
#[inline]
pub(crate) unsafe fn list_add_tail(node: *mut ListHead, head: *mut ListHead) {
    let prev = (*head).prev;
    (*node).next = head;
    (*node).prev = prev;
    (*prev).next = node;
    (*head).prev = node;
}
#[inline]
pub(crate) unsafe fn list_del_init(node: *mut ListHead) {
    let p = (*node).prev;
    let n = (*node).next;
    (*p).next = n;
    (*n).prev = p;
    list_init(node);
}

/// Test the signaled flag bit. `pub(crate)` so sibling dma-buf modules
/// (`dma_resv`) can check a fence without going through the inline-only
/// `dma_fence_is_signaled`.
#[inline]
pub(crate) unsafe fn is_signaled(fence: *mut DmaFence) -> bool {
    !fence.is_null() && (*fence).flags & (1 << SIGNALED_BIT) != 0
}

/// Detach the callback list and run every callback once (each node is reset to
/// "empty" before its func runs, so a racing `remove_callback` sees it gone).
unsafe fn run_callbacks(fence: *mut DmaFence) {
    let head = core::ptr::addr_of_mut!((*fence).cb_list);
    let mut cur = (*head).next;
    list_init(head);
    while cur != head && !cur.is_null() {
        let next = (*cur).next; // capture before we reset this node
        let cb = cur as *mut DmaFenceCb;
        (*cur).next = cur;
        (*cur).prev = cur;
        if let Some(f) = (*cb).func {
            f(fence, cb);
        }
        cur = next;
    }
}

// ── public C ABI ──────────────────────────────────────────────────────────────

/// Global fence-context counter (Linux's `dma_fence_context_counter`).
static FENCE_CONTEXT: AtomicU64 = AtomicU64::new(1);

/// `dma_fence_context_alloc(num)` — reserve `num` contexts, return the first.
#[no_mangle]
pub extern "C" fn dma_fence_context_alloc(num: u64) -> u64 {
    FENCE_CONTEXT.fetch_add(num, Ordering::Relaxed)
}

/// `dma_fence_init(fence, ops, lock, context, seqno)`.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_init(
    fence: *mut DmaFence,
    ops: *const DmaFenceOps,
    lock: *mut u8,
    context: u64,
    seqno: u64,
) {
    if fence.is_null() {
        return;
    }
    (*fence).lock = lock;
    (*fence).ops = ops;
    list_init(core::ptr::addr_of_mut!((*fence).cb_list));
    (*fence).context = context;
    (*fence).seqno = seqno;
    (*fence).flags = 0;
    (*fence).refcount = 1;
    (*fence).error = 0;
}

/// `dma_fence_init64(fence, ops, lock, context, seqno)`.
///
/// Upstream amdgpu uses this 64-bit-sequence entry point for ring timelines.
/// Its layout and initialization contract are identical to `dma_fence_init`.
/// Keeping it as an empty bring-up export leaves the callback list, refcount,
/// ops table, and sequence number invalid, so a real completion can look like a
/// fence timeout or corrupt callback state. Keep both C ABI entry points beside
/// the typed implementation so they cannot diverge.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_init64(
    fence: *mut DmaFence,
    ops: *const DmaFenceOps,
    lock: *mut u8,
    context: u64,
    seqno: u64,
) {
    dma_fence_init(fence, ops, lock, context, seqno);
}

/// `dma_fence_signal_locked(fence)` → 0, or `-EINVAL` if already signaled.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_signal_locked(fence: *mut DmaFence) -> i32 {
    if fence.is_null() {
        return -22;
    }
    if is_signaled(fence) {
        return -22; // -EINVAL: already signaled
    }
    (*fence).flags |= 1 << SIGNALED_BIT;
    run_callbacks(fence);
    0
}

/// `dma_fence_signal(fence)` — same in the single-threaded daemon (no real lock
/// contention).
#[no_mangle]
pub unsafe extern "C" fn dma_fence_signal(fence: *mut DmaFence) -> i32 {
    dma_fence_signal_locked(fence)
}

/// Signal a fence and preserve the completion timestamp in the union field.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_signal_timestamp(fence: *mut DmaFence, timestamp: i64) -> i32 {
    let result = dma_fence_signal_locked(fence);
    if result == 0 {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*fence).cb_list).cast(), timestamp);
        (*fence).flags |= 1 << 1; // DMA_FENCE_FLAG_TIMESTAMP_BIT
    }
    result
}

/// `dma_fence_get(fence)` — take a reference; returns the fence (NULL-safe).
#[no_mangle]
pub unsafe extern "C" fn dma_fence_get(fence: *mut DmaFence) -> *mut DmaFence {
    if !fence.is_null() {
        (*fence).refcount += 1;
    }
    fence
}

/// `dma_fence_get_rcu(fence)` — the daemon is a single address space with no RCU
/// grace period to race, so a plain ref is safe.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_get_rcu(fence: *mut DmaFence) -> *mut DmaFence {
    dma_fence_get(fence)
}

/// `dma_fence_get_rcu_safe(fpp)` — load `*fpp` and ref it (no RCU race here).
#[no_mangle]
pub unsafe extern "C" fn dma_fence_get_rcu_safe(fpp: *mut *mut DmaFence) -> *mut DmaFence {
    if fpp.is_null() {
        return core::ptr::null_mut();
    }
    dma_fence_get(*fpp)
}

/// `dma_fence_put(fence)` — drop a reference; release at zero via `ops->release`.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_put(fence: *mut DmaFence) {
    if fence.is_null() {
        return;
    }
    (*fence).refcount -= 1;
    if (*fence).refcount <= 0 && !(*fence).ops.is_null() {
        if let Some(release) = (*(*fence).ops).release {
            release(fence);
        }
    }
}

/// `dma_fence_is_signaled_locked(fence)` — if not already flagged, query
/// `ops->signaled`; flag (+ run callbacks) when it reports completion.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_is_signaled_locked(fence: *mut DmaFence) -> bool {
    if fence.is_null() {
        return false;
    }
    if is_signaled(fence) {
        return true;
    }
    if !(*fence).ops.is_null() {
        if let Some(signaled) = (*(*fence).ops).signaled {
            if signaled(fence) {
                dma_fence_signal_locked(fence);
                return true;
            }
        }
    }
    false
}

/// `dma_fence_is_signaled(fence)` — same in the single-threaded daemon.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_is_signaled(fence: *mut DmaFence) -> bool {
    dma_fence_is_signaled_locked(fence)
}

/// `dma_fence_add_callback(fence, cb, func)` → 0 if queued, `-ENOENT` if the
/// fence is already signaled (caller must handle that case itself).
#[no_mangle]
pub unsafe extern "C" fn dma_fence_add_callback(
    fence: *mut DmaFence,
    cb: *mut DmaFenceCb,
    func: Option<DmaFenceFunc>,
) -> i32 {
    if fence.is_null() || cb.is_null() {
        return -22;
    }
    if is_signaled(fence) {
        return -2; // -ENOENT
    }
    (*cb).func = func;
    list_add_tail(
        core::ptr::addr_of_mut!((*cb).node),
        core::ptr::addr_of_mut!((*fence).cb_list),
    );
    0
}

/// `dma_fence_remove_callback(fence, cb)` → true if the callback was still
/// pending and got removed (false if it already ran / the fence signaled).
#[no_mangle]
pub unsafe extern "C" fn dma_fence_remove_callback(
    _fence: *mut DmaFence,
    cb: *mut DmaFenceCb,
) -> bool {
    if cb.is_null() {
        return false;
    }
    let node = core::ptr::addr_of_mut!((*cb).node);
    if list_empty(node) {
        return false;
    }
    list_del_init(node);
    true
}

/// `dma_fence_release(kref)` — the kref release the inlined `dma_fence_put`
/// calls at refcount 0. Recovers the fence from the embedded kref and invokes
/// `ops->release` so driver cleanup runs.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_release(kref: *mut u8) {
    if kref.is_null() {
        return;
    }
    let fence = (kref as usize - core::mem::offset_of!(DmaFence, refcount)) as *mut DmaFence;
    if !(*fence).ops.is_null() {
        if let Some(rel) = (*(*fence).ops).release {
            rel(fence);
        }
    }
}

/// `dma_fence_wait_timeout(fence, intr, timeout)` — poll the signaled flag,
/// sleeping in 1-jiffy steps. Returns remaining jiffies (>0) or 0 on timeout.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_wait_timeout(
    fence: *mut DmaFence,
    _intr: bool,
    timeout: i64,
) -> i64 {
    if fence.is_null() {
        return -22;
    }
    if is_signaled(fence) {
        return timeout.max(1);
    }
    let mut remaining = timeout.max(0);
    while remaining > 0 {
        if is_signaled(fence) {
            return remaining;
        }
        host::sys_linuxkpi_msleep(1);
        remaining -= 1;
    }
    if is_signaled(fence) {
        1
    } else {
        0
    }
}

/// `dma_fence_wait_any_timeout(fences, count, intr, timeout, idx)` — return when
/// ANY fence signals; writes its index to `*idx`. Returns remaining jiffies or 0.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_wait_any_timeout(
    fences: *const *mut DmaFence,
    count: u32,
    _intr: bool,
    timeout: i64,
    idx: *mut u32,
) -> i64 {
    if fences.is_null() || count == 0 {
        return -22;
    }
    let mut remaining = timeout.max(0);
    loop {
        for i in 0..count as usize {
            let f = *fences.add(i);
            if !f.is_null() && is_signaled(f) {
                if !idx.is_null() {
                    *idx = i as u32;
                }
                return remaining.max(1);
            }
        }
        if remaining == 0 {
            return 0;
        }
        host::sys_linuxkpi_msleep(1);
        remaining -= 1;
    }
}

// ── dma_fence_get_stub: a process-wide always-signaled fence ──────────────────

extern "C" fn stub_driver_name(_f: *mut DmaFence) -> *const u8 {
    b"athena-linuxkpi\0".as_ptr()
}
extern "C" fn stub_timeline_name(_f: *mut DmaFence) -> *const u8 {
    b"stub\0".as_ptr()
}

/// Minimal ops for the stub fence (and a reasonable default for simple driver
/// fences): names only; signalling is explicit via the flag bit.
#[no_mangle]
pub static athena_dma_fence_stub_ops: DmaFenceOps = DmaFenceOps {
    use_64bit_seqno: false,
    get_driver_name: Some(stub_driver_name),
    get_timeline_name: Some(stub_timeline_name),
    enable_signaling: None,
    signaled: None,
    wait: None,
    release: None,
    fence_value_str: None,
    timeline_value_str: None,
    set_deadline: None,
};

extern "C" fn private_stub_release(fence: *mut DmaFence) {
    crate::mm::kfree(fence.cast());
}

static RAEEN_DMA_FENCE_PRIVATE_STUB_OPS: DmaFenceOps = DmaFenceOps {
    use_64bit_seqno: false,
    get_driver_name: Some(stub_driver_name),
    get_timeline_name: Some(stub_timeline_name),
    enable_signaling: None,
    signaled: None,
    wait: None,
    release: Some(private_stub_release),
    fence_value_str: None,
    timeline_value_str: None,
    set_deadline: None,
};

static STUB_INIT: AtomicBool = AtomicBool::new(false);
static mut STUB_LOCK: u32 = 0;
static mut STUB_FENCE: DmaFence = DmaFence {
    lock: core::ptr::null_mut(),
    ops: core::ptr::null(),
    cb_list: ListHead {
        next: core::ptr::null_mut(),
        prev: core::ptr::null_mut(),
    },
    context: 0,
    seqno: 0,
    flags: 0,
    refcount: 0,
    error: 0,
};

/// `dma_fence_get_stub()` — a singleton fence that is always already signaled.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_get_stub() -> *mut DmaFence {
    let f = core::ptr::addr_of_mut!(STUB_FENCE);
    if STUB_INIT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        dma_fence_init(
            f,
            &athena_dma_fence_stub_ops as *const DmaFenceOps,
            core::ptr::addr_of_mut!(STUB_LOCK) as *mut u8,
            0,
            0,
        );
        dma_fence_signal(f);
    }
    dma_fence_get(f)
}

/// Allocate a distinct already-signaled fence carrying the requested timestamp.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_allocate_private_stub(timestamp: i64) -> *mut DmaFence {
    let fence = crate::mm::kzalloc(core::mem::size_of::<DmaFence>(), 0) as *mut DmaFence;
    if fence.is_null() {
        return core::ptr::null_mut();
    }
    dma_fence_init(
        fence,
        core::ptr::addr_of!(RAEEN_DMA_FENCE_PRIVATE_STUB_OPS),
        core::ptr::addr_of_mut!(STUB_LOCK).cast(),
        0,
        0,
    );
    (*fence).flags |= 1 << 2; // DMA_FENCE_FLAG_ENABLE_SIGNAL_BIT
    let _ = dma_fence_signal_timestamp(fence, timestamp);
    fence
}

// ── dma_fence_array: a composite fence that signals when all children do ─────
// BTF: dma_fence_array size 120 (base@0 lock@64 num_fences@68 num_pending@72
// fences@80 work@88); dma_fence_array_cb size 32 (cb@0 array@24). The per-child
// callbacks live in a trailing array right after the struct. The kernel defers
// the base signal via irq_work; the cooperative daemon signals inline instead
// (no IRQ context), so `work` is reserved for layout only.

/// `struct dma_fence_array`.
#[repr(C)]
pub struct DmaFenceArray {
    pub base: DmaFence,
    pub lock: u32, // spinlock_t
    pub num_fences: u32,
    pub num_pending: i32, // atomic_t
    pub fences: *mut *mut DmaFence,
    pub work: [u8; 32], // irq_work (reserved; unused in the daemon)
}

/// `struct dma_fence_array_cb` — one per child; `cb` is first so a `*cb`
/// `container_of`s to the array_cb by identity.
#[repr(C)]
pub struct DmaFenceArrayCb {
    pub cb: DmaFenceCb,
    pub array: *mut DmaFenceArray,
}

extern "C" fn array_driver_name(_f: *mut DmaFence) -> *const u8 {
    b"dma_fence_array\0".as_ptr()
}
extern "C" fn array_timeline_name(_f: *mut DmaFence) -> *const u8 {
    b"unbound\0".as_ptr()
}
extern "C" fn array_release(f: *mut DmaFence) {
    // base is at offset 0 of the array, so the fence pointer IS the array.
    crate::mm::kfree(f as *mut u8);
}

/// Ops table for array fences (drivers also identity-check `ops == &this`).
#[no_mangle]
pub static dma_fence_array_ops: DmaFenceOps = DmaFenceOps {
    use_64bit_seqno: false,
    get_driver_name: Some(array_driver_name),
    get_timeline_name: Some(array_timeline_name),
    enable_signaling: None,
    signaled: None,
    wait: None,
    release: Some(array_release),
    fence_value_str: None,
    timeline_value_str: None,
    set_deadline: None,
};

unsafe fn array_dec(arr: *mut DmaFenceArray) {
    (*arr).num_pending -= 1; // single-threaded daemon: no atomic race
    if (*arr).num_pending <= 0 {
        dma_fence_signal(core::ptr::addr_of_mut!((*arr).base));
    }
}

extern "C" fn array_cb_func(_f: *mut DmaFence, cb: *mut DmaFenceCb) {
    let acb = cb as *mut DmaFenceArrayCb; // cb is the first member
    unsafe { array_dec((*acb).array) };
}

/// `dma_fence_array_create(num_fences, fences, context, seqno, signal_on_any)` —
/// a fence that signals once its children signal (all, or any if `signal_on_any`).
/// Returns the array (NULL on bad args / OOM); freed via the base fence's kref.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_array_create(
    num_fences: i32,
    fences: *mut *mut DmaFence,
    context: u64,
    seqno: u32,
    signal_on_any: bool,
) -> *mut DmaFenceArray {
    if num_fences <= 0 || fences.is_null() {
        return core::ptr::null_mut();
    }
    let n = num_fences as usize;
    let size = core::mem::size_of::<DmaFenceArray>() + n * core::mem::size_of::<DmaFenceArrayCb>();
    let arr = crate::mm::kzalloc(size, 0) as *mut DmaFenceArray;
    if arr.is_null() {
        return core::ptr::null_mut();
    }
    dma_fence_init(
        core::ptr::addr_of_mut!((*arr).base),
        &dma_fence_array_ops as *const DmaFenceOps,
        core::ptr::addr_of_mut!((*arr).lock) as *mut u8,
        context,
        seqno as u64,
    );
    (*arr).num_fences = num_fences as u32;
    (*arr).num_pending = if signal_on_any { 1 } else { num_fences };
    (*arr).fences = fences;
    // trailing per-child callback array
    let cbs = (arr as *mut u8).add(core::mem::size_of::<DmaFenceArray>()) as *mut DmaFenceArrayCb;
    for i in 0..n {
        let acb = cbs.add(i);
        (*acb).array = arr;
        let child = *fences.add(i);
        // -ENOENT means the child is already signaled — count it now.
        if dma_fence_add_callback(
            child,
            core::ptr::addr_of_mut!((*acb).cb),
            Some(array_cb_func),
        ) == -2
        {
            array_dec(arr);
        }
    }
    arr
}

/// Return the first contained fence, or `head` itself when it is not an array.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_array_first(head: *mut DmaFence) -> *mut DmaFence {
    if head.is_null() {
        return core::ptr::null_mut();
    }
    if (*head).ops != core::ptr::addr_of!(dma_fence_array_ops) {
        return head;
    }
    let array = head as *mut DmaFenceArray;
    if (*array).num_fences == 0 || (*array).fences.is_null() {
        core::ptr::null_mut()
    } else {
        *(*array).fences
    }
}

/// Continue iteration through an array returned by `dma_fence_array_first`.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_array_next(head: *mut DmaFence, index: u32) -> *mut DmaFence {
    if head.is_null() || (*head).ops != core::ptr::addr_of!(dma_fence_array_ops) {
        return core::ptr::null_mut();
    }
    let array = head as *mut DmaFenceArray;
    if index >= (*array).num_fences || (*array).fences.is_null() {
        core::ptr::null_mut()
    } else {
        *(*array).fences.add(index as usize)
    }
}

// ── dma_fence_chain: timeline syncobj links ──────────────────────────────────
// BTF: dma_fence_chain size 128 (base@0 prev@64 prev_seqno@72 fence@80
// cb/work-union@88 lock@120). A chain links a wrapped `fence` to a `prev` chain;
// it signals once its fence (and transitively all prev) signal. Drivers detect
// chains by `fence->ops == &dma_fence_chain_ops`.

/// `struct dma_fence_chain`.
#[repr(C)]
pub struct DmaFenceChain {
    pub base: DmaFence,
    pub prev: *mut DmaFence,
    pub prev_seqno: u64,
    pub fence: *mut DmaFence,
    pub cb_work: [u8; 32], // union { dma_fence_cb cb; irq_work work; }
    pub lock: u32,
}

extern "C" fn chain_driver_name(_f: *mut DmaFence) -> *const u8 {
    b"dma_fence_chain\0".as_ptr()
}
extern "C" fn chain_timeline_name(_f: *mut DmaFence) -> *const u8 {
    b"unbound\0".as_ptr()
}
extern "C" fn chain_signaled(f: *mut DmaFence) -> bool {
    // a chain link is signaled once its wrapped fence is
    unsafe {
        let c = f as *mut DmaFenceChain;
        (*c).fence.is_null() || is_signaled((*c).fence)
    }
}

/// `dma_fence_chain_ops` — the exact symbol drivers identity-check against.
#[no_mangle]
pub static dma_fence_chain_ops: DmaFenceOps = DmaFenceOps {
    use_64bit_seqno: true,
    get_driver_name: Some(chain_driver_name),
    get_timeline_name: Some(chain_timeline_name),
    enable_signaling: None,
    signaled: Some(chain_signaled),
    wait: None,
    release: None,
    fence_value_str: None,
    timeline_value_str: None,
    set_deadline: None,
};

#[inline]
unsafe fn to_chain(fence: *mut DmaFence) -> *mut DmaFenceChain {
    if !fence.is_null() && (*fence).ops == core::ptr::addr_of!(dma_fence_chain_ops) {
        fence as *mut DmaFenceChain
    } else {
        core::ptr::null_mut()
    }
}

/// `dma_fence_chain_init(chain, prev, fence, seqno)` — splice `fence` onto the
/// timeline after `prev`.
#[no_mangle]
pub unsafe extern "C" fn dma_fence_chain_init(
    chain: *mut DmaFenceChain,
    prev: *mut DmaFence,
    fence: *mut DmaFence,
    seqno: u64,
) {
    if chain.is_null() {
        return;
    }
    (*chain).prev = prev;
    (*chain).fence = fence;
    (*chain).prev_seqno = if to_chain(prev).is_null() {
        0
    } else {
        (*prev).seqno
    };
    dma_fence_init(
        core::ptr::addr_of_mut!((*chain).base),
        core::ptr::addr_of!(dma_fence_chain_ops),
        core::ptr::addr_of_mut!((*chain).lock) as *mut u8,
        if prev.is_null() { 0 } else { (*prev).context },
        seqno,
    );
}

/// `dma_fence_chain_walk(fence)` — walk back through the timeline and return the
/// first link whose wrapped fence is still unsignaled (NULL if all signaled).
/// The kernel also garbage-collects fully-signaled links in place; the
/// cooperative daemon does the read-only walk (no refcount churn).
#[no_mangle]
pub unsafe extern "C" fn dma_fence_chain_walk(fence: *mut DmaFence) -> *mut DmaFence {
    let mut chain = to_chain(fence);
    if chain.is_null() {
        return core::ptr::null_mut();
    }
    loop {
        let prev = (*chain).prev;
        if prev.is_null() {
            return core::ptr::null_mut();
        }
        let prev_chain = to_chain(prev);
        let inner = if prev_chain.is_null() {
            prev
        } else {
            (*prev_chain).fence
        };
        if !is_signaled(inner) {
            return prev; // first still-pending link
        }
        if prev_chain.is_null() {
            return core::ptr::null_mut(); // plain signaled fence, end of chain
        }
        chain = prev_chain;
    }
}

// ── compile-time layout guard (BTF: Linux 6.6 x86_64) ────────────────────────
// These FAIL THE BUILD if the structs ever drift from the verified offsets.
const _: () = assert!(core::mem::size_of::<DmaFence>() == 64);
const _: () = assert!(core::mem::offset_of!(DmaFence, ops) == 8);
const _: () = assert!(core::mem::offset_of!(DmaFence, cb_list) == 16);
const _: () = assert!(core::mem::offset_of!(DmaFence, context) == 32);
const _: () = assert!(core::mem::offset_of!(DmaFence, seqno) == 40);
const _: () = assert!(core::mem::offset_of!(DmaFence, flags) == 48);
const _: () = assert!(core::mem::offset_of!(DmaFence, refcount) == 56);
const _: () = assert!(core::mem::offset_of!(DmaFence, error) == 60);
const _: () = assert!(core::mem::size_of::<DmaFenceOps>() == 80);
const _: () = assert!(core::mem::offset_of!(DmaFenceOps, signaled) == 32);
const _: () = assert!(core::mem::offset_of!(DmaFenceOps, release) == 48);
const _: () = assert!(core::mem::offset_of!(DmaFenceOps, set_deadline) == 72);
const _: () = assert!(core::mem::size_of::<DmaFenceCb>() == 24);
const _: () = assert!(core::mem::offset_of!(DmaFenceCb, func) == 16);
const _: () = assert!(core::mem::size_of::<DmaFenceArray>() == 120);
const _: () = assert!(core::mem::offset_of!(DmaFenceArray, num_pending) == 72);
const _: () = assert!(core::mem::offset_of!(DmaFenceArray, fences) == 80);
const _: () = assert!(core::mem::size_of::<DmaFenceArrayCb>() == 32);
const _: () = assert!(core::mem::offset_of!(DmaFenceArrayCb, array) == 24);
const _: () = assert!(core::mem::size_of::<DmaFenceChain>() == 128);
const _: () = assert!(core::mem::offset_of!(DmaFenceChain, prev) == 64);
const _: () = assert!(core::mem::offset_of!(DmaFenceChain, prev_seqno) == 72);
const _: () = assert!(core::mem::offset_of!(DmaFenceChain, fence) == 80);
const _: () = assert!(core::mem::offset_of!(DmaFenceChain, lock) == 120);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init64_initializes_a_real_ring_timeline_fence() {
        let ops = DmaFenceOps {
            use_64bit_seqno: true,
            get_driver_name: None,
            get_timeline_name: None,
            enable_signaling: None,
            signaled: None,
            wait: None,
            release: None,
            fence_value_str: None,
            timeline_value_str: None,
            set_deadline: None,
        };
        let mut lock = 0u32;
        let mut fence = DmaFence {
            lock: core::ptr::null_mut(),
            ops: core::ptr::null(),
            cb_list: ListHead {
                next: core::ptr::null_mut(),
                prev: core::ptr::null_mut(),
            },
            context: 0,
            seqno: 0,
            flags: usize::MAX,
            refcount: 0,
            error: -1,
        };

        unsafe {
            dma_fence_init64(
                &mut fence,
                &ops,
                &mut lock as *mut u32 as *mut u8,
                0xfeed_cafe,
                0x1_0000_0001,
            );
        }

        assert_eq!(fence.ops, &ops as *const DmaFenceOps);
        assert_eq!(fence.lock, &mut lock as *mut u32 as *mut u8);
        assert_eq!(fence.context, 0xfeed_cafe);
        assert_eq!(fence.seqno, 0x1_0000_0001);
        assert_eq!(fence.flags, 0);
        assert_eq!(fence.refcount, 1);
        assert_eq!(fence.error, 0);
        let list = &mut fence.cb_list as *mut ListHead;
        assert_eq!(fence.cb_list.next, list);
        assert_eq!(fence.cb_list.prev, list);
    }

    static NULL_OPS: DmaFenceOps = DmaFenceOps {
        use_64bit_seqno: false,
        get_driver_name: None,
        get_timeline_name: None,
        enable_signaling: None,
        signaled: None,
        wait: None,
        release: None,
        fence_value_str: None,
        timeline_value_str: None,
        set_deadline: None,
    };

    fn uninit_fence() -> DmaFence {
        DmaFence {
            lock: core::ptr::null_mut(),
            ops: core::ptr::null(),
            cb_list: ListHead {
                next: core::ptr::null_mut(),
                prev: core::ptr::null_mut(),
            },
            context: 0,
            seqno: 0,
            flags: 0,
            refcount: 0,
            error: 0,
        }
    }

    #[test]
    fn array_first_and_next_walk_children_and_pass_through_non_arrays() {
        let mut lock = 0u32;
        let lock_ptr = &mut lock as *mut u32 as *mut u8;
        let mut child0 = uninit_fence();
        let mut child1 = uninit_fence();
        unsafe {
            dma_fence_init(&mut child0, &NULL_OPS, lock_ptr, 1, 1);
            dma_fence_init(&mut child1, &NULL_OPS, lock_ptr, 1, 2);
        }

        // A plain (non-array) fence is returned as-is by `first`; `next` ends it.
        unsafe {
            assert_eq!(
                dma_fence_array_first(&mut child0),
                &mut child0 as *mut DmaFence,
                "a non-array fence is its own first element"
            );
            assert!(
                dma_fence_array_next(&mut child0, 0).is_null(),
                "a non-array fence has no next element"
            );
            assert!(dma_fence_array_first(core::ptr::null_mut()).is_null());
            assert!(dma_fence_array_next(core::ptr::null_mut(), 0).is_null());
        }

        let mut children: [*mut DmaFence; 2] = [&mut child0, &mut child1];
        let arr = unsafe { dma_fence_array_create(2, children.as_mut_ptr(), 7, 1, false) };
        assert!(!arr.is_null(), "array create must succeed");
        let head = arr as *mut DmaFence;
        unsafe {
            assert_eq!(
                dma_fence_array_first(head),
                &mut child0 as *mut DmaFence,
                "first walks to child 0"
            );
            assert_eq!(
                dma_fence_array_next(head, 1),
                &mut child1 as *mut DmaFence,
                "next(1) walks to child 1"
            );
            assert!(
                dma_fence_array_next(head, 2).is_null(),
                "iteration past num_fences is NULL"
            );
        }
        // Intentionally leak `arr`: `dma_fence_put` would run the array release
        // path over the stack-owned children. The heap block is reclaimed when
        // the test process exits.
        let _ = arr;
    }

    #[test]
    fn private_stub_is_signaled_and_carries_its_timestamp() {
        let ts: i64 = 0x0123_4567_89ab_cdef;
        let f = unsafe { dma_fence_allocate_private_stub(ts) };
        assert!(!f.is_null(), "private stub allocation must succeed");
        unsafe {
            assert!(is_signaled(f), "a private stub is pre-signaled");
            assert_ne!((*f).flags & (1 << 1), 0, "TIMESTAMP flag bit is set");
            assert_ne!((*f).flags & (1 << 2), 0, "ENABLE_SIGNAL flag bit is set");
            let stored = core::ptr::read_unaligned(core::ptr::addr_of!((*f).cb_list).cast::<i64>());
            assert_eq!(stored, ts, "timestamp is preserved in the union slot");
            // The private release frees the heap fence — clean teardown.
            dma_fence_put(f);
        }
    }
}
