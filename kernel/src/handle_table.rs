//! Generic O(1) handle table — Concept §"Security by default": every kernel
//! object a task can name (socket, IPC channel, future capability handle) needs
//! a dense, reuse-safe id→object map. `slab` gives O(1) insert/remove with id
//! recycling and no per-op BTreeMap rebalancing, which the scattered
//! `(pid, fd)` BTreeMaps (net sockets, IPC) can migrate onto.
//!
//! This is the shared primitive; adopters keep their own per-pid ownership index
//! on top so teardown (BUG-23/32) stays a single sweep.

extern crate alloc;
use slab::Slab;

/// A dense table mapping small integer handles to `T`. Handles are reused after
/// removal, so the id space stays compact under churn.
pub struct HandleTable<T> {
    slots: Slab<T>,
}

impl<T> HandleTable<T> {
    pub const fn new() -> Self {
        Self { slots: Slab::new() }
    }

    /// Insert a value, returning its handle.
    pub fn insert(&mut self, value: T) -> usize {
        self.slots.insert(value)
    }

    pub fn get(&self, handle: usize) -> Option<&T> {
        self.slots.get(handle)
    }

    pub fn get_mut(&mut self, handle: usize) -> Option<&mut T> {
        self.slots.get_mut(handle)
    }

    /// Remove and return the value, freeing the handle for reuse.
    pub fn remove(&mut self, handle: usize) -> Option<T> {
        if self.slots.contains(handle) {
            Some(self.slots.remove(handle))
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

impl<T> Default for HandleTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// R10 smoketest — must be able to print FAIL. Exercises insert / get / remove
/// and verifies the freed handle is recycled (slab's contract).
pub fn run_boot_smoketest() {
    let mut t: HandleTable<u64> = HandleTable::new();
    let a = t.insert(0xA);
    let b = t.insert(0xB);
    let got_b = t.get(b) == Some(&0xB);
    let removed_a = t.remove(a) == Some(0xA);
    // The next insert must recycle handle `a` (dense reuse — the whole point).
    let c = t.insert(0xC);
    let reused = c == a;
    let len_ok = t.len() == 2; // b and c
    let gone = t.get(a) == Some(&0xC) && t.remove(a).is_some();

    let pass = got_b && removed_a && reused && len_ok && gone;
    crate::selftest::record_smoketest("handle_table", pass);
    crate::serial_println!(
        "[handle_table] smoketest: get={} remove={} reuse={} len={} -> {}",
        got_b,
        removed_a,
        reused,
        len_ok,
        if pass { "PASS" } else { "FAIL" }
    );
}
