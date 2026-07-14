#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ─── Work Item Types ────────────────────────────────────────────────────────

pub type WorkFn = Box<dyn FnOnce() + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkState {
    Pending,
    Running,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkPriority {
    Low,
    Normal,
    High,
    Critical,
}

pub struct WorkItem {
    pub id: u64,
    pub name: String,
    pub state: WorkState,
    pub priority: WorkPriority,
    pub func: Option<WorkFn>,
    pub cpu_affinity: Option<u32>,
    pub submit_tick: u64,
    pub start_tick: u64,
    pub end_tick: u64,
}

impl WorkItem {
    pub fn new(id: u64, name: String, func: WorkFn) -> Self {
        Self {
            id,
            name,
            state: WorkState::Pending,
            priority: WorkPriority::Normal,
            func: Some(func),
            cpu_affinity: None,
            submit_tick: 0,
            start_tick: 0,
            end_tick: 0,
        }
    }

    pub fn with_priority(mut self, priority: WorkPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_cpu(mut self, cpu: u32) -> Self {
        self.cpu_affinity = Some(cpu);
        self
    }

    pub fn execute(&mut self) {
        self.state = WorkState::Running;
        if let Some(func) = self.func.take() {
            func();
        }
        self.state = WorkState::Completed;
    }

    pub fn cancel(&mut self) -> bool {
        if self.state == WorkState::Pending {
            self.state = WorkState::Cancelled;
            self.func = None;
            return true;
        }
        false
    }
}

// ─── Delayed Work ───────────────────────────────────────────────────────────

pub struct DelayedWork {
    pub work: WorkItem,
    pub delay_ticks: u64,
    pub enqueue_tick: u64,
    pub fire_tick: u64,
}

impl DelayedWork {
    pub fn new(work: WorkItem, delay_ticks: u64) -> Self {
        Self {
            fire_tick: delay_ticks,
            work,
            delay_ticks,
            enqueue_tick: 0,
        }
    }

    pub fn is_ready(&self, current_tick: u64) -> bool {
        current_tick >= self.fire_tick
    }

    pub fn reschedule(&mut self, new_delay: u64, current_tick: u64) {
        self.delay_ticks = new_delay;
        self.enqueue_tick = current_tick;
        self.fire_tick = current_tick + new_delay;
    }
}

// ─── Periodic Work ──────────────────────────────────────────────────────────

pub struct PeriodicWork {
    pub id: u64,
    pub name: String,
    pub interval_ticks: u64,
    pub next_fire_tick: u64,
    pub generator: Box<dyn Fn() -> WorkFn + Send + 'static>,
    pub enabled: bool,
    pub execution_count: u64,
    pub last_duration_ticks: u64,
}

impl PeriodicWork {
    pub fn new(
        id: u64,
        name: String,
        interval_ticks: u64,
        generator: Box<dyn Fn() -> WorkFn + Send + 'static>,
    ) -> Self {
        Self {
            id,
            name,
            interval_ticks,
            next_fire_tick: interval_ticks,
            generator,
            enabled: true,
            execution_count: 0,
            last_duration_ticks: 0,
        }
    }

    pub fn is_due(&self, current_tick: u64) -> bool {
        self.enabled && current_tick >= self.next_fire_tick
    }

    pub fn advance(&mut self) {
        self.next_fire_tick += self.interval_ticks;
        self.execution_count += 1;
    }

    pub fn generate_work(&self) -> WorkFn {
        (self.generator)()
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }
}

// ─── Workqueue Types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkqueueType {
    Bound,
    Unbound,
    Ordered,
    HighPriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkqueueFlags {
    None,
    HighPriority,
    Unbound,
    Freezable,
    MemReclaim,
    PowerEfficient,
    CpuIntensive,
}

// ─── Worker Thread ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Idle,
    Busy,
    Sleeping,
    Dying,
}

pub struct Worker {
    pub id: u32,
    pub cpu_id: Option<u32>,
    pub state: WorkerState,
    pub current_work_id: Option<u64>,
    pub processed_count: u64,
    pub idle_since_tick: u64,
    pub total_busy_ticks: u64,
    pub pool_id: u32,
}

impl Worker {
    pub fn new(id: u32, cpu_id: Option<u32>, pool_id: u32) -> Self {
        Self {
            id,
            cpu_id,
            state: WorkerState::Idle,
            current_work_id: None,
            processed_count: 0,
            idle_since_tick: 0,
            total_busy_ticks: 0,
            pool_id,
        }
    }

    pub fn start_work(&mut self, work_id: u64) {
        self.state = WorkerState::Busy;
        self.current_work_id = Some(work_id);
    }

    pub fn finish_work(&mut self, current_tick: u64) {
        self.state = WorkerState::Idle;
        self.current_work_id = None;
        self.processed_count += 1;
        self.idle_since_tick = current_tick;
    }

    pub fn go_to_sleep(&mut self) {
        self.state = WorkerState::Sleeping;
    }

    pub fn wake_up(&mut self) {
        if self.state == WorkerState::Sleeping {
            self.state = WorkerState::Idle;
        }
    }

    pub fn mark_dying(&mut self) {
        self.state = WorkerState::Dying;
    }

    pub fn is_idle(&self) -> bool {
        self.state == WorkerState::Idle
    }

    pub fn idle_duration(&self, current_tick: u64) -> u64 {
        if self.state == WorkerState::Idle || self.state == WorkerState::Sleeping {
            current_tick.saturating_sub(self.idle_since_tick)
        } else {
            0
        }
    }
}

// ─── Worker Pool ────────────────────────────────────────────────────────────

pub struct WorkerPool {
    pub id: u32,
    pub cpu_id: Option<u32>,
    pub numa_node: Option<u32>,
    workers: BTreeMap<u32, Worker>,
    work_queue: VecDeque<WorkItem>,
    delayed_queue: Vec<DelayedWork>,
    periodic_works: Vec<PeriodicWork>,
    max_active: u32,
    min_workers: u32,
    max_workers: u32,
    next_worker_id: u32,
    idle_worker_timeout_ticks: u64,
    pub stats: PoolStats,
}

#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    pub total_queued: u64,
    pub total_processed: u64,
    pub total_cancelled: u64,
    pub peak_pending: u64,
    pub current_pending: u64,
    pub current_active: u64,
    pub workers_created: u64,
    pub workers_destroyed: u64,
}

impl WorkerPool {
    pub fn new(id: u32, cpu_id: Option<u32>, min_workers: u32, max_workers: u32) -> Self {
        Self {
            id,
            cpu_id,
            numa_node: None,
            workers: BTreeMap::new(),
            work_queue: VecDeque::new(),
            delayed_queue: Vec::new(),
            periodic_works: Vec::new(),
            max_active: max_workers,
            min_workers,
            max_workers,
            next_worker_id: 0,
            idle_worker_timeout_ticks: 30000,
            stats: PoolStats::default(),
        }
    }

    pub fn create_worker(&mut self) -> u32 {
        let wid = self.next_worker_id;
        self.next_worker_id += 1;
        let worker = Worker::new(wid, self.cpu_id, self.id);
        self.workers.insert(wid, worker);
        self.stats.workers_created += 1;
        wid
    }

    pub fn destroy_worker(&mut self, wid: u32) -> bool {
        if let Some(w) = self.workers.get_mut(&wid) {
            if w.is_idle() {
                w.mark_dying();
                self.workers.remove(&wid);
                self.stats.workers_destroyed += 1;
                return true;
            }
        }
        false
    }

    pub fn submit(&mut self, item: WorkItem) {
        self.stats.total_queued += 1;
        self.stats.current_pending += 1;
        if self.stats.current_pending > self.stats.peak_pending {
            self.stats.peak_pending = self.stats.current_pending;
        }
        self.work_queue.push_back(item);
    }

    pub fn submit_delayed(&mut self, dw: DelayedWork) {
        self.delayed_queue.push(dw);
    }

    pub fn submit_periodic(&mut self, pw: PeriodicWork) {
        self.periodic_works.push(pw);
    }

    pub fn process_tick(&mut self, current_tick: u64) {
        let mut ready_indices = Vec::new();
        for (i, dw) in self.delayed_queue.iter().enumerate() {
            if dw.is_ready(current_tick) {
                ready_indices.push(i);
            }
        }
        for i in ready_indices.into_iter().rev() {
            let dw = self.delayed_queue.remove(i);
            self.submit(dw.work);
        }

        let mut periodic_items = Vec::new();
        for pw in self.periodic_works.iter_mut() {
            if pw.is_due(current_tick) {
                let func = pw.generate_work();
                periodic_items.push(WorkItem::new(pw.id, pw.name.clone(), func));
                pw.advance();
            }
        }
        for item in periodic_items {
            self.submit(item);
        }

        self.try_dispatch();
        self.manage_workers(current_tick);
    }

    fn try_dispatch(&mut self) {
        let idle_workers: Vec<u32> = self
            .workers
            .iter()
            .filter(|(_, w)| w.is_idle())
            .map(|(&id, _)| id)
            .collect();

        for wid in idle_workers {
            if let Some(mut item) = self.work_queue.pop_front() {
                self.stats.current_pending = self.stats.current_pending.saturating_sub(1);
                self.stats.current_active += 1;
                if let Some(worker) = self.workers.get_mut(&wid) {
                    worker.start_work(item.id);
                }
                item.execute();
                self.stats.current_active = self.stats.current_active.saturating_sub(1);
                self.stats.total_processed += 1;
                if let Some(worker) = self.workers.get_mut(&wid) {
                    worker.finish_work(0);
                }
            } else {
                break;
            }
        }
    }

    fn manage_workers(&mut self, current_tick: u64) {
        if !self.work_queue.is_empty()
            && self.idle_worker_count() == 0
            && (self.workers.len() as u32) < self.max_workers
        {
            self.create_worker();
        }

        let idle_to_remove: Vec<u32> = self
            .workers
            .iter()
            .filter(|(_, w)| {
                w.is_idle()
                    && w.idle_duration(current_tick) > self.idle_worker_timeout_ticks
                    && (self.workers.len() as u32) > self.min_workers
            })
            .map(|(&id, _)| id)
            .collect();

        for wid in idle_to_remove {
            self.destroy_worker(wid);
        }
    }

    fn idle_worker_count(&self) -> usize {
        self.workers.values().filter(|w| w.is_idle()).count()
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn pending_count(&self) -> usize {
        self.work_queue.len()
    }

    pub fn cancel_work(&mut self, work_id: u64) -> bool {
        if let Some(pos) = self.work_queue.iter().position(|w| w.id == work_id) {
            if let Some(mut item) = self.work_queue.remove(pos) {
                item.cancel();
                self.stats.current_pending = self.stats.current_pending.saturating_sub(1);
                self.stats.total_cancelled += 1;
                return true;
            }
        }
        false
    }

    pub fn flush(&mut self) {
        while let Some(mut item) = self.work_queue.pop_front() {
            self.stats.current_pending = self.stats.current_pending.saturating_sub(1);
            item.execute();
            self.stats.total_processed += 1;
        }
    }

    pub fn drain(&mut self) {
        self.flush();
        let wids: Vec<u32> = self.workers.keys().copied().collect();
        for wid in wids {
            self.destroy_worker(wid);
        }
    }
}

// ─── Workqueue ──────────────────────────────────────────────────────────────

pub struct Workqueue {
    pub name: String,
    pub wq_type: WorkqueueType,
    pub flags: WorkqueueFlags,
    pub max_active: u32,
    pools: BTreeMap<u32, WorkerPool>,
    next_pool_id: u32,
    next_work_id: AtomicU64,
    pub frozen: AtomicBool,
    pub draining: AtomicBool,
}

impl Workqueue {
    pub fn new(name: &str, wq_type: WorkqueueType, max_active: u32) -> Self {
        Self {
            name: String::from(name),
            wq_type,
            flags: WorkqueueFlags::None,
            max_active,
            pools: BTreeMap::new(),
            next_pool_id: 0,
            next_work_id: AtomicU64::new(1),
            frozen: AtomicBool::new(false),
            draining: AtomicBool::new(false),
        }
    }

    pub fn with_flags(mut self, flags: WorkqueueFlags) -> Self {
        self.flags = flags;
        self
    }

    pub fn create_pool(&mut self, cpu_id: Option<u32>) -> u32 {
        let pid = self.next_pool_id;
        self.next_pool_id += 1;
        let mut pool = WorkerPool::new(pid, cpu_id, 1, self.max_active);
        pool.create_worker();
        self.pools.insert(pid, pool);
        pid
    }

    fn alloc_work_id(&self) -> u64 {
        self.next_work_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn queue_work(&mut self, func: WorkFn) -> u64 {
        if self.frozen.load(Ordering::Relaxed) {
            return 0;
        }
        let id = self.alloc_work_id();
        let item = WorkItem::new(id, self.name.clone(), func);
        if let Some(pool) = self.select_pool(item.cpu_affinity) {
            pool.submit(item);
        }
        id
    }

    pub fn queue_work_on(&mut self, cpu: u32, func: WorkFn) -> u64 {
        if self.frozen.load(Ordering::Relaxed) {
            return 0;
        }
        let id = self.alloc_work_id();
        let item = WorkItem::new(id, self.name.clone(), func).with_cpu(cpu);
        if let Some(pool) = self.select_pool(Some(cpu)) {
            pool.submit(item);
        }
        id
    }

    pub fn queue_delayed_work(&mut self, func: WorkFn, delay_ticks: u64) -> u64 {
        if self.frozen.load(Ordering::Relaxed) {
            return 0;
        }
        let id = self.alloc_work_id();
        let item = WorkItem::new(id, self.name.clone(), func);
        let dw = DelayedWork::new(item, delay_ticks);
        if let Some(pool) = self.select_pool(None) {
            pool.submit_delayed(dw);
        }
        id
    }

    pub fn mod_delayed_work(&mut self, work_id: u64, new_delay: u64, current_tick: u64) -> bool {
        for pool in self.pools.values_mut() {
            for dw in pool.delayed_queue.iter_mut() {
                if dw.work.id == work_id {
                    dw.reschedule(new_delay, current_tick);
                    return true;
                }
            }
        }
        false
    }

    pub fn cancel_work_sync(&mut self, work_id: u64) -> bool {
        for pool in self.pools.values_mut() {
            if pool.cancel_work(work_id) {
                return true;
            }
        }
        false
    }

    pub fn flush_work(&mut self, work_id: u64) {
        for pool in self.pools.values_mut() {
            if let Some(pos) = pool.work_queue.iter().position(|w| w.id == work_id) {
                if let Some(mut item) = pool.work_queue.remove(pos) {
                    item.execute();
                    pool.stats.total_processed += 1;
                    pool.stats.current_pending = pool.stats.current_pending.saturating_sub(1);
                }
                return;
            }
        }
    }

    pub fn flush_workqueue(&mut self) {
        for pool in self.pools.values_mut() {
            pool.flush();
        }
    }

    pub fn drain_workqueue(&mut self) {
        self.draining.store(true, Ordering::Relaxed);
        for pool in self.pools.values_mut() {
            pool.drain();
        }
        self.draining.store(false, Ordering::Relaxed);
    }

    pub fn process_tick(&mut self, current_tick: u64) {
        if self.frozen.load(Ordering::Relaxed) {
            return;
        }
        for pool in self.pools.values_mut() {
            pool.process_tick(current_tick);
        }
    }

    fn select_pool(&mut self, cpu_affinity: Option<u32>) -> Option<&mut WorkerPool> {
        match self.wq_type {
            WorkqueueType::Bound => {
                let key = if let Some(cpu) = cpu_affinity {
                    self.pools
                        .iter()
                        .find(|(_, p)| p.cpu_id == Some(cpu))
                        .map(|(&id, _)| id)
                } else {
                    None
                };
                let key = key.or_else(|| self.pools.keys().next().copied());
                key.and_then(move |k| self.pools.get_mut(&k))
            }
            WorkqueueType::Ordered => self.pools.values_mut().next(),
            _ => {
                let mut min_pending = usize::MAX;
                let mut best_id = None;
                for (id, pool) in self.pools.iter() {
                    if pool.pending_count() < min_pending {
                        min_pending = pool.pending_count();
                        best_id = Some(*id);
                    }
                }
                if let Some(id) = best_id {
                    self.pools.get_mut(&id)
                } else {
                    None
                }
            }
        }
    }

    pub fn total_pending(&self) -> usize {
        self.pools.values().map(|p| p.pending_count()).sum()
    }

    pub fn total_workers(&self) -> usize {
        self.pools.values().map(|p| p.worker_count()).sum()
    }

    pub fn total_processed(&self) -> u64 {
        self.pools.values().map(|p| p.stats.total_processed).sum()
    }

    pub fn pool_count(&self) -> usize {
        self.pools.len()
    }
}

// ─── RCU Callback Scheduling ────────────────────────────────────────────────

pub struct RcuCallback {
    pub id: u64,
    pub func: Option<WorkFn>,
    pub grace_period: u64,
    pub submitted_tick: u64,
}

pub struct RcuScheduler {
    callbacks: VecDeque<RcuCallback>,
    current_grace_period: AtomicU64,
    completed_grace_period: AtomicU64,
    next_id: u64,
    total_callbacks: u64,
    total_completed: u64,
}

impl RcuScheduler {
    pub fn new() -> Self {
        Self {
            callbacks: VecDeque::new(),
            current_grace_period: AtomicU64::new(0),
            completed_grace_period: AtomicU64::new(0),
            next_id: 1,
            total_callbacks: 0,
            total_completed: 0,
        }
    }

    pub fn call_rcu(&mut self, func: WorkFn) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let gp = self.current_grace_period.load(Ordering::Relaxed);
        self.callbacks.push_back(RcuCallback {
            id,
            func: Some(func),
            grace_period: gp + 1,
            submitted_tick: 0,
        });
        self.total_callbacks += 1;
        id
    }

    pub fn advance_grace_period(&mut self) {
        let gp = self.current_grace_period.fetch_add(1, Ordering::SeqCst) + 1;
        self.completed_grace_period.store(gp, Ordering::Release);
    }

    pub fn process_callbacks(&mut self) {
        let completed = self.completed_grace_period.load(Ordering::Acquire);
        while let Some(front) = self.callbacks.front() {
            if front.grace_period <= completed {
                if let Some(mut cb) = self.callbacks.pop_front() {
                    if let Some(func) = cb.func.take() {
                        func();
                    }
                    self.total_completed += 1;
                }
            } else {
                break;
            }
        }
    }

    pub fn pending_count(&self) -> usize {
        self.callbacks.len()
    }

    pub fn current_grace_period(&self) -> u64 {
        self.current_grace_period.load(Ordering::Relaxed)
    }
}

// ─── Tasklet Emulation ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskletState {
    Idle,
    Scheduled,
    Running,
    Disabled,
}

pub struct TaskletStruct {
    pub id: u32,
    pub name: String,
    pub state: TaskletState,
    pub func: Option<Box<dyn Fn() + Send + 'static>>,
    pub count: AtomicU32,
    pub data: u64,
    pub high_priority: bool,
    pub execution_count: u64,
}

impl TaskletStruct {
    pub fn new(id: u32, name: &str, func: Box<dyn Fn() + Send + 'static>) -> Self {
        Self {
            id,
            name: String::from(name),
            state: TaskletState::Idle,
            func: Some(func),
            count: AtomicU32::new(0),
            data: 0,
            high_priority: false,
            execution_count: 0,
        }
    }

    pub fn new_hi(id: u32, name: &str, func: Box<dyn Fn() + Send + 'static>) -> Self {
        let mut t = Self::new(id, name, func);
        t.high_priority = true;
        t
    }

    pub fn schedule(&mut self) -> bool {
        if self.state == TaskletState::Disabled {
            return false;
        }
        if self.count.load(Ordering::Relaxed) > 0 {
            return false;
        }
        self.state = TaskletState::Scheduled;
        true
    }

    pub fn execute(&mut self) {
        if self.state != TaskletState::Scheduled {
            return;
        }
        if self.count.load(Ordering::Relaxed) > 0 {
            return;
        }
        self.state = TaskletState::Running;
        if let Some(ref func) = self.func {
            func();
        }
        self.state = TaskletState::Idle;
        self.execution_count += 1;
    }

    pub fn disable(&mut self) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.state = TaskletState::Disabled;
    }

    pub fn enable(&mut self) {
        let prev = self.count.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            self.state = TaskletState::Idle;
        }
    }

    pub fn kill(&mut self) {
        self.state = TaskletState::Disabled;
        self.func = None;
    }

    pub fn is_enabled(&self) -> bool {
        self.count.load(Ordering::Relaxed) == 0
    }
}

pub struct TaskletManager {
    tasklets: BTreeMap<u32, TaskletStruct>,
    hi_pending: VecDeque<u32>,
    normal_pending: VecDeque<u32>,
    next_id: u32,
    total_scheduled: u64,
    total_executed: u64,
}

impl TaskletManager {
    pub fn new() -> Self {
        Self {
            tasklets: BTreeMap::new(),
            hi_pending: VecDeque::new(),
            normal_pending: VecDeque::new(),
            next_id: 1,
            total_scheduled: 0,
            total_executed: 0,
        }
    }

    pub fn register(&mut self, mut tasklet: TaskletStruct) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        tasklet.id = id;
        self.tasklets.insert(id, tasklet);
        id
    }

    pub fn tasklet_schedule(&mut self, id: u32) -> bool {
        if let Some(t) = self.tasklets.get_mut(&id) {
            if t.schedule() {
                if t.high_priority {
                    self.hi_pending.push_back(id);
                } else {
                    self.normal_pending.push_back(id);
                }
                self.total_scheduled += 1;
                return true;
            }
        }
        false
    }

    pub fn tasklet_hi_schedule(&mut self, id: u32) -> bool {
        if let Some(t) = self.tasklets.get_mut(&id) {
            t.high_priority = true;
            if t.schedule() {
                self.hi_pending.push_back(id);
                self.total_scheduled += 1;
                return true;
            }
        }
        false
    }

    pub fn tasklet_disable(&mut self, id: u32) {
        if let Some(t) = self.tasklets.get_mut(&id) {
            t.disable();
        }
    }

    pub fn tasklet_enable(&mut self, id: u32) {
        if let Some(t) = self.tasklets.get_mut(&id) {
            t.enable();
        }
    }

    pub fn tasklet_kill(&mut self, id: u32) {
        if let Some(t) = self.tasklets.get_mut(&id) {
            t.kill();
        }
        self.hi_pending.retain(|&tid| tid != id);
        self.normal_pending.retain(|&tid| tid != id);
    }

    pub fn process_hi(&mut self) {
        while let Some(id) = self.hi_pending.pop_front() {
            if let Some(t) = self.tasklets.get_mut(&id) {
                t.execute();
                self.total_executed += 1;
            }
        }
    }

    pub fn process_normal(&mut self) {
        while let Some(id) = self.normal_pending.pop_front() {
            if let Some(t) = self.tasklets.get_mut(&id) {
                t.execute();
                self.total_executed += 1;
            }
        }
    }

    pub fn process_all(&mut self) {
        self.process_hi();
        self.process_normal();
    }

    pub fn pending_count(&self) -> usize {
        self.hi_pending.len() + self.normal_pending.len()
    }
}

// ─── SoftIRQ ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SoftIrqType {
    HiSoftirq = 0,
    TimerSoftirq = 1,
    NetTxSoftirq = 2,
    NetRxSoftirq = 3,
    BlockSoftirq = 4,
    IrqPollSoftirq = 5,
    TaskletSoftirq = 6,
    SchedSoftirq = 7,
    HrtimerSoftirq = 8,
    RcuSoftirq = 9,
}

impl SoftIrqType {
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::HiSoftirq),
            1 => Some(Self::TimerSoftirq),
            2 => Some(Self::NetTxSoftirq),
            3 => Some(Self::NetRxSoftirq),
            4 => Some(Self::BlockSoftirq),
            5 => Some(Self::IrqPollSoftirq),
            6 => Some(Self::TaskletSoftirq),
            7 => Some(Self::SchedSoftirq),
            8 => Some(Self::HrtimerSoftirq),
            9 => Some(Self::RcuSoftirq),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::HiSoftirq => "HI",
            Self::TimerSoftirq => "TIMER",
            Self::NetTxSoftirq => "NET_TX",
            Self::NetRxSoftirq => "NET_RX",
            Self::BlockSoftirq => "BLOCK",
            Self::IrqPollSoftirq => "IRQ_POLL",
            Self::TaskletSoftirq => "TASKLET",
            Self::SchedSoftirq => "SCHED",
            Self::HrtimerSoftirq => "HRTIMER",
            Self::RcuSoftirq => "RCU",
        }
    }

    pub const COUNT: usize = 10;
}

pub type SoftIrqHandler = Box<dyn Fn() + Send + 'static>;

pub struct SoftIrqEntry {
    pub handler: Option<SoftIrqHandler>,
    pub count: u64,
    pub enabled: bool,
}

pub struct SoftIrqSystem {
    entries: [Option<SoftIrqEntry>; SoftIrqType::COUNT],
    pending: AtomicU32,
    per_cpu_pending: BTreeMap<u32, u32>,
    total_raised: u64,
    total_handled: u64,
}

impl SoftIrqSystem {
    pub fn new() -> Self {
        Self {
            entries: [None, None, None, None, None, None, None, None, None, None],
            pending: AtomicU32::new(0),
            per_cpu_pending: BTreeMap::new(),
            total_raised: 0,
            total_handled: 0,
        }
    }

    pub fn register(&mut self, irq: SoftIrqType, handler: SoftIrqHandler) {
        let idx = irq as usize;
        self.entries[idx] = Some(SoftIrqEntry {
            handler: Some(handler),
            count: 0,
            enabled: true,
        });
    }

    pub fn raise(&mut self, irq: SoftIrqType) {
        let bit = 1u32 << (irq as u32);
        self.pending.fetch_or(bit, Ordering::Release);
        self.total_raised += 1;
    }

    pub fn raise_on_cpu(&mut self, irq: SoftIrqType, cpu: u32) {
        let bit = 1u32 << (irq as u32);
        let entry = self.per_cpu_pending.entry(cpu).or_insert(0);
        *entry |= bit;
        self.total_raised += 1;
    }

    pub fn process(&mut self) {
        let pending = self.pending.swap(0, Ordering::AcqRel);
        if pending == 0 {
            return;
        }

        for i in 0..SoftIrqType::COUNT {
            if pending & (1 << i) != 0 {
                if let Some(ref mut entry) = self.entries[i] {
                    if entry.enabled {
                        if let Some(ref handler) = entry.handler {
                            handler();
                        }
                        entry.count += 1;
                        self.total_handled += 1;
                    }
                }
            }
        }
    }

    pub fn process_on_cpu(&mut self, cpu: u32) {
        let pending = self.per_cpu_pending.remove(&cpu).unwrap_or(0);
        if pending == 0 {
            return;
        }

        for i in 0..SoftIrqType::COUNT {
            if pending & (1 << i) != 0 {
                if let Some(ref mut entry) = self.entries[i] {
                    if entry.enabled {
                        if let Some(ref handler) = entry.handler {
                            handler();
                        }
                        entry.count += 1;
                        self.total_handled += 1;
                    }
                }
            }
        }
    }

    pub fn disable(&mut self, irq: SoftIrqType) {
        let idx = irq as usize;
        if let Some(ref mut entry) = self.entries[idx] {
            entry.enabled = false;
        }
    }

    pub fn enable(&mut self, irq: SoftIrqType) {
        let idx = irq as usize;
        if let Some(ref mut entry) = self.entries[idx] {
            entry.enabled = true;
        }
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.total_raised, self.total_handled)
    }

    pub fn irq_count(&self, irq: SoftIrqType) -> u64 {
        let idx = irq as usize;
        self.entries[idx].as_ref().map_or(0, |e| e.count)
    }

    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed) != 0
    }
}

// ─── Global Workqueue System ────────────────────────────────────────────────

pub struct WorkqueueSystem {
    workqueues: BTreeMap<String, Workqueue>,
    rcu: RcuScheduler,
    tasklets: TaskletManager,
    softirq: SoftIrqSystem,
    initialized: bool,
    tick_count: AtomicU64,
}

impl WorkqueueSystem {
    pub const fn new() -> Self {
        Self {
            workqueues: BTreeMap::new(),
            rcu: RcuScheduler {
                callbacks: VecDeque::new(),
                current_grace_period: AtomicU64::new(0),
                completed_grace_period: AtomicU64::new(0),
                next_id: 1,
                total_callbacks: 0,
                total_completed: 0,
            },
            tasklets: TaskletManager {
                tasklets: BTreeMap::new(),
                hi_pending: VecDeque::new(),
                normal_pending: VecDeque::new(),
                next_id: 1,
                total_scheduled: 0,
                total_executed: 0,
            },
            softirq: SoftIrqSystem {
                entries: [None, None, None, None, None, None, None, None, None, None],
                pending: AtomicU32::new(0),
                per_cpu_pending: BTreeMap::new(),
                total_raised: 0,
                total_handled: 0,
            },
            initialized: false,
            tick_count: AtomicU64::new(0),
        }
    }

    pub fn init(&mut self) {
        let mut system_wq = Workqueue::new("system_wq", WorkqueueType::Bound, 256);
        system_wq.create_pool(Some(0));
        self.workqueues.insert(String::from("system_wq"), system_wq);

        let mut highpri_wq = Workqueue::new("system_highpri_wq", WorkqueueType::Bound, 256)
            .with_flags(WorkqueueFlags::HighPriority);
        highpri_wq.create_pool(Some(0));
        self.workqueues
            .insert(String::from("system_highpri_wq"), highpri_wq);

        let mut long_wq = Workqueue::new("system_long_wq", WorkqueueType::Unbound, 256);
        long_wq.create_pool(None);
        self.workqueues
            .insert(String::from("system_long_wq"), long_wq);

        let mut unbound_wq = Workqueue::new("system_unbound_wq", WorkqueueType::Unbound, 512)
            .with_flags(WorkqueueFlags::Unbound);
        unbound_wq.create_pool(None);
        self.workqueues
            .insert(String::from("system_unbound_wq"), unbound_wq);

        let mut freezable_wq = Workqueue::new("system_freezable_wq", WorkqueueType::Bound, 256)
            .with_flags(WorkqueueFlags::Freezable);
        freezable_wq.create_pool(Some(0));
        self.workqueues
            .insert(String::from("system_freezable_wq"), freezable_wq);

        let mut power_wq = Workqueue::new("system_power_efficient_wq", WorkqueueType::Unbound, 256)
            .with_flags(WorkqueueFlags::PowerEfficient);
        power_wq.create_pool(None);
        self.workqueues
            .insert(String::from("system_power_efficient_wq"), power_wq);

        self.initialized = true;
    }

    pub fn create_workqueue(
        &mut self,
        name: &str,
        wq_type: WorkqueueType,
        max_active: u32,
    ) -> bool {
        if self.workqueues.contains_key(name) {
            return false;
        }
        let mut wq = Workqueue::new(name, wq_type, max_active);
        match wq_type {
            WorkqueueType::Bound => {
                wq.create_pool(Some(0));
            }
            _ => {
                wq.create_pool(None);
            }
        }
        self.workqueues.insert(String::from(name), wq);
        true
    }

    pub fn destroy_workqueue(&mut self, name: &str) -> bool {
        if let Some(mut wq) = self.workqueues.remove(name) {
            wq.drain_workqueue();
            true
        } else {
            false
        }
    }

    pub fn queue_work(&mut self, wq_name: &str, func: WorkFn) -> u64 {
        if let Some(wq) = self.workqueues.get_mut(wq_name) {
            wq.queue_work(func)
        } else {
            0
        }
    }

    pub fn queue_delayed_work(&mut self, wq_name: &str, func: WorkFn, delay: u64) -> u64 {
        if let Some(wq) = self.workqueues.get_mut(wq_name) {
            wq.queue_delayed_work(func, delay)
        } else {
            0
        }
    }

    pub fn cancel_work_sync(&mut self, wq_name: &str, work_id: u64) -> bool {
        if let Some(wq) = self.workqueues.get_mut(wq_name) {
            wq.cancel_work_sync(work_id)
        } else {
            false
        }
    }

    pub fn flush_workqueue(&mut self, wq_name: &str) {
        if let Some(wq) = self.workqueues.get_mut(wq_name) {
            wq.flush_workqueue();
        }
    }

    pub fn call_rcu(&mut self, func: WorkFn) -> u64 {
        self.rcu.call_rcu(func)
    }

    pub fn rcu_advance(&mut self) {
        self.rcu.advance_grace_period();
    }

    pub fn register_tasklet(&mut self, tasklet: TaskletStruct) -> u32 {
        self.tasklets.register(tasklet)
    }

    pub fn tasklet_schedule(&mut self, id: u32) -> bool {
        self.tasklets.tasklet_schedule(id)
    }

    pub fn tasklet_hi_schedule(&mut self, id: u32) -> bool {
        self.tasklets.tasklet_hi_schedule(id)
    }

    pub fn tasklet_disable(&mut self, id: u32) {
        self.tasklets.tasklet_disable(id);
    }

    pub fn tasklet_enable(&mut self, id: u32) {
        self.tasklets.tasklet_enable(id);
    }

    pub fn tasklet_kill(&mut self, id: u32) {
        self.tasklets.tasklet_kill(id);
    }

    pub fn register_softirq(&mut self, irq: SoftIrqType, handler: SoftIrqHandler) {
        self.softirq.register(irq, handler);
    }

    pub fn raise_softirq(&mut self, irq: SoftIrqType) {
        self.softirq.raise(irq);
    }

    pub fn process_tick(&mut self) {
        let tick = self.tick_count.fetch_add(1, Ordering::Relaxed);
        for wq in self.workqueues.values_mut() {
            wq.process_tick(tick);
        }
        self.softirq.process();
        self.tasklets.process_all();
        self.rcu.process_callbacks();
    }

    pub fn workqueue_count(&self) -> usize {
        self.workqueues.len()
    }

    pub fn total_pending(&self) -> usize {
        self.workqueues.values().map(|wq| wq.total_pending()).sum()
    }

    pub fn total_workers(&self) -> usize {
        self.workqueues.values().map(|wq| wq.total_workers()).sum()
    }

    pub fn rcu_pending(&self) -> usize {
        self.rcu.pending_count()
    }

    pub fn tasklet_pending(&self) -> usize {
        self.tasklets.pending_count()
    }

    pub fn softirq_pending(&self) -> bool {
        self.softirq.is_pending()
    }
}

pub static WORKQUEUE_SYSTEM: Mutex<WorkqueueSystem> = Mutex::new(WorkqueueSystem::new());

pub fn init() {
    WORKQUEUE_SYSTEM.lock().init();
}
