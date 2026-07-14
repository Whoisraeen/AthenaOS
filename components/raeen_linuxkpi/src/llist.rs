//! `struct llist_head` / `struct llist_node` — the lock-less singly-linked list,
//! the GPU scheduler's free-job and pending-work conduit.
//!
//! Linux's llist is the one list you can push onto from any context (IRQ, NMI,
//! another CPU) with no lock, because `llist_add` is a single `cmpxchg` on the
//! head and `llist_del_all` is a single `xchg` that steals the whole chain. The
//! DRM GPU scheduler uses exactly this: completed jobs are pushed onto
//! `sched->pending_list`-adjacent llists from the fence-signal callback (which can
//! run in IRQ context) and the scheduler's worker `llist_del_all`s the batch to
//! free them off the hot path — `drm_sched_free_job_work` / the entity kill path.
//! amdgpu's deferred-free and TTM's delayed-delete walk llists the same way. A
//! driver that links this shim and does `llist_add(&job->node, &sched->free_list)`
//! from a signal callback, then `llist_del_all` in its worker, must find symbols
//! with the LIFO push / steal-all semantics or it leaks or double-frees jobs.
//!
//! **Cooperative single-thread daemon model — `cmpxchg` collapses to a swap.**
//! The RaeenOS LinuxKPI daemon is cooperatively scheduled: there is no preemption
//! inside `llist_add`, and no second CPU racing the head, so the lock-less
//! `cmpxchg` loop that Linux needs to defend against concurrent pushers reduces to
//! a plain read-modify-write of the head pointer. This is the same simplification
//! the `rcu`/`workqueue`/`irq` facades make: the symbol exists and is *behaviourally
//! identical* (same final chain, same LIFO order), the asynchrony it guards against
//! just cannot occur in this execution model. We keep a `compiler_fence(Release)`
//! at the publish point so the ordering is correct the day the model gains a real
//! second pusher. The host KATs walk the resulting chain by hand to prove order.
//!
//! **Layout is the contract.** `struct llist_node { struct llist_node *next; }`
//! and `struct llist_head { struct llist_node *first; }` are each a single
//! pointer, reached by offset through inlined `container_of` / `llist_entry` in
//! the driver. The `const _: () = assert!` checks at the bottom FAIL THE BUILD if
//! either drifts.

use core::ptr;
use core::sync::atomic::{compiler_fence, Ordering};

/// Linux `struct llist_node { struct llist_node *next; }`.
#[repr(C)]
pub struct LlistNode {
    pub next: *mut LlistNode,
}

/// Linux `struct llist_head { struct llist_node *first; }`.
#[repr(C)]
pub struct LlistHead {
    pub first: *mut LlistNode,
}

/// `init_llist_head(head)` — empty list (`first = NULL`).
#[no_mangle]
pub extern "C" fn init_llist_head(head: *mut LlistHead) {
    if !head.is_null() {
        unsafe { (*head).first = ptr::null_mut() };
    }
}

/// `llist_empty(head)` → 1 if the list has no nodes.
///
/// NOTE: in Linux this is only an *advisory* hint (a racing pusher can make it
/// stale the instant it returns); in the cooperative daemon it is exact.
#[no_mangle]
pub extern "C" fn llist_empty(head: *const LlistHead) -> i32 {
    if head.is_null() {
        return 1;
    }
    (unsafe { (*head).first }.is_null()) as i32
}

/// `llist_add(new, head)` → 1 if the list was previously empty (Linux uses this
/// return to decide whether to kick a worker), else 0. Pushes `new` to the FRONT
/// (LIFO) — the lock-less `cmpxchg` reduces to a swap here (see module docs).
#[no_mangle]
pub extern "C" fn llist_add(new: *mut LlistNode, head: *mut LlistHead) -> i32 {
    if new.is_null() || head.is_null() {
        return 0;
    }
    llist_add_batch(new, new, head)
}

/// `llist_add_batch(new_first, new_last, head)` → 1 if the list was previously
/// empty. Pushes a pre-linked chain `[new_first .. new_last]` onto the front in
/// one operation (`new_last->next` is set to the old head). The caller guarantees
/// `new_first` reaches `new_last` via `next`.
#[no_mangle]
pub extern "C" fn llist_add_batch(
    new_first: *mut LlistNode,
    new_last: *mut LlistNode,
    head: *mut LlistHead,
) -> i32 {
    if new_first.is_null() || new_last.is_null() || head.is_null() {
        return 0;
    }
    unsafe {
        let old = (*head).first;
        (*new_last).next = old;
        compiler_fence(Ordering::Release); // cmpxchg publish point
        (*head).first = new_first;
        old.is_null() as i32
    }
}

/// `llist_del_first(head)` → the first node (NULL if empty), unlinking only it.
/// In Linux this is the *only* unsafe-against-concurrent-`del_first` op (it has
/// the ABA hazard), but with a single consumer it is a plain pop.
#[no_mangle]
pub extern "C" fn llist_del_first(head: *mut LlistHead) -> *mut LlistNode {
    if head.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let first = (*head).first;
        if first.is_null() {
            return ptr::null_mut();
        }
        (*head).first = (*first).next;
        (*first).next = ptr::null_mut(); // poison: detached node
        first
    }
}

/// `llist_del_all(head)` → the entire chain (NULL if empty); the list is left
/// empty. The returned chain is walkable via `next` until NULL — this is the
/// scheduler's "steal the whole batch" primitive.
#[no_mangle]
pub extern "C" fn llist_del_all(head: *mut LlistHead) -> *mut LlistNode {
    if head.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let first = (*head).first;
        (*head).first = ptr::null_mut();
        first
    }
}

/// `llist_reverse_order(head)` — reverse a detached chain (returns the new head).
/// Workers often `llist_del_all` (which yields LIFO) then reverse to process in
/// FIFO/insertion order. NULL-safe.
#[no_mangle]
pub extern "C" fn llist_reverse_order(head: *mut LlistNode) -> *mut LlistNode {
    let mut new_head: *mut LlistNode = ptr::null_mut();
    let mut cur = head;
    let mut guard = 0usize;
    while !cur.is_null() {
        unsafe {
            let next = (*cur).next;
            (*cur).next = new_head;
            new_head = cur;
            cur = next;
        }
        guard += 1;
        if guard > 1_000_000 {
            break; // cycle guard — a corrupt chain must not hang the daemon
        }
    }
    new_head
}

/// `llist_next(node)` → the next node in a detached chain (NULL at the end).
/// The walk primitive behind `llist_for_each` / `llist_for_each_safe` (whose
/// macros expand at the driver call site against this `next` field).
#[no_mangle]
pub extern "C" fn llist_next(node: *const LlistNode) -> *mut LlistNode {
    if node.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*node).next }
}

// ── Layout contract: each is exactly one pointer wide ─────────────────────────
const _: () = assert!(core::mem::size_of::<LlistNode>() == core::mem::size_of::<usize>());
const _: () = assert!(core::mem::size_of::<LlistHead>() == core::mem::size_of::<usize>());

#[cfg(test)]
mod tests {
    //! Pure host KATs (no syscall path — safe on Windows-native per project
    //! memory). Every assert can FAIL: concrete expected-vs-actual over a real
    //! spliced chain.
    use super::*;

    fn node() -> LlistNode {
        LlistNode {
            next: core::ptr::null_mut(),
        }
    }

    /// Walk a detached chain (from a node ptr) into a fixed buffer of addresses.
    fn walk(mut cur: *mut LlistNode) -> ([usize; 16], usize) {
        let mut buf = [0usize; 16];
        let mut n = 0usize;
        while !cur.is_null() {
            assert!(n < buf.len(), "cycle/overrun in llist walk");
            buf[n] = cur as usize;
            n += 1;
            cur = unsafe { (*cur).next };
        }
        (buf, n)
    }

    #[test]
    fn add_is_lifo_and_empty_tracks_state() {
        let mut head = LlistHead {
            first: core::ptr::null_mut(),
        };
        let mut a = node();
        let mut b = node();
        let mut c = node();
        let hp = &mut head as *mut _;
        let (ap, bp, cp) = (&mut a as *mut _, &mut b as *mut _, &mut c as *mut _);
        init_llist_head(hp);
        assert_eq!(llist_empty(hp), 1, "fresh llist empty");

        // First add reports "was empty" (1); subsequent adds report 0.
        assert_eq!(llist_add(ap, hp), 1, "add to empty returns 1");
        assert_eq!(llist_add(bp, hp), 0, "add to non-empty returns 0");
        assert_eq!(llist_add(cp, hp), 0);
        assert_eq!(llist_empty(hp), 0);

        // LIFO: head -> c -> b -> a
        let (buf, n) = walk(head.first);
        assert_eq!(n, 3, "all three present");
        assert_eq!(
            &buf[..3],
            &[cp as usize, bp as usize, ap as usize],
            "llist_add is LIFO"
        );
    }

    #[test]
    fn add_batch_prepends_chain() {
        let mut head = LlistHead {
            first: core::ptr::null_mut(),
        };
        let mut existing = node();
        let mut b1 = node();
        let mut b2 = node();
        let hp = &mut head as *mut _;
        init_llist_head(hp);
        llist_add(&mut existing as *mut _, hp); // head -> existing

        // Pre-link a batch b1 -> b2, then add_batch(first=b1, last=b2).
        b1.next = &mut b2 as *mut _;
        let r = llist_add_batch(&mut b1 as *mut _, &mut b2 as *mut _, hp);
        assert_eq!(r, 0, "list was non-empty");
        // Expect head -> b1 -> b2 -> existing
        let (buf, n) = walk(head.first);
        assert_eq!(n, 3);
        assert_eq!(
            &buf[..3],
            &[
                &b1 as *const _ as usize,
                &b2 as *const _ as usize,
                &existing as *const _ as usize
            ],
            "add_batch prepends the whole chain in order"
        );
    }

    #[test]
    fn del_first_pops_lifo() {
        let mut head = LlistHead {
            first: core::ptr::null_mut(),
        };
        let mut a = node();
        let mut b = node();
        let hp = &mut head as *mut _;
        let (ap, bp) = (&mut a as *mut _, &mut b as *mut _);
        init_llist_head(hp);
        llist_add(ap, hp);
        llist_add(bp, hp); // head -> b -> a

        // del_first returns the most-recently-added (b), then a, then NULL.
        let f1 = llist_del_first(hp);
        assert_eq!(f1, bp, "del_first pops the LIFO front");
        assert!(b.next.is_null(), "popped node is detached/poisoned");
        let f2 = llist_del_first(hp);
        assert_eq!(f2, ap);
        assert!(llist_del_first(hp).is_null(), "empty del_first is NULL");
        assert_eq!(llist_empty(hp), 1, "list empty after draining");
    }

    #[test]
    fn del_all_steals_chain_and_empties() {
        let mut head = LlistHead {
            first: core::ptr::null_mut(),
        };
        let mut a = node();
        let mut b = node();
        let mut c = node();
        let hp = &mut head as *mut _;
        let (ap, bp, cp) = (&mut a as *mut _, &mut b as *mut _, &mut c as *mut _);
        init_llist_head(hp);
        llist_add(ap, hp);
        llist_add(bp, hp);
        llist_add(cp, hp); // head -> c -> b -> a

        let chain = llist_del_all(hp);
        assert_eq!(llist_empty(hp), 1, "del_all leaves the list empty");
        assert!(head.first.is_null());
        // Stolen chain is LIFO: c -> b -> a
        let (buf, n) = walk(chain);
        assert_eq!(n, 3, "del_all returns the whole chain");
        assert_eq!(&buf[..3], &[cp as usize, bp as usize, ap as usize]);

        // Reverse to insertion (FIFO) order: a -> b -> c
        let rev = llist_reverse_order(chain);
        let (rbuf, rn) = walk(rev);
        assert_eq!(rn, 3);
        assert_eq!(
            &rbuf[..3],
            &[ap as usize, bp as usize, cp as usize],
            "reverse_order yields FIFO/insertion order"
        );
    }

    #[test]
    fn del_all_on_empty_is_null() {
        let mut head = LlistHead {
            first: core::ptr::null_mut(),
        };
        let hp = &mut head as *mut _;
        init_llist_head(hp);
        assert!(llist_del_all(hp).is_null(), "del_all of empty is NULL");
        assert!(
            llist_next(core::ptr::null()).is_null(),
            "next(NULL) is NULL"
        );
    }
}
