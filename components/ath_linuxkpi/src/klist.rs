//! `struct klist` / `struct klist_node` — the lock-protected, ref-counted list
//! the Linux device model is built on.
//!
//! `drivers/base/*` threads every `struct device` onto its parent bus/class via a
//! `klist`: `bus_type.p->klist_devices`, `class.p->klist_devices`, and the
//! driver-bound device list all use it. The contract that makes it more than a
//! plain `list_head` is **safe concurrent iteration under deletion**: a
//! `klist_iter` pins each node it visits with a reference (`kref`), so another
//! thread may `klist_del` a node mid-walk and the storage stays alive until the
//! iterator releases it (`klist_iter_exit`) — only then does the deferred free
//! run. A driver that links this shim and walks its bus's device list
//! (`bus_for_each_dev` → `klist_iter_init` / `klist_next` / `klist_iter_exit`)
//! must get exactly those refcount-pin semantics or it dereferences freed
//! devices during hot-unplug.
//!
//! **Layout is the contract.** Drivers reach `n_node`/`n_ref` by offset through
//! inlined `container_of`, so the structs match Linux field-for-field:
//! `struct klist { spinlock_t k_lock; struct list_head k_list; get; put; }` and
//! `struct klist_node { void *n_klist; struct list_head n_node; struct kref n_ref; }`.
//! The low bit of `n_klist` is Linux's "dead" flag (`KNODE_DEAD`); we honour it
//! so a node deleted while pinned is skipped by the iterator but kept alive. The
//! `const _: () = assert!` checks at the bottom FAIL THE BUILD if the layout
//! drifts.
//!
//! Cooperative-daemon model: the `k_lock` is a real atomic word (shared
//! `sync::acquire`/`release`), so even a future preempting daemon thread is
//! serialised. `get`/`put` are the driver's own callbacks (e.g. `get_device` /
//! `put_device`); when absent we still maintain `n_ref` so the pin/defer contract
//! holds for drivers that rely on it without custom callbacks.

use crate::dma_fence::ListHead;
use crate::{list, refcount, sync};
use core::ptr;

/// Per-node callback: `void (*)(struct klist_node *)` (e.g. `get_device`).
pub type KlistNodeFn = extern "C" fn(*mut KlistNode);

/// Linux `struct klist`.
///
/// `k_lock` is the first word of a `spinlock_t` (we use one `u32` of mutual
/// exclusion, same as `sync.rs`); the trailing padding keeps `k_list` at the
/// real `spinlock_t` offset so a driver's `container_of(node->n_klist, ...)`
/// math lines up on the targets we build for.
#[repr(C)]
pub struct Klist {
    /// `spinlock_t k_lock` — we touch only the first word atomically.
    pub k_lock: u32,
    _lock_pad: u32,
    /// `struct list_head k_list`.
    pub k_list: ListHead,
    /// `void (*get)(struct klist_node *)`.
    pub get: Option<KlistNodeFn>,
    /// `void (*put)(struct klist_node *)`.
    pub put: Option<KlistNodeFn>,
}

/// Linux `struct klist_node`.
///
/// `n_klist` carries the owning `klist *` with its **low bit** as the dead flag
/// (`KNODE_DEAD`), exactly like Linux's `knode_set_klist`/`knode_dead`.
#[repr(C)]
pub struct KlistNode {
    /// `void *n_klist` — owning klist pointer, low bit = KNODE_DEAD.
    pub n_klist: *mut u8,
    /// `struct list_head n_node`.
    pub n_node: ListHead,
    /// `struct kref n_ref` — a single refcount word (kref aliases refcount_t).
    pub n_ref: i32,
}

const KNODE_DEAD: usize = 1;

#[inline]
fn knode_klist(n: *mut KlistNode) -> *mut Klist {
    (unsafe { ((*n).n_klist as usize) & !KNODE_DEAD }) as *mut Klist
}

#[inline]
fn knode_dead(n: *mut KlistNode) -> bool {
    unsafe { ((*n).n_klist as usize) & KNODE_DEAD != 0 }
}

#[inline]
fn knode_set(n: *mut KlistNode, k: *mut Klist, dead: bool) {
    let bits = (k as usize) | if dead { KNODE_DEAD } else { 0 };
    unsafe { (*n).n_klist = bits as *mut u8 };
}

/// `klist_init(k, get, put)` — empty list + the device-model get/put callbacks.
#[no_mangle]
pub extern "C" fn klist_init(k: *mut Klist, get: Option<KlistNodeFn>, put: Option<KlistNodeFn>) {
    if k.is_null() {
        return;
    }
    unsafe {
        (*k).k_lock = 0;
        (*k)._lock_pad = 0;
        (*k).get = get;
        (*k).put = put;
    }
    list::INIT_LIST_HEAD(unsafe { ptr::addr_of_mut!((*k).k_list) });
}

/// Internal: take a reference (kref +1) and run the driver `get` callback.
unsafe fn klist_node_init(k: *mut Klist, n: *mut KlistNode) {
    list::INIT_LIST_HEAD(ptr::addr_of_mut!((*n).n_node));
    knode_set(n, k, false);
    refcount::kref_init(ptr::addr_of_mut!((*n).n_ref)); // count = 1 (on-list ref)
    if let Some(g) = (*k).get {
        g(n);
    }
}

/// `klist_add_tail(n, k)` — append `n` (FIFO), pinned with its on-list reference.
#[no_mangle]
pub extern "C" fn klist_add_tail(n: *mut KlistNode, k: *mut Klist) {
    if n.is_null() || k.is_null() {
        return;
    }
    unsafe {
        klist_node_init(k, n);
        sync::acquire(ptr::addr_of_mut!((*k).k_lock));
        list::list_add_tail(
            ptr::addr_of_mut!((*n).n_node),
            ptr::addr_of_mut!((*k).k_list),
        );
        sync::release(ptr::addr_of_mut!((*k).k_lock));
    }
}

/// `klist_add_head(n, k)` — prepend `n` (LIFO), pinned with its on-list reference.
#[no_mangle]
pub extern "C" fn klist_add_head(n: *mut KlistNode, k: *mut Klist) {
    if n.is_null() || k.is_null() {
        return;
    }
    unsafe {
        klist_node_init(k, n);
        sync::acquire(ptr::addr_of_mut!((*k).k_lock));
        list::list_add(
            ptr::addr_of_mut!((*n).n_node),
            ptr::addr_of_mut!((*k).k_list),
        );
        sync::release(ptr::addr_of_mut!((*k).k_lock));
    }
}

/// Internal: drop the on-list reference; when it hits 0 run the driver `put`.
/// This is the deferred free — while an iterator still pins the node, the kref
/// is >0 so the storage survives until the iterator releases it.
unsafe fn klist_release(n: *mut KlistNode) {
    let k = knode_klist(n);
    let put = if k.is_null() { None } else { (*k).put };
    if refcount::refcount_dec_and_test(ptr::addr_of_mut!((*n).n_ref)) {
        // last reference: node is fully unlinked + dead; hand back to the driver.
        if let Some(p) = put {
            p(n);
        }
    }
}

/// `klist_del(n)` — unlink `n` and drop its on-list reference. If an iterator
/// still holds a reference, the node is marked dead but kept alive until that
/// iterator's `klist_next`/`klist_iter_exit` releases the last reference.
#[no_mangle]
pub extern "C" fn klist_del(n: *mut KlistNode) {
    if n.is_null() {
        return;
    }
    let k = knode_klist(n);
    if k.is_null() {
        return;
    }
    unsafe {
        sync::acquire(ptr::addr_of_mut!((*k).k_lock));
        knode_set(n, k, true); // KNODE_DEAD: hidden from new iterators
                               // Unlink RCU-style: rewire neighbours around `n` but leave `n.n_node.next`
                               // intact. A `klist_iter` parked ON `n` (it holds a pin, so `n` is still
                               // alive) advances by following `n.n_node.next` in `klist_next`; if we
                               // poisoned it to null the parked iterator would walk off a cliff. Linux's
                               // klist relies on exactly this — the dead-but-pinned node stays
                               // forward-traversable until the last pin drops.
        let node = ptr::addr_of_mut!((*n).n_node);
        let prev = (*node).prev;
        let next = (*node).next;
        (*next).prev = prev;
        (*prev).next = next; // n.n_node.next deliberately left pointing at `next`
        sync::release(ptr::addr_of_mut!((*k).k_lock));
        klist_release(n); // drop the on-list reference
    }
}

/// `klist_remove(n)` — `klist_del` then wait for all references to drain. In the
/// cooperative single-threaded daemon there is no other waiter, so once
/// `klist_del` returns the node is fully released; this is the synchronous form
/// used by `device_del`.
#[no_mangle]
pub extern "C" fn klist_remove(n: *mut KlistNode) {
    klist_del(n);
}

/// `klist_node_attached(n)` → 1 if `n` is currently on a list (not dead).
#[no_mangle]
pub extern "C" fn klist_node_attached(n: *mut KlistNode) -> i32 {
    if n.is_null() {
        return 0;
    }
    (!knode_klist(n).is_null() && !knode_dead(n)) as i32
}

// ── klist_iter — refcount-pinned, deletion-safe iteration ────────────────────

/// Linux `struct klist_iter { struct klist *i_klist; struct klist_node *i_cur; }`.
#[repr(C)]
pub struct KlistIter {
    pub i_klist: *mut Klist,
    /// Currently-pinned node (holds one reference), or null.
    pub i_cur: *mut KlistNode,
}

/// `klist_iter_init(k, iter)` — start before the first node.
#[no_mangle]
pub extern "C" fn klist_iter_init(k: *mut Klist, iter: *mut KlistIter) {
    if iter.is_null() {
        return;
    }
    unsafe {
        (*iter).i_klist = k;
        (*iter).i_cur = ptr::null_mut();
    }
}

/// `klist_iter_init_node(k, iter, n)` — start positioned at `n` (which the caller
/// must already hold a reference on). We take an additional pin so the contract
/// is uniform with `klist_next`.
#[no_mangle]
pub extern "C" fn klist_iter_init_node(k: *mut Klist, iter: *mut KlistIter, n: *mut KlistNode) {
    if iter.is_null() {
        return;
    }
    unsafe {
        (*iter).i_klist = k;
        (*iter).i_cur = n;
        if !n.is_null() {
            refcount::kref_get(ptr::addr_of_mut!((*n).n_ref));
        }
    }
}

/// `klist_next(iter)` — advance to and pin the next live node, releasing the
/// previously-pinned one. Returns the next node, or null at end-of-list.
///
/// The pin is what makes concurrent `klist_del` safe: the returned node carries
/// a reference for the duration of the caller's use of it, and dead nodes
/// (already `klist_del`'d but kept alive by a pin) are skipped.
#[no_mangle]
pub extern "C" fn klist_next(iter: *mut KlistIter) -> *mut KlistNode {
    if iter.is_null() {
        return ptr::null_mut();
    }
    let k = unsafe { (*iter).i_klist };
    if k.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        sync::acquire(ptr::addr_of_mut!((*k).k_lock));
        let head = ptr::addr_of_mut!((*k).k_list) as *mut ListHead;
        // Starting point: node after the current cursor, or the list head's first.
        let mut cur_node_lh = if (*iter).i_cur.is_null() {
            (*head).next
        } else {
            (*ptr::addr_of_mut!((*(*iter).i_cur).n_node)).next
        };
        // Skip dead nodes; stop at the sentinel head.
        let mut next: *mut KlistNode = ptr::null_mut();
        while cur_node_lh != head {
            let cand = container_of_node(cur_node_lh);
            if !knode_dead(cand) {
                refcount::kref_get(ptr::addr_of_mut!((*cand).n_ref)); // pin it
                next = cand;
                break;
            }
            cur_node_lh = (*cur_node_lh).next;
        }
        sync::release(ptr::addr_of_mut!((*k).k_lock));

        // Release the previously-pinned node OUTSIDE the lock (its put callback /
        // deferred free must not run under k_lock).
        let prev = (*iter).i_cur;
        (*iter).i_cur = next;
        if !prev.is_null() {
            klist_put_iter_ref(prev);
        }
        next
    }
}

/// `klist_iter_exit(iter)` — release the final pin and clear the iterator.
#[no_mangle]
pub extern "C" fn klist_iter_exit(iter: *mut KlistIter) {
    if iter.is_null() {
        return;
    }
    unsafe {
        let cur = (*iter).i_cur;
        (*iter).i_cur = ptr::null_mut();
        (*iter).i_klist = ptr::null_mut();
        if !cur.is_null() {
            klist_put_iter_ref(cur);
        }
    }
}

/// Drop an iterator's pin. If this was the last reference (the node was already
/// `klist_del`'d), the deferred free runs now.
unsafe fn klist_put_iter_ref(n: *mut KlistNode) {
    klist_release(n);
}

/// `container_of(lh, struct klist_node, n_node)` — recover the node from its
/// embedded `list_head`. `n_node` is the second field (after `n_klist`, one
/// pointer), so subtract that offset.
#[inline]
fn container_of_node(lh: *mut ListHead) -> *mut KlistNode {
    let off = core::mem::offset_of!(KlistNode, n_node);
    ((lh as usize) - off) as *mut KlistNode
}

// ── Layout contract ──────────────────────────────────────────────────────────
// klist_node: n_klist (1 ptr), n_node (2 ptr), n_ref (1 word, padded to ptr).
const _: () = assert!(core::mem::offset_of!(KlistNode, n_klist) == 0);
const _: () = assert!(
    core::mem::offset_of!(KlistNode, n_node) == core::mem::size_of::<usize>(),
    "n_node must follow the n_klist pointer"
);
const _: () = assert!(core::mem::offset_of!(Klist, k_lock) == 0);
// k_list must sit one machine word past k_lock (the lock + its pad), matching the
// real spinlock_t footprint on the targets we build for.
const _: () = assert!(
    core::mem::offset_of!(Klist, k_list) == core::mem::size_of::<usize>(),
    "k_list must follow k_lock"
);
const _: () = assert!(core::mem::size_of::<KlistIter>() == 2 * core::mem::size_of::<usize>());

#[cfg(test)]
mod tests {
    //! Pure host KATs (no syscall path). Each assert is a concrete
    //! expected-vs-actual check and CAN FAIL.
    //!
    //! **Parallel-safety note:** the `put` callback is a C-ABI `extern "C" fn`
    //! pointer and so cannot capture a per-test counter. Pointing every test at
    //! one shared `static` counter made the suite flaky under the DEFAULT
    //! parallel `cargo test` runner — `store(0)` in one test would race the
    //! `fetch_add` from another test's deferred free. The fix: each test that
    //! tracks frees gets its OWN dedicated `static` counter + its OWN dedicated
    //! `extern "C" fn` put callback, so no two tests ever touch the same mutable
    //! word. No lock, no `std::`-ism, no shared state — race-free by isolation.
    use super::*;
    use core::sync::atomic::{AtomicI32, Ordering as O};

    // A test "device" embedding a klist_node, with a free-counter so we can
    // prove the deferred free fires exactly when the last reference drops.
    #[repr(C)]
    struct Dev {
        node: KlistNode,
        id: i32,
    }

    fn new_klist(put: Option<KlistNodeFn>) -> Klist {
        Klist {
            k_lock: 0,
            _lock_pad: 0,
            k_list: ListHead {
                next: ptr::null_mut(),
                prev: ptr::null_mut(),
            },
            get: None,
            put,
        }
    }
    fn new_dev(id: i32) -> Dev {
        Dev {
            node: KlistNode {
                n_klist: ptr::null_mut(),
                n_node: ListHead {
                    next: ptr::null_mut(),
                    prev: ptr::null_mut(),
                },
                n_ref: 0,
            },
            id,
        }
    }

    /// Walk a klist via the iterator into a fixed buffer of ids (no Vec).
    fn iter_ids(k: *mut Klist, out: &mut [i32]) -> usize {
        let mut it = KlistIter {
            i_klist: ptr::null_mut(),
            i_cur: ptr::null_mut(),
        };
        klist_iter_init(k, &mut it as *mut _);
        let mut n = 0usize;
        loop {
            let node = klist_next(&mut it as *mut _);
            if node.is_null() {
                break;
            }
            assert!(n < out.len(), "iterator overrun");
            // container_of: Dev.node is at offset 0, so node == &Dev.
            let dev = node as *mut Dev;
            out[n] = unsafe { (*dev).id };
            n += 1;
        }
        klist_iter_exit(&mut it as *mut _);
        n
    }

    // Per-test dedicated free-counters + put callbacks. Each test owns exactly
    // one of these — no cross-test sharing, so the DEFAULT parallel runner can
    // never cross-contaminate the counts.
    static PUT_FIFO: AtomicI32 = AtomicI32::new(0);
    extern "C" fn put_fifo(_n: *mut KlistNode) {
        PUT_FIFO.fetch_add(1, O::Release);
    }
    static PUT_DEL: AtomicI32 = AtomicI32::new(0);
    extern "C" fn put_del(_n: *mut KlistNode) {
        PUT_DEL.fetch_add(1, O::Release);
    }
    static PUT_PIN: AtomicI32 = AtomicI32::new(0);
    extern "C" fn put_pin(_n: *mut KlistNode) {
        PUT_PIN.fetch_add(1, O::Release);
    }

    #[test]
    fn klist_add_tail_is_fifo_add_head_is_lifo() {
        PUT_FIFO.store(0, O::Release);
        let mut k = new_klist(Some(put_fifo));
        let kp = &mut k as *mut _;
        klist_init(kp, None, Some(put_fifo));

        let mut a = new_dev(1);
        let mut b = new_dev(2);
        let mut c = new_dev(3);
        klist_add_tail(&mut a.node as *mut _, kp);
        klist_add_tail(&mut b.node as *mut _, kp);
        // a,b on list; each carries the on-list ref (count==1).
        assert_eq!(refcount::refcount_read(&mut a.node.n_ref as *mut _), 1);
        assert_eq!(klist_node_attached(&mut a.node as *mut _), 1);

        let mut ids = [0i32; 8];
        let n = iter_ids(kp, &mut ids);
        assert_eq!(&ids[..n], &[1, 2], "add_tail must be FIFO");

        // add_head prepends.
        klist_add_head(&mut c.node as *mut _, kp);
        let n = iter_ids(kp, &mut ids);
        assert_eq!(&ids[..n], &[3, 1, 2], "add_head must prepend");
    }

    #[test]
    fn klist_del_removes_and_frees_when_unpinned() {
        PUT_DEL.store(0, O::Release);
        let mut k = new_klist(Some(put_del));
        let kp = &mut k as *mut _;
        klist_init(kp, None, Some(put_del));
        let mut a = new_dev(1);
        let mut b = new_dev(2);
        klist_add_tail(&mut a.node as *mut _, kp);
        klist_add_tail(&mut b.node as *mut _, kp);

        // Delete b while no iterator pins it → put fires immediately (count 1→0).
        let before = PUT_DEL.load(O::Acquire);
        klist_del(&mut b.node as *mut _);
        let after = PUT_DEL.load(O::Acquire);
        assert_eq!(
            after - before,
            1,
            "unpinned klist_del must free immediately"
        );
        assert_eq!(
            klist_node_attached(&mut b.node as *mut _),
            0,
            "b must be detached"
        );

        let mut ids = [0i32; 8];
        let n = iter_ids(kp, &mut ids);
        assert_eq!(&ids[..n], &[1], "b must be gone from the walk");
    }

    #[test]
    fn iterator_pins_node_deferring_free_until_exit() {
        // THE device-model contract: a node deleted *during* iteration stays
        // alive (put deferred) until the iterator releases its pin.
        PUT_PIN.store(0, O::Release);
        let mut k = new_klist(Some(put_pin));
        let kp = &mut k as *mut _;
        klist_init(kp, None, Some(put_pin));
        let mut a = new_dev(10);
        let mut b = new_dev(20);
        klist_add_tail(&mut a.node as *mut _, kp);
        klist_add_tail(&mut b.node as *mut _, kp);

        let mut it = KlistIter {
            i_klist: ptr::null_mut(),
            i_cur: ptr::null_mut(),
        };
        klist_iter_init(kp, &mut it as *mut _);

        // Advance to a → pinned (on-list ref + iterator ref == 2).
        let n0 = klist_next(&mut it as *mut _);
        assert_eq!(n0 as *mut Dev as usize, &mut a as *mut Dev as usize);
        assert_eq!(
            refcount::refcount_read(&mut a.node.n_ref as *mut _),
            2,
            "iterator must pin"
        );

        // Concurrent delete of the pinned node a: drops on-list ref (2→1) but the
        // iterator's pin keeps it alive → put MUST NOT fire yet.
        klist_del(&mut a.node as *mut _);
        assert_eq!(
            refcount::refcount_read(&mut a.node.n_ref as *mut _),
            1,
            "pin must survive del"
        );
        assert_eq!(
            PUT_PIN.load(O::Acquire),
            0,
            "free must be deferred while pinned"
        );

        // Next advance: releases a's pin (1→0 → put fires) and pins b.
        let n1 = klist_next(&mut it as *mut _);
        assert_eq!(
            PUT_PIN.load(O::Acquire),
            1,
            "deferred free must run when pin released"
        );
        assert_eq!(
            n1 as *mut Dev as usize, &mut b as *mut Dev as usize,
            "del'd node skipped to b"
        );

        // Exit releases b's pin (its on-list ref remains since b not deleted).
        klist_iter_exit(&mut it as *mut _);
        assert_eq!(
            refcount::refcount_read(&mut b.node.n_ref as *mut _),
            1,
            "b keeps on-list ref"
        );
        assert_eq!(
            PUT_PIN.load(O::Acquire),
            1,
            "live node must not be freed on exit"
        );
    }
}
