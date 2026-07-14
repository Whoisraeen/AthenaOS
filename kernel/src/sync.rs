//! Kernel futex table — the cross-process blocking primitive.
//!
//! Concept (§"Gaming isn't a mode" / AthBridge): every Win32 synchronization
//! object (Event / Mutex / Semaphore / CriticalSection → `WaitForSingleObject`)
//! bottoms out on a futex word in a page shared between two processes. For a
//! wait to truly *block* (not spin/yield) and for a wake in one process to
//! reach a waiter in another, the wait queue MUST be keyed by the **physical
//! frame** the word lives in — two processes that `SYS_CHANNEL_SHMEM_MAP` the
//! same frame see different virtual addresses but the same physical page, so
//! phys-keying makes cross-process sharing implicit. This table is that queue;
//! it is also used by the in-kernel NVMe driver to park on a completion word.
//!
//! MasterChecklist Phase 11, item 1828 (Slice 2b kernel half): re-key SYS_FUTEX
//! by shared-frame physical identity + a real blocking `futex_wait`. Landed with
//! explicit owner authorization (the AthBridge kernel-half human-gate lifted).

use crate::task::TaskId;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

lazy_static! {
    pub static ref FUTEX_MANAGER: Mutex<FutexManager> = Mutex::new(FutexManager::new());
}

pub struct FutexManager {
    // Physical address of the futex word -> tasks waiting on it, in FIFO wake
    // order. Keyed by physical (not virtual) address so an aliased/shared frame
    // mapped at different virtual addresses in two processes resolves to one
    // queue — the basis of cross-process blocking.
    waiting_tasks: BTreeMap<u64, Vec<TaskId>>,
}

impl FutexManager {
    pub fn new() -> Self {
        Self {
            waiting_tasks: BTreeMap::new(),
        }
    }

    /// Register `task_id` as a waiter on `phys_addr`. Pure queue op (no memory
    /// read, no scheduler call) — the host-KAT-able seam. Callers that need the
    /// expected-value compare use [`wait`](Self::wait); callers that already
    /// hold the value invariant (e.g. the NVMe completion path) may register
    /// directly.
    pub fn register(&mut self, phys_addr: u64, task_id: TaskId) {
        self.waiting_tasks
            .entry(phys_addr)
            .or_insert_with(Vec::new)
            .push(task_id);
    }

    /// Remove up to `count` waiters from `phys_addr` in FIFO order and return
    /// their ids. Pure queue op — the caller decides what to do with the ids
    /// (the syscall path unblocks each). Prunes the key when its queue empties.
    pub fn drain(&mut self, phys_addr: u64, count: usize) -> Vec<TaskId> {
        let mut drained = Vec::new();
        if let Some(queue) = self.waiting_tasks.get_mut(&phys_addr) {
            while drained.len() < count && !queue.is_empty() {
                drained.push(queue.remove(0));
            }
            if queue.is_empty() {
                self.waiting_tasks.remove(&phys_addr);
            }
        }
        drained
    }

    /// Remove one specific `(phys_addr, task_id)` waiter if present, returning
    /// whether it was found. Used by the cooperative (Linux-ABI) wait path to
    /// tell "I was woken" (absent) from "still parked" (present) after a yield.
    pub fn deregister(&mut self, phys_addr: u64, task_id: TaskId) -> bool {
        if let Some(queue) = self.waiting_tasks.get_mut(&phys_addr) {
            if let Some(i) = queue.iter().position(|&t| t == task_id) {
                queue.remove(i);
                if queue.is_empty() {
                    self.waiting_tasks.remove(&phys_addr);
                }
                return true;
            }
        }
        false
    }

    /// Wait on a futex: if the word at `phys_addr` still equals `expected_val`,
    /// register `current_task_id` as a waiter and return `true` (caller should
    /// block). If the word already changed, return `false` (caller returns
    /// EAGAIN without blocking).
    ///
    /// The compare + register happen under this table lock, so a waker that
    /// mutated the word *before* this call is observed here and we do not park
    /// on a stale value. The remaining window — a waker mutates + drains *after*
    /// we register but before the caller finishes parking — is handled for a
    /// mid-block waiter by `unblock_futex_waiter`'s switch-stash race-catch; the
    /// even-narrower "registered but still Running" case is the same class the
    /// proven channel/IRQ block paths carry, and is backstopped by the userspace
    /// sync engine's bounded wait-retry (a real futex caller loops re-checking
    /// the word, so a missed wake becomes a re-check, not a permanent hang). A
    /// fully lost-wake-free enqueue (park-state set under one lock the waker
    /// also holds, Linux-style) is a tracked follow-up.
    pub fn wait(&mut self, phys_addr: u64, expected_val: u32, current_task_id: TaskId) -> bool {
        // Read the shared word through the kernel's physical alias — this is the
        // same physical u32 regardless of which process's page tables mapped it,
        // and it is guaranteed mapped (the caller translated a live user vaddr
        // to get `phys_addr`). Futex words are 4-byte aligned, so the read is
        // atomic.
        let ptr = crate::memory::phys_to_virt(phys_addr).as_ptr::<u32>();
        let val = unsafe { core::ptr::read_volatile(ptr) };
        if val != expected_val {
            return false; // condition already changed; do not park
        }
        self.register(phys_addr, current_task_id);
        true
    }

    /// Wake up to `count` tasks waiting on `phys_addr`. Drains them and routes
    /// each through the scheduler's futex-wake path (stash-race-safe,
    /// dead-task-guarded). Returns the number woken.
    pub fn wake(&mut self, phys_addr: u64, count: usize) -> usize {
        let drained = self.drain(phys_addr, count);
        let awoken = drained.len();
        for task_id in drained {
            crate::scheduler::unblock_futex_waiter(task_id);
        }
        awoken
    }

    fn waiter_count(&self) -> usize {
        self.waiting_tasks.values().map(|q| q.len()).sum()
    }

    fn key_count(&self) -> usize {
        self.waiting_tasks.len()
    }
}

/// Translate a user virtual futex address to its physical-frame key. `None` if
/// the address is misaligned, out of the user range, or not currently mapped.
/// Must be called while the owning process's CR3 is active (true at syscall
/// dispatch). Includes the in-page offset so distinct words in one page are
/// distinct futexes.
pub fn phys_key(virt_addr: u64) -> Option<u64> {
    if virt_addr == 0 || (virt_addr & 0x3) != 0 || virt_addr >= 0x0000_8000_0000_0000 {
        return None;
    }
    crate::memory::virt_to_phys(x86_64::VirtAddr::new(virt_addr)).map(|p| p.as_u64())
}

/// `/proc/athena/futex` — live wait-queue occupancy.
pub fn dump_text() -> String {
    let g = FUTEX_MANAGER.lock();
    let mut out = String::new();
    out.push_str("# AthenaOS futex table (phys-frame-keyed cross-process wait queue)\n");
    out.push_str(&alloc::format!("keys: {}\n", g.key_count()));
    out.push_str(&alloc::format!("waiters: {}\n", g.waiter_count()));
    out
}

/// R10 FAIL-able smoketest: exercises the real phys-keyed compare + queue on a
/// kernel word without needing two live tasks. Proves (1) a value mismatch does
/// NOT park, (2) a match DOES register, (3) drain returns the registered id in
/// FIFO order and is one-shot, (4) keys are isolated. A regression in any arm
/// prints FAIL.
pub fn run_boot_smoketest() {
    use core::sync::atomic::{AtomicU32, Ordering};
    static SMOKE_WORD: AtomicU32 = AtomicU32::new(0xF00D);
    SMOKE_WORD.store(0xF00D, Ordering::SeqCst);

    // Translate the kernel static directly — `phys_key` intentionally rejects
    // high (kernel-half) addresses because it is the USER-word entry point; the
    // smoketest word lives in kernel .data, so go straight to virt_to_phys.
    let vaddr = &SMOKE_WORD as *const AtomicU32 as u64;
    let phys = match crate::memory::virt_to_phys(x86_64::VirtAddr::new(vaddr)) {
        Some(p) => p.as_u64(),
        None => {
            crate::serial_println!("[futex] smoketest: FAIL (could not translate SMOKE_WORD)");
            return;
        }
    };
    let fake = TaskId::from_raw(0xFEED_0000_0000_0001);
    let other_phys = phys ^ 0x1000; // a different frame key

    let mut g = FUTEX_MANAGER.lock();
    // 1. mismatch must NOT register.
    let mismatch_parked = g.wait(phys, 0xBEEF, fake);
    // 2. match must register.
    let match_parked = g.wait(phys, 0xF00D, fake);
    // 3. drain returns exactly the registered id, one-shot.
    let first = g.drain(phys, 1);
    let second = g.drain(phys, 1);
    // 4. key isolation: a fresh registration is invisible under a different key.
    g.register(phys, fake);
    let wrong_key = g.drain(other_phys, 1);
    let right_key = g.drain(phys, 1);
    drop(g);

    let ok = !mismatch_parked
        && match_parked
        && first.len() == 1
        && first.first() == Some(&fake)
        && second.is_empty()
        && wrong_key.is_empty()
        && right_key.len() == 1;

    if ok {
        crate::serial_println!(
            "[futex] smoketest: PASS (phys-keyed compare+park+drain: mismatch={} match={} fifo=1 oneshot=1 key_iso=1)",
            mismatch_parked as u8,
            match_parked as u8,
        );
    } else {
        crate::serial_println!(
            "[futex] smoketest: FAIL (mismatch_parked={} match_parked={} first={} second={} wrong_key={} right_key={})",
            mismatch_parked as u8,
            match_parked as u8,
            first.len(),
            second.len(),
            wrong_key.len(),
            right_key.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_drain_fifo() {
        let mut m = FutexManager::new();
        let a = TaskId::from_raw(1);
        let b = TaskId::from_raw(2);
        m.register(0x1000, a);
        m.register(0x1000, b);
        assert_eq!(m.drain(0x1000, 10), alloc::vec![a, b]); // FIFO
        assert!(m.drain(0x1000, 10).is_empty()); // one-shot
    }

    #[test]
    fn drain_respects_count() {
        let mut m = FutexManager::new();
        for i in 0..5 {
            m.register(0x2000, TaskId::from_raw(i));
        }
        assert_eq!(m.drain(0x2000, 2).len(), 2);
        assert_eq!(m.drain(0x2000, 10).len(), 3);
    }

    #[test]
    fn keys_are_isolated() {
        let mut m = FutexManager::new();
        let a = TaskId::from_raw(1);
        m.register(0x1000, a);
        assert!(m.drain(0x2000, 10).is_empty()); // different key: nothing
        assert_eq!(m.drain(0x1000, 10), alloc::vec![a]);
    }

    #[test]
    fn deregister_finds_and_reports() {
        let mut m = FutexManager::new();
        let a = TaskId::from_raw(1);
        let b = TaskId::from_raw(2);
        m.register(0x1000, a);
        m.register(0x1000, b);
        assert!(m.deregister(0x1000, a)); // present
        assert!(!m.deregister(0x1000, a)); // already gone
        assert_eq!(m.drain(0x1000, 10), alloc::vec![b]); // b survived
    }
}
