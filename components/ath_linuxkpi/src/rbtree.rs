//! Red-black tree — the Linux `lib/rbtree.c` balancing/iteration algorithm,
//! reimplemented as original MPL Rust over the C `struct rb_node` ABI.
//!
//! drm_mm (the GPU VA allocator), amdgpu's VM interval tree, dma_resv, the DRM
//! scheduler and mmu_notifier all keep ordered state in an rbtree, so this is a
//! foundational export — a fake that skipped rebalancing would silently corrupt
//! every one of them (SCOPE.md rule 9). The augmented variant (subtree-augment
//! callbacks invoked at each rotation) backs `interval_tree_generic.h`.
//!
//! Node layout matches the kernel ABI exactly: `__rb_parent_color` packs the
//! parent pointer with the colour in bit 0 (RED=0, BLACK=1); nodes are
//! `long`-aligned so the low 2 bits are free. The C side owns node storage; we
//! only manipulate links through raw pointers.

const RB_RED: usize = 0;
const RB_BLACK: usize = 1;

#[repr(C, align(8))]
pub struct RbNode {
    parent_color: usize,
    right: *mut RbNode,
    left: *mut RbNode,
}

#[repr(C)]
pub struct RbRoot {
    rb_node: *mut RbNode,
}

#[repr(C)]
pub struct RbRootCached {
    rb_root: RbRoot,
    rb_leftmost: *mut RbNode,
}

/// Augment callback: invoked with (old_top, new_top) after each rotation so an
/// augmented tree (e.g. interval tree) can recompute the moved subtrees' summary.
type AugmentRotate = extern "C" fn(*mut RbNode, *mut RbNode);
/// Augment propagate: recompute summaries from `node` up to (but not past) `stop`.
type AugmentPropagate = extern "C" fn(*mut RbNode, *mut RbNode);
/// Augment copy: the erased node's summary is inherited by its replacement.
type AugmentCopy = extern "C" fn(*mut RbNode, *mut RbNode);

extern "C" fn dummy_rotate(_old: *mut RbNode, _new: *mut RbNode) {}
extern "C" fn dummy_propagate(_node: *mut RbNode, _stop: *mut RbNode) {}
extern "C" fn dummy_copy(_old: *mut RbNode, _new: *mut RbNode) {}

#[inline]
unsafe fn rb_parent(n: *const RbNode) -> *mut RbNode {
    ((*n).parent_color & !3) as *mut RbNode
}
#[inline]
fn pc_parent(pc: usize) -> *mut RbNode {
    (pc & !3) as *mut RbNode
}
#[inline]
unsafe fn rb_color(n: *const RbNode) -> usize {
    (*n).parent_color & 1
}
#[inline]
unsafe fn rb_is_red(n: *const RbNode) -> bool {
    rb_color(n) == RB_RED
}
#[inline]
unsafe fn rb_is_black(n: *const RbNode) -> bool {
    rb_color(n) == RB_BLACK
}
#[inline]
unsafe fn rb_set_parent(n: *mut RbNode, p: *mut RbNode) {
    (*n).parent_color = rb_color(n) | (p as usize);
}
#[inline]
unsafe fn rb_set_parent_color(n: *mut RbNode, p: *mut RbNode, color: usize) {
    (*n).parent_color = (p as usize) | color;
}
#[inline]
unsafe fn rb_set_black(n: *mut RbNode) {
    (*n).parent_color |= RB_BLACK;
}

#[inline]
unsafe fn rb_change_child(
    old: *mut RbNode,
    new: *mut RbNode,
    parent: *mut RbNode,
    root: *mut RbRoot,
) {
    if !parent.is_null() {
        if (*parent).left == old {
            (*parent).left = new;
        } else {
            (*parent).right = new;
        }
    } else {
        (*root).rb_node = new;
    }
}

#[inline]
unsafe fn rb_rotate_set_parents(
    old: *mut RbNode,
    new: *mut RbNode,
    root: *mut RbRoot,
    color: usize,
) {
    let parent = rb_parent(old);
    (*new).parent_color = (*old).parent_color;
    rb_set_parent_color(old, new, color);
    rb_change_child(old, new, parent, root);
}

// ───────────────────────── insert rebalance ─────────────────────────

unsafe fn rb_insert(node: *mut RbNode, root: *mut RbRoot, augment: AugmentRotate) {
    let mut node = node;
    let mut parent = rb_parent(node);
    let mut gparent: *mut RbNode;
    let mut tmp: *mut RbNode;

    loop {
        if parent.is_null() {
            rb_set_parent_color(node, core::ptr::null_mut(), RB_BLACK);
            break;
        }
        if rb_is_black(parent) {
            break;
        }
        gparent = rb_parent(parent);
        tmp = (*gparent).right;
        if parent != tmp {
            // parent == gparent->left
            if !tmp.is_null() && rb_is_red(tmp) {
                rb_set_parent_color(tmp, gparent, RB_BLACK);
                rb_set_parent_color(parent, gparent, RB_BLACK);
                node = gparent;
                parent = rb_parent(node);
                rb_set_parent_color(node, parent, RB_RED);
                continue;
            }
            tmp = (*parent).right;
            if node == tmp {
                // left-right: rotate left at parent
                tmp = (*node).left;
                (*parent).right = tmp;
                (*node).left = parent;
                if !tmp.is_null() {
                    rb_set_parent_color(tmp, parent, RB_BLACK);
                }
                rb_set_parent_color(parent, node, RB_RED);
                augment(parent, node);
                parent = node;
                tmp = (*node).right;
            }
            // rotate right at gparent
            (*gparent).left = tmp;
            if !tmp.is_null() {
                rb_set_parent_color(tmp, gparent, RB_BLACK);
            }
            (*parent).right = gparent;
            rb_rotate_set_parents(gparent, parent, root, RB_RED);
            augment(gparent, parent);
            break;
        } else {
            // parent == gparent->right (mirror)
            tmp = (*gparent).left;
            if !tmp.is_null() && rb_is_red(tmp) {
                rb_set_parent_color(tmp, gparent, RB_BLACK);
                rb_set_parent_color(parent, gparent, RB_BLACK);
                node = gparent;
                parent = rb_parent(node);
                rb_set_parent_color(node, parent, RB_RED);
                continue;
            }
            tmp = (*parent).left;
            if node == tmp {
                // right-left: rotate right at parent
                tmp = (*node).right;
                (*parent).left = tmp;
                (*node).right = parent;
                if !tmp.is_null() {
                    rb_set_parent_color(tmp, parent, RB_BLACK);
                }
                rb_set_parent_color(parent, node, RB_RED);
                augment(parent, node);
                parent = node;
                tmp = (*node).left;
            }
            // rotate left at gparent
            (*gparent).right = tmp;
            if !tmp.is_null() {
                rb_set_parent_color(tmp, gparent, RB_BLACK);
            }
            (*parent).left = gparent;
            rb_rotate_set_parents(gparent, parent, root, RB_RED);
            augment(gparent, parent);
            break;
        }
    }
}

// ───────────────────────── erase colour fixup ─────────────────────────

/// Colour fixup after a black node is removed. Exported so the augmented erase
/// inline wrapper (rbtree_augmented.h) can drive it with the augment's rotate cb.
#[no_mangle]
pub unsafe extern "C" fn __rb_erase_color(
    parent_in: *mut RbNode,
    root: *mut RbRoot,
    augment: AugmentRotate,
) {
    let mut node: *mut RbNode = core::ptr::null_mut();
    let mut parent = parent_in;
    let mut sibling: *mut RbNode;
    let mut tmp1: *mut RbNode;
    let mut tmp2: *mut RbNode;

    loop {
        sibling = (*parent).right;
        if node != sibling {
            // node == parent->left
            if rb_is_red(sibling) {
                tmp1 = (*sibling).left;
                (*parent).right = tmp1;
                (*sibling).left = parent;
                rb_set_parent_color(tmp1, parent, RB_BLACK);
                rb_rotate_set_parents(parent, sibling, root, RB_RED);
                augment(parent, sibling);
                sibling = tmp1;
            }
            tmp1 = (*sibling).right;
            if tmp1.is_null() || rb_is_black(tmp1) {
                tmp2 = (*sibling).left;
                if tmp2.is_null() || rb_is_black(tmp2) {
                    rb_set_parent_color(sibling, parent, RB_RED);
                    if rb_is_red(parent) {
                        rb_set_black(parent);
                    } else {
                        node = parent;
                        parent = rb_parent(node);
                        if !parent.is_null() {
                            continue;
                        }
                    }
                    break;
                }
                // sibling's left child is red: rotate right at sibling
                tmp1 = (*tmp2).right;
                (*sibling).left = tmp1;
                (*tmp2).right = sibling;
                (*parent).right = tmp2;
                if !tmp1.is_null() {
                    rb_set_parent_color(tmp1, sibling, RB_BLACK);
                }
                augment(sibling, tmp2);
                tmp1 = sibling;
                sibling = tmp2;
            }
            // rotate left at parent
            tmp2 = (*sibling).left;
            (*parent).right = tmp2;
            (*sibling).left = parent;
            rb_set_parent_color(tmp1, sibling, RB_BLACK);
            if !tmp2.is_null() {
                rb_set_parent(tmp2, parent);
            }
            rb_rotate_set_parents(parent, sibling, root, RB_BLACK);
            augment(parent, sibling);
            break;
        } else {
            // node == parent->right (mirror)
            sibling = (*parent).left;
            if rb_is_red(sibling) {
                tmp1 = (*sibling).right;
                (*parent).left = tmp1;
                (*sibling).right = parent;
                rb_set_parent_color(tmp1, parent, RB_BLACK);
                rb_rotate_set_parents(parent, sibling, root, RB_RED);
                augment(parent, sibling);
                sibling = tmp1;
            }
            tmp1 = (*sibling).left;
            if tmp1.is_null() || rb_is_black(tmp1) {
                tmp2 = (*sibling).right;
                if tmp2.is_null() || rb_is_black(tmp2) {
                    rb_set_parent_color(sibling, parent, RB_RED);
                    if rb_is_red(parent) {
                        rb_set_black(parent);
                    } else {
                        node = parent;
                        parent = rb_parent(node);
                        if !parent.is_null() {
                            continue;
                        }
                    }
                    break;
                }
                // sibling's right child is red: rotate left at sibling
                tmp1 = (*tmp2).left;
                (*sibling).right = tmp1;
                (*tmp2).left = sibling;
                (*parent).left = tmp2;
                if !tmp1.is_null() {
                    rb_set_parent_color(tmp1, sibling, RB_BLACK);
                }
                augment(sibling, tmp2);
                tmp1 = sibling;
                sibling = tmp2;
            }
            // rotate right at parent
            tmp2 = (*sibling).right;
            (*parent).left = tmp2;
            (*sibling).right = parent;
            rb_set_parent_color(tmp1, sibling, RB_BLACK);
            if !tmp2.is_null() {
                rb_set_parent(tmp2, parent);
            }
            rb_rotate_set_parents(parent, sibling, root, RB_BLACK);
            augment(parent, sibling);
            break;
        }
    }
}

/// Unlink `node`; returns the node from which colour rebalancing must start
/// (or null if the tree stays balanced). Faithful port of `__rb_erase_augmented`
/// (lib/rbtree.c). Straight-line raw-pointer code — no borrows to juggle.
/// Exported: the augmented erase inline wrapper calls this then `__rb_erase_color`.
#[no_mangle]
pub unsafe extern "C" fn __rb_erase_augmented(
    node: *mut RbNode,
    root: *mut RbRoot,
    propagate: AugmentPropagate,
    copy: AugmentCopy,
) -> *mut RbNode {
    let child = (*node).right;
    let mut tmp = (*node).left;
    let parent;
    let rebalance: *mut RbNode;
    let pc;

    if tmp.is_null() {
        // Case 1: at most one child, on the right.
        pc = (*node).parent_color;
        parent = pc_parent(pc);
        rb_change_child(node, child, parent, root);
        if !child.is_null() {
            (*child).parent_color = pc;
            rebalance = core::ptr::null_mut();
        } else {
            rebalance = if pc & 1 == RB_BLACK {
                parent
            } else {
                core::ptr::null_mut()
            };
        }
        tmp = parent;
    } else if child.is_null() {
        // Case 1 mirror: only a left child.
        pc = (*node).parent_color;
        (*tmp).parent_color = pc;
        parent = pc_parent(pc);
        rb_change_child(node, tmp, parent, root);
        rebalance = core::ptr::null_mut();
        tmp = parent;
    } else {
        // Two children: find the in-order successor in the right subtree.
        let mut successor = child;
        let child2;
        tmp = (*child).left;
        if tmp.is_null() {
            // Case 2: successor is node's right child directly.
            parent = successor;
            child2 = (*successor).right;
            copy(node, successor);
        } else {
            // Case 3: successor is the leftmost node under node's right child.
            let mut p;
            loop {
                p = successor;
                successor = tmp;
                tmp = (*tmp).left;
                if tmp.is_null() {
                    break;
                }
            }
            parent = p;
            child2 = (*successor).right;
            (*parent).left = child2;
            (*successor).right = child;
            rb_set_parent(child, successor);
            copy(node, successor);
            propagate(parent, successor);
        }

        tmp = (*node).left;
        (*successor).left = tmp;
        rb_set_parent(tmp, successor);

        pc = (*node).parent_color;
        tmp = pc_parent(pc);
        rb_change_child(node, successor, tmp, root);

        if !child2.is_null() {
            rb_set_parent_color(child2, parent, RB_BLACK);
            rebalance = core::ptr::null_mut();
        } else {
            rebalance = if rb_is_black(successor) {
                parent
            } else {
                core::ptr::null_mut()
            };
        }
        (*successor).parent_color = pc;
        tmp = successor;
    }

    propagate(tmp, core::ptr::null_mut());
    rebalance
}

// ───────────────────────── public C ABI ─────────────────────────

#[no_mangle]
pub unsafe extern "C" fn rb_insert_color(node: *mut RbNode, root: *mut RbRoot) {
    rb_insert(node, root, dummy_rotate);
}

#[no_mangle]
pub unsafe extern "C" fn __rb_insert_augmented(
    node: *mut RbNode,
    root: *mut RbRoot,
    augment: AugmentRotate,
) {
    rb_insert(node, root, augment);
}

#[no_mangle]
pub unsafe extern "C" fn rb_erase(node: *mut RbNode, root: *mut RbRoot) {
    let rebalance = __rb_erase_augmented(node, root, dummy_propagate, dummy_copy);
    if !rebalance.is_null() {
        __rb_erase_color(rebalance, root, dummy_rotate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rb_insert_color_cached(
    node: *mut RbNode,
    root: *mut RbRootCached,
    leftmost: bool,
) {
    if leftmost {
        (*root).rb_leftmost = node;
    }
    rb_insert(node, &mut (*root).rb_root, dummy_rotate);
}

#[no_mangle]
pub unsafe extern "C" fn rb_erase_cached(node: *mut RbNode, root: *mut RbRootCached) {
    if (*root).rb_leftmost == node {
        (*root).rb_leftmost = rb_next(node);
    }
    rb_erase(node, &mut (*root).rb_root);
}

// The augmented CACHED + struct-callback wrappers (rb_insert_augmented[_cached],
// rb_erase_augmented[_cached]) live as inline functions in <linux/rbtree_augmented.h>
// (they read the rb_augment_callbacks struct and call the __-cores above), exactly
// as the kernel factors them — no extra crate symbols needed.

#[no_mangle]
pub unsafe extern "C" fn rb_first(root: *const RbRoot) -> *mut RbNode {
    let mut n = (*root).rb_node;
    if n.is_null() {
        return core::ptr::null_mut();
    }
    while !(*n).left.is_null() {
        n = (*n).left;
    }
    n
}

#[no_mangle]
pub unsafe extern "C" fn rb_last(root: *const RbRoot) -> *mut RbNode {
    let mut n = (*root).rb_node;
    if n.is_null() {
        return core::ptr::null_mut();
    }
    while !(*n).right.is_null() {
        n = (*n).right;
    }
    n
}

#[no_mangle]
pub unsafe extern "C" fn rb_first_cached(root: *const RbRootCached) -> *mut RbNode {
    (*root).rb_leftmost
}

#[no_mangle]
pub unsafe extern "C" fn rb_next(node: *const RbNode) -> *mut RbNode {
    let node = node as *mut RbNode;
    if RB_EMPTY_NODE(node) {
        return core::ptr::null_mut();
    }
    // if there is a right child, the successor is its leftmost descendant
    if !(*node).right.is_null() {
        let mut n = (*node).right;
        while !(*n).left.is_null() {
            n = (*n).left;
        }
        return n;
    }
    // otherwise walk up until we come from a left child
    let mut node = node;
    let mut parent = rb_parent(node);
    while !parent.is_null() && node == (*parent).right {
        node = parent;
        parent = rb_parent(node);
    }
    parent
}

#[no_mangle]
pub unsafe extern "C" fn rb_prev(node: *const RbNode) -> *mut RbNode {
    let node = node as *mut RbNode;
    if RB_EMPTY_NODE(node) {
        return core::ptr::null_mut();
    }
    if !(*node).left.is_null() {
        let mut n = (*node).left;
        while !(*n).right.is_null() {
            n = (*n).right;
        }
        return n;
    }
    let mut node = node;
    let mut parent = rb_parent(node);
    while !parent.is_null() && node == (*parent).left {
        node = parent;
        parent = rb_parent(node);
    }
    parent
}

#[no_mangle]
pub unsafe extern "C" fn rb_replace_node(victim: *mut RbNode, new: *mut RbNode, root: *mut RbRoot) {
    let parent = rb_parent(victim);
    // copy children + parent links
    rb_change_child(victim, new, parent, root);
    if !(*victim).left.is_null() {
        rb_set_parent((*victim).left, new);
    }
    if !(*victim).right.is_null() {
        rb_set_parent((*victim).right, new);
    }
    (*new).parent_color = (*victim).parent_color;
    (*new).left = (*victim).left;
    (*new).right = (*victim).right;
}

#[inline]
#[allow(non_snake_case)]
unsafe fn RB_EMPTY_NODE(node: *const RbNode) -> bool {
    (*node).parent_color == node as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    // no_std crate: pull `alloc` (not `std`) for the test's Box/Vec — the test
    // harness links the allocator. Keeps the §R7 no-std-ism gate satisfied.
    extern crate alloc;
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    // A test node embedding an RbNode + a key, mirroring how C drivers embed it.
    struct TNode {
        rb: RbNode,
        key: i64,
    }

    unsafe fn new_node(key: i64) -> *mut TNode {
        Box::into_raw(Box::new(TNode {
            rb: RbNode {
                parent_color: 0,
                right: core::ptr::null_mut(),
                left: core::ptr::null_mut(),
            },
            key,
        }))
    }
    unsafe fn tnode_of(rb: *mut RbNode) -> *mut TNode {
        // rb is the first field, so the offset is 0.
        rb as *mut TNode
    }

    unsafe fn insert(root: *mut RbRoot, t: *mut TNode) {
        let mut link = &mut (*root).rb_node as *mut *mut RbNode;
        let mut parent: *mut RbNode = core::ptr::null_mut();
        while !(*link).is_null() {
            parent = *link;
            let pk = (*tnode_of(parent)).key;
            if (*t).key < pk {
                link = &mut (*parent).left;
            } else {
                link = &mut (*parent).right;
            }
        }
        let n = &mut (*t).rb as *mut RbNode;
        (*n).parent_color = parent as usize; // colour RED (0)
        (*n).left = core::ptr::null_mut();
        (*n).right = core::ptr::null_mut();
        *link = n;
        rb_insert_color(n, root);
    }

    // Validate the red-black invariants; returns black-height or panics.
    unsafe fn check(n: *const RbNode, parent: *mut RbNode) -> i32 {
        if n.is_null() {
            return 1;
        }
        assert_eq!(rb_parent(n), parent, "parent pointer mismatch");
        if rb_is_red(n) {
            if !(*n).left.is_null() {
                assert!(rb_is_black((*n).left), "red node has red left child");
            }
            if !(*n).right.is_null() {
                assert!(rb_is_black((*n).right), "red node has red right child");
            }
        }
        let lh = check((*n).left, n as *mut RbNode);
        let rh = check((*n).right, n as *mut RbNode);
        assert_eq!(lh, rh, "black-height mismatch");
        lh + if rb_is_black(n) { 1 } else { 0 }
    }

    unsafe fn inorder(root: *const RbRoot) -> Vec<i64> {
        let mut out = Vec::new();
        let mut n = rb_first(root);
        while !n.is_null() {
            out.push((*tnode_of(n)).key);
            n = rb_next(n);
        }
        out
    }

    #[test]
    fn randomized_insert_erase_keeps_invariants() {
        unsafe {
            // simple xorshift PRNG (deterministic, no external dep)
            let mut state: u64 = 0x9e3779b97f4a7c15;
            let mut rnd = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };

            let mut root = RbRoot {
                rb_node: core::ptr::null_mut(),
            };
            let mut live: Vec<*mut TNode> = Vec::new();
            let mut keys: Vec<i64> = Vec::new();

            for _ in 0..4000 {
                if live.is_empty() || (rnd() & 1) == 0 {
                    let k = (rnd() % 2000) as i64;
                    let t = new_node(k);
                    insert(&mut root, t);
                    live.push(t);
                    keys.push(k);
                } else {
                    let idx = (rnd() as usize) % live.len();
                    let t = live.swap_remove(idx);
                    let k = (*t).key;
                    // remove first matching key from the model
                    let pos = keys.iter().position(|&x| x == k).unwrap();
                    keys.remove(pos);
                    rb_erase(&mut (*t).rb, &mut root);
                    drop(Box::from_raw(t));
                }
                // validate root colour + invariants every few ops
                if !root.rb_node.is_null() {
                    assert!(rb_is_black(root.rb_node), "root must be black");
                    check(root.rb_node, core::ptr::null_mut());
                }
            }

            // final: in-order traversal equals the sorted model
            keys.sort_unstable();
            assert_eq!(inorder(&root), keys);

            // free remaining
            for t in live {
                drop(Box::from_raw(t));
            }
        }
    }
}
