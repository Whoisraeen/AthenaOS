//! RCU-flavoured `list_head` / `hlist` operations — the lock-free-reader list
//! variants the DRM and dma-fence paths lean on.
//!
//! Linux's RCU lists let readers walk a list with no lock while a writer
//! adds/deletes concurrently, by (a) publishing the new node with a
//! release-ordered pointer store (`rcu_assign_pointer`) so a reader can never
//! observe a half-initialised node, and (b) deferring the freed node's
//! reclamation past a grace period (`synchronize_rcu` / `call_rcu`) so a reader
//! mid-walk never touches freed memory. DRM uses this heavily:
//! `drm_for_each_connector_rcu`, the dma-resv fence arrays
//! (`dma_resv_for_each_fence` walks `rcu`-protected fence pointers), and the
//! GPU-scheduler entity lists all iterate under `rcu_read_lock()` so a
//! hot-path frame submit never blocks on the mode-config lock. A driver that
//! links this shim and uses `list_add_rcu` / `hlist_for_each_entry_rcu` must
//! find symbols with the same splice semantics as the non-RCU list, or its
//! lock-free walks corrupt.
//!
//! **Cooperative single-threaded daemon model.** The RaeenOS LinuxKPI daemon is
//! cooperatively scheduled — there is no preemption inside a list splice, and a
//! reader cannot run between a writer's two pointer stores. So the RCU machinery
//! collapses to its non-RCU equivalent: `rcu_read_lock`/`unlock` are no-ops,
//! `synchronize_rcu`/`rcu_barrier` drain immediately (there is no pending grace
//! period), and `list_add_rcu` is structurally the same splice as `list_add`
//! (we still order the stores so the layout is correct, but no reader can be
//! mid-walk to benefit). This mirrors the pump-driven `irq`/`workqueue` facades:
//! the symbols exist and behave correctly; the asynchrony they guard against
//! cannot occur in this execution model. The **walk order is identical** to the
//! non-RCU list, which the host KATs assert directly.
//!
//! Layout is reused verbatim from [`list`] — no fork: `list_*_rcu` operate on the
//! same [`ListHead`], `hlist_*_rcu` on the same [`HlistNode`]/[`HlistHead`].

use crate::dma_fence::ListHead;
use crate::list::{HlistHead, HlistNode};
use core::ptr;
use core::sync::atomic::{compiler_fence, Ordering};

// ── list_head RCU variants ────────────────────────────────────────────────────

/// Internal splice with a release barrier before publishing `new` to readers —
/// the body of `__list_add_rcu`. The `compiler_fence(Release)` is where
/// `rcu_assign_pointer` would put its memory barrier: `new`'s own fields and its
/// `prev`/`next` back-links are fully written before the predecessor's `next` is
/// flipped to point at `new`, so a lock-free reader either sees the old chain or
/// the fully-formed new node, never a torn one.
#[inline]
unsafe fn __list_add_rcu(new: *mut ListHead, prev: *mut ListHead, next: *mut ListHead) {
    (*new).next = next;
    (*new).prev = prev;
    compiler_fence(Ordering::Release); // rcu_assign_pointer publish point
    (*prev).next = new; // the store a reader can observe
    (*next).prev = new;
}

/// `list_add_rcu(new, head)` — insert after `head` (front), RCU-publish ordered.
#[no_mangle]
pub extern "C" fn list_add_rcu(new: *mut ListHead, head: *mut ListHead) {
    if new.is_null() || head.is_null() {
        return;
    }
    unsafe { __list_add_rcu(new, head, (*head).next) };
}

/// `list_add_tail_rcu(new, head)` — insert before `head` (tail), publish ordered.
#[no_mangle]
pub extern "C" fn list_add_tail_rcu(new: *mut ListHead, head: *mut ListHead) {
    if new.is_null() || head.is_null() {
        return;
    }
    unsafe { __list_add_rcu(new, (*head).prev, head) };
}

/// `list_del_rcu(entry)` — unlink for RCU: rewire neighbours but DO NOT poison
/// `entry->next` (a concurrent reader still walking through `entry` must be able
/// to reach the rest of the list via `entry->next`). `entry->prev` IS poisoned,
/// matching Linux (`LIST_POISON2`), because no forward reader follows `prev`.
/// The caller defers the actual free past `synchronize_rcu`.
#[no_mangle]
pub extern "C" fn list_del_rcu(entry: *mut ListHead) {
    if entry.is_null() {
        return;
    }
    unsafe {
        let prev = (*entry).prev;
        let next = (*entry).next;
        (*next).prev = prev;
        (*prev).next = next;
        // next left intact for any reader mid-traversal; prev poisoned.
        (*entry).prev = ptr::null_mut();
    }
}

/// `list_replace_rcu(old, new)` — atomically (publish-ordered) swap `old` for
/// `new`. `old->prev` poisoned; `old->next` left for in-flight readers.
#[no_mangle]
pub extern "C" fn list_replace_rcu(old: *mut ListHead, new: *mut ListHead) {
    if old.is_null() || new.is_null() {
        return;
    }
    unsafe {
        (*new).next = (*old).next;
        (*new).prev = (*old).prev;
        compiler_fence(Ordering::Release);
        (*(*new).prev).next = new;
        (*(*new).next).prev = new;
        (*old).prev = ptr::null_mut();
    }
}

// ── hlist RCU variants ────────────────────────────────────────────────────────

/// `hlist_add_head_rcu(n, h)` — push to front with publish ordering. The new
/// node's `next`/`pprev` are set before `h->first` is flipped to it.
#[no_mangle]
pub extern "C" fn hlist_add_head_rcu(n: *mut HlistNode, h: *mut HlistHead) {
    if n.is_null() || h.is_null() {
        return;
    }
    unsafe {
        let first = (*h).first;
        (*n).next = first;
        (*n).pprev = ptr::addr_of_mut!((*h).first);
        compiler_fence(Ordering::Release); // publish point
        (*h).first = n;
        if !first.is_null() {
            (*first).pprev = ptr::addr_of_mut!((*n).next);
        }
    }
}

/// `hlist_add_before_rcu(n, next)` — insert `n` ahead of `next`, publish ordered.
#[no_mangle]
pub extern "C" fn hlist_add_before_rcu(n: *mut HlistNode, next: *mut HlistNode) {
    if n.is_null() || next.is_null() {
        return;
    }
    unsafe {
        (*n).pprev = (*next).pprev;
        (*n).next = next;
        compiler_fence(Ordering::Release);
        *(*n).pprev = n;
        (*next).pprev = ptr::addr_of_mut!((*n).next);
    }
}

/// `hlist_add_behind_rcu(n, prev)` — insert `n` after `prev`, publish ordered.
#[no_mangle]
pub extern "C" fn hlist_add_behind_rcu(n: *mut HlistNode, prev: *mut HlistNode) {
    if n.is_null() || prev.is_null() {
        return;
    }
    unsafe {
        (*n).next = (*prev).next;
        (*n).pprev = ptr::addr_of_mut!((*prev).next);
        compiler_fence(Ordering::Release);
        (*prev).next = n;
        if !(*n).next.is_null() {
            (*(*n).next).pprev = ptr::addr_of_mut!((*n).next);
        }
    }
}

/// `hlist_del_rcu(n)` — unlink for RCU: rewire `pprev`'s target around `n` but
/// leave `n->next` for any reader mid-walk; `pprev` poisoned (Linux `LIST_POISON2`).
#[no_mangle]
pub extern "C" fn hlist_del_rcu(n: *mut HlistNode) {
    if n.is_null() || unsafe { (*n).pprev }.is_null() {
        return;
    }
    unsafe {
        let next = (*n).next;
        let pprev = (*n).pprev;
        *pprev = next;
        if !next.is_null() {
            (*next).pprev = pprev;
        }
        (*n).pprev = ptr::null_mut(); // next left intact for in-flight readers
    }
}

// ── RCU read-side + grace-period primitives (no-ops in the cooperative model) ──

/// `rcu_read_lock()` — no-op: the daemon cannot be preempted mid-walk, so there
/// is no read-side critical section to demarcate.
#[no_mangle]
pub extern "C" fn rcu_read_lock() {}

/// `rcu_read_unlock()` — no-op (see `rcu_read_lock`).
#[no_mangle]
pub extern "C" fn rcu_read_unlock() {}

/// `synchronize_rcu()` — wait for all pre-existing readers to finish. None can be
/// in flight in a cooperative single-threaded daemon, so the grace period is
/// already over; this is an immediate compiler-fence drain.
#[no_mangle]
pub extern "C" fn synchronize_rcu() {
    compiler_fence(Ordering::SeqCst);
}

/// `rcu_barrier()` — wait for outstanding `call_rcu` callbacks. We invoke
/// callbacks inline (see `call_rcu`), so nothing is pending; drain immediately.
#[no_mangle]
pub extern "C" fn rcu_barrier() {
    compiler_fence(Ordering::SeqCst);
}

/// `synchronize_rcu_expedited()` — same immediate drain.
#[no_mangle]
pub extern "C" fn synchronize_rcu_expedited() {
    compiler_fence(Ordering::SeqCst);
}

/// RCU callback signature: `void (*)(struct rcu_head *)`.
pub type RcuCallback = extern "C" fn(*mut u8);

/// `call_rcu(head, func)` — schedule `func(head)` after a grace period. Since no
/// reader can be mid-walk, the grace period is already satisfied: invoke `func`
/// immediately (still after this writer's mutations, matching the contract).
/// `head` is the driver's `struct rcu_head` storage; we pass it straight through.
#[no_mangle]
pub extern "C" fn call_rcu(head: *mut u8, func: Option<RcuCallback>) {
    compiler_fence(Ordering::SeqCst);
    if let Some(f) = func {
        f(head);
    }
}

#[cfg(test)]
mod tests {
    //! Pure host KATs (no syscall path). Walk order of the RCU list MUST match
    //! the non-RCU list; del leaves a forward-traversable chain; grace-period
    //! drains invoke deferred callbacks. Every assert can FAIL.
    use super::*;
    use crate::list;
    use core::sync::atomic::{AtomicI32, Ordering as O};

    fn lh() -> ListHead {
        ListHead {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }
    fn hn() -> HlistNode {
        HlistNode {
            next: ptr::null_mut(),
            pprev: ptr::null_mut(),
        }
    }

    struct Walk {
        buf: [usize; 16],
        len: usize,
    }
    fn collect(head: *mut ListHead) -> Walk {
        let mut w = Walk {
            buf: [0; 16],
            len: 0,
        };
        let mut cur = unsafe { (*head).next };
        while cur != head {
            assert!(w.len < w.buf.len(), "cycle in rcu list walk");
            w.buf[w.len] = cur as usize;
            w.len += 1;
            cur = unsafe { (*cur).next };
        }
        w
    }
    fn hcollect(head: *mut HlistHead) -> Walk {
        let mut w = Walk {
            buf: [0; 16],
            len: 0,
        };
        let mut cur = unsafe { (*head).first };
        while !cur.is_null() {
            assert!(w.len < w.buf.len(), "cycle in rcu hlist walk");
            w.buf[w.len] = cur as usize;
            w.len += 1;
            cur = unsafe { (*cur).next };
        }
        w
    }
    fn eq(w: &Walk, expect: &[usize]) -> bool {
        w.len == expect.len() && w.buf[..w.len] == *expect
    }

    #[test]
    fn rcu_list_walk_order_matches_nonrcu() {
        let mut head = lh();
        let mut a = lh();
        let mut b = lh();
        let mut c = lh();
        let hp = &mut head as *mut _;
        let (ap, bp, cp) = (&mut a as *mut _, &mut b as *mut _, &mut c as *mut _);
        list::INIT_LIST_HEAD(hp);

        // add_rcu = front (LIFO): head -> b -> a
        list_add_rcu(ap, hp);
        list_add_rcu(bp, hp);
        assert!(
            eq(&collect(hp), &[bp as usize, ap as usize]),
            "add_rcu must be LIFO like list_add"
        );

        // add_tail_rcu appends: head -> b -> a -> c
        list_add_tail_rcu(cp, hp);
        assert!(
            eq(&collect(hp), &[bp as usize, ap as usize, cp as usize]),
            "add_tail_rcu must append"
        );
    }

    #[test]
    fn rcu_del_keeps_forward_chain_for_readers() {
        let mut head = lh();
        let mut a = lh();
        let mut b = lh();
        let mut c = lh();
        let hp = &mut head as *mut _;
        let (ap, bp, cp) = (&mut a as *mut _, &mut b as *mut _, &mut c as *mut _);
        list::INIT_LIST_HEAD(hp);
        list_add_tail_rcu(ap, hp);
        list_add_tail_rcu(bp, hp);
        list_add_tail_rcu(cp, hp); // a, b, c

        // Delete the middle node b. The list (from head) becomes a, c ...
        list_del_rcu(bp);
        assert!(
            eq(&collect(hp), &[ap as usize, cp as usize]),
            "rcu del must unlink from the list"
        );

        // ...but a reader that had ALREADY loaded b must still reach c via
        // b->next (the RCU contract: next is NOT poisoned).
        assert_eq!(
            unsafe { (*bp).next },
            cp,
            "del_rcu must leave next intact for in-flight readers"
        );
        assert!(unsafe { (*bp).prev }.is_null(), "del_rcu must poison prev");
    }

    #[test]
    fn rcu_hlist_add_del_matches_nonrcu_order() {
        let mut h = HlistHead {
            first: ptr::null_mut(),
        };
        let mut a = hn();
        let mut b = hn();
        let mut c = hn();
        let hp = &mut h as *mut _;
        let (ap, bp, cp) = (&mut a as *mut _, &mut b as *mut _, &mut c as *mut _);
        list::INIT_HLIST_HEAD(hp);

        hlist_add_head_rcu(ap, hp);
        hlist_add_head_rcu(bp, hp); // h -> b -> a
        assert!(
            eq(&hcollect(hp), &[bp as usize, ap as usize]),
            "add_head_rcu order"
        );

        hlist_add_behind_rcu(cp, bp); // h -> b -> c -> a
        assert!(
            eq(&hcollect(hp), &[bp as usize, cp as usize, ap as usize]),
            "add_behind_rcu order"
        );

        // del middle (c) → b -> a; c->next left for in-flight readers.
        hlist_del_rcu(cp);
        assert!(
            eq(&hcollect(hp), &[bp as usize, ap as usize]),
            "hlist_del_rcu unlink"
        );
        assert_eq!(
            unsafe { (*cp).next },
            ap,
            "hlist_del_rcu leaves next for readers"
        );
        assert!(
            unsafe { (*cp).pprev }.is_null(),
            "hlist_del_rcu poisons pprev"
        );
    }

    #[test]
    fn rcu_grace_period_drains_and_call_rcu_fires() {
        static CB_CALLS: AtomicI32 = AtomicI32::new(0);
        extern "C" fn cb(_h: *mut u8) {
            CB_CALLS.fetch_add(1, O::Release);
        }
        CB_CALLS.store(0, O::Release);

        // Read-side lock/unlock are no-ops but must link + not deadlock.
        rcu_read_lock();
        rcu_read_unlock();

        // synchronize_rcu / rcu_barrier return (no pending grace period to hang on).
        synchronize_rcu();
        rcu_barrier();
        synchronize_rcu_expedited();

        // call_rcu invokes the callback (grace period already satisfied).
        let mut dummy = 0u8;
        call_rcu(&mut dummy as *mut _, Some(cb));
        assert_eq!(
            CB_CALLS.load(O::Acquire),
            1,
            "call_rcu must run the deferred callback"
        );

        // A null callback must be a safe no-op (not a crash).
        call_rcu(&mut dummy as *mut _, None);
        assert_eq!(
            CB_CALLS.load(O::Acquire),
            1,
            "null call_rcu callback must be a no-op"
        );
    }
}
