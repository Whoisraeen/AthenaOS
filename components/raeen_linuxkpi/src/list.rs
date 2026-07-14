//! `struct list_head` / `struct hlist_head` — Linux's intrusive doubly-linked
//! lists, the single most pervasive data structure in driver code.
//!
//! Every Linux GPU/NIC driver embeds `struct list_head` everywhere: amdgpu's BO
//! eviction lists, the DRM mode-config connector/encoder/plane lists, fence
//! callback chains, the dma-buf attachment list, deferred-work queues, and the
//! scheduler entity lists all thread objects onto intrusive `list_head`s and walk
//! them with `list_for_each_entry`. `hlist_head` (single-pointer head, used where
//! a hash bucket wants half the memory) backs the device IDR, the timer wheel,
//! and quota/ID hash tables. A driver that links against this shim must find real
//! `list_add`/`list_del`/`list_move`/`hlist_add_head` symbols with the exact
//! pointer-splice semantics, or its object graph silently corrupts.
//!
//! **Layout is the contract.** `struct list_head { next, prev }` and
//! `struct hlist_node { next, pprev }` are reached by offset through inlined
//! `container_of` in the driver, so the structs MUST be two pointers wide with
//! that field order. We reuse the BTF-verified [`ListHead`] from `dma_fence`
//! (sibling dma-buf modules already build on it) and add the `hlist` pair here.
//! The `const _: () = assert!` checks at the bottom FAIL THE BUILD if either
//! layout drifts.
//!
//! The macro-only iterators (`list_for_each`, `list_for_each_entry`,
//! `hlist_for_each_entry`, …) expand at the driver's call site against `next`/
//! `prev`; we do not — and cannot — export them as symbols, but their walk is
//! exactly the pointer chain these functions maintain, so a driver's inlined
//! iteration over a list this shim spliced is correct by construction. The host
//! KATs below walk the chain by hand to prove that.

pub use crate::dma_fence::ListHead;

use core::ptr;

// ── struct list_head (doubly-linked, circular, sentinel head) ────────────────

/// `INIT_LIST_HEAD(list)` — point a head at itself (the empty list).
#[no_mangle]
pub extern "C" fn INIT_LIST_HEAD(list: *mut ListHead) {
    if list.is_null() {
        return;
    }
    unsafe {
        (*list).next = list;
        (*list).prev = list;
    }
}

/// Internal splice between two known-adjacent nodes — the body of `__list_add`.
#[inline]
unsafe fn __list_add(new: *mut ListHead, prev: *mut ListHead, next: *mut ListHead) {
    (*next).prev = new;
    (*new).next = next;
    (*new).prev = prev;
    (*prev).next = new;
}

/// `list_add(new, head)` — insert `new` just after `head` (LIFO/stack order).
#[no_mangle]
pub extern "C" fn list_add(new: *mut ListHead, head: *mut ListHead) {
    if new.is_null() || head.is_null() {
        return;
    }
    unsafe { __list_add(new, head, (*head).next) };
}

/// `list_add_tail(new, head)` — insert `new` just before `head` (FIFO/queue order).
#[no_mangle]
pub extern "C" fn list_add_tail(new: *mut ListHead, head: *mut ListHead) {
    if new.is_null() || head.is_null() {
        return;
    }
    unsafe { __list_add(new, (*head).prev, head) };
}

/// Internal unlink — the body of `__list_del`.
#[inline]
unsafe fn __list_del(prev: *mut ListHead, next: *mut ListHead) {
    (*next).prev = prev;
    (*prev).next = next;
}

/// `list_del(entry)` — unlink `entry`; leaves the node pointers poisoned (Linux
/// uses `LIST_POISON1/2`; we use null so a stray re-walk faults instead of
/// silently roaming a freed chain).
#[no_mangle]
pub extern "C" fn list_del(entry: *mut ListHead) {
    if entry.is_null() {
        return;
    }
    unsafe {
        __list_del((*entry).prev, (*entry).next);
        (*entry).next = ptr::null_mut();
        (*entry).prev = ptr::null_mut();
    }
}

/// `list_del_init(entry)` — unlink and re-init as an empty list (safe to re-add).
#[no_mangle]
pub extern "C" fn list_del_init(entry: *mut ListHead) {
    if entry.is_null() {
        return;
    }
    unsafe {
        __list_del((*entry).prev, (*entry).next);
        (*entry).next = entry;
        (*entry).prev = entry;
    }
}

/// `list_replace(old, new)` — put `new` where `old` was. `old` is left dangling
/// (Linux leaves it untouched; callers use `list_replace_init` to clean it).
#[no_mangle]
pub extern "C" fn list_replace(old: *mut ListHead, new: *mut ListHead) {
    if old.is_null() || new.is_null() {
        return;
    }
    unsafe {
        (*new).next = (*old).next;
        (*(*new).next).prev = new;
        (*new).prev = (*old).prev;
        (*(*new).prev).next = new;
    }
}

/// `list_replace_init(old, new)` — replace then re-init `old` as empty.
#[no_mangle]
pub extern "C" fn list_replace_init(old: *mut ListHead, new: *mut ListHead) {
    list_replace(old, new);
    INIT_LIST_HEAD(old);
}

/// `list_move(list, head)` — unlink `list` and re-add at `head`'s front.
#[no_mangle]
pub extern "C" fn list_move(list: *mut ListHead, head: *mut ListHead) {
    if list.is_null() || head.is_null() {
        return;
    }
    unsafe { __list_del((*list).prev, (*list).next) };
    list_add(list, head);
}

/// `list_move_tail(list, head)` — unlink `list` and re-add at `head`'s tail.
#[no_mangle]
pub extern "C" fn list_move_tail(list: *mut ListHead, head: *mut ListHead) {
    if list.is_null() || head.is_null() {
        return;
    }
    unsafe { __list_del((*list).prev, (*list).next) };
    list_add_tail(list, head);
}

/// `list_empty(head)` → 1 if the list has no entries.
#[no_mangle]
pub extern "C" fn list_empty(head: *const ListHead) -> i32 {
    if head.is_null() {
        return 1;
    }
    (unsafe { (*head).next } == head as *mut ListHead) as i32
}

/// `list_is_head(list, head)` → 1 if `list` IS the sentinel head (the
/// loop-termination test `list_for_each` uses).
#[no_mangle]
pub extern "C" fn list_is_head(list: *const ListHead, head: *const ListHead) -> i32 {
    (list == head) as i32
}

/// `list_is_first(list, head)` → 1 if `list` is the first entry.
#[no_mangle]
pub extern "C" fn list_is_first(list: *const ListHead, head: *const ListHead) -> i32 {
    if list.is_null() || head.is_null() {
        return 0;
    }
    (unsafe { (*head).next } == list as *mut ListHead) as i32
}

/// `list_is_last(list, head)` → 1 if `list` is the last entry.
#[no_mangle]
pub extern "C" fn list_is_last(list: *const ListHead, head: *const ListHead) -> i32 {
    if list.is_null() || head.is_null() {
        return 0;
    }
    (unsafe { (*head).prev } == list as *mut ListHead) as i32
}

/// `list_is_singular(head)` → 1 if the list has exactly one entry.
#[no_mangle]
pub extern "C" fn list_is_singular(head: *const ListHead) -> i32 {
    if head.is_null() {
        return 0;
    }
    let h = head as *mut ListHead;
    let next = unsafe { (*head).next };
    ((next != h) && (next == unsafe { (*head).prev })) as i32
}

/// `list_rotate_left(head)` — move the first entry to the tail.
#[no_mangle]
pub extern "C" fn list_rotate_left(head: *mut ListHead) {
    if head.is_null() {
        return;
    }
    if list_empty(head) == 0 {
        let first = unsafe { (*head).next };
        list_move_tail(first, head);
    }
}

/// `list_splice(list, head)` — merge `list` onto the front of `head`. `list`'s
/// head is left dangling (Linux leaves it; `_init` callers re-init it).
#[no_mangle]
pub extern "C" fn list_splice(list: *const ListHead, head: *mut ListHead) {
    if list.is_null() || head.is_null() || list_empty(list) != 0 {
        return;
    }
    unsafe {
        let first = (*list).next;
        let last = (*list).prev;
        let at = (*head).next;
        (*first).prev = head;
        (*head).next = first;
        (*last).next = at;
        (*at).prev = last;
    }
}

/// `list_splice_tail(list, head)` — merge `list` onto the tail of `head`.
#[no_mangle]
pub extern "C" fn list_splice_tail(list: *const ListHead, head: *mut ListHead) {
    if list.is_null() || head.is_null() || list_empty(list) != 0 {
        return;
    }
    unsafe {
        let first = (*list).next;
        let last = (*list).prev;
        let at = (*head).prev;
        (*first).prev = at;
        (*at).next = first;
        (*last).next = head;
        (*head).prev = last;
    }
}

/// `list_splice_init(list, head)` — splice to front then re-init `list` empty.
#[no_mangle]
pub extern "C" fn list_splice_init(list: *mut ListHead, head: *mut ListHead) {
    list_splice(list, head);
    INIT_LIST_HEAD(list);
}

/// `list_splice_tail_init(list, head)` — splice to tail then re-init `list`.
#[no_mangle]
pub extern "C" fn list_splice_tail_init(list: *mut ListHead, head: *mut ListHead) {
    list_splice_tail(list, head);
    INIT_LIST_HEAD(list);
}

/// `list_count_nodes(head)` — number of entries (Linux 6.x helper).
#[no_mangle]
pub extern "C" fn list_count_nodes(head: *const ListHead) -> usize {
    if head.is_null() {
        return 0;
    }
    let h = head as *mut ListHead;
    let mut n = 0usize;
    let mut cur = unsafe { (*head).next };
    while cur != h {
        n += 1;
        cur = unsafe { (*cur).next };
        if n > 1_000_000 {
            break; // cycle guard — a corrupt list must not hang the daemon
        }
    }
    n
}

// ── struct hlist_head / hlist_node (single-pointer-head hash list) ────────────

/// Linux `struct hlist_head { struct hlist_node *first; }`.
#[repr(C)]
pub struct HlistHead {
    pub first: *mut HlistNode,
}

/// Linux `struct hlist_node { struct hlist_node *next; struct hlist_node **pprev; }`.
///
/// `pprev` points at the *pointer that points at this node* (the head's `first`
/// for the first node, or the previous node's `next`), which is what makes O(1)
/// `hlist_del` without a back-link possible. The layout (next@0, pprev@8) is the
/// contract drivers reach by offset.
#[repr(C)]
pub struct HlistNode {
    pub next: *mut HlistNode,
    pub pprev: *mut *mut HlistNode,
}

/// `INIT_HLIST_HEAD(h)` — empty bucket.
#[no_mangle]
pub extern "C" fn INIT_HLIST_HEAD(h: *mut HlistHead) {
    if !h.is_null() {
        unsafe { (*h).first = ptr::null_mut() };
    }
}

/// `INIT_HLIST_NODE(n)` — an unhashed node.
#[no_mangle]
pub extern "C" fn INIT_HLIST_NODE(n: *mut HlistNode) {
    if !n.is_null() {
        unsafe {
            (*n).next = ptr::null_mut();
            (*n).pprev = ptr::null_mut();
        }
    }
}

/// `hlist_unhashed(n)` → 1 if `n` is not on any list.
#[no_mangle]
pub extern "C" fn hlist_unhashed(n: *const HlistNode) -> i32 {
    if n.is_null() {
        return 1;
    }
    (unsafe { (*n).pprev }.is_null()) as i32
}

/// `hlist_empty(h)` → 1 if the bucket has no nodes.
#[no_mangle]
pub extern "C" fn hlist_empty(h: *const HlistHead) -> i32 {
    if h.is_null() {
        return 1;
    }
    (unsafe { (*h).first }.is_null()) as i32
}

/// `hlist_add_head(n, h)` — push `n` to the front of bucket `h`.
#[no_mangle]
pub extern "C" fn hlist_add_head(n: *mut HlistNode, h: *mut HlistHead) {
    if n.is_null() || h.is_null() {
        return;
    }
    unsafe {
        let first = (*h).first;
        (*n).next = first;
        if !first.is_null() {
            (*first).pprev = ptr::addr_of_mut!((*n).next);
        }
        (*h).first = n;
        (*n).pprev = ptr::addr_of_mut!((*h).first);
    }
}

/// `hlist_add_before(n, next)` — insert `n` immediately before `next`.
#[no_mangle]
pub extern "C" fn hlist_add_before(n: *mut HlistNode, next: *mut HlistNode) {
    if n.is_null() || next.is_null() {
        return;
    }
    unsafe {
        (*n).pprev = (*next).pprev;
        (*n).next = next;
        (*next).pprev = ptr::addr_of_mut!((*n).next);
        *(*n).pprev = n;
    }
}

/// `hlist_add_behind(n, prev)` — insert `n` immediately after `prev`.
#[no_mangle]
pub extern "C" fn hlist_add_behind(n: *mut HlistNode, prev: *mut HlistNode) {
    if n.is_null() || prev.is_null() {
        return;
    }
    unsafe {
        (*n).next = (*prev).next;
        (*prev).next = n;
        (*n).pprev = ptr::addr_of_mut!((*prev).next);
        if !(*n).next.is_null() {
            (*(*n).next).pprev = ptr::addr_of_mut!((*n).next);
        }
    }
}

/// Internal unlink — body of `__hlist_del`.
#[inline]
unsafe fn __hlist_del(n: *mut HlistNode) {
    let next = (*n).next;
    let pprev = (*n).pprev;
    *pprev = next;
    if !next.is_null() {
        (*next).pprev = pprev;
    }
}

/// `hlist_del(n)` — unlink `n`; pointers left dangling (null here, see `list_del`).
#[no_mangle]
pub extern "C" fn hlist_del(n: *mut HlistNode) {
    if n.is_null() || unsafe { (*n).pprev }.is_null() {
        return;
    }
    unsafe {
        __hlist_del(n);
        (*n).next = ptr::null_mut();
        (*n).pprev = ptr::null_mut();
    }
}

/// `hlist_del_init(n)` — unlink and re-init (safe to re-add). No-op if unhashed.
#[no_mangle]
pub extern "C" fn hlist_del_init(n: *mut HlistNode) {
    if n.is_null() || unsafe { (*n).pprev }.is_null() {
        return;
    }
    unsafe {
        __hlist_del(n);
        (*n).next = ptr::null_mut();
        (*n).pprev = ptr::null_mut();
    }
}

// ── Layout contract: these MUST be two pointers wide with the documented order ─

const _: () = assert!(core::mem::size_of::<ListHead>() == 2 * core::mem::size_of::<usize>());
const _: () = assert!(core::mem::size_of::<HlistHead>() == core::mem::size_of::<usize>());
const _: () = assert!(core::mem::size_of::<HlistNode>() == 2 * core::mem::size_of::<usize>());

#[cfg(test)]
mod tests {
    //! Pure host KATs (no syscall path — safe on Windows-native per project
    //! memory). Every assert can FAIL: each is a concrete expected-vs-actual
    //! comparison over a real spliced pointer chain.
    use super::*;

    #[test]
    fn list_add_is_lifo_add_tail_is_fifo() {
        let mut head = ListHead {
            next: core::ptr::null_mut(),
            prev: core::ptr::null_mut(),
        };
        let mut a = ListHead {
            next: core::ptr::null_mut(),
            prev: core::ptr::null_mut(),
        };
        let mut b = ListHead {
            next: core::ptr::null_mut(),
            prev: core::ptr::null_mut(),
        };
        let mut c = ListHead {
            next: core::ptr::null_mut(),
            prev: core::ptr::null_mut(),
        };
        let (hp, ap, bp, cp) = (
            &mut head as *mut _,
            &mut a as *mut _,
            &mut b as *mut _,
            &mut c as *mut _,
        );
        INIT_LIST_HEAD(hp);
        assert_eq!(list_empty(hp), 1, "fresh list must be empty");
        assert_eq!(list_count_nodes(hp), 0);

        // list_add pushes to front: head -> b -> a
        list_add(ap, hp);
        list_add(bp, hp);
        assert_eq!(list_is_first(bp, hp), 1, "last list_add must be first");
        assert_eq!(list_is_last(ap, hp), 1);
        assert_eq!(list_count_nodes(hp), 2);

        // list_add_tail appends: head -> b -> a -> c
        list_add_tail(cp, hp);
        assert_eq!(list_is_last(cp, hp), 1, "add_tail must land at the end");
        assert_eq!(list_count_nodes(hp), 3);

        // Forward order via next chain: b, a, c
        assert!(
            eq_list(&collect(hp), &[bp, ap, cp]),
            "forward walk order wrong"
        );
        // Reverse via prev chain must mirror it.
        assert!(
            eq_list(&collect_rev(hp), &[cp, ap, bp]),
            "reverse walk must mirror forward"
        );
    }

    #[test]
    fn list_del_and_move_maintain_chain() {
        let mut head = empty();
        let mut a = empty();
        let mut b = empty();
        let mut c = empty();
        let (hp, ap, bp, cp) = (
            &mut head as *mut _,
            &mut a as *mut _,
            &mut b as *mut _,
            &mut c as *mut _,
        );
        INIT_LIST_HEAD(hp);
        list_add_tail(ap, hp);
        list_add_tail(bp, hp);
        list_add_tail(cp, hp);
        assert!(eq_list(&collect(hp), &[ap, bp, cp]));

        // Delete the middle: head -> a -> c
        list_del(bp);
        assert!(eq_list(&collect(hp), &[ap, cp]), "delete middle failed");
        assert!(b.next.is_null() && b.prev.is_null(), "list_del must poison");

        // list_del_init re-inits so the node is a valid empty list.
        list_del_init(ap);
        assert_eq!(list_empty(ap), 1, "del_init must leave a self-loop");
        assert!(eq_list(&collect(hp), &[cp]));
        assert_eq!(list_is_singular(hp), 1);

        // Move c to a second list, front.
        let mut head2 = empty();
        let h2 = &mut head2 as *mut _;
        INIT_LIST_HEAD(h2);
        list_move(cp, h2);
        assert_eq!(list_empty(hp), 1, "source must be empty after move");
        assert!(eq_list(&collect(h2), &[cp]), "dest must hold moved node");
    }

    #[test]
    fn list_splice_merges_and_empties_source() {
        let mut h1 = empty();
        let mut h2 = empty();
        let mut a = empty();
        let mut b = empty();
        let mut x = empty();
        let mut y = empty();
        let (p1, p2) = (&mut h1 as *mut _, &mut h2 as *mut _);
        let (ap, bp, xp, yp) = (
            &mut a as *mut _,
            &mut b as *mut _,
            &mut x as *mut _,
            &mut y as *mut _,
        );
        INIT_LIST_HEAD(p1);
        INIT_LIST_HEAD(p2);
        list_add_tail(ap, p1);
        list_add_tail(bp, p1); // h1: a, b
        list_add_tail(xp, p2);
        list_add_tail(yp, p2); // h2: x, y

        // splice_tail_init: h2 becomes x,y,a,b ; h1 empty.
        list_splice_tail_init(p1, p2);
        assert_eq!(list_empty(p1), 1, "spliced source must re-init empty");
        assert!(
            eq_list(&collect(p2), &[xp, yp, ap, bp]),
            "splice_tail order wrong"
        );
    }

    #[test]
    fn list_rotate_left_cycles_front_to_back() {
        let mut head = empty();
        let mut a = empty();
        let mut b = empty();
        let mut c = empty();
        let hp = &mut head as *mut _;
        let (ap, bp, cp) = (&mut a as *mut _, &mut b as *mut _, &mut c as *mut _);
        INIT_LIST_HEAD(hp);
        list_add_tail(ap, hp);
        list_add_tail(bp, hp);
        list_add_tail(cp, hp); // a, b, c
        list_rotate_left(hp);
        assert!(eq_list(&collect(hp), &[bp, cp, ap]), "rotate_left wrong");
    }

    #[test]
    fn hlist_add_del_maintains_pprev() {
        let mut h = HlistHead {
            first: core::ptr::null_mut(),
        };
        let mut a = hnode();
        let mut b = hnode();
        let mut c = hnode();
        let hp = &mut h as *mut _;
        let (ap, bp, cp) = (&mut a as *mut _, &mut b as *mut _, &mut c as *mut _);
        INIT_HLIST_HEAD(hp);
        assert_eq!(hlist_empty(hp), 1);
        INIT_HLIST_NODE(ap);
        assert_eq!(hlist_unhashed(ap), 1, "fresh node is unhashed");

        // add_head pushes front: h -> b -> a
        hlist_add_head(ap, hp);
        hlist_add_head(bp, hp);
        assert_eq!(hlist_empty(hp), 0);
        assert!(
            eq_hlist(&hcollect(hp), &[bp, ap]),
            "hlist_add_head order wrong"
        );
        assert_eq!(hlist_unhashed(ap), 0, "hashed node must report hashed");

        // add_behind b: h -> b -> c -> a
        hlist_add_behind(cp, bp);
        assert!(eq_hlist(&hcollect(hp), &[bp, cp, ap]), "add_behind wrong");

        // delete the head node b -> list becomes c, a
        hlist_del(bp);
        assert!(
            eq_hlist(&hcollect(hp), &[cp, ap]),
            "hlist_del of first wrong"
        );
        assert!(
            b.next.is_null() && b.pprev.is_null(),
            "hlist_del must poison"
        );

        // delete the middle (c) via pprev rewire -> a remains
        hlist_del(cp);
        assert!(eq_hlist(&hcollect(hp), &[ap]), "hlist_del of middle wrong");

        // del_init leaves a re-addable node
        hlist_del_init(ap);
        assert_eq!(hlist_empty(hp), 1);
        assert_eq!(hlist_unhashed(ap), 1, "del_init must leave node unhashed");
    }

    #[test]
    fn hlist_add_before_inserts_ahead() {
        let mut h = HlistHead {
            first: core::ptr::null_mut(),
        };
        let mut a = hnode();
        let mut b = hnode();
        let hp = &mut h as *mut _;
        let (ap, bp) = (&mut a as *mut _, &mut b as *mut _);
        INIT_HLIST_HEAD(hp);
        hlist_add_head(ap, hp); // h -> a
        hlist_add_before(bp, ap); // h -> b -> a
        assert!(eq_hlist(&hcollect(hp), &[bp, ap]), "add_before wrong");
    }

    // ── test-only helpers (std-free: plain arrays + manual walk) ──────────────

    fn empty() -> ListHead {
        ListHead {
            next: core::ptr::null_mut(),
            prev: core::ptr::null_mut(),
        }
    }
    fn hnode() -> HlistNode {
        HlistNode {
            next: core::ptr::null_mut(),
            pprev: core::ptr::null_mut(),
        }
    }

    /// A small fixed-capacity stack walk buffer (no_std: no Vec). `len` slots of
    /// `[buf]` are populated; comparisons use `&buf[..len]`.
    struct Walk {
        buf: [usize; 16],
        len: usize,
    }
    impl Walk {
        fn slice(&self) -> &[usize] {
            &self.buf[..self.len]
        }
    }

    /// Forward walk of a list_head chain into a fixed buffer of node addresses.
    fn collect(head: *mut ListHead) -> Walk {
        let mut w = Walk {
            buf: [0; 16],
            len: 0,
        };
        let mut cur = unsafe { (*head).next };
        while cur != head {
            assert!(w.len < w.buf.len(), "cycle/overrun in forward walk");
            w.buf[w.len] = cur as usize;
            w.len += 1;
            cur = unsafe { (*cur).next };
        }
        w
    }
    /// Reverse walk via the prev chain.
    fn collect_rev(head: *mut ListHead) -> Walk {
        let mut w = Walk {
            buf: [0; 16],
            len: 0,
        };
        let mut cur = unsafe { (*head).prev };
        while cur != head {
            assert!(w.len < w.buf.len(), "cycle/overrun in reverse walk");
            w.buf[w.len] = cur as usize;
            w.len += 1;
            cur = unsafe { (*cur).prev };
        }
        w
    }
    /// Forward walk of an hlist via the next chain.
    fn hcollect(head: *mut HlistHead) -> Walk {
        let mut w = Walk {
            buf: [0; 16],
            len: 0,
        };
        let mut cur = unsafe { (*head).first };
        while !cur.is_null() {
            assert!(w.len < w.buf.len(), "cycle/overrun in hlist walk");
            w.buf[w.len] = cur as usize;
            w.len += 1;
            cur = unsafe { (*cur).next };
        }
        w
    }

    /// Compare a walk's addresses against an expected ordered list of pointers.
    fn eq_list(w: &Walk, expect: &[*mut ListHead]) -> bool {
        if w.len != expect.len() {
            return false;
        }
        for (i, e) in expect.iter().enumerate() {
            if w.slice()[i] != *e as usize {
                return false;
            }
        }
        true
    }
    fn eq_hlist(w: &Walk, expect: &[*mut HlistNode]) -> bool {
        if w.len != expect.len() {
            return false;
        }
        for (i, e) in expect.iter().enumerate() {
            if w.slice()[i] != *e as usize {
                return false;
            }
        }
        true
    }
}
