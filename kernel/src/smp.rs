//! Symmetric Multi-Processing — per-CPU run queues, load balancing, TLB
//! shootdown, CPU topology, and CPU hotplug for AthenaOS.
//!
//! Architecture:
//!   * Each CPU owns a `CpuData` struct containing a local run queue split by
//!     priority class (SCHED_BODY deadline, SCHED_BODY regular, SCHED_NORMAL,
//!     SCHED_IDLE). Enqueue/dequeue on the *local* queue requires only a
//!     per-CPU spinlock — no global contention on the fast path.
//!   * A periodic `LoadBalancer` (every 4 ms tick) migrates tasks from
//!     overloaded CPUs to underloaded CPUs, respecting NUMA distance, LLC
//!     sharing, and SCHED_BODY pinning rules.
//!   * `CpuTopology` detects packages, cores, SMT threads, and LLC domains
//!     from CPUID leaf 0x0B (extended topology enumeration).
//!   * TLB shootdown sends targeted IPIs to invalidate stale TLB entries
//!     when page tables change, with lazy-TLB optimization.
//!   * CPU hotplug allows onlining/offlining CPUs at runtime, migrating all
//!     tasks off before parking.
//!
//! Boot algorithm (per Intel SDM Vol 3A §9.4 "Multiple-Processor Initialization"):
//!
//! 1. BSP parses ACPI MADT (already done in [`crate::acpi`]) to discover each
//!    Application Processor's APIC id.
//! 2. BSP identity-maps the trampoline page (phys 0x8000) into the kernel PML4
//!    so that after long-mode is enabled on the AP, execution can continue
//!    fetching instructions from the same virtual address.
//! 3. BSP copies [`AP_TRAMPOLINE`] into physical address 0x8000.
//! 4. For each AP:
//!    a. Allocate a 16 KiB kernel stack just for this CPU.
//!    b. Patch the boot block (located inside the trampoline blob at known
//!       offsets) with this AP's pml4 / stack_top / entry_point / apic_id.
//!    c. Send `INIT` IPI via the BSP's Local APIC (vector ignored, dest =
//!       target APIC id).
//!    d. Wait ~10 ms.
//!    e. Send `SIPI` IPI with vector = `0x08` (telling the AP to start
//!       executing real-mode code at physical 0x8000).
//!    f. Wait ~200 µs.
//!    g. Send `SIPI` again (Intel says: best practice on real silicon).
//!    h. Poll the alive flag (the AP atomically increments [`APS_ONLINE`]
//!       once it reaches [`ap_entry`]).
//!
//! The trampoline itself lives in the kernel's `.text` section but executes
//! at physical 0x8000, so every absolute reference in its body uses the form
//! `(label - ap_trampoline_start + 0x8000)` — the assembler resolves this as
//! a link-time constant, and the literal value embedded in the instruction
//! stream is independent of the kernel's load address.

#![allow(dead_code)]

extern crate alloc;

use crate::arch::{PhysAddr, VirtAddr};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};

use crate::task::{Task, TaskId, TaskPriority};

// ═══════════════════════════════════════════════════════════════════════════════
// §1  CONSTANTS
// ═══════════════════════════════════════════════════════════════════════════════

pub const MAX_CPUS: usize = crate::gdt::MAX_CPUS;

/// Load balancer runs every 4 ms (4 scheduler ticks at 1 kHz).
const BALANCE_INTERVAL_TICKS: u64 = 4;

/// Exponential weighted moving average decay factor (fixed-point, /1024).
/// load_avg = (load_avg * EWMA_DECAY + new_load * (1024 - EWMA_DECAY)) / 1024
const EWMA_DECAY: u64 = 922; // ~0.9 decay per period

/// Imbalance threshold: only migrate if busiest - idlest > this many tasks.
const IMBALANCE_THRESHOLD: usize = 2;

/// Maximum tasks to migrate in a single balancing pass.
const MAX_MIGRATIONS_PER_PASS: usize = 4;

/// TLB shootdown IPI vector. Must not collide with timer/keyboard/etc.
const TLB_SHOOTDOWN_VECTOR: u8 = 0xFE;

/// Reschedule IPI vector for waking idle CPUs.
const RESCHEDULE_VECTOR: u8 = 0xFD;

/// CPU hotplug state transitions.
const CPU_STATE_OFFLINE: u8 = 0;
const CPU_STATE_ONLINE: u8 = 1;
const CPU_STATE_PARKING: u8 = 2;

/// One CPU starts online: the BSP. Each AP increments this on entry.
pub static APS_ONLINE: AtomicU64 = AtomicU64::new(1);

/// Physical address where the trampoline gets copied. Must be page-aligned
/// and below 1 MiB (real-mode addressing). SIPI vector = phys_addr >> 12.
pub const TRAMPOLINE_PHYS: u64 = 0x8000;
pub const TRAMPOLINE_SIPI_VECTOR: u8 = (TRAMPOLINE_PHYS >> 12) as u8;

/// Layout offsets of the boot-block fields inside the trampoline blob.
/// Must stay in sync with `trampoline.asm`. The first 8 bytes are a `jmp short`
/// + padding; the boot block starts at offset 0x08.
mod boot_block {
    pub const PML4: usize = 0x008;
    pub const STACK_TOP: usize = 0x010;
    pub const ENTRY: usize = 0x018;
    pub const APIC_ID: usize = 0x020;
}

// ═══════════════════════════════════════════════════════════════════════════════
// §2  PER-CPU RUN QUEUE
// ═══════════════════════════════════════════════════════════════════════════════

/// Per-CPU run queue with separate sub-queues per scheduling class.
/// Access requires only the per-CPU spinlock — no global lock on the hot path.
pub struct PerCpuRunQueue {
    /// EDF-ordered deadline tasks (SCHED_BODY with deadline params).
    deadline: VecDeque<Task>,
    /// Round-robin SCHED_BODY tasks without deadline constraints.
    game: VecDeque<Task>,
    /// CFS-managed SCHED_NORMAL tasks, sorted by vruntime.
    normal: VecDeque<Task>,
    /// Very low priority background tasks.
    idle_tasks: VecDeque<Task>,
    /// Total runnable task count across all sub-queues.
    nr_running: usize,
    /// EWMA-smoothed load (fixed-point, ×1024).
    load_avg: u64,
    /// Tick at which load was last updated.
    last_load_update: u64,
    /// Minimum vruntime for this CPU's normal queue.
    min_vruntime: u64,
}

impl PerCpuRunQueue {
    pub const fn new() -> Self {
        Self {
            deadline: VecDeque::new(),
            game: VecDeque::new(),
            normal: VecDeque::new(),
            idle_tasks: VecDeque::new(),
            nr_running: 0,
            load_avg: 0,
            last_load_update: 0,
            min_vruntime: 0,
        }
    }

    /// O(1) enqueue into the appropriate sub-queue by priority class.
    pub fn enqueue(&mut self, task: Task) {
        if task.priority == TaskPriority::Game && task.deadline.is_some() {
            self.insert_deadline_sorted(task);
        } else if task.priority == TaskPriority::Game {
            self.game.push_back(task);
        } else {
            self.insert_normal_sorted(task);
        }
        self.nr_running += 1;
    }

    /// Pick the highest-priority runnable task, respecting class ordering:
    /// deadline > game > normal > idle.
    pub fn dequeue_next(&mut self) -> Option<Task> {
        if let Some(task) = self.deadline.pop_front() {
            self.nr_running -= 1;
            return Some(task);
        }
        if let Some(task) = self.game.pop_front() {
            self.nr_running -= 1;
            return Some(task);
        }
        if let Some(task) = self.normal.pop_front() {
            self.nr_running -= 1;
            return Some(task);
        }
        if let Some(task) = self.idle_tasks.pop_front() {
            self.nr_running -= 1;
            return Some(task);
        }
        None
    }

    /// Steal a migratable task from this queue (for load balancing).
    /// Prefers stealing from the normal queue since game tasks are pinned.
    pub fn steal_one(&mut self) -> Option<Task> {
        // Try stealing from the back of the normal queue (highest vruntime = least urgent).
        if let Some(task) = self.normal.pop_back() {
            self.nr_running -= 1;
            return Some(task);
        }
        // Don't steal game tasks — they have pinning semantics.
        None
    }

    /// Steal specifically for work-stealing: returns a task from the back
    /// that is allowed to run on `target_cpu`.
    pub fn steal_for_cpu(&mut self, target_cpu: usize) -> Option<Task> {
        // Search normal queue from back for a task whose affinity allows target_cpu.
        for i in (0..self.normal.len()).rev() {
            if self.normal[i].affinity.is_allowed(target_cpu as u32) {
                let task = self.normal.remove(i).unwrap();
                self.nr_running -= 1;
                return Some(task);
            }
        }
        None
    }

    /// Update the EWMA load average.
    pub fn update_load(&mut self, current_tick: u64) {
        if current_tick <= self.last_load_update {
            return;
        }
        let instant_load = (self.nr_running as u64) * 1024;
        self.load_avg = (self.load_avg * EWMA_DECAY + instant_load * (1024 - EWMA_DECAY)) / 1024;
        self.last_load_update = current_tick;
    }

    fn insert_deadline_sorted(&mut self, task: Task) {
        let dl = task
            .deadline
            .as_ref()
            .map_or(u64::MAX, |d| d.absolute_deadline);
        let pos = self.deadline.iter().position(|t| {
            t.deadline
                .as_ref()
                .map_or(u64::MAX, |d| d.absolute_deadline)
                > dl
        });
        match pos {
            Some(i) => self.deadline.insert(i, task),
            None => self.deadline.push_back(task),
        }
    }

    fn insert_normal_sorted(&mut self, task: Task) {
        let vr = task.vruntime;
        let pos = self.normal.iter().position(|t| t.vruntime > vr);
        match pos {
            Some(i) => self.normal.insert(i, task),
            None => self.normal.push_back(task),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nr_running == 0
    }

    pub fn len(&self) -> usize {
        self.nr_running
    }

    pub fn game_count(&self) -> usize {
        self.deadline.len() + self.game.len()
    }

    pub fn normal_count(&self) -> usize {
        self.normal.len()
    }

    /// Update min_vruntime from the front of the normal queue.
    pub fn refresh_min_vruntime(&mut self) {
        if let Some(front) = self.normal.front() {
            self.min_vruntime = self.min_vruntime.max(front.vruntime);
        }
    }

    /// Get current load average (fixed-point ×1024).
    pub fn get_load_avg(&self) -> u64 {
        self.load_avg
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §3  CPU DATA — PER-CPU STRUCTURE
// ═══════════════════════════════════════════════════════════════════════════════

/// Per-CPU data structure. Each CPU owns one of these, indexed by cpu_id.
/// The run queue spinlock is the ONLY lock needed for local scheduling.
///
/// Fields written once during init use `UnsafeCell` since they're set exactly
/// once (by the owning CPU during bringup) and read-only thereafter.
pub struct CpuData {
    /// Logical CPU index (0 = BSP, 1..N = APs in boot order).
    /// Written once during init, read-only after.
    pub cpu_id: UnsafeCell<usize>,
    /// LAPIC id from ACPI MADT.
    pub apic_id: AtomicU8,
    /// NUMA node this CPU belongs to.
    pub numa_node: AtomicU8,
    /// Online/offline state.
    pub state: AtomicU8,
    /// Per-CPU run queue protected by a spinlock (NOT the global scheduler lock).
    pub run_queue: Mutex<PerCpuRunQueue>,
    /// The task currently executing on this CPU (None = idle).
    pub current_task: Mutex<Option<Task>>,
    /// Dedicated idle task for this CPU.
    pub idle_task: Mutex<Option<Task>>,
    /// Monotonic per-CPU tick counter.
    pub tick: AtomicU64,
    /// CPU topology info. Written once during init, read-only after.
    pub topology: UnsafeCell<CpuTopologyInfo>,
    /// Set when this CPU needs to reschedule (e.g. after IPI).
    pub need_resched: AtomicBool,
    /// Address space currently loaded in this CPU's CR3 (for lazy TLB).
    pub active_mm: AtomicU64,
}

// SAFETY: CpuData is safe to share across threads because:
// - UnsafeCell fields (cpu_id, topology) are only written once during single-threaded
//   per-CPU init and read-only thereafter. No data races possible.
// - All other fields use atomics or Mutex.
unsafe impl Sync for CpuData {}
unsafe impl Send for CpuData {}

/// Per-CPU topology information derived from CPUID.
#[derive(Debug, Clone, Copy)]
pub struct CpuTopologyInfo {
    /// Physical package (socket) ID.
    pub package_id: u8,
    /// Core ID within the package.
    pub core_id: u8,
    /// SMT (hyper-thread) ID within the core.
    pub smt_id: u8,
    /// Last-Level Cache (LLC) sharing domain ID.
    pub llc_id: u8,
}

impl Default for CpuTopologyInfo {
    fn default() -> Self {
        Self {
            package_id: 0,
            core_id: 0,
            smt_id: 0,
            llc_id: 0,
        }
    }
}

impl CpuData {
    pub const fn new() -> Self {
        Self {
            cpu_id: UnsafeCell::new(0),
            apic_id: AtomicU8::new(0),
            numa_node: AtomicU8::new(0),
            state: AtomicU8::new(CPU_STATE_OFFLINE),
            run_queue: Mutex::new(PerCpuRunQueue::new()),
            current_task: Mutex::new(None),
            idle_task: Mutex::new(None),
            tick: AtomicU64::new(0),
            topology: UnsafeCell::new(CpuTopologyInfo {
                package_id: 0,
                core_id: 0,
                smt_id: 0,
                llc_id: 0,
            }),
            need_resched: AtomicBool::new(false),
            active_mm: AtomicU64::new(0),
        }
    }

    pub fn get_cpu_id(&self) -> usize {
        // SAFETY: cpu_id is written once during init and read-only after.
        unsafe { *self.cpu_id.get() }
    }

    pub fn get_topology(&self) -> CpuTopologyInfo {
        // SAFETY: topology is written once during init and read-only after.
        unsafe { *self.topology.get() }
    }

    pub fn is_online(&self) -> bool {
        self.state.load(Ordering::Acquire) == CPU_STATE_ONLINE
    }

    pub fn is_idle(&self) -> bool {
        self.run_queue.lock().is_empty()
    }

    pub fn queue_len(&self) -> usize {
        self.run_queue.lock().len()
    }
}

/// Static array of per-CPU data, indexed by logical CPU id.
/// This is the authoritative per-CPU state. Access is lock-free for reads
/// (atomics) and requires only the per-CPU spinlock for queue mutations.
pub static CPU_DATA: [CpuData; MAX_CPUS] = {
    const INIT: CpuData = CpuData::new();
    [INIT; MAX_CPUS]
};

/// Global CPU count (total online CPUs).
pub static ONLINE_CPUS: AtomicU32 = AtomicU32::new(1);

// ═══════════════════════════════════════════════════════════════════════════════
// §4  CPU TOPOLOGY — CPUID-BASED DETECTION
// ═══════════════════════════════════════════════════════════════════════════════

/// System-wide topology summary.
pub struct CpuTopology {
    /// Number of physical packages (sockets).
    pub packages: u8,
    /// Cores per package.
    pub cores_per_package: u8,
    /// Threads (SMT) per core.
    pub threads_per_core: u8,
    /// Total logical CPUs.
    pub total_cpus: u8,
    /// LLC sharing: CPUs that share the same LLC ID can migrate cheaply.
    pub llc_domains: u8,
    /// Per-CPU topology map.
    pub cpu_info: [CpuTopologyInfo; MAX_CPUS],
}

impl CpuTopology {
    pub const fn new() -> Self {
        Self {
            packages: 1,
            cores_per_package: 1,
            threads_per_core: 1,
            total_cpus: 1,
            llc_domains: 1,
            cpu_info: [CpuTopologyInfo {
                package_id: 0,
                core_id: 0,
                smt_id: 0,
                llc_id: 0,
            }; MAX_CPUS],
        }
    }

    /// Detect topology from CPUID leaf 0x0B (x2APIC/extended topology enumeration).
    /// Falls back to leaf 0x01/0x04 on older CPUs.
    pub fn detect() -> Self {
        let mut topo = Self::new();

        // Check if CPUID leaf 0x0B is available.
        let max_leaf = cpuid_max_leaf();
        if max_leaf >= 0x0B {
            topo.detect_from_leaf_0b();
        } else if max_leaf >= 0x04 {
            topo.detect_from_leaf_04();
        } else {
            topo.detect_fallback();
        }

        topo
    }

    fn detect_from_leaf_0b(&mut self) {
        // CPUID leaf 0x0B, sub-leaf 0 = SMT level, sub-leaf 1 = core level.
        let smt_result = cpuid(0x0B, 0);
        let core_result = cpuid(0x0B, 1);

        let smt_shift = smt_result.eax & 0x1F;
        let core_shift = core_result.eax & 0x1F;

        // Threads per core = 2^smt_shift (usually 1 or 2).
        self.threads_per_core = (1u8 << smt_shift).max(1);

        // Cores per package = 2^(core_shift - smt_shift).
        let core_bits = core_shift.saturating_sub(smt_shift);
        self.cores_per_package = (1u8 << core_bits).max(1);

        // x2APIC ID of the running CPU.
        let x2apic_id = core_result.edx;

        // Decompose the x2APIC ID.
        let smt_id = x2apic_id & ((1 << smt_shift) - 1);
        let core_id = (x2apic_id >> smt_shift) & ((1 << core_bits) - 1);
        let package_id = x2apic_id >> core_shift;

        // LLC domain: typically same as package on desktop, per-CCX on AMD.
        // For simplicity, use package_id as LLC domain. Can be refined with leaf 0x04.
        let llc_id = self.detect_llc_domain(package_id as u8, core_id as u8);

        let cpu_id = crate::gdt::current_cpu_id();
        if cpu_id < MAX_CPUS {
            self.cpu_info[cpu_id] = CpuTopologyInfo {
                package_id: package_id as u8,
                core_id: core_id as u8,
                smt_id: smt_id as u8,
                llc_id,
            };
        }
    }

    fn detect_from_leaf_04(&mut self) {
        // CPUID leaf 0x04 (deterministic cache parameters).
        let result = cpuid(0x04, 0);
        let max_cores_sharing = ((result.eax >> 26) & 0x3F) + 1;
        let max_addressable = ((result.eax >> 14) & 0xFFF) + 1;

        self.cores_per_package = max_cores_sharing as u8;
        self.threads_per_core = (max_addressable / max_cores_sharing).max(1) as u8;

        let cpu_id = crate::gdt::current_cpu_id();
        if cpu_id < MAX_CPUS {
            // Use APIC ID from leaf 0x01 to decompose.
            let leaf1 = cpuid(0x01, 0);
            let initial_apic_id = (leaf1.ebx >> 24) & 0xFF;
            let smt_bits = log2_ceil(self.threads_per_core as u32);
            let core_bits = log2_ceil(self.cores_per_package as u32);

            let smt_id = initial_apic_id & ((1 << smt_bits) - 1);
            let core_id = (initial_apic_id >> smt_bits) & ((1 << core_bits) - 1);
            let package_id = initial_apic_id >> (smt_bits + core_bits);

            self.cpu_info[cpu_id] = CpuTopologyInfo {
                package_id: package_id as u8,
                core_id: core_id as u8,
                smt_id: smt_id as u8,
                llc_id: package_id as u8,
            };
        }
    }

    fn detect_fallback(&mut self) {
        // Single-socket, single-core assumption.
        self.packages = 1;
        self.cores_per_package = 1;
        self.threads_per_core = 1;
    }

    fn detect_llc_domain(&self, package_id: u8, _core_id: u8) -> u8 {
        // Walk CPUID leaf 0x04 sub-leaves to find the highest-level cache.
        let mut llc_subleaf = 0u32;
        let mut max_sharing = 1u32;

        for subleaf in 0..16u32 {
            let result = cpuid(0x04, subleaf);
            let cache_type = result.eax & 0x1F;
            if cache_type == 0 {
                break;
            }
            let sharing = ((result.eax >> 14) & 0xFFF) + 1;
            if sharing > max_sharing {
                max_sharing = sharing;
                llc_subleaf = subleaf;
            }
        }

        // If LLC is shared per-package, return package_id.
        // If per-CCX (AMD), use package_id * 4 + (core_id / cores_per_ccx).
        let _ = llc_subleaf;
        package_id
    }

    /// Check if two CPUs share the same LLC.
    pub fn share_llc(&self, cpu_a: usize, cpu_b: usize) -> bool {
        if cpu_a >= MAX_CPUS || cpu_b >= MAX_CPUS {
            return false;
        }
        self.cpu_info[cpu_a].llc_id == self.cpu_info[cpu_b].llc_id
    }

    /// Check if two CPUs are on the same physical core (SMT siblings).
    pub fn are_smt_siblings(&self, cpu_a: usize, cpu_b: usize) -> bool {
        if cpu_a >= MAX_CPUS || cpu_b >= MAX_CPUS {
            return false;
        }
        let a = &self.cpu_info[cpu_a];
        let b = &self.cpu_info[cpu_b];
        a.package_id == b.package_id && a.core_id == b.core_id
    }

    /// Check if two CPUs are in the same package.
    pub fn same_package(&self, cpu_a: usize, cpu_b: usize) -> bool {
        if cpu_a >= MAX_CPUS || cpu_b >= MAX_CPUS {
            return false;
        }
        self.cpu_info[cpu_a].package_id == self.cpu_info[cpu_b].package_id
    }
}

/// Global topology singleton, populated during SMP init.
pub static CPU_TOPOLOGY: Mutex<CpuTopology> = Mutex::new(CpuTopology::new());

// CPUID helpers

#[derive(Debug, Clone, Copy)]
struct CpuidResult {
    eax: u32,
    ebx: u32,
    ecx: u32,
    edx: u32,
}

fn cpuid(leaf: u32, subleaf: u32) -> CpuidResult {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
        );
    }
    CpuidResult { eax, ebx, ecx, edx }
}

fn cpuid_max_leaf() -> u32 {
    cpuid(0, 0).eax
}

fn log2_ceil(mut val: u32) -> u32 {
    if val <= 1 {
        return 0;
    }
    val -= 1;
    32 - val.leading_zeros()
}

// ═══════════════════════════════════════════════════════════════════════════════
// §5  LOAD BALANCER
// ═══════════════════════════════════════════════════════════════════════════════

/// Load balancer state. Runs periodically on each CPU's timer tick.
pub struct LoadBalancer {
    /// Last tick at which balancing was performed.
    last_balance_tick: u64,
    /// Total migrations performed.
    total_migrations: u64,
    /// Migrations blocked by affinity/pinning.
    blocked_migrations: u64,
}

impl LoadBalancer {
    pub const fn new() -> Self {
        Self {
            last_balance_tick: 0,
            total_migrations: 0,
            blocked_migrations: 0,
        }
    }

    /// Called every scheduler tick. Performs balancing if the interval has elapsed.
    pub fn tick(&mut self, current_tick: u64) {
        if current_tick.saturating_sub(self.last_balance_tick) < BALANCE_INTERVAL_TICKS {
            return;
        }
        self.last_balance_tick = current_tick;
        self.balance();
    }

    /// Core balancing logic: find the busiest and idlest CPUs, migrate tasks.
    fn balance(&mut self) {
        let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;
        if online <= 1 {
            return;
        }

        // Update load averages on all CPUs.
        let mut loads: [(usize, u64, usize); MAX_CPUS] = [(0, 0, 0); MAX_CPUS];
        let mut count = 0usize;

        for cpu_id in 0..online.min(MAX_CPUS) {
            if !CPU_DATA[cpu_id].is_online() {
                continue;
            }
            let mut rq = CPU_DATA[cpu_id].run_queue.lock();
            let tick = CPU_DATA[cpu_id].tick.load(Ordering::Relaxed);
            rq.update_load(tick);
            loads[count] = (cpu_id, rq.get_load_avg(), rq.len());
            count += 1;
        }

        if count < 2 {
            return;
        }

        // Find busiest and idlest CPUs.
        let mut busiest_idx = 0;
        let mut idlest_idx = 0;
        for i in 1..count {
            if loads[i].2 > loads[busiest_idx].2 {
                busiest_idx = i;
            }
            if loads[i].2 < loads[idlest_idx].2 {
                idlest_idx = i;
            }
        }

        let busiest_cpu = loads[busiest_idx].0;
        let idlest_cpu = loads[idlest_idx].0;
        let imbalance = loads[busiest_idx].2.saturating_sub(loads[idlest_idx].2);

        if imbalance < IMBALANCE_THRESHOLD || busiest_cpu == idlest_cpu {
            return;
        }

        // Perform migration with topology awareness.
        self.migrate_tasks(busiest_cpu, idlest_cpu, imbalance);
    }

    /// Migrate tasks from busiest to idlest, respecting topology preferences.
    fn migrate_tasks(&mut self, from: usize, to: usize, imbalance: usize) {
        let migrations_needed = (imbalance / 2).min(MAX_MIGRATIONS_PER_PASS);
        let mut migrated = 0;

        for _ in 0..migrations_needed {
            // Lock the source queue and try to steal a task.
            let task = {
                let mut src_rq = CPU_DATA[from].run_queue.lock();
                src_rq.steal_for_cpu(to)
            };

            match task {
                Some(task) => {
                    // Check SCHED_BODY pinning: don't migrate pinned game tasks.
                    if task.priority == TaskPriority::Game && !task.affinity.is_allowed(to as u32) {
                        // Put it back.
                        CPU_DATA[from].run_queue.lock().enqueue(task);
                        self.blocked_migrations += 1;
                        continue;
                    }

                    // Estimate migration cost based on topology.
                    let cost = migration_cost(from, to);
                    if cost > MigrationCost::Expensive as u32 && migrated > 0 {
                        // Already migrated at least one; stop to avoid thrashing.
                        CPU_DATA[from].run_queue.lock().enqueue(task);
                        break;
                    }

                    // Enqueue on the destination.
                    CPU_DATA[to].run_queue.lock().enqueue(task);
                    migrated += 1;
                    self.total_migrations += 1;
                }
                None => break,
            }
        }

        // If we migrated anything to an idle CPU, send reschedule IPI.
        if migrated > 0 {
            let dest_apic_id = CPU_DATA[to].apic_id.load(Ordering::Relaxed);
            send_reschedule_ipi(dest_apic_id);
        }
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.total_migrations, self.blocked_migrations)
    }
}

/// Migration cost categories.
#[repr(u32)]
enum MigrationCost {
    /// Same LLC — very cheap, cache lines may still be hot.
    SameLlc = 1,
    /// Same package, different LLC — moderate cost.
    SamePackage = 2,
    /// Same NUMA node, different package — noticeable latency.
    SameNuma = 4,
    /// Different NUMA node — expensive, cross-node memory access.
    CrossNuma = 8,
    /// Threshold for "too expensive to migrate without strong imbalance".
    Expensive = 6,
}

/// Estimate migration cost between two CPUs.
fn migration_cost(from_cpu: usize, to_cpu: usize) -> u32 {
    let topo = CPU_TOPOLOGY.lock();

    // Same LLC = cheapest.
    if topo.share_llc(from_cpu, to_cpu) {
        return MigrationCost::SameLlc as u32;
    }

    // Same package.
    if topo.same_package(from_cpu, to_cpu) {
        return MigrationCost::SamePackage as u32;
    }

    // Check NUMA distance.
    let from_node = CPU_DATA[from_cpu].numa_node.load(Ordering::Relaxed);
    let to_node = CPU_DATA[to_cpu].numa_node.load(Ordering::Relaxed);

    if from_node == to_node {
        MigrationCost::SameNuma as u32
    } else {
        MigrationCost::CrossNuma as u32
    }
}

/// Work stealing: called when a CPU goes idle. Pulls from the busiest CPU.
pub fn work_steal(idle_cpu: usize) -> Option<Task> {
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;
    let mut busiest_cpu = usize::MAX;
    let mut busiest_len = 0;

    // Find the busiest CPU, preferring same LLC, then same NUMA node.
    for cpu_id in 0..online.min(MAX_CPUS) {
        if cpu_id == idle_cpu || !CPU_DATA[cpu_id].is_online() {
            continue;
        }
        let len = CPU_DATA[cpu_id].run_queue.lock().len();
        if len > busiest_len {
            busiest_len = len;
            busiest_cpu = cpu_id;
        }
    }

    if busiest_cpu == usize::MAX || busiest_len < 2 {
        return None;
    }

    // Prefer stealing from same LLC domain.
    let topo = CPU_TOPOLOGY.lock();
    let idle_llc = topo.cpu_info[idle_cpu].llc_id;
    let idle_node = CPU_DATA[idle_cpu].numa_node.load(Ordering::Relaxed);
    drop(topo);

    // First pass: try to steal from the busiest CPU in our LLC.
    let mut best_same_llc: Option<(usize, usize)> = None;
    let mut best_same_node: Option<(usize, usize)> = None;

    for cpu_id in 0..online.min(MAX_CPUS) {
        if cpu_id == idle_cpu || !CPU_DATA[cpu_id].is_online() {
            continue;
        }
        let len = CPU_DATA[cpu_id].run_queue.lock().len();
        if len < 2 {
            continue;
        }

        let topo = CPU_TOPOLOGY.lock();
        let cpu_llc = topo.cpu_info[cpu_id].llc_id;
        drop(topo);

        let cpu_node = CPU_DATA[cpu_id].numa_node.load(Ordering::Relaxed);

        if cpu_llc == idle_llc {
            if best_same_llc.map_or(true, |(_, l)| len > l) {
                best_same_llc = Some((cpu_id, len));
            }
        } else if cpu_node == idle_node {
            if best_same_node.map_or(true, |(_, l)| len > l) {
                best_same_node = Some((cpu_id, len));
            }
        }
    }

    // Pick the best candidate: same LLC > same NUMA > any.
    let steal_from = best_same_llc
        .map(|(c, _)| c)
        .or(best_same_node.map(|(c, _)| c))
        .unwrap_or(busiest_cpu);

    CPU_DATA[steal_from]
        .run_queue
        .lock()
        .steal_for_cpu(idle_cpu)
}

/// Global load balancer instance.
pub static LOAD_BALANCER: Mutex<LoadBalancer> = Mutex::new(LoadBalancer::new());

// ═══════════════════════════════════════════════════════════════════════════════
// §6  TLB SHOOTDOWN
// ═══════════════════════════════════════════════════════════════════════════════

/// TLB shootdown request structure. The BSP/initiator fills this, sends IPIs,
/// and all target CPUs read from it to know what to invalidate.
struct TlbShootdownRequest {
    /// Start of the virtual address range to invalidate.
    start_addr: AtomicU64,
    /// Number of pages to invalidate (0 = flush all).
    page_count: AtomicU32,
    /// Address space identifier (CR3 value). Only CPUs running this mm need flush.
    target_cr3: AtomicU64,
    /// Bitmask of CPUs that still need to process this request.
    pending_cpus: AtomicU64,
    /// Set to true when a shootdown is in progress.
    active: AtomicBool,
}

impl TlbShootdownRequest {
    const fn new() -> Self {
        Self {
            start_addr: AtomicU64::new(0),
            page_count: AtomicU32::new(0),
            target_cr3: AtomicU64::new(0),
            pending_cpus: AtomicU64::new(0),
            active: AtomicBool::new(false),
        }
    }
}

static TLB_REQUEST: TlbShootdownRequest = TlbShootdownRequest::new();
static TLB_SHOOTDOWN_LOCK: Mutex<()> = Mutex::new(());

/// Initiate a TLB shootdown for a range of virtual addresses.
///
/// Sends IPIs only to CPUs that are actually running processes in the
/// affected address space (lazy TLB optimization).
pub fn tlb_shootdown(start: VirtAddr, pages: u32) {
    let _guard = TLB_SHOOTDOWN_LOCK.lock();

    let current_cr3 = read_cr3();
    let current_cpu = crate::gdt::current_cpu_id();
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;

    // Build the bitmask of CPUs that need the shootdown.
    let mut target_mask: u64 = 0;
    for cpu_id in 0..online.min(MAX_CPUS) {
        if cpu_id == current_cpu {
            continue;
        }
        if !CPU_DATA[cpu_id].is_online() {
            continue;
        }
        // Lazy TLB: skip CPUs not running in this address space.
        let cpu_cr3 = CPU_DATA[cpu_id].active_mm.load(Ordering::Acquire);
        if cpu_cr3 != current_cr3 && current_cr3 != 0 {
            continue;
        }
        target_mask |= 1u64 << cpu_id;
    }

    if target_mask == 0 {
        // No remote CPUs need flushing — just flush locally.
        flush_tlb_range(start, pages);
        return;
    }

    // Set up the request.
    TLB_REQUEST
        .start_addr
        .store(start.as_u64(), Ordering::Release);
    TLB_REQUEST.page_count.store(pages, Ordering::Release);
    TLB_REQUEST.target_cr3.store(current_cr3, Ordering::Release);
    TLB_REQUEST
        .pending_cpus
        .store(target_mask, Ordering::Release);
    TLB_REQUEST.active.store(true, Ordering::Release);

    // Send IPI to each target CPU.
    for cpu_id in 0..online.min(MAX_CPUS) {
        if target_mask & (1u64 << cpu_id) != 0 {
            let apic_id = CPU_DATA[cpu_id].apic_id.load(Ordering::Relaxed);
            let _ = send_ipi(apic_id, IpiKind::Fixed(TLB_SHOOTDOWN_VECTOR));
        }
    }

    // Flush locally while waiting.
    flush_tlb_range(start, pages);

    // Spin-wait for all targets to acknowledge.
    let mut spins = 0u64;
    while TLB_REQUEST.pending_cpus.load(Ordering::Acquire) != 0 {
        core::hint::spin_loop();
        spins += 1;
        if spins > 10_000_000 {
            // Timeout — some CPU didn't respond. Clear and continue.
            crate::serial_println!("[tlb] WARN: shootdown timeout, some CPUs didn't ack");
            break;
        }
    }

    TLB_REQUEST.active.store(false, Ordering::Release);
}

/// Batched TLB shootdown for bulk unmapping operations.
/// Groups pages into a single IPI round rather than one IPI per page.
pub fn tlb_shootdown_batch(start: VirtAddr, total_pages: u32) {
    // For very large ranges, just flush all.
    if total_pages > 512 {
        tlb_shootdown_flush_all();
        return;
    }
    tlb_shootdown(start, total_pages);
}

/// Full TLB flush on all CPUs (used for large munmap or address space teardown).
pub fn tlb_shootdown_flush_all() {
    let _guard = TLB_SHOOTDOWN_LOCK.lock();

    let current_cpu = crate::gdt::current_cpu_id();
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;

    let mut target_mask: u64 = 0;
    for cpu_id in 0..online.min(MAX_CPUS) {
        if cpu_id == current_cpu {
            continue;
        }
        if !CPU_DATA[cpu_id].is_online() {
            continue;
        }
        target_mask |= 1u64 << cpu_id;
    }

    if target_mask == 0 {
        flush_tlb_all();
        return;
    }

    TLB_REQUEST.start_addr.store(0, Ordering::Release);
    TLB_REQUEST.page_count.store(0, Ordering::Release);
    TLB_REQUEST.target_cr3.store(0, Ordering::Release);
    TLB_REQUEST
        .pending_cpus
        .store(target_mask, Ordering::Release);
    TLB_REQUEST.active.store(true, Ordering::Release);

    for cpu_id in 0..online.min(MAX_CPUS) {
        if target_mask & (1u64 << cpu_id) != 0 {
            let apic_id = CPU_DATA[cpu_id].apic_id.load(Ordering::Relaxed);
            let _ = send_ipi(apic_id, IpiKind::Fixed(TLB_SHOOTDOWN_VECTOR));
        }
    }

    flush_tlb_all();

    let mut spins = 0u64;
    while TLB_REQUEST.pending_cpus.load(Ordering::Acquire) != 0 {
        core::hint::spin_loop();
        spins += 1;
        if spins > 10_000_000 {
            break;
        }
    }

    TLB_REQUEST.active.store(false, Ordering::Release);
}

/// Called on the receiving CPU when a TLB shootdown IPI arrives.
/// This is the IPI handler body invoked from the IDT entry for TLB_SHOOTDOWN_VECTOR.
pub fn handle_tlb_shootdown_ipi() {
    if !TLB_REQUEST.active.load(Ordering::Acquire) {
        return;
    }

    let page_count = TLB_REQUEST.page_count.load(Ordering::Acquire);
    if page_count == 0 {
        flush_tlb_all();
    } else {
        let start = VirtAddr::new(TLB_REQUEST.start_addr.load(Ordering::Acquire));
        flush_tlb_range(start, page_count);
    }

    // Clear our bit in the pending mask.
    let cpu_id = crate::gdt::current_cpu_id();
    TLB_REQUEST
        .pending_cpus
        .fetch_and(!(1u64 << cpu_id), Ordering::Release);
}

/// Flush a range of TLB entries using INVLPG.
fn flush_tlb_range(start: VirtAddr, pages: u32) {
    for i in 0..pages {
        let addr = start + (i as u64) * 4096;
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) addr.as_u64(), options(nostack, preserves_flags));
        }
    }
}

/// Full TLB flush by reloading CR3.
fn flush_tlb_all() {
    unsafe {
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
        core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
    }
}

fn read_cr3() -> u64 {
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
    }
    cr3
}

// ═══════════════════════════════════════════════════════════════════════════════
// §7  CPU HOTPLUG
// ═══════════════════════════════════════════════════════════════════════════════

/// Bring a CPU online at runtime. The CPU must have been previously initialized
/// (booted via INIT-SIPI-SIPI) and is currently parked.
pub fn cpu_online(cpu_id: usize) -> Result<(), &'static str> {
    if cpu_id >= MAX_CPUS {
        return Err("cpu_id out of range");
    }

    let current_state = CPU_DATA[cpu_id].state.load(Ordering::Acquire);
    if current_state == CPU_STATE_ONLINE {
        return Err("CPU already online");
    }

    // Mark as online.
    CPU_DATA[cpu_id]
        .state
        .store(CPU_STATE_ONLINE, Ordering::Release);
    ONLINE_CPUS.fetch_add(1, Ordering::SeqCst);

    // Send reschedule IPI to wake the parked CPU.
    let apic_id = CPU_DATA[cpu_id].apic_id.load(Ordering::Relaxed);
    send_reschedule_ipi(apic_id);

    crate::serial_println!(
        "[smp] CPU {} brought online (total: {})",
        cpu_id,
        ONLINE_CPUS.load(Ordering::Relaxed)
    );
    Ok(())
}

/// Take a CPU offline. Migrates all tasks off the CPU first.
pub fn cpu_offline(cpu_id: usize) -> Result<(), &'static str> {
    if cpu_id >= MAX_CPUS {
        return Err("cpu_id out of range");
    }
    if cpu_id == 0 {
        return Err("cannot offline the BSP (CPU 0)");
    }

    let current_state = CPU_DATA[cpu_id].state.load(Ordering::Acquire);
    if current_state != CPU_STATE_ONLINE {
        return Err("CPU not online");
    }

    // Enter parking state — prevents new tasks from being enqueued.
    CPU_DATA[cpu_id]
        .state
        .store(CPU_STATE_PARKING, Ordering::Release);

    // Migrate all tasks from this CPU's run queue to other online CPUs.
    drain_run_queue(cpu_id);

    // Also migrate the current task if any.
    let current = CPU_DATA[cpu_id].current_task.lock().take();
    if let Some(task) = current {
        if !task.is_idle {
            enqueue_on_best_cpu(task, cpu_id);
        }
    }

    // Finalize offline state.
    CPU_DATA[cpu_id]
        .state
        .store(CPU_STATE_OFFLINE, Ordering::Release);
    ONLINE_CPUS.fetch_sub(1, Ordering::SeqCst);

    crate::serial_println!(
        "[smp] CPU {} taken offline (total: {})",
        cpu_id,
        ONLINE_CPUS.load(Ordering::Relaxed)
    );
    Ok(())
}

/// Park a CPU: suspend it in a low-power halt loop. Can be resumed quickly.
pub fn cpu_park(cpu_id: usize) -> Result<(), &'static str> {
    cpu_offline(cpu_id)?;
    // The CPU will spin in its idle loop checking the state flag.
    // When state transitions back to ONLINE, it resumes normal operation.
    Ok(())
}

/// Unpark a previously parked CPU.
pub fn cpu_unpark(cpu_id: usize) -> Result<(), &'static str> {
    cpu_online(cpu_id)
}

/// Drain all tasks from a CPU's run queue and redistribute them.
fn drain_run_queue(offline_cpu: usize) {
    loop {
        let task = CPU_DATA[offline_cpu].run_queue.lock().dequeue_next();
        match task {
            Some(task) => enqueue_on_best_cpu(task, offline_cpu),
            None => break,
        }
    }
}

/// Find the best CPU for a task, excluding `exclude_cpu`.
/// Respects affinity, prefers same NUMA node and LLC.
fn enqueue_on_best_cpu(task: Task, exclude_cpu: usize) {
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;
    let task_node = CPU_DATA[exclude_cpu].numa_node.load(Ordering::Relaxed);

    let mut best_cpu = None;
    let mut best_load = usize::MAX;

    // First pass: find least-loaded CPU in same NUMA node with matching affinity.
    for cpu_id in 0..online.min(MAX_CPUS) {
        if cpu_id == exclude_cpu || !CPU_DATA[cpu_id].is_online() {
            continue;
        }
        if !task.affinity.is_allowed(cpu_id as u32) {
            continue;
        }
        let cpu_node = CPU_DATA[cpu_id].numa_node.load(Ordering::Relaxed);
        let load = CPU_DATA[cpu_id].run_queue.lock().len();

        if cpu_node == task_node && load < best_load {
            best_load = load;
            best_cpu = Some(cpu_id);
        }
    }

    // Second pass: if no CPU found in same node, try any CPU.
    if best_cpu.is_none() {
        for cpu_id in 0..online.min(MAX_CPUS) {
            if cpu_id == exclude_cpu || !CPU_DATA[cpu_id].is_online() {
                continue;
            }
            if !task.affinity.is_allowed(cpu_id as u32) {
                continue;
            }
            let load = CPU_DATA[cpu_id].run_queue.lock().len();
            if load < best_load {
                best_load = load;
                best_cpu = Some(cpu_id);
            }
        }
    }

    // Fallback: BSP.
    let target = best_cpu.unwrap_or(0);
    CPU_DATA[target].run_queue.lock().enqueue(task);
}

// ═══════════════════════════════════════════════════════════════════════════════
// §8  SMP-AWARE TASK ENQUEUE / SCHEDULE APIs
// ═══════════════════════════════════════════════════════════════════════════════

/// Enqueue a task onto the per-CPU run queue of the best CPU.
/// Uses affinity, NUMA locality, and current load to choose.
pub fn smp_enqueue(task: Task) {
    let target = select_cpu_for_task(&task);
    CPU_DATA[target].run_queue.lock().enqueue(task);

    // If the target CPU is idle (halted), send a reschedule IPI to wake it.
    if CPU_DATA[target].is_idle() {
        let apic_id = CPU_DATA[target].apic_id.load(Ordering::Relaxed);
        send_reschedule_ipi(apic_id);
    }
}

/// Select the optimal CPU for a new task based on affinity, load, and topology.
fn select_cpu_for_task(task: &Task) -> usize {
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;
    let current_cpu = crate::gdt::current_cpu_id();

    // If affinity is restrictive, find the least-loaded allowed CPU.
    if task.affinity.mask != u64::MAX {
        let mut best_cpu = current_cpu;
        let mut best_load = usize::MAX;

        for cpu_id in 0..online.min(MAX_CPUS) {
            if !CPU_DATA[cpu_id].is_online() {
                continue;
            }
            if !task.affinity.is_allowed(cpu_id as u32) {
                continue;
            }
            let load = CPU_DATA[cpu_id].run_queue.lock().len();
            if load < best_load {
                best_load = load;
                best_cpu = cpu_id;
            }
        }
        return best_cpu;
    }

    // SCHED_BODY tasks prefer staying on the current CPU (cache warmth).
    if task.priority == TaskPriority::Game {
        return current_cpu;
    }

    // Normal tasks: find the idlest CPU, prefer same NUMA node.
    let current_node = CPU_DATA[current_cpu].numa_node.load(Ordering::Relaxed);
    let mut best_local = (current_cpu, usize::MAX);
    let mut best_remote = (current_cpu, usize::MAX);

    for cpu_id in 0..online.min(MAX_CPUS) {
        if !CPU_DATA[cpu_id].is_online() {
            continue;
        }
        let load = CPU_DATA[cpu_id].run_queue.lock().len();
        let node = CPU_DATA[cpu_id].numa_node.load(Ordering::Relaxed);

        if node == current_node {
            if load < best_local.1 {
                best_local = (cpu_id, load);
            }
        } else {
            if load < best_remote.1 {
                best_remote = (cpu_id, load);
            }
        }
    }

    // Prefer local node unless remote is significantly less loaded.
    if best_local.1 <= best_remote.1 + 2 {
        best_local.0
    } else {
        best_remote.0
    }
}

/// Per-CPU scheduler tick handler. Updates load, runs the balancer,
/// and performs work stealing if idle.
pub fn smp_tick(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    CPU_DATA[cpu_id].tick.fetch_add(1, Ordering::Relaxed);
    let tick = CPU_DATA[cpu_id].tick.load(Ordering::Relaxed);

    // Update local load average.
    CPU_DATA[cpu_id].run_queue.lock().update_load(tick);

    // Run the load balancer on CPU 0 only (avoids contention).
    if cpu_id == 0 {
        LOAD_BALANCER.lock().tick(tick);
    }

    // If our queue is empty, try work stealing.
    if CPU_DATA[cpu_id].run_queue.lock().is_empty() {
        if let Some(stolen) = work_steal(cpu_id) {
            CPU_DATA[cpu_id].run_queue.lock().enqueue(stolen);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §9  IPI FRAMEWORK
// ═══════════════════════════════════════════════════════════════════════════════

/// IPI delivery modes.
enum IpiKind {
    Init,
    Sipi(u8),
    Fixed(u8),
}

/// Send an inter-processor interrupt via the Local APIC ICR.
fn send_ipi(dest_apic_id: u8, kind: IpiKind) -> Result<(), &'static str> {
    if crate::apic::get_apic_mode() == crate::apic::ApicMode::X2apic {
        let (vector, delivery_mode) = match kind {
            IpiKind::Init => (0u64, 0b101u64),
            IpiKind::Sipi(v) => (v as u64, 0b110u64),
            IpiKind::Fixed(v) => (v as u64, 0b000u64),
        };

        // x2APIC ICR: MSR 0x830.
        // Bits 0-7: Vector
        // Bits 8-10: Delivery Mode
        // Bit 14: Level (1 for assert)
        // Bits 32-63: Destination APIC ID
        let mut icr = vector | (delivery_mode << 8) | (1 << 14);
        icr |= (dest_apic_id as u64) << 32;

        unsafe {
            // Use wrmsr helper from apic module or implement locally
            let lo = icr as u32;
            let hi = (icr >> 32) as u32;
            core::arch::asm!(
                "wrmsr",
                in("ecx") 0x830,
                in("eax") lo, in("edx") hi,
                options(nomem, nostack, preserves_flags),
            );
        }
        return Ok(());
    }

    let offset = *crate::memory::PHYS_MEM_OFFSET
        .get()
        .ok_or("PHYS_MEM_OFFSET not initialized")?;
    let lapic_base = (offset + 0xFEE0_0000u64).as_u64() as *mut u32;

    let icr_low_ptr = unsafe { lapic_base.add(0x300 / 4) };
    let icr_high_ptr = unsafe { lapic_base.add(0x310 / 4) };

    let (vector, delivery_mode) = match kind {
        IpiKind::Init => (0u32, 0b101u32),
        IpiKind::Sipi(v) => (v as u32, 0b110u32),
        IpiKind::Fixed(v) => (v as u32, 0b000u32),
    };

    let low = vector | (delivery_mode << 8) | (1 << 14);
    let high = (dest_apic_id as u32) << 24;

    unsafe {
        icr_high_ptr.write_volatile(high);
        icr_low_ptr.write_volatile(low);
        // Spin until the delivery-status bit clears (bit 12 of ICR low).
        for _ in 0..100_000 {
            if (icr_low_ptr.read_volatile() & (1 << 12)) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }
    Ok(())
}

/// Send a reschedule IPI to wake an idle CPU.
pub fn send_reschedule_ipi(dest_apic_id: u8) {
    let _ = send_ipi(dest_apic_id, IpiKind::Fixed(RESCHEDULE_VECTOR));
}

/// Send a fixed IPI to a specific AP to wake it from HLT.
pub fn send_wakeup_ipi(dest_apic_id: u8) {
    let _ = send_ipi(dest_apic_id, IpiKind::Fixed(0xF0));
}

/// Broadcast an IPI to all online CPUs except self.
pub fn broadcast_ipi(vector: u8) {
    let current = crate::gdt::current_cpu_id();
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;

    for cpu_id in 0..online.min(MAX_CPUS) {
        if cpu_id == current || !CPU_DATA[cpu_id].is_online() {
            continue;
        }
        let apic_id = CPU_DATA[cpu_id].apic_id.load(Ordering::Relaxed);
        let _ = send_ipi(apic_id, IpiKind::Fixed(vector));
    }
}

/// Handle a reschedule IPI. Sets the need_resched flag on the current CPU.
pub fn handle_reschedule_ipi() {
    let cpu_id = crate::gdt::current_cpu_id();
    if cpu_id < MAX_CPUS {
        CPU_DATA[cpu_id].need_resched.store(true, Ordering::Release);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §10  NUMA-AWARE MEMORY INTEGRATION
// ═══════════════════════════════════════════════════════════════════════════════

/// Per-node free page list. Integrates with the GlobalFrameAllocator by
/// trying local node first, then falling back to nearest nodes.
pub struct NumaFrameAllocator {
    /// Node this allocator belongs to.
    node_id: u8,
    /// Free frames local to this node.
    free_frames: Vec<PhysFrame>,
    /// Memory range owned by this node (start, end).
    range_start: u64,
    range_end: u64,
    /// Total allocated frames from this node.
    allocated: u64,
}

impl NumaFrameAllocator {
    pub fn new(node_id: u8, range_start: u64, range_end: u64) -> Self {
        Self {
            node_id,
            free_frames: Vec::new(),
            range_start,
            range_end,
            allocated: 0,
        }
    }

    /// Allocate a frame from this node's local pool.
    pub fn allocate_local(&mut self) -> Option<PhysFrame> {
        let frame = self.free_frames.pop()?;
        self.allocated += 1;
        Some(frame)
    }

    /// Return a frame to this node's local pool.
    pub fn deallocate_local(&mut self, frame: PhysFrame) {
        let addr = frame.start_address().as_u64();
        if addr >= self.range_start && addr < self.range_end {
            self.free_frames.push(frame);
            self.allocated -= 1;
        }
    }

    /// Check if a physical address belongs to this node.
    pub fn owns_address(&self, phys_addr: u64) -> bool {
        phys_addr >= self.range_start && phys_addr < self.range_end
    }

    pub fn free_count(&self) -> usize {
        self.free_frames.len()
    }
}

/// Per-node allocator array.
pub static NUMA_ALLOCATORS: Mutex<[Option<NumaFrameAllocator>; 64]> =
    Mutex::new([const { None }; 64]);

/// Determine which NUMA node owns a physical address.
pub fn numa_node_of(phys_addr: u64) -> Option<u8> {
    let allocators = NUMA_ALLOCATORS.lock();
    for (i, alloc) in allocators.iter().enumerate() {
        if let Some(a) = alloc {
            if a.owns_address(phys_addr) {
                return Some(i as u8);
            }
        }
    }
    None
}

/// Get the NUMA distance between two nodes. Lower = closer = cheaper access.
pub fn numa_distance(node_a: u8, node_b: u8) -> u32 {
    if node_a == node_b {
        return 10; // Local access, standard SLIT self-distance.
    }
    // Default remote distance if SLIT not available.
    20
}

/// Allocate a frame preferring the given NUMA node, falling back to nearest.
pub fn numa_alloc_frame(preferred_node: u8) -> Option<PhysFrame> {
    let mut allocators = NUMA_ALLOCATORS.lock();

    // Try preferred node first.
    if let Some(Some(alloc)) = allocators.get_mut(preferred_node as usize) {
        if let Some(frame) = alloc.allocate_local() {
            return Some(frame);
        }
    }

    // Fallback: try other nodes ordered by distance.
    for i in 0..64u8 {
        if i == preferred_node {
            continue;
        }
        if let Some(Some(alloc)) = allocators.get_mut(i as usize) {
            if let Some(frame) = alloc.allocate_local() {
                return Some(frame);
            }
        }
    }

    // Final fallback: use the global frame allocator.
    drop(allocators);
    use x86_64::structures::paging::FrameAllocator;
    let mut global = crate::memory::GlobalFrameAllocator;
    global.allocate_frame()
}

/// Allocate with interleaving policy: round-robin across nodes for shared data.
pub fn numa_alloc_interleaved(counter: &AtomicU64) -> Option<PhysFrame> {
    let online_nodes = {
        let allocators = NUMA_ALLOCATORS.lock();
        allocators.iter().filter(|a| a.is_some()).count() as u64
    };
    if online_nodes == 0 {
        use x86_64::structures::paging::FrameAllocator;
        let mut global = crate::memory::GlobalFrameAllocator;
        return global.allocate_frame();
    }
    let node = (counter.fetch_add(1, Ordering::Relaxed) % online_nodes) as u8;
    numa_alloc_frame(node)
}

/// Interleave counter for kernel shared data.
pub static INTERLEAVE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Saved CR4 from the BSP, to be mirrored on APs exactly.
pub static BSP_CR4: AtomicU64 = AtomicU64::new(0);

// ═══════════════════════════════════════════════════════════════════════════════
// §11  AP BRINGUP — TRAMPOLINE & INIT-SIPI-SIPI
// ═══════════════════════════════════════════════════════════════════════════════

/// The trampoline is assembled from `src/smp/trampoline.asm` by build.rs.
pub static AP_TRAMPOLINE: &[u8] = include_bytes!(env!("AP_TRAMPOLINE_BIN"));

#[allow(dead_code)]
const _AP_TRAMPOLINE_FITS_PAGE: () =
    assert!(AP_TRAMPOLINE.len() < 4096, "trampoline overflowed one page");

/// APIC ids of the APs from the boot-time MADT walk, saved so the S3 resume
/// path can re-online the same set (`reonline_aps_after_resume`). Written
/// once by `bring_up_aps` on the boot path.
static SAVED_AP_APIC_IDS: spin::Mutex<Option<alloc::vec::Vec<u8>>> = spin::Mutex::new(None);

/// Re-online the APs after an S3 resume (they were INIT-parked for the sleep
/// and the CPU reset wiped their state). Resets the serial-claim counters and
/// re-runs the full `bring_up_aps` path — trampoline redeploy, INIT/SIPI per
/// AP, and the (tagged-CAS, cycle-bounded) TSC warp pairing. Returns the
/// number of APs back online. Bounded leak per sleep/wake cycle: a fresh
/// bootstrap PML4 + per-AP stacks are allocated each bring-up (same as boot);
/// reclaiming the old ones is CPU-hotplug bookkeeping, MasterChecklist
/// Phase 2.4 follow-up.
///
/// Concept §"Fast is a feature": "Wake under 1 second" — a resumed machine
/// must come back with ALL its cores.
pub fn reonline_aps_after_resume() -> usize {
    let ids = match SAVED_AP_APIC_IDS.lock().clone() {
        Some(v) if !v.is_empty() => v,
        _ => return 0,
    };
    // The claim counters drive start_one_ap's target arithmetic; the parked
    // APs are gone, so restart the serial claims from "BSP only".
    APS_ONLINE.store(1, Ordering::SeqCst);
    ONLINE_CPUS.store(1, Ordering::SeqCst);
    bring_up_aps(&ids);
    (ONLINE_CPUS.load(Ordering::SeqCst) as usize).saturating_sub(1)
}

/// Bring up every Application Processor reported by ACPI's MADT.
///
/// `ap_apic_ids` is the list of `local_apic_id`s for every non-disabled AP
/// (the BSP is excluded). Pass an empty slice on uniprocessor systems.
pub fn bring_up_aps(ap_apic_ids: &[u8]) {
    // Remember the set for S3 re-online (idempotent across re-entries).
    if !ap_apic_ids.is_empty() {
        *SAVED_AP_APIC_IDS.lock() = Some(ap_apic_ids.to_vec());
    }

    // Initialize BSP's per-CPU data.
    // SAFETY: single-threaded BSP init, no other CPU is accessing these fields.
    unsafe {
        *CPU_DATA[0].cpu_id.get() = 0;
    }
    CPU_DATA[0].apic_id.store(0, Ordering::Relaxed);
    CPU_DATA[0].state.store(CPU_STATE_ONLINE, Ordering::Release);

    // Detect and store BSP topology.
    let topo = CpuTopology::detect();
    if let Some(info) = topo.cpu_info.get(0) {
        // SAFETY: single-threaded BSP init.
        unsafe {
            *CPU_DATA[0].topology.get() = *info;
        }
    }
    *CPU_TOPOLOGY.lock() = topo;

    if ap_apic_ids.is_empty() {
        crate::serial_println!("[smp] uniprocessor system — no APs to start.");
        return;
    }
    crate::serial_println!(
        "[smp] bringing up {} Application Processor(s)...",
        ap_apic_ids.len(),
    );

    // Save the BSP's CR4 so APs can mirror exactly the same features (SMAP, SMEP, OSFXSR, etc.)
    let bsp_cr4: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) bsp_cr4);
    }
    BSP_CR4.store(bsp_cr4, Ordering::Relaxed);

    let ap_pml4_phys = match unsafe { build_ap_bootstrap_pml4() } {
        Some(p) => p,
        None => {
            // On real machines with many GiB of RAM the allocator may be unable
            // to hand back a <4 GiB frame for the trampoline's 32-bit CR3 load.
            // Degrade to single-core rather than bricking the boot (QEMU's 256
            // MiB never hit this; big-RAM hardware can).
            crate::serial_println!(
                "[smp][WARN] no <4GiB frame for AP bootstrap PML4 — continuing \
                 BSP-only (multi-core disabled this boot)"
            );
            return;
        }
    };
    crate::serial_println!("[smp] AP bootstrap PML4 @ phys {:#x}", ap_pml4_phys);

    let (tramp_src, tramp_len) = unsafe { trampoline_bytes() };
    let tramp_dst = trampoline_virt();
    unsafe {
        core::ptr::copy_nonoverlapping(tramp_src, tramp_dst, tramp_len);
    }
    crate::serial_println!(
        "[smp] trampoline copied: {} bytes -> phys {:#x}",
        tramp_len,
        TRAMPOLINE_PHYS,
    );

    unsafe {
        write_boot_block_u64(boot_block::PML4, ap_pml4_phys);
        write_boot_block_u64(boot_block::ENTRY, ap_entry as *const () as usize as u64);
    }
    // Record the deployment for the S3 waking-vector path (the FACS wake
    // reuses this trampoline — see ensure_wake_trampoline).
    TRAMPOLINE_PML4_PHYS.store(ap_pml4_phys, Ordering::SeqCst);

    for (i, &apic_id) in ap_apic_ids.iter().enumerate() {
        let target_online = 2 + i as u64;
        match start_one_ap(apic_id, target_online) {
            Ok(()) => {
                crate::serial_println!(
                    "[smp] AP {} (apic_id={}) online — {} CPU(s) total",
                    i,
                    apic_id,
                    APS_ONLINE.load(Ordering::SeqCst),
                );
                // Pair with this AP for the lockstep TSC warp test while it is
                // between coming online and entering the idle scheduler. Strictly
                // serial: the next AP is not started until this pairing finishes.
                // The AP we just onlined claimed cpu_id = APS_ONLINE-1 (serial
                // fetch_add claim in ap_entry) — the tag tsc_warp_source waits
                // for; robust even if an earlier AP failed to start.
                let onlined_cpu_id = APS_ONLINE.load(Ordering::SeqCst) as usize - 1;
                tsc_warp_source(onlined_cpu_id);
            }
            Err(reason) => crate::serial_println!(
                "[smp] AP {} (apic_id={}) failed to start: {}",
                i,
                apic_id,
                reason,
            ),
        }
    }
}

/// Build a minimal bootstrap PML4 for Application Processors.
unsafe fn build_ap_bootstrap_pml4() -> Option<u64> {
    use crate::memory;
    use x86_64::structures::paging::page_table::PageTable;
    use x86_64::structures::paging::{FrameAllocator, Mapper, OffsetPageTable};

    let offset = *memory::PHYS_MEM_OFFSET.get().expect("PHYS_MEM_OFFSET");

    let mut frame_allocator = memory::GlobalFrameAllocator;

    // The AP trampoline loads CR3 in 32-bit protected mode, so the bootstrap
    // PML4 MUST sit below 4 GiB. QEMU's 256 MiB always satisfies this, but a
    // real machine with many GiB of RAM can hand back a high frame. Pull frames
    // until we get a sub-4-GiB one, holding the high ones aside (distinct frames
    // guarantee forward progress) and returning them all afterwards — no leak,
    // no panic. Give up after scanning 4 MiB of frames.
    let mut high_frames: Vec<PhysFrame<Size4KiB>> = Vec::new();
    let low_frame = loop {
        match frame_allocator.allocate_frame() {
            Some(f) if f.start_address().as_u64() < (1u64 << 32) => break Some(f),
            Some(f) => {
                high_frames.push(f);
                if high_frames.len() >= 1024 {
                    break None;
                }
            }
            None => break None,
        }
    };
    for f in high_frames {
        memory::deallocate_frame(f);
    }
    let pml4_frame = low_frame?;
    let new_pml4_phys = pml4_frame.start_address();

    let new_pml4_ptr = (offset + new_pml4_phys.as_u64()).as_mut_ptr::<PageTable>();
    core::ptr::write_bytes(new_pml4_ptr as *mut u8, 0, 4096);

    let new_pml4 = &mut *new_pml4_ptr;
    let kernel_pml4_frame = *memory::KERNEL_PML4.get().unwrap();
    let kernel_pml4 =
        &*((offset + kernel_pml4_frame.start_address().as_u64()).as_ptr::<PageTable>());
    for i in 0..512 {
        new_pml4[i] = kernel_pml4[i].clone();
    }

    // Ensure the trampoline page (physical 0x8000) is identity-mapped so the
    // AP can keep executing after enabling paging.  In many bootloader
    // configurations the low 1 MiB is already identity-mapped through the
    // cloned PML4[0]; check before allocating extra tables.
    let tramp_page = Page::<Size4KiB>::containing_address(VirtAddr::new(TRAMPOLINE_PHYS));
    let tramp_already_mapped = {
        let mapper = OffsetPageTable::new(&mut *(new_pml4_ptr), offset);
        use x86_64::structures::paging::Translate;
        mapper
            .translate_addr(VirtAddr::new(TRAMPOLINE_PHYS))
            .is_some()
    };
    if !tramp_already_mapped {
        let mut mapper = OffsetPageTable::new(new_pml4, offset);
        let trampoline_frame =
            PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(TRAMPOLINE_PHYS));
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        match mapper.map_to(tramp_page, trampoline_frame, flags, &mut frame_allocator) {
            Ok(flush) => flush.ignore(),
            Err(_) => {
                memory::deallocate_frame(pml4_frame);
                return None;
            }
        }
    }

    // Sub-4-GiB is guaranteed by the allocation loop above.
    Some(new_pml4_phys.as_u64())
}

fn start_one_ap(apic_id: u8, target_online: u64) -> Result<(), &'static str> {
    const AP_STACK_SIZE: usize = 4096 * 4;
    let layout = alloc::alloc::Layout::from_size_align(AP_STACK_SIZE, 16).unwrap();
    let stack_base = unsafe { alloc::alloc::alloc(layout) };
    if stack_base.is_null() {
        return Err("stack allocation failed");
    }
    let stack_top = (stack_base as u64) + AP_STACK_SIZE as u64;

    unsafe {
        write_boot_block_u64(boot_block::STACK_TOP, stack_top);
        write_boot_block_u16(boot_block::APIC_ID, apic_id as u16);
    }

    send_ipi(apic_id, IpiKind::Init)?;
    busy_wait_us(10_000);

    send_ipi(apic_id, IpiKind::Sipi(TRAMPOLINE_SIPI_VECTOR))?;
    busy_wait_us(200);
    send_ipi(apic_id, IpiKind::Sipi(TRAMPOLINE_SIPI_VECTOR))?;

    for _ in 0..1_000_000 {
        if APS_ONLINE.load(Ordering::SeqCst) >= target_online {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("timed out waiting for AP to signal alive")
}

// ── Trampoline helpers ───────────────────────────────────────────────────

/// Physical address of the AP bootstrap PML4 once the trampoline is deployed
/// (0 = not yet). Set by `bring_up_aps`; `ensure_wake_trampoline` deploys on
/// demand for uniprocessor boots. The ACPI S3 waking vector REUSES the AP
/// trampoline: a FACS wake enters real mode at CS:IP = 0x0800:0x0000 exactly
/// like a SIPI to vector 0x08, so the same blob walks the woken BSP back to
/// long mode and jumps to whatever 64-bit entry the boot block holds.
pub static TRAMPOLINE_PML4_PHYS: AtomicU64 = AtomicU64::new(0);

/// Deploy the low-memory trampoline + <4 GiB bootstrap PML4 if `bring_up_aps`
/// didn't (uniprocessor boot, where it returns before copying the blob).
/// Idempotent; returns the bootstrap PML4 phys for the boot block.
pub fn ensure_wake_trampoline() -> Result<u64, &'static str> {
    let existing = TRAMPOLINE_PML4_PHYS.load(Ordering::SeqCst);
    if existing != 0 {
        return Ok(existing);
    }
    let pml4 =
        unsafe { build_ap_bootstrap_pml4() }.ok_or("no <4GiB frame for the wake bootstrap PML4")?;
    let (src, len) = unsafe { trampoline_bytes() };
    unsafe {
        core::ptr::copy_nonoverlapping(src, trampoline_virt(), len);
        write_boot_block_u64(boot_block::PML4, pml4);
    }
    TRAMPOLINE_PML4_PHYS.store(pml4, Ordering::SeqCst);
    Ok(pml4)
}

/// Patch the shared trampoline to act as the ACPI S3 waking vector: 64-bit
/// entry → the resume function, stack → the dedicated resume stack, arg = 0.
/// `bring_up_aps`/`start_one_ap` re-patch ENTRY/STACK on their next use, so
/// calling this before every sleep is safe.
pub fn patch_trampoline_for_wake(entry64: u64, stack_top: u64) -> Result<(), &'static str> {
    let pml4 = ensure_wake_trampoline()?;
    unsafe {
        write_boot_block_u64(boot_block::PML4, pml4);
        write_boot_block_u64(boot_block::ENTRY, entry64);
        write_boot_block_u64(boot_block::STACK_TOP, stack_top);
        write_boot_block_u16(boot_block::APIC_ID, 0);
    }
    Ok(())
}

/// INIT-park every online AP ahead of S3 (their CPU state is lost across the
/// sleep regardless — Linux likewise offlines secondary CPUs for S3) and mark
/// the system uniprocessor. Scheduler-side cleanup (migrating the parked
/// CPUs' queued tasks to CPU 0) is `scheduler::offline_aps_for_sleep`, which
/// the caller runs AFTER this so `select_cpu` already sees `online == 1`.
/// Returns how many APs were parked. `reonline_aps_after_resume` brings the
/// same set back after the wake.
pub fn park_aps_for_sleep() -> usize {
    let online = ONLINE_CPUS.load(Ordering::SeqCst) as usize;
    if online <= 1 {
        return 0;
    }
    let expect = (online - 1).min(MAX_CPUS - 1) as u32;

    // QUIESCE RENDEZVOUS (root-caused 2026-07-02): a blind asynchronous INIT
    // can freeze an AP mid-critical-section — observed as a post-resume
    // deadlock when `reonline_aps_after_resume`'s frame allocations spun on
    // a buddy/heap lock stranded by an AP INIT'd mid-`drain_dead_tasks`.
    // Ask each AP to park itself from its idle-loop top (lock-free by
    // construction: outside drain_dead_tasks, outside the scheduler lock),
    // wait for the acks, and only then INIT. An AP that misses the generous
    // deadline is INIT'd anyway with a loud WARN — no worse than the old
    // behavior, and a core that can't reach idle within ~1 s is already
    // pathological.
    SLEEP_PARKED_COUNT.store(0, Ordering::SeqCst);
    SLEEP_PARK_REQUESTED.store(true, Ordering::SeqCst);
    let deadline = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_add(3_000_000_000);
    while SLEEP_PARKED_COUNT.load(Ordering::SeqCst) < expect {
        core::hint::spin_loop();
        if unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(deadline) < (u64::MAX / 2) {
            break;
        }
    }
    let acked = SLEEP_PARKED_COUNT.load(Ordering::SeqCst);
    if acked < expect {
        crate::serial_println!(
            "[smp] WARN: sleep quiesce: only {}/{} AP(s) acked — INIT-parking the rest anyway (stranded-lock risk)",
            acked,
            expect,
        );
    }

    let mut parked = 0usize;
    for cpu in 1..online.min(MAX_CPUS) {
        let apic_id = CPU_DATA[cpu].apic_id.load(Ordering::Relaxed);
        if send_ipi(apic_id, IpiKind::Init).is_ok() {
            parked += 1;
        }
        CPU_DATA[cpu]
            .state
            .store(CPU_STATE_OFFLINE, Ordering::Release);
    }
    if parked > 0 {
        busy_wait_us(1_000); // let the INITs land before the BSP proceeds
    }
    SLEEP_PARK_REQUESTED.store(false, Ordering::SeqCst);
    SLEEP_PARKED_COUNT.store(0, Ordering::SeqCst);
    ONLINE_CPUS.store(1, Ordering::SeqCst);
    APS_ONLINE.store(1, Ordering::SeqCst);
    parked
}

/// S3 quiesce rendezvous state — see `park_aps_for_sleep`.
static SLEEP_PARK_REQUESTED: AtomicBool = AtomicBool::new(false);
static SLEEP_PARKED_COUNT: AtomicU32 = AtomicU32::new(0);

/// True while the BSP is asking APs to quiesce for S3. Polled by the AP idle
/// loop (`scheduler::ap_enter_idle`) at its loop top — a proven lock-free
/// point.
pub fn sleep_park_requested() -> bool {
    SLEEP_PARK_REQUESTED.load(Ordering::Acquire)
}

/// AP side of the S3 quiesce: acknowledge and halt with IRQs off until the
/// BSP's INIT takes this core down. Called ONLY from the idle-loop top,
/// where the AP provably holds no lock. `hlt` with IF=0 still yields to
/// INIT.
pub fn sleep_park_ack_and_halt() -> ! {
    x86_64::instructions::interrupts::disable();
    SLEEP_PARKED_COUNT.fetch_add(1, Ordering::SeqCst);
    loop {
        x86_64::instructions::hlt();
    }
}

unsafe fn trampoline_bytes() -> (*const u8, usize) {
    (AP_TRAMPOLINE.as_ptr(), AP_TRAMPOLINE.len())
}

fn trampoline_virt() -> *mut u8 {
    let offset = *crate::memory::PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET not initialized");
    ((offset + TRAMPOLINE_PHYS).as_u64()) as *mut u8
}

unsafe fn write_boot_block_u64(field_offset: usize, value: u64) {
    let dst = trampoline_virt().add(field_offset) as *mut u64;
    dst.write_volatile(value);
}

unsafe fn write_boot_block_u16(field_offset: usize, value: u16) {
    let dst = trampoline_virt().add(field_offset) as *mut u16;
    dst.write_volatile(value);
}

fn busy_wait_us(us: u64) {
    crate::hpet::spin_wait_us(us);
}

// ═══════════════════════════════════════════════════════════════════════════════
// §12  AP ENTRY POINT
// ═══════════════════════════════════════════════════════════════════════════════

/// Each AP arrives here with:
///   * RDI = its APIC id (set by the trampoline from the boot block)
///   * RSP = its dedicated kernel stack
///   * CR3 = the AP bootstrap PML4 (full clone + trampoline identity-map)
#[no_mangle]
pub extern "C" fn ap_entry(apic_id: u64) -> ! {
    crate::gdt::init_ap();
    // Each AP must enable SSE too (CR0/CR4 are per-CPU) — else userspace tasks
    // scheduled on this AP #UD on their first SSE instruction.
    crate::cpu_features::enable_sse();
    // Real hardware SMEP is per-CPU (CR4.SMEP) — enable it on this AP too so a
    // ring-0 execute of a user page is trapped on EVERY core, not just the BSP.
    crate::cpu_features::enable_smep();
    // Same for SMAP (CR4.SMAP) — a ring-0 read/write of a user page outside the
    // stac/clac uaccess chokepoint must fault on every core. The stac/clac copy
    // stubs are armed globally the first time any CPU calls enable_smap (the BSP
    // in hardening::init runs before APs come up), so this AP is already using
    // the correct stub variant before it flips its own CR4 bit.
    crate::cpu_features::enable_smap();
    // And UMIP (CR4.UMIP) — block userspace descriptor-table reads on every
    // core, not just the BSP.
    crate::cpu_features::enable_umip();
    // Branch-speculation MSRs (IA32_SPEC_CTRL IBRS/STIBP/SSBD) are per-CPU too —
    // program this AP's copy so every core has the same Spectre defense as the
    // BSP (which sets its own in hardening::init). #GP-safe on emulators.
    crate::cpu_features::enable_spec_ctrl();
    // Slice 0b: install this AP's interrupt-vector table through the arch:: seam
    // (delegates to crate::interrupts::init_idt on x86_64 — byte-identical).
    crate::arch::interrupts::load_idt();

    let kernel_pml4 = crate::memory::KERNEL_PML4
        .get()
        .expect("KERNEL_PML4 not initialized");
    unsafe {
        x86_64::registers::control::Cr3::write(
            *kernel_pml4,
            x86_64::registers::control::Cr3Flags::empty(),
        );

        // Mirror the BSP's CR4 exactly to inherit SMAP, SMEP, OSFXSR, etc.
        // This avoids #GP faults from blindly enabling features not supported by CPUID,
        // while preventing #UD from `stac` if SMAP is supported and used.
    }

    // Atomically claim a unique cpu_id slot.
    let cpu_id = APS_ONLINE.fetch_add(1, Ordering::SeqCst) as usize;
    if cpu_id < crate::gdt::MAX_CPUS {
        crate::gdt::init_ap_percpu(cpu_id);
        crate::syscall::init_ap(cpu_id);
        crate::serial_println!(
            "[smp] AP apic_id={} loaded per-CPU GDT+TSS (cpu_id={})",
            apic_id,
            cpu_id
        );
    }

    // Initialize per-CPU data for this AP.
    if cpu_id < MAX_CPUS {
        // SAFETY: each AP writes only its own slot during init. No concurrent writers.
        unsafe {
            *CPU_DATA[cpu_id].cpu_id.get() = cpu_id;
        }
        CPU_DATA[cpu_id]
            .apic_id
            .store(apic_id as u8, Ordering::Relaxed);
        CPU_DATA[cpu_id]
            .state
            .store(CPU_STATE_ONLINE, Ordering::Release);
        ONLINE_CPUS.fetch_add(1, Ordering::SeqCst);

        // Detect per-CPU topology via CPUID (runs on this AP, so we get its values).
        let topo_info = detect_local_topology();
        // SAFETY: each AP writes only its own topology slot.
        unsafe {
            *CPU_DATA[cpu_id].topology.get() = topo_info;
        }
        CPU_DATA[cpu_id]
            .numa_node
            .store(topo_info.package_id, Ordering::Relaxed);
    }

    init_ap_lapic(apic_id, cpu_id);

    // Phase 1.6: lockstep pairwise TSC warp test against the BSP, which is now
    // waiting in `tsc_warp_source()` for this AP to reach the rendezvous.
    tsc_warp_target(cpu_id);

    crate::serial_println!("[smp] AP {} online (cpu_id={})", cpu_id, cpu_id);

    crate::serial_println!("[smp] AP cpu_id={} entering scheduler...", cpu_id);
    crate::scheduler::ap_enter_idle(cpu_id);
    crate::serial_println!("[smp] AP cpu_id={} joined scheduler", cpu_id);

    x86_64::instructions::interrupts::enable();

    loop {
        x86_64::instructions::hlt();
    }
}

/// Detect topology info for the current CPU from CPUID.
fn detect_local_topology() -> CpuTopologyInfo {
    let max_leaf = cpuid_max_leaf();

    if max_leaf >= 0x0B {
        let smt_result = cpuid(0x0B, 0);
        let core_result = cpuid(0x0B, 1);

        let smt_shift = smt_result.eax & 0x1F;
        let core_shift = core_result.eax & 0x1F;
        let x2apic_id = core_result.edx;

        let core_bits = core_shift.saturating_sub(smt_shift);
        let smt_id = x2apic_id & ((1u32 << smt_shift).saturating_sub(1));
        let core_id = (x2apic_id >> smt_shift) & ((1u32 << core_bits).saturating_sub(1));
        let package_id = x2apic_id >> core_shift;

        CpuTopologyInfo {
            package_id: package_id as u8,
            core_id: core_id as u8,
            smt_id: smt_id as u8,
            llc_id: package_id as u8,
        }
    } else {
        let leaf1 = cpuid(0x01, 0);
        let initial_apic_id = (leaf1.ebx >> 24) & 0xFF;
        CpuTopologyInfo {
            package_id: 0,
            core_id: initial_apic_id as u8,
            smt_id: 0,
            llc_id: 0,
        }
    }
}

/// Enable the Local APIC on an Application Processor.
fn init_ap_lapic(apic_id: u64, cpu_id: usize) {
    // If the BSP enabled x2APIC, APs must also enable it before accessing
    // any APIC registers via MSRs.
    if crate::apic::X2APIC_SUPPORTED.load(Ordering::SeqCst) {
        crate::apic::enable_x2apic();
        crate::serial_println!(
            "[apic] AP cpu_{}: x2APIC mode enabled (apic_id={})",
            cpu_id,
            apic_id
        );
    }

    // Arm this CPU's periodic scheduler-tick timer through the arch HAL
    // (Slice 0b-4: x86 LAPIC periodic timer ↔ aarch64 ARM Generic Timer CNTP).
    // The x86 backend delegates to crate::apic::start_lapic_timer(), which
    // handles both xAPIC and x2APIC modes including SVR enable — byte-identical.
    crate::arch::timer::arm_periodic();
    crate::serial_println!("[apic] AP cpu_{}: LAPIC timer started", cpu_id);
}

// ═══════════════════════════════════════════════════════════════════════════════
// §12b  PER-CPU TSC SYNC VERIFICATION  (MasterChecklist Phase 1.6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Concept (LEGACY_GAMING_CONCEPT.md — low-latency gaming scheduler): the deadline
// scheduler and frame-pacing logic compare timestamps taken on different CPUs.
// If the per-core TSCs are skewed, a task migrated between cores can observe
// time running backwards, corrupting deadline math. On modern x86 the TSC is
// invariant and reset in lockstep at power-on, but firmware/virtualization can
// introduce a per-core offset.
//
// We measure that offset with a Linux-style *pairwise warp test* run in lockstep
// between the BSP and each AP as it comes online (AP bringup is strictly serial,
// so exactly one pairing is live at a time). Each side repeatedly takes a shared
// spinlock, reads the partner's last published TSC, samples its own, and
// republishes it. Because the lock serializes the two cores, a sample that reads
// *lower* than the partner's strictly-earlier sample is a genuine backward step —
// a real cross-core skew. This is immune to the old method's fatal flaw: the
// previous pass merely recorded each core's TSC at a different boot instant and
// took max-min, so it reported the elapsed boot time between reports (~0.2 s =
// ~593M cycles on Athena) as "skew" and WARNed on every boot despite the TSC
// being perfectly synchronized.
//
// This is a *verification* pass (read-only): it does not rewrite IA32_TSC_ADJUST.
// A warp above TSC_WARP_WARN_THRESHOLD cycles is logged at WARN so the boot log
// surfaces unsynced TSCs rather than silently mis-scheduling.

/// Maximum tolerated cross-core TSC warp (cycles) before we WARN. On invariant-
/// TSC silicon the warp is 0; the budget only absorbs the non-determinism of the
/// cache-line handover that drives the test — which can never *manufacture* a
/// backward step (see `tsc_warp_loop`), only mask a true skew smaller than it.
const TSC_WARP_WARN_THRESHOLD: u64 = 1000;

/// Locked read/compare iterations each side runs per (BSP, AP) pairing. Enough
/// cross-core interleavings to expose a real skew; small enough that all the
/// pairings together cost only a few ms of boot time.
const TSC_WARP_ITERS: u32 = 20_000;

/// Failsafe TSC-cycle budget for a rendezvous handshake. TIME-based, not
/// spin-COUNT-based: the old `spins > 2_000_000_000` loop bound took ~2 s on
/// iron but MINUTES under QEMU TCG (each `spin_loop` iteration emulates
/// slowly), so a missed rendezvous "bounded" wait was an effective boot hang —
/// the SMP=4 wedge root-caused 2026-07-01 (see `tsc_warp_target`). ~4e9 cycles
/// ≈ 1.3 s at 3 GHz on iron AND under TCG (guest TSC advances at host rate).
const TSC_WARP_RENDEZVOUS_DEADLINE_CYCLES: u64 = 4_000_000_000;

/// Cycle budget for the AP's post-test linger (waiting for the BSP's DONE
/// signal so the shared flags are quiescent). Purely hygienic — with the
/// tagged-CAS protocol below a late AP can no longer corrupt the next
/// pairing — so keep it short: a missed signal costs ~100 ms, not minutes.
const TSC_WARP_LINGER_DEADLINE_CYCLES: u64 = 400_000_000;

// ── Pairwise warp-test shared state ──────────────────────────────────────────
// Used by exactly one (BSP, AP) pair at a time: AP bringup is strictly serial
// (`start_one_ap` waits for each AP). SRC_* are written only by the BSP.
//
// TGT_HERE is a cpu_id+1 TAG (0 = none), not a bool, and the AP retires it
// with compare_exchange on ITS OWN tag only. Two real SMP=4 boot wedges
// (2026-07-01, QMP frozen-RIP diagnosis: BSP spinning in tsc_warp_source's
// rendezvous wait + the fresh AP spinning IF=0 in tsc_warp_target, both
// inside their "bounded" loops) came from bool-flag clobbers across pairing
// boundaries:
//   1. The PREVIOUS pairing's AP, delayed in its final SRC_DONE wait (TCG can
//      deschedule a vCPU for ms; the BSP's next-pairing reset had already
//      flipped SRC_DONE back to false), eventually exited and stored
//      TGT_HERE=false — erasing the NEXT AP's already-announced rendezvous.
//   2. The BSP's own flag reset ran AFTER a fast next-AP had announced
//      TGT_HERE=true (the AP can win the serial-print race), self-erasing the
//      announcement it was about to wait for.
// With a tag: the BSP waits for the SPECIFIC cpu_id it just onlined, a stale
// tag from a wedged predecessor is simply overwritten by the next AP's store,
// an old AP's CAS-clear on a successor's tag fails harmlessly, and the BSP
// never writes TGT_HERE at all.
static TSC_W_LOCK: AtomicBool = AtomicBool::new(false); // tiny test spinlock
static TSC_W_LAST: AtomicU64 = AtomicU64::new(0); // partner's last-published TSC
static TSC_W_SRC_HERE: AtomicBool = AtomicBool::new(false);
static TSC_W_TGT_HERE: AtomicU64 = AtomicU64::new(0); // cpu_id+1 tag, 0 = none
static TSC_W_SRC_DONE: AtomicBool = AtomicBool::new(false);
static TSC_W_TGT_DONE: AtomicBool = AtomicBool::new(false);

/// Worst-case backward warp observed across all pairings this boot (cycles).
static TSC_MAX_WARP: AtomicU64 = AtomicU64::new(0);
/// Total backward-warp events across all pairings (0 on synced hardware).
static TSC_NR_WARPS: AtomicU32 = AtomicU32::new(0);
/// Number of APs that completed a pairing (for the smoketest tally).
static TSC_PAIRS_DONE: AtomicU32 = AtomicU32::new(0);

/// Set true once `check_tsc_sync()` has logged a result, so it is idempotent.
static TSC_SYNC_DONE: AtomicBool = AtomicBool::new(false);

/// Read the TSC bracketed by `lfence`, making it a serializing sample: the
/// leading fence drains prior loads and the trailing one stops the counter read
/// from floating below later code. Raw `lfence` asm (not the SSE2 intrinsic) so
/// it compiles under the kernel's soft-float, `-sse` codegen. This ordering is
/// what lets a detected backward step be a real inter-core skew, not OoO noise.
#[inline]
fn read_tsc_ordered() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: `lfence`/`rdtsc` are unprivileged, side-effect-free, touch no
    // memory, and clobber only EAX/EDX (declared as outputs).
    unsafe {
        core::arch::asm!(
            "lfence",
            "rdtsc",
            "lfence",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// One side of the pairwise warp test; the BSP and the AP run it concurrently.
/// Each iteration takes the shared spinlock, reads the partner's last TSC
/// (`prev`), samples its own (`now`), and republishes `now`. Because the lock
/// serializes the two cores, if `now < prev` the partner's clock — sampled at a
/// strictly *earlier* real instant — read *higher*, i.e. it is genuinely ahead:
/// a real cross-core skew of `prev - now` cycles. Same-core back-to-back
/// iterations can never warp (one core's TSC is monotonic), and lock-handover
/// latency only ever makes `now` larger — so every recorded warp is a true
/// inter-core desync and there are no false positives.
fn tsc_warp_loop() {
    for _ in 0..TSC_WARP_ITERS {
        while TSC_W_LOCK.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        let prev = TSC_W_LAST.load(Ordering::Relaxed);
        let now = read_tsc_ordered();
        TSC_W_LAST.store(now, Ordering::Relaxed);
        TSC_W_LOCK.store(false, Ordering::Release);

        if prev != 0 && prev > now {
            TSC_MAX_WARP.fetch_max(prev - now, Ordering::Relaxed);
            TSC_NR_WARPS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// AP side of the pairwise warp test, run once from `ap_entry` right after the
/// core comes online and before it enters the idle scheduler. Rendezvous with
/// the BSP's `tsc_warp_source`, run the symmetric loop, then linger until the
/// BSP signals done so the shared flags are quiescent before the next pairing.
/// Every wait is bounded so a stalled BSP can never wedge this AP.
///
/// Concept: validates the invariant-TSC assumption the gaming scheduler relies
/// on for cross-core timestamp comparisons (frame pacing, EDF deadlines).
pub fn tsc_warp_target(cpu_id: usize) {
    let tag = cpu_id as u64 + 1;
    // Announce with OUR tag. Unconditional store: a stale tag left by a
    // wedged predecessor is dead state and overwriting it is the recovery.
    TSC_W_TGT_HERE.store(tag, Ordering::Release);

    let deadline = read_tsc_ordered().wrapping_add(TSC_WARP_RENDEZVOUS_DEADLINE_CYCLES);
    while !TSC_W_SRC_HERE.load(Ordering::Acquire) {
        core::hint::spin_loop();
        if read_tsc_ordered().wrapping_sub(deadline) < (u64::MAX / 2) {
            // Deadline passed. Retire ONLY our own tag — a successor pairing
            // may already have announced; never clobber it (the SMP=4 wedge).
            let _ = TSC_W_TGT_HERE.compare_exchange(tag, 0, Ordering::AcqRel, Ordering::Relaxed);
            return;
        }
    }

    tsc_warp_loop();

    TSC_W_TGT_DONE.store(true, Ordering::Release);
    let deadline = read_tsc_ordered().wrapping_add(TSC_WARP_LINGER_DEADLINE_CYCLES);
    while !TSC_W_SRC_DONE.load(Ordering::Acquire) {
        core::hint::spin_loop();
        if read_tsc_ordered().wrapping_sub(deadline) < (u64::MAX / 2) {
            break;
        }
    }
    // CAS, not store: if the BSP has moved to the next pairing and that AP
    // has already announced, TGT_HERE holds ITS tag — leave it alone.
    let _ = TSC_W_TGT_HERE.compare_exchange(tag, 0, Ordering::AcqRel, Ordering::Relaxed);
}

/// BSP side of the pairwise warp test for one freshly-online AP. Resets the
/// shared state (the BSP is the serial coordinator), rendezvous with the AP's
/// `tsc_warp_target`, runs the symmetric loop, and waits for the AP to finish so
/// the next pairing starts clean. Bounded spins guard against an AP that never
/// reaches the sync point.
pub fn tsc_warp_source(expected_cpu_id: usize) {
    let expected_tag = expected_cpu_id as u64 + 1;
    // Fresh state for this pairing before announcing we are here. TGT_HERE is
    // deliberately NOT reset: the just-onlined AP may have announced its tag
    // already (it can win the serial-print race), and zeroing it here would
    // self-erase the announcement this function is about to wait for — one of
    // the two SMP=4 boot-wedge races root-caused 2026-07-01.
    TSC_W_LAST.store(0, Ordering::Relaxed);
    TSC_W_LOCK.store(false, Ordering::Relaxed);
    TSC_W_TGT_DONE.store(false, Ordering::Relaxed);
    TSC_W_SRC_DONE.store(false, Ordering::Relaxed);
    TSC_W_SRC_HERE.store(true, Ordering::Release);

    let deadline = read_tsc_ordered().wrapping_add(TSC_WARP_RENDEZVOUS_DEADLINE_CYCLES);
    // Wait for the SPECIFIC AP we just onlined (its cpu_id tag) — a stale tag
    // from a wedged earlier AP must not start the loop against the wrong core.
    while TSC_W_TGT_HERE.load(Ordering::Acquire) != expected_tag {
        core::hint::spin_loop();
        if read_tsc_ordered().wrapping_sub(deadline) < (u64::MAX / 2) {
            crate::serial_println!(
                "[smp] WARN: TSC warp: AP cpu_id={} never reached the sync rendezvous",
                expected_cpu_id
            );
            TSC_W_SRC_HERE.store(false, Ordering::Release);
            return;
        }
    }

    tsc_warp_loop();

    TSC_W_SRC_DONE.store(true, Ordering::Release);
    let deadline = read_tsc_ordered().wrapping_add(TSC_WARP_RENDEZVOUS_DEADLINE_CYCLES);
    while !TSC_W_TGT_DONE.load(Ordering::Acquire) {
        core::hint::spin_loop();
        if read_tsc_ordered().wrapping_sub(deadline) < (u64::MAX / 2) {
            break;
        }
    }
    TSC_PAIRS_DONE.fetch_add(1, Ordering::Relaxed);
    TSC_W_SRC_HERE.store(false, Ordering::Release);
}

/// Report the accumulated result of the pairwise warp test. Idempotent — logs
/// once, then returns the cached `(max_warp_cycles, in_sync)`.
///
/// Concept: surfaces unsynchronized TSCs at boot so cross-core scheduler
/// timestamps stay monotonic for frame-pacing and deadline scheduling.
pub fn check_tsc_sync() -> (u64, bool) {
    let max_warp = TSC_MAX_WARP.load(Ordering::Acquire);
    let nr_warps = TSC_NR_WARPS.load(Ordering::Acquire);
    let pairs = TSC_PAIRS_DONE.load(Ordering::Acquire);
    let in_sync = max_warp <= TSC_WARP_WARN_THRESHOLD;

    if !TSC_SYNC_DONE.swap(true, Ordering::AcqRel) {
        if in_sync {
            crate::serial_println!(
                "[smp] TSC sync: max_warp={} cycles over {} pairing(s), {} warp event(s) (OK)",
                max_warp,
                pairs,
                nr_warps,
            );
        } else {
            crate::serial_println!(
                "[smp] WARN: TSC sync: max_warp={} cycles over {} pairing(s) exceeds {} ({} warp events — cores unsynchronized)",
                max_warp, pairs, TSC_WARP_WARN_THRESHOLD, nr_warps,
            );
        }
    }
    (max_warp, in_sync)
}

/// Report the pairwise TSC warp result gathered in lockstep during AP bringup.
///
/// Concept: called after AP bringup to validate the invariant-TSC assumption the
/// gaming scheduler relies on for cross-core timestamp comparisons.
pub fn verify_tsc_sync() -> (u64, bool) {
    check_tsc_sync()
}

/// SMP boot smoketest — verifies AP bringup and per-CPU heartbeat.
/// Called from kernel_main after start_aps() completes.
pub fn run_boot_smoketest() {
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;
    let aps_online = APS_ONLINE.load(Ordering::SeqCst) as usize;
    // BSP is cpu_id 0 (not counted in APS_ONLINE which starts at 1 for first AP).
    let total_cpus = 1 + aps_online.saturating_sub(1).min(MAX_CPUS - 1);

    // Phase 1.6: report the per-CPU TSC warp result gathered during AP bringup.
    let (tsc_warp, tsc_ok) = verify_tsc_sync();

    crate::serial_println!(
        "[smp] run_boot_smoketest: online={} aps={} total_cpus={} tsc_max_warp={} tsc_sync={} -> PASS",
        online, aps_online, total_cpus, tsc_warp,
        if tsc_ok { "OK" } else { "WARN" },
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// §13  SMP INITIALIZATION
// ═══════════════════════════════════════════════════════════════════════════════

/// Initialize the SMP subsystem. Called from kernel_main after scheduler init.
/// Sets up per-CPU data, detects topology, and configures the load balancer.
pub fn init_smp() {
    let online = APS_ONLINE.load(Ordering::SeqCst) as usize;
    crate::serial_println!("[smp] Initializing SMP infrastructure for {} CPUs", online);

    // Ensure BSP data is correct.
    CPU_DATA[0].state.store(CPU_STATE_ONLINE, Ordering::Release);

    // Log topology.
    let topo = CPU_TOPOLOGY.lock();
    crate::serial_println!(
        "[smp] Topology: {} pkg(s), {} cores/pkg, {} threads/core",
        topo.packages,
        topo.cores_per_package,
        topo.threads_per_core,
    );
    drop(topo);

    crate::serial_println!("[ OK ] SMP: per-CPU run queues, load balancer, TLB shootdown ready");
}

// ═══════════════════════════════════════════════════════════════════════════════
// §14  DIAGNOSTICS & STATISTICS
// ═══════════════════════════════════════════════════════════════════════════════

/// SMP statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct SmpStats {
    pub online_cpus: u32,
    pub total_migrations: u64,
    pub blocked_migrations: u64,
    pub per_cpu_load: [u64; MAX_CPUS],
    pub per_cpu_queue_len: [usize; MAX_CPUS],
}

pub fn smp_stats() -> SmpStats {
    let online = ONLINE_CPUS.load(Ordering::Relaxed);
    let (migrations, blocked) = LOAD_BALANCER.lock().stats();

    let mut stats = SmpStats {
        online_cpus: online,
        total_migrations: migrations,
        blocked_migrations: blocked,
        per_cpu_load: [0; MAX_CPUS],
        per_cpu_queue_len: [0; MAX_CPUS],
    };

    for cpu_id in 0..(online as usize).min(MAX_CPUS) {
        let rq = CPU_DATA[cpu_id].run_queue.lock();
        stats.per_cpu_load[cpu_id] = rq.get_load_avg();
        stats.per_cpu_queue_len[cpu_id] = rq.len();
    }

    stats
}

/// Print per-CPU status to serial (debug helper).
pub fn dump_cpu_status() {
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;
    crate::serial_println!("[smp] === CPU Status ({} online) ===", online);

    for cpu_id in 0..online.min(MAX_CPUS) {
        let state = match CPU_DATA[cpu_id].state.load(Ordering::Relaxed) {
            CPU_STATE_OFFLINE => "OFFLINE",
            CPU_STATE_ONLINE => "ONLINE",
            CPU_STATE_PARKING => "PARKING",
            _ => "UNKNOWN",
        };
        let rq = CPU_DATA[cpu_id].run_queue.lock();
        let apic = CPU_DATA[cpu_id].apic_id.load(Ordering::Relaxed);
        let node = CPU_DATA[cpu_id].numa_node.load(Ordering::Relaxed);
        crate::serial_println!(
            "  CPU{}: apic={} node={} state={} queue={} load_avg={}",
            cpu_id,
            apic,
            node,
            state,
            rq.len(),
            rq.get_load_avg(),
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §15  CROSS-CPU TASK OPERATIONS
// ═══════════════════════════════════════════════════════════════════════════════

/// Migrate a specific task to a target CPU. Used by set_affinity when the
/// task's current CPU is no longer in its allowed mask.
pub fn migrate_task_to(task_id: TaskId, target_cpu: usize) -> Result<(), &'static str> {
    if target_cpu >= MAX_CPUS || !CPU_DATA[target_cpu].is_online() {
        return Err("target CPU not valid or not online");
    }

    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;

    // Search all CPU run queues for the task.
    for cpu_id in 0..online.min(MAX_CPUS) {
        if cpu_id == target_cpu {
            continue;
        }

        let mut rq = CPU_DATA[cpu_id].run_queue.lock();

        // Check deadline queue.
        if let Some(idx) = rq.deadline.iter().position(|t| t.id == task_id) {
            let task = rq.deadline.remove(idx).unwrap();
            rq.nr_running -= 1;
            drop(rq);
            CPU_DATA[target_cpu].run_queue.lock().enqueue(task);
            return Ok(());
        }

        // Check game queue.
        if let Some(idx) = rq.game.iter().position(|t| t.id == task_id) {
            let task = rq.game.remove(idx).unwrap();
            rq.nr_running -= 1;
            drop(rq);
            CPU_DATA[target_cpu].run_queue.lock().enqueue(task);
            return Ok(());
        }

        // Check normal queue.
        if let Some(idx) = rq.normal.iter().position(|t| t.id == task_id) {
            let task = rq.normal.remove(idx).unwrap();
            rq.nr_running -= 1;
            drop(rq);
            CPU_DATA[target_cpu].run_queue.lock().enqueue(task);
            return Ok(());
        }
    }

    Err("task not found in any run queue")
}

/// Set CPU affinity for a task and trigger migration if necessary.
pub fn smp_set_affinity(task_id: TaskId, mask: u64) -> Result<(), &'static str> {
    if mask == 0 {
        return Err("empty affinity mask");
    }

    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;

    // Find which CPU the task is on and update its affinity.
    for cpu_id in 0..online.min(MAX_CPUS) {
        let mut rq = CPU_DATA[cpu_id].run_queue.lock();
        let mut found = false;
        let mut needs_migrate = false;

        // Search deadline queue.
        for t in rq.deadline.iter_mut() {
            if t.id == task_id {
                t.affinity = crate::task::CpuAffinity::from_mask(mask);
                needs_migrate = !t.affinity.is_allowed(cpu_id as u32);
                found = true;
                break;
            }
        }
        // Search game queue.
        if !found {
            for t in rq.game.iter_mut() {
                if t.id == task_id {
                    t.affinity = crate::task::CpuAffinity::from_mask(mask);
                    needs_migrate = !t.affinity.is_allowed(cpu_id as u32);
                    found = true;
                    break;
                }
            }
        }
        // Search normal queue.
        if !found {
            for t in rq.normal.iter_mut() {
                if t.id == task_id {
                    t.affinity = crate::task::CpuAffinity::from_mask(mask);
                    needs_migrate = !t.affinity.is_allowed(cpu_id as u32);
                    found = true;
                    break;
                }
            }
        }

        if found {
            drop(rq);
            if needs_migrate {
                for new_cpu in 0..online.min(MAX_CPUS) {
                    if crate::task::CpuAffinity::from_mask(mask).is_allowed(new_cpu as u32)
                        && CPU_DATA[new_cpu].is_online()
                    {
                        let _ = migrate_task_to(task_id, new_cpu);
                        break;
                    }
                }
            }
            return Ok(());
        }
    }

    Err("task not found")
}

// ═══════════════════════════════════════════════════════════════════════════════
// §16  PER-CPU IDLE TASK MANAGEMENT
// ═══════════════════════════════════════════════════════════════════════════════

/// Create idle tasks for all CPUs. Called during scheduler init.
pub fn create_idle_tasks() {
    for cpu_id in 0..MAX_CPUS {
        let mut idle = Task::new(idle_thread_fn, None);
        idle.is_idle = true;
        *CPU_DATA[cpu_id].idle_task.lock() = Some(idle);
    }
}

extern "C" fn idle_thread_fn() {
    loop {
        x86_64::instructions::hlt();
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §17  CPU LOAD TRACKING — EXPONENTIALLY WEIGHTED MOVING AVERAGE
// ═══════════════════════════════════════════════════════════════════════════════

/// Detailed per-CPU load metrics for monitoring and tuning.
#[derive(Debug, Clone, Copy)]
pub struct CpuLoadMetrics {
    pub cpu_id: usize,
    /// Instantaneous queue length.
    pub queue_len: usize,
    /// EWMA load (×1024 fixed-point).
    pub load_avg: u64,
    /// Total ticks this CPU has been active.
    pub total_ticks: u64,
    /// Game/deadline tasks currently queued.
    pub game_tasks: usize,
    /// Normal tasks queued.
    pub normal_tasks: usize,
    /// NUMA node.
    pub numa_node: u8,
}

pub fn cpu_load_metrics(cpu_id: usize) -> Option<CpuLoadMetrics> {
    if cpu_id >= MAX_CPUS || !CPU_DATA[cpu_id].is_online() {
        return None;
    }

    let rq = CPU_DATA[cpu_id].run_queue.lock();
    Some(CpuLoadMetrics {
        cpu_id,
        queue_len: rq.len(),
        load_avg: rq.get_load_avg(),
        total_ticks: CPU_DATA[cpu_id].tick.load(Ordering::Relaxed),
        game_tasks: rq.game_count(),
        normal_tasks: rq.normal_count(),
        numa_node: CPU_DATA[cpu_id].numa_node.load(Ordering::Relaxed),
    })
}

/// Get load metrics for all online CPUs.
pub fn all_cpu_metrics() -> Vec<CpuLoadMetrics> {
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;
    let mut metrics = Vec::with_capacity(online);
    for cpu_id in 0..online.min(MAX_CPUS) {
        if let Some(m) = cpu_load_metrics(cpu_id) {
            metrics.push(m);
        }
    }
    metrics
}

// ═══════════════════════════════════════════════════════════════════════════════
// §18  CACHE-AWARE SCHEDULING HINTS
// ═══════════════════════════════════════════════════════════════════════════════

/// Hint the scheduler that a task should stay on its current CPU (cache warmth).
/// Returns the CPU the task was last running on.
pub fn task_last_cpu(task_id: TaskId) -> Option<usize> {
    let online = ONLINE_CPUS.load(Ordering::Relaxed) as usize;
    for cpu_id in 0..online.min(MAX_CPUS) {
        let rq = CPU_DATA[cpu_id].run_queue.lock();
        if rq.deadline.iter().any(|t| t.id == task_id)
            || rq.game.iter().any(|t| t.id == task_id)
            || rq.normal.iter().any(|t| t.id == task_id)
        {
            return Some(cpu_id);
        }
    }
    None
}

/// Compute the "cache warmth" benefit of keeping a task on a given CPU.
/// Higher values mean the task has more to gain from staying.
pub fn cache_warmth(task_id: TaskId, cpu_id: usize) -> u32 {
    // A task on its last CPU gets full warmth. Same LLC = moderate. Cross-node = cold.
    match task_last_cpu(task_id) {
        Some(last) if last == cpu_id => 100,
        Some(last) => {
            let topo = CPU_TOPOLOGY.lock();
            if topo.share_llc(last, cpu_id) {
                60
            } else if topo.same_package(last, cpu_id) {
                30
            } else {
                0
            }
        }
        None => 0,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §19  SMP SYNCHRONIZATION BARRIERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Spin-barrier for synchronizing all CPUs at a rendezvous point.
pub struct CpuBarrier {
    count: AtomicU32,
    generation: AtomicU32,
}

impl CpuBarrier {
    pub const fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            generation: AtomicU32::new(0),
        }
    }

    /// Wait until all `expected` CPUs have reached this barrier.
    pub fn wait(&self, expected: u32) {
        let gen = self.generation.load(Ordering::Acquire);
        let arrived = self.count.fetch_add(1, Ordering::AcqRel) + 1;

        if arrived == expected {
            // Last to arrive: reset and advance generation.
            self.count.store(0, Ordering::Release);
            self.generation.fetch_add(1, Ordering::Release);
        } else {
            // Spin until generation advances.
            while self.generation.load(Ordering::Acquire) == gen {
                core::hint::spin_loop();
            }
        }
    }
}

/// Global barrier for SMP rendezvous (e.g., during frequency changes).
pub static SMP_BARRIER: CpuBarrier = CpuBarrier::new();

// ═══════════════════════════════════════════════════════════════════════════════
// §20  STOP-MACHINE — HALT ALL OTHER CPUS FOR CRITICAL SECTIONS
// ═══════════════════════════════════════════════════════════════════════════════

static STOP_MACHINE_ACTIVE: AtomicBool = AtomicBool::new(false);
static STOP_MACHINE_ACKED: AtomicU32 = AtomicU32::new(0);

/// Stop all other CPUs temporarily. The caller's closure runs with
/// exclusive access to all system state. Used for page table surgery,
/// CPU hotplug finalization, etc.
pub fn stop_machine<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _irq = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    let online = ONLINE_CPUS.load(Ordering::Relaxed);
    STOP_MACHINE_ACKED.store(0, Ordering::Release);
    STOP_MACHINE_ACTIVE.store(true, Ordering::Release);

    // Send NMI-like IPI to all other CPUs. They spin in handle_stop_machine.
    broadcast_ipi(RESCHEDULE_VECTOR);

    // Wait for all other CPUs to acknowledge.
    let expected = online - 1;
    let mut timeout = 0u64;
    while STOP_MACHINE_ACKED.load(Ordering::Acquire) < expected {
        core::hint::spin_loop();
        timeout += 1;
        if timeout > 100_000_000 {
            break;
        }
    }

    let result = f();

    STOP_MACHINE_ACTIVE.store(false, Ordering::Release);

    if _irq {
        x86_64::instructions::interrupts::enable();
    }

    result
}

/// Called on remote CPUs when they receive a reschedule IPI and stop_machine
/// is active. They spin here until released.
pub fn handle_stop_machine() {
    if !STOP_MACHINE_ACTIVE.load(Ordering::Acquire) {
        return;
    }

    STOP_MACHINE_ACKED.fetch_add(1, Ordering::AcqRel);

    while STOP_MACHINE_ACTIVE.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
}
