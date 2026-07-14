//! Fast boot path optimization — target boot under 6 seconds on NVMe (goal: 3s).
//!
//! Provides boot profiling, parallel initialization, deferred init for
//! non-critical subsystems, dependency graph scheduling, and boot cache
//! for warm reboots.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════════════════════

const MAX_BOOT_STAGES: usize = 64;
const MAX_DEFERRED_TASKS: usize = 32;
const BOOT_CACHE_MAGIC: u64 = 0x5241_4542_4F4F_5443; // "RAEBOOTC"
const TSC_DEFAULT_KHZ: u64 = 3_000_000;

// Boot time budget (microseconds) — targets for each critical stage
const BUDGET_FIRMWARE_US: u64 = 500_000; // 500ms firmware handoff
const BUDGET_KERNEL_LOAD_US: u64 = 200_000; // 200ms kernel load
const BUDGET_MEMORY_INIT_US: u64 = 100_000; // 100ms memory init
const BUDGET_FILESYSTEM_US: u64 = 300_000; // 300ms filesystem mount
const BUDGET_COMPOSITOR_US: u64 = 200_000; // 200ms compositor up
const BUDGET_SHELL_US: u64 = 500_000; // 500ms shell ready
const BUDGET_TOTAL_TARGET_US: u64 = 3_000_000; // 3s total target

// ═══════════════════════════════════════════════════════════════════════════════
//  Boot Stage Definitions
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum BootStage {
    FirmwareHandoff = 0,
    SerialInit = 1,
    GdtInit = 2,
    IdtInit = 3,
    MemoryInit = 4,
    HeapInit = 5,
    AcpiInit = 6,
    ApicInit = 7,
    SmpInit = 8,
    FramebufferInit = 9,
    CompositorInit = 10,
    PciEnumeration = 11,
    StorageInit = 12,
    FilesystemMount = 13,
    NetworkInit = 14,
    SecurityInit = 15,
    ProcessInit = 16,
    DriverInit = 17,
    UsbInit = 18,
    AudioInit = 19,
    GpuInit = 20,
    BluetoothInit = 21,
    ShellReady = 22,
    DesktopVisible = 23,
    DeferredInit = 24,
    BootComplete = 25,
}

impl BootStage {
    pub fn name(&self) -> &'static str {
        match self {
            Self::FirmwareHandoff => "firmware_handoff",
            Self::SerialInit => "serial_init",
            Self::GdtInit => "gdt_init",
            Self::IdtInit => "idt_init",
            Self::MemoryInit => "memory_init",
            Self::HeapInit => "heap_init",
            Self::AcpiInit => "acpi_init",
            Self::ApicInit => "apic_init",
            Self::SmpInit => "smp_init",
            Self::FramebufferInit => "framebuffer_init",
            Self::CompositorInit => "compositor_init",
            Self::PciEnumeration => "pci_enumeration",
            Self::StorageInit => "storage_init",
            Self::FilesystemMount => "filesystem_mount",
            Self::NetworkInit => "network_init",
            Self::SecurityInit => "security_init",
            Self::ProcessInit => "process_init",
            Self::DriverInit => "driver_init",
            Self::UsbInit => "usb_init",
            Self::AudioInit => "audio_init",
            Self::GpuInit => "gpu_init",
            Self::BluetoothInit => "bluetooth_init",
            Self::ShellReady => "shell_ready",
            Self::DesktopVisible => "desktop_visible",
            Self::DeferredInit => "deferred_init",
            Self::BootComplete => "boot_complete",
        }
    }

    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            Self::FirmwareHandoff
                | Self::SerialInit
                | Self::GdtInit
                | Self::IdtInit
                | Self::MemoryInit
                | Self::HeapInit
                | Self::AcpiInit
                | Self::FramebufferInit
                | Self::CompositorInit
                | Self::FilesystemMount
                | Self::ShellReady
                | Self::DesktopVisible
        )
    }

    pub fn budget_us(&self) -> u64 {
        match self {
            Self::FirmwareHandoff => BUDGET_FIRMWARE_US,
            Self::MemoryInit => BUDGET_MEMORY_INIT_US,
            Self::HeapInit => 50_000,
            Self::FilesystemMount => BUDGET_FILESYSTEM_US,
            Self::CompositorInit => BUDGET_COMPOSITOR_US,
            Self::ShellReady => BUDGET_SHELL_US,
            _ => 200_000, // 200ms default budget
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  1. Boot Profiler
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct BootStageRecord {
    pub stage: BootStage,
    pub start_tsc: u64,
    pub end_tsc: u64,
    pub duration_us: u64,
    pub budget_us: u64,
    pub over_budget: bool,
    pub completed: bool,
}

impl BootStageRecord {
    pub const fn empty() -> Self {
        Self {
            stage: BootStage::FirmwareHandoff,
            start_tsc: 0,
            end_tsc: 0,
            duration_us: 0,
            budget_us: 0,
            over_budget: false,
            completed: false,
        }
    }
}

pub struct BootProfiler {
    pub stages: [BootStageRecord; MAX_BOOT_STAGES],
    pub stage_count: usize,
    pub boot_start_tsc: u64,
    pub boot_end_tsc: u64,
    pub tsc_freq_khz: u64,
    pub total_boot_us: u64,
    pub critical_path_us: u64,
    pub parallel_savings_us: u64,
    pub deferred_us: u64,
    pub active_stage: Option<usize>,
}

impl BootProfiler {
    pub const fn new() -> Self {
        Self {
            stages: [BootStageRecord::empty(); MAX_BOOT_STAGES],
            stage_count: 0,
            boot_start_tsc: 0,
            boot_end_tsc: 0,
            tsc_freq_khz: TSC_DEFAULT_KHZ,
            total_boot_us: 0,
            critical_path_us: 0,
            parallel_savings_us: 0,
            deferred_us: 0,
            active_stage: None,
        }
    }

    pub fn start_boot(&mut self) {
        self.boot_start_tsc = read_tsc_boot();
    }

    pub fn begin_stage(&mut self, stage: BootStage) -> usize {
        let idx = self.stage_count;
        if idx >= MAX_BOOT_STAGES {
            return idx;
        }
        self.stages[idx] = BootStageRecord {
            stage,
            start_tsc: read_tsc_boot(),
            end_tsc: 0,
            duration_us: 0,
            budget_us: stage.budget_us(),
            over_budget: false,
            completed: false,
        };
        self.stage_count += 1;
        self.active_stage = Some(idx);
        idx
    }

    pub fn end_stage(&mut self, idx: usize) {
        if idx >= self.stage_count {
            return;
        }
        let end = read_tsc_boot();
        let record = &mut self.stages[idx];
        record.end_tsc = end;
        let elapsed_tsc = end.saturating_sub(record.start_tsc);
        record.duration_us = tsc_to_us(elapsed_tsc, self.tsc_freq_khz);
        record.over_budget = record.duration_us > record.budget_us;
        record.completed = true;
        self.active_stage = None;
    }

    pub fn end_boot(&mut self) {
        self.boot_end_tsc = read_tsc_boot();
        let total_tsc = self.boot_end_tsc.saturating_sub(self.boot_start_tsc);
        self.total_boot_us = tsc_to_us(total_tsc, self.tsc_freq_khz);
        self.compute_critical_path();
    }

    fn compute_critical_path(&mut self) {
        self.critical_path_us = 0;
        for i in 0..self.stage_count {
            if self.stages[i].completed && self.stages[i].stage.is_critical() {
                self.critical_path_us += self.stages[i].duration_us;
            }
        }
    }

    pub fn stage_duration_us(&self, stage: BootStage) -> Option<u64> {
        for i in 0..self.stage_count {
            if self.stages[i].stage == stage && self.stages[i].completed {
                return Some(self.stages[i].duration_us);
            }
        }
        None
    }

    pub fn over_budget_stages(&self) -> Vec<(BootStage, u64, u64)> {
        let mut result = Vec::new();
        for i in 0..self.stage_count {
            let s = &self.stages[i];
            if s.completed && s.over_budget {
                result.push((s.stage, s.duration_us, s.budget_us));
            }
        }
        result
    }

    pub fn total_boot_ms(&self) -> u64 {
        self.total_boot_us / 1000
    }

    pub fn met_target(&self) -> bool {
        self.total_boot_us <= BUDGET_TOTAL_TARGET_US
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  2. Parallel Init
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitTaskState {
    Pending,
    Running,
    Completed,
    Failed,
    Deferred,
}

#[derive(Debug, Clone)]
pub struct InitTask {
    pub id: u32,
    pub stage: BootStage,
    pub state: InitTaskState,
    pub dependencies: Vec<u32>,
    pub dependents: Vec<u32>,
    pub priority: u8,
    pub deferrable: bool,
    pub start_tsc: u64,
    pub end_tsc: u64,
}

pub struct ParallelInit {
    pub tasks: Vec<InitTask>,
    pub next_id: u32,
    pub completed_count: u32,
    pub failed_count: u32,
    pub parallelism_level: u8,
}

impl ParallelInit {
    pub fn new(parallelism: u8) -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
            completed_count: 0,
            failed_count: 0,
            parallelism_level: parallelism,
        }
    }

    pub fn add_task(
        &mut self,
        stage: BootStage,
        deps: Vec<u32>,
        priority: u8,
        deferrable: bool,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(InitTask {
            id,
            stage,
            state: InitTaskState::Pending,
            dependencies: deps,
            dependents: Vec::new(),
            priority,
            deferrable,
            start_tsc: 0,
            end_tsc: 0,
        });
        id
    }

    /// Resolve dependents (reverse mapping) after all tasks are added.
    pub fn resolve_dependents(&mut self) {
        let ids_and_deps: Vec<(u32, Vec<u32>)> = self
            .tasks
            .iter()
            .map(|t| (t.id, t.dependencies.clone()))
            .collect();

        for (id, deps) in &ids_and_deps {
            for dep_id in deps {
                if let Some(dep_task) = self.tasks.iter_mut().find(|t| t.id == *dep_id) {
                    dep_task.dependents.push(*id);
                }
            }
        }
    }

    /// Get the next batch of tasks that can run in parallel.
    pub fn ready_tasks(&self) -> Vec<u32> {
        let mut ready = Vec::new();
        for task in &self.tasks {
            if task.state != InitTaskState::Pending {
                continue;
            }
            let deps_met = task.dependencies.iter().all(|dep_id| {
                self.tasks
                    .iter()
                    .find(|t| t.id == *dep_id)
                    .map(|t| t.state == InitTaskState::Completed)
                    .unwrap_or(true)
            });
            if deps_met {
                ready.push(task.id);
            }
        }
        ready.sort_by(|a, b| {
            let ta = self.tasks.iter().find(|t| t.id == *a).unwrap();
            let tb = self.tasks.iter().find(|t| t.id == *b).unwrap();
            tb.priority.cmp(&ta.priority)
        });
        ready.truncate(self.parallelism_level as usize);
        ready
    }

    pub fn mark_running(&mut self, id: u32) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            task.state = InitTaskState::Running;
            task.start_tsc = read_tsc_boot();
        }
    }

    pub fn mark_completed(&mut self, id: u32) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            task.state = InitTaskState::Completed;
            task.end_tsc = read_tsc_boot();
            self.completed_count += 1;
        }
    }

    pub fn mark_failed(&mut self, id: u32) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            task.state = InitTaskState::Failed;
            task.end_tsc = read_tsc_boot();
            self.failed_count += 1;
        }
    }

    pub fn defer_non_critical(&mut self) {
        for task in &mut self.tasks {
            if task.state == InitTaskState::Pending && task.deferrable {
                task.state = InitTaskState::Deferred;
            }
        }
    }

    pub fn all_critical_done(&self) -> bool {
        self.tasks
            .iter()
            .filter(|t| !t.deferrable)
            .all(|t| t.state == InitTaskState::Completed || t.state == InitTaskState::Failed)
    }

    pub fn deferred_tasks(&self) -> Vec<u32> {
        self.tasks
            .iter()
            .filter(|t| t.state == InitTaskState::Deferred)
            .map(|t| t.id)
            .collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  3. Deferred Init
// ═══════════════════════════════════════════════════════════════════════════════

pub type DeferredFn = fn() -> Result<(), &'static str>;

pub struct DeferredTask {
    pub id: u32,
    pub name: &'static str,
    pub func: DeferredFn,
    pub completed: bool,
    pub error: Option<&'static str>,
    pub duration_us: u64,
}

pub struct DeferredInit {
    pub tasks: Vec<DeferredTask>,
    pub next_id: u32,
    pub all_completed: bool,
    pub desktop_visible: AtomicBool,
}

impl DeferredInit {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
            all_completed: false,
            desktop_visible: AtomicBool::new(false),
        }
    }

    pub fn register(&mut self, name: &'static str, func: DeferredFn) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(DeferredTask {
            id,
            name,
            func,
            completed: false,
            error: None,
            duration_us: 0,
        });
        id
    }

    /// Run all deferred tasks. Called after desktop is visible.
    pub fn run_all(&mut self, tsc_freq_khz: u64) {
        for task in &mut self.tasks {
            if task.completed {
                continue;
            }
            let start = read_tsc_boot();
            match (task.func)() {
                Ok(()) => {
                    task.completed = true;
                }
                Err(e) => {
                    task.error = Some(e);
                    task.completed = true; // mark done even on error
                }
            }
            let end = read_tsc_boot();
            task.duration_us = tsc_to_us(end.saturating_sub(start), tsc_freq_khz);
        }
        self.all_completed = true;
    }

    pub fn pending_count(&self) -> usize {
        self.tasks.iter().filter(|t| !t.completed).count()
    }

    pub fn signal_desktop_visible(&self) {
        self.desktop_visible.store(true, Ordering::SeqCst);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  4. Boot Stage Graph (dependency DAG)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct StageNode {
    pub stage: BootStage,
    pub dependencies: Vec<BootStage>,
    pub duration_estimate_us: u64,
    pub visited: bool,
    pub order: u32,
}

pub struct BootStageGraph {
    pub nodes: BTreeMap<u8, StageNode>,
    pub topological_order: Vec<BootStage>,
    pub critical_path: Vec<BootStage>,
    pub critical_path_duration_us: u64,
}

impl BootStageGraph {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            topological_order: Vec::new(),
            critical_path: Vec::new(),
            critical_path_duration_us: 0,
        }
    }

    pub fn add_stage(&mut self, stage: BootStage, deps: Vec<BootStage>, estimate_us: u64) {
        self.nodes.insert(
            stage as u8,
            StageNode {
                stage,
                dependencies: deps,
                duration_estimate_us: estimate_us,
                visited: false,
                order: 0,
            },
        );
    }

    /// Topological sort — determines safe initialization order.
    pub fn compute_order(&mut self) {
        self.topological_order.clear();
        for node in self.nodes.values_mut() {
            node.visited = false;
        }

        let keys: Vec<u8> = self.nodes.keys().copied().collect();
        for key in &keys {
            if !self.nodes[key].visited {
                self.topo_visit(*key);
            }
        }

        self.topological_order.reverse();
    }

    fn topo_visit(&mut self, key: u8) {
        if let Some(node) = self.nodes.get_mut(&key) {
            if node.visited {
                return;
            }
            node.visited = true;
            let deps: Vec<BootStage> = node.dependencies.clone();
            for dep in deps {
                self.topo_visit(dep as u8);
            }
            if let Some(n) = self.nodes.get(&key) {
                self.topological_order.push(n.stage);
            }
        }
    }

    /// Compute the critical path (longest dependency chain).
    pub fn compute_critical_path(&mut self) {
        self.critical_path.clear();
        self.critical_path_duration_us = 0;

        // Simple longest-path using topological order
        let mut dist: BTreeMap<u8, u64> = BTreeMap::new();
        let mut pred: BTreeMap<u8, Option<u8>> = BTreeMap::new();

        for node in self.nodes.values() {
            dist.insert(node.stage as u8, 0);
            pred.insert(node.stage as u8, None);
        }

        for &stage in &self.topological_order {
            let key = stage as u8;
            if let Some(node) = self.nodes.get(&key) {
                let current_dist = dist[&key];
                let new_dist = current_dist + node.duration_estimate_us;
                for dep_node in self.nodes.values() {
                    if dep_node.dependencies.contains(&stage) {
                        let dep_key = dep_node.stage as u8;
                        if new_dist > dist[&dep_key] {
                            dist.insert(dep_key, new_dist);
                            pred.insert(dep_key, Some(key));
                        }
                    }
                }
            }
        }

        // Find the endpoint with longest distance
        if let Some((&end_key, &max_dist)) = dist.iter().max_by_key(|(_, &d)| d) {
            self.critical_path_duration_us = max_dist;
            let mut current = Some(end_key);
            while let Some(k) = current {
                if let Some(node) = self.nodes.get(&k) {
                    self.critical_path.push(node.stage);
                }
                current = pred.get(&k).copied().flatten();
            }
            self.critical_path.reverse();
        }
    }

    pub fn parallelizable_stages(&self) -> Vec<Vec<BootStage>> {
        let mut levels: Vec<Vec<BootStage>> = Vec::new();
        let mut placed: Vec<u8> = Vec::new();

        loop {
            let mut level: Vec<BootStage> = Vec::new();
            for node in self.nodes.values() {
                let key = node.stage as u8;
                if placed.contains(&key) {
                    continue;
                }
                let deps_placed = node
                    .dependencies
                    .iter()
                    .all(|d| placed.contains(&(*d as u8)));
                if deps_placed {
                    level.push(node.stage);
                }
            }
            if level.is_empty() {
                break;
            }
            for &s in &level {
                placed.push(s as u8);
            }
            levels.push(level);
        }

        levels
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  5. Boot Cache (warm reboot acceleration)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CachedDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub bar0: u64,
    pub bar1: u64,
}

#[repr(C)]
pub struct BootCacheHeader {
    pub magic: u64,
    pub version: u32,
    pub device_count: u32,
    pub last_boot_us: u64,
    pub checksum: u32,
    pub _reserved: u32,
}

pub struct BootCache {
    pub header: BootCacheHeader,
    pub devices: Vec<CachedDevice>,
    pub valid: bool,
    pub warm_boot: bool,
    pub savings_us: u64,
}

impl BootCache {
    pub fn new() -> Self {
        Self {
            header: BootCacheHeader {
                magic: BOOT_CACHE_MAGIC,
                version: 1,
                device_count: 0,
                last_boot_us: 0,
                checksum: 0,
                _reserved: 0,
            },
            devices: Vec::new(),
            valid: false,
            warm_boot: false,
            savings_us: 0,
        }
    }

    pub fn cache_device(&mut self, dev: CachedDevice) {
        self.devices.push(dev);
        self.header.device_count = self.devices.len() as u32;
    }

    pub fn lookup_device(&self, bus: u8, device: u8, function: u8) -> Option<&CachedDevice> {
        self.devices
            .iter()
            .find(|d| d.bus == bus && d.device == device && d.function == function)
    }

    pub fn is_warm_boot(&self) -> bool {
        self.warm_boot && self.valid
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
        self.devices.clear();
        self.header.device_count = 0;
    }

    pub fn compute_checksum(&mut self) -> u32 {
        let mut sum: u32 = 0;
        for dev in &self.devices {
            sum = sum.wrapping_add(dev.vendor_id as u32);
            sum = sum.wrapping_add(dev.device_id as u32);
            sum = sum.wrapping_add(dev.bus as u32);
        }
        self.header.checksum = sum;
        sum
    }

    pub fn validate_checksum(&self) -> bool {
        let mut sum: u32 = 0;
        for dev in &self.devices {
            sum = sum.wrapping_add(dev.vendor_id as u32);
            sum = sum.wrapping_add(dev.device_id as u32);
            sum = sum.wrapping_add(dev.bus as u32);
        }
        sum == self.header.checksum
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  6. Splash Screen Controller
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplashState {
    Hidden,
    Showing,
    FadingOut,
    Done,
}

pub struct SplashScreen {
    pub state: SplashState,
    pub show_tsc: u64,
    pub fade_start_tsc: u64,
    pub fade_duration_us: u64,
    pub progress_percent: u8,
    pub message: &'static str,
}

impl SplashScreen {
    pub const fn new() -> Self {
        Self {
            state: SplashState::Hidden,
            show_tsc: 0,
            fade_start_tsc: 0,
            fade_duration_us: 300_000, // 300ms fade
            progress_percent: 0,
            message: "Starting AthenaOS...",
        }
    }

    pub fn show(&mut self) {
        self.state = SplashState::Showing;
        self.show_tsc = read_tsc_boot();
        self.progress_percent = 0;
    }

    pub fn set_progress(&mut self, percent: u8) {
        self.progress_percent = percent.min(100);
    }

    pub fn set_message(&mut self, msg: &'static str) {
        self.message = msg;
    }

    pub fn begin_fade_out(&mut self) {
        self.state = SplashState::FadingOut;
        self.fade_start_tsc = read_tsc_boot();
    }

    pub fn update(&mut self, tsc_freq_khz: u64) {
        if self.state == SplashState::FadingOut {
            let elapsed = read_tsc_boot().saturating_sub(self.fade_start_tsc);
            let elapsed_us = tsc_to_us(elapsed, tsc_freq_khz);
            if elapsed_us >= self.fade_duration_us {
                self.state = SplashState::Done;
            }
        }
    }

    pub fn is_done(&self) -> bool {
        self.state == SplashState::Done
    }

    /// Paint the full splash frame: dark `bg.base` field, centered Rae mark,
    /// wordmark, progress track and status caption. The ONLY thing on screen
    /// during boot — Concept §"Fast is a feature": Windows/macOS boot to a
    /// logo + progress, not a scrolling kernel log. Returns pixels painted
    /// for the mark (the FAIL-able smoketest signal).
    pub fn render_full(&self) -> u64 {
        let Some(fb) = crate::framebuffer::fb_info() else {
            return 0;
        };
        // athgfx::Canvas is ARGB32-only and has no row-stride support: a
        // 24bpp mode (the BIOS/VBE path — writing 4-byte pixels into a 3-byte
        // grid overruns the buffer and faults pre-interrupts) or a padded GOP
        // stride would corrupt. Fall back to the text mirror there.
        if fb.stride != fb.width || fb.bytes_per_pixel != 4 {
            return 0;
        }
        let (w, h) = (fb.width as usize, fb.height as usize);
        let mut canvas = unsafe { athgfx::Canvas::new(fb.ptr, w, h, 4) };
        let p = &ath_tokens::DARK;
        canvas.fill_rect(0, 0, w, h, p.bg_base);

        // Rae mark — accent-tinted, centered, sized to the screen.
        let a = ath_tokens::derive_accent(ath_tokens::RAEBLUE, p);
        let mark = (h / 8).clamp(64, 128);
        let mark_x = (w - mark) / 2;
        let mark_y = h / 2 - mark;
        canvas.draw_icon(
            athgfx::icon::Icon::RaeLogo,
            mark_x as i32,
            mark_y as i32,
            mark as i32,
            a.base,
        );
        // Wordmark under the mark.
        let word = "AthenaOS";
        let style = ath_tokens::TYPE_TITLE;
        let ww = canvas.measure_text_aa(word, style, athgfx::text::FontFamily::Sans);
        canvas.draw_text_aa(
            (w as i32 - ww) / 2,
            (mark_y + mark + 16) as i32,
            word,
            style,
            p.text_primary,
            athgfx::text::FontFamily::Sans,
        );
        self.render_progress();

        // Count the mark's painted (non-bg) pixels — the smoketest proof that
        // the splash actually draws (its predecessor was pure state, rule 9).
        let mut painted = 0u64;
        for py in mark_y..(mark_y + mark).min(h) {
            for px in mark_x..(mark_x + mark).min(w) {
                if canvas.get_pixel(px, py) != p.bg_base {
                    painted += 1;
                }
            }
        }
        painted
    }

    /// Repaint only the progress track + status caption band (called at every
    /// boot-tier boundary — cheap, no full-screen redraw).
    pub fn render_progress(&self) {
        let Some(fb) = crate::framebuffer::fb_info() else {
            return;
        };
        if fb.stride != fb.width || fb.bytes_per_pixel != 4 {
            return;
        }
        let (w, h) = (fb.width as usize, fb.height as usize);
        let mut canvas = unsafe { athgfx::Canvas::new(fb.ptr, w, h, 4) };
        let p = &ath_tokens::DARK;
        let a = ath_tokens::derive_accent(ath_tokens::RAEBLUE, p);

        let track_w = (w / 4).clamp(200, 360);
        let track_h = 4usize;
        let tx = (w - track_w) / 2;
        let ty = h / 2 + 40;
        // Clear the band (track + caption) back to the field, then repaint.
        let cap_lh = ath_tokens::TYPE_CAPTION.line_height as usize;
        canvas.fill_rect(0, ty, w, track_h + 14 + cap_lh, p.bg_base);
        canvas.fill_rounded_rect(tx, ty, track_w, track_h, 2, p.bg_elevated);
        let fill = track_w * (self.progress_percent as usize).min(100) / 100;
        if fill > 0 {
            canvas.fill_rounded_rect(tx, ty, fill, track_h, 2, a.base);
        }
        let msg_w = canvas.measure_text_aa(
            self.message,
            ath_tokens::TYPE_CAPTION,
            athgfx::text::FontFamily::Sans,
        );
        canvas.draw_text_aa(
            (w as i32 - msg_w) / 2,
            (ty + track_h + 14) as i32,
            self.message,
            ath_tokens::TYPE_CAPTION,
            p.text_secondary,
            athgfx::text::FontFamily::Sans,
        );
    }
}

/// Painted-pixel count from the initial splash draw (0 = splash never drew) —
/// consumed by the boot smoketest so a regression back to a state-only
/// "phantom splash" prints FAIL.
pub static SPLASH_MARK_PIXELS: AtomicU64 = AtomicU64::new(0);

/// Show the splash as soon as the framebuffer console is up (called from
/// `kernel_main` right after `console::init()`): paints the full frame and
/// gates the serial→GOP text mirror OFF so the user sees a product booting,
/// not a kernel log crawl. The mirror STAYS ON in safe mode (the flash-stick
/// diagnostic image — CLAUDE.md §9 prefers diagnosability there) and is
/// re-enabled by the panic handler so failures always show their log.
pub fn splash_show_early() {
    if crate::block_io::SAFE_MODE.load(Ordering::Relaxed) {
        return; // safe-mode boots keep the on-screen log (diagnostic image)
    }
    let painted = {
        let mut splash = SPLASH.lock();
        splash.show();
        splash.set_message("Starting AthenaOS...");
        splash.render_full()
    };
    SPLASH_MARK_PIXELS.store(painted, Ordering::Relaxed);
    // Only silence the on-screen log if the splash actually painted (a padded
    // GOP stride or missing framebuffer falls back to the text mirror).
    if painted > 0 {
        crate::console::set_console_mirror(false);
    }
}

/// Advance the splash progress bar (tier boundaries in `kernel_main`).
///
/// Mid-boot painters (the compositor's effects selftest, GOP smoketests) can
/// repaint the framebuffer under the splash; a lone band repaint over that
/// leaves their leftovers on screen. A single-pixel probe of the top-left
/// field detects the lost backdrop and re-renders the whole frame instead.
pub fn splash_progress(percent: u8, msg: &'static str) {
    let mut splash = SPLASH.lock();
    if splash.state != SplashState::Showing {
        return;
    }
    splash.set_progress(percent);
    splash.set_message(msg);
    let backdrop_intact = crate::framebuffer::fb_info()
        .filter(|f| f.stride == f.width && f.bytes_per_pixel == 4)
        .map(|f| {
            let c = unsafe { athgfx::Canvas::new(f.ptr, f.width as usize, f.height as usize, 4) };
            c.get_pixel(8, 8) == ath_tokens::DARK.bg_base
        })
        .unwrap_or(false);
    if backdrop_intact {
        splash.render_progress();
    } else {
        let painted = splash.render_full();
        if painted > 0 {
            SPLASH_MARK_PIXELS.store(painted, Ordering::Relaxed);
        }
    }
}

/// R10 smoketest for the boot face. FAIL-able: if this boot SHOULD have shown
/// the splash (not safe mode, drawable un-padded framebuffer) but the mark
/// painted zero pixels, the splash regressed to its old state-only phantom.
pub fn run_splash_smoketest() {
    let painted = SPLASH_MARK_PIXELS.load(Ordering::Relaxed);
    let safe = crate::block_io::SAFE_MODE.load(Ordering::Relaxed);
    // Un-drawable for the ARGB32 stride-free Canvas: padded stride OR 24bpp
    // (the BIOS/VBE mode) OR no framebuffer at all.
    let fb_undrawable = crate::framebuffer::fb_info()
        .map(|f| f.stride != f.width || f.bytes_per_pixel != 4)
        .unwrap_or(true);
    let shown = { SPLASH.lock().state != SplashState::Hidden };
    // The Rae mark at >=64px paints thousands of px; 500 is a generous floor
    // that still catches "drew nothing".
    let pass = if safe || fb_undrawable {
        true // legitimately fell back to the text mirror
    } else {
        shown && painted >= 500
    };
    crate::serial_println!(
        "[splash] boot-face smoketest: shown={} mark_px={} safe_mode={} fb_undrawable={} -> {}",
        shown,
        painted,
        safe,
        fb_undrawable,
        if pass { "PASS" } else { "FAIL" },
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
//  7. Readahead Prefetcher
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct ReadaheadEntry {
    pub path_hash: u64,
    pub offset_sectors: u64,
    pub length_sectors: u32,
    pub priority: u8,
    pub fetched: bool,
}

pub struct ReadaheadPrefetcher {
    pub entries: Vec<ReadaheadEntry>,
    pub total_sectors: u64,
    pub fetched_sectors: u64,
    pub active: bool,
}

impl ReadaheadPrefetcher {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            total_sectors: 0,
            fetched_sectors: 0,
            active: false,
        }
    }

    pub fn add_entry(&mut self, path_hash: u64, offset: u64, length: u32, priority: u8) {
        self.entries.push(ReadaheadEntry {
            path_hash,
            offset_sectors: offset,
            length_sectors: length,
            priority,
            fetched: false,
        });
        self.total_sectors += length as u64;
        self.entries.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    pub fn start(&mut self) {
        self.active = true;
        self.fetched_sectors = 0;
        for entry in &mut self.entries {
            entry.fetched = false;
        }
    }

    pub fn next_unfetched(&self) -> Option<&ReadaheadEntry> {
        self.entries.iter().find(|e| !e.fetched)
    }

    pub fn mark_fetched(&mut self, path_hash: u64) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.path_hash == path_hash && !e.fetched)
        {
            entry.fetched = true;
            self.fetched_sectors += entry.length_sectors as u64;
        }
    }

    pub fn progress_percent(&self) -> u8 {
        if self.total_sectors == 0 {
            return 100;
        }
        ((self.fetched_sectors * 100) / self.total_sectors) as u8
    }

    pub fn is_complete(&self) -> bool {
        self.entries.iter().all(|e| e.fetched)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  8. Boot Time Budget Tracker
// ═══════════════════════════════════════════════════════════════════════════════

pub struct BootBudget {
    pub stage_budgets: BTreeMap<u8, u64>,
    pub stage_actuals: BTreeMap<u8, u64>,
    pub total_budget_us: u64,
    pub total_actual_us: u64,
}

impl BootBudget {
    pub fn new() -> Self {
        Self {
            stage_budgets: BTreeMap::new(),
            stage_actuals: BTreeMap::new(),
            total_budget_us: BUDGET_TOTAL_TARGET_US,
            total_actual_us: 0,
        }
    }

    pub fn set_budget(&mut self, stage: BootStage, budget_us: u64) {
        self.stage_budgets.insert(stage as u8, budget_us);
    }

    pub fn record_actual(&mut self, stage: BootStage, actual_us: u64) {
        self.stage_actuals.insert(stage as u8, actual_us);
        self.total_actual_us = self.stage_actuals.values().sum();
    }

    pub fn remaining_budget_us(&self) -> u64 {
        self.total_budget_us.saturating_sub(self.total_actual_us)
    }

    pub fn is_over_budget(&self, stage: BootStage) -> bool {
        let budget = self
            .stage_budgets
            .get(&(stage as u8))
            .copied()
            .unwrap_or(u64::MAX);
        let actual = self.stage_actuals.get(&(stage as u8)).copied().unwrap_or(0);
        actual > budget
    }

    pub fn budget_utilization_percent(&self) -> u8 {
        if self.total_budget_us == 0 {
            return 100;
        }
        ((self.total_actual_us * 100) / self.total_budget_us).min(255) as u8
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TSC Helpers
// ═══════════════════════════════════════════════════════════════════════════════

#[inline]
fn read_tsc_boot() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

#[inline]
fn tsc_to_us(tsc_delta: u64, tsc_freq_khz: u64) -> u64 {
    if tsc_freq_khz == 0 {
        return 0;
    }
    tsc_delta / (tsc_freq_khz / 1000)
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Global State & Init
// ═══════════════════════════════════════════════════════════════════════════════

pub static BOOT_PROFILER: Mutex<BootProfiler> = Mutex::new(BootProfiler::new());
pub static SPLASH: Mutex<SplashScreen> = Mutex::new(SplashScreen::new());
pub static BOOT_COMPLETE: AtomicBool = AtomicBool::new(false);
pub static DESKTOP_VISIBLE: AtomicBool = AtomicBool::new(false);

// ═══════════════════════════════════════════════════════════════════════════════
//  Boot-time wall-clock measurement (Phase 0 — <6 s gate)
//
//  Usage:
//    record_boot_start(boot_start_tsc)  — call from kernel_main with the TSC
//                                         snapshot captured at entry.
//    set_tsc_mhz(apic::TSC_FREQ_MHZ)   — call after APIC calibration.
//    record_boot_complete()             — call just before "System successfully
//                                         booted." log line.
//    check_boot_time_gate()             — call after record_boot_complete() to
//                                         print the PASS/WARN verdict.
// ═══════════════════════════════════════════════════════════════════════════════

/// TSC value captured at the very first instruction of kernel_main.
static BOOT_START_TSC: AtomicU64 = AtomicU64::new(0);
/// TSC value captured just before "System successfully booted."
static BOOT_END_TSC: AtomicU64 = AtomicU64::new(0);
/// TSC frequency in MHz (populated from apic::TSC_FREQ_MHZ after calibration).
static TSC_MHZ: AtomicU64 = AtomicU64::new(0);

/// Store the TSC captured at kernel entry so the boot-time gate can use it.
pub fn record_boot_start(tsc: u64) {
    BOOT_START_TSC.store(tsc, Ordering::Relaxed);
}

/// Store the TSC frequency in MHz. Call after `apic::calibrate_tsc()`.
pub fn set_tsc_mhz(mhz: u64) {
    TSC_MHZ.store(mhz, Ordering::Relaxed);
}

/// Calibrated TSC frequency in MHz (= ticks per microsecond), or 0 if not yet
/// calibrated. Used by `/proc/athena/perf` to convert TSC deltas to µs.
pub fn tsc_mhz() -> u64 {
    TSC_MHZ.load(Ordering::Relaxed)
}

/// Snap the end TSC. Call just before the "System successfully booted." line.
pub fn record_boot_complete() {
    BOOT_END_TSC.store(read_tsc_boot(), Ordering::Relaxed);
}

/// Returns the measured kernel boot duration in milliseconds.
/// Returns 0 if `record_boot_start` / `record_boot_complete` have not been
/// called, or if the TSC frequency is unknown.
pub fn boot_time_ms() -> u64 {
    let mhz = TSC_MHZ.load(Ordering::Relaxed).max(1);
    let end = BOOT_END_TSC.load(Ordering::Relaxed);
    let start = BOOT_START_TSC.load(Ordering::Relaxed);
    if end == 0 || end <= start {
        return 0;
    }
    let delta = end.saturating_sub(start);
    // delta / (MHz * 1_000) = delta ticks / (ticks per millisecond)
    delta / (mhz * 1_000)
}

/// Print a PASS/WARN verdict against the 6 000 ms Concept target.
/// Call from kernel_main after `record_boot_complete()`.
pub fn check_boot_time_gate() {
    let ms = boot_time_ms();
    if ms == 0 {
        crate::serial_println!(
            "[boot] boot_time_ms: unknown (TSC not calibrated or start/end not recorded)"
        );
        return;
    }
    if ms > 6_000 {
        crate::serial_println!("[boot] WARN: boot time {}ms exceeds 6000ms target!", ms);
    } else {
        crate::serial_println!("[boot] boot time: {}ms (target <6000ms) -> OK", ms);
    }
}

pub fn init() {
    let mut profiler = BOOT_PROFILER.lock();
    profiler.start_boot();

    let mut splash = SPLASH.lock();
    splash.show();
    splash.set_message("Initializing AthenaOS...");
}

/// Mark a boot stage as starting. Returns a handle to end it.
pub fn begin_stage(stage: BootStage) -> usize {
    let mut profiler = BOOT_PROFILER.lock();
    profiler.begin_stage(stage)
}

/// Mark a boot stage as complete.
pub fn end_stage(handle: usize) {
    let mut profiler = BOOT_PROFILER.lock();
    profiler.end_stage(handle);
}

/// Signal that the desktop is visible — triggers deferred init.
pub fn signal_desktop_visible() {
    DESKTOP_VISIBLE.store(true, Ordering::SeqCst);
    let mut splash = SPLASH.lock();
    splash.begin_fade_out();
}

/// Signal boot is fully complete (all deferred tasks done).
pub fn signal_boot_complete() {
    BOOT_COMPLETE.store(true, Ordering::SeqCst);
    let mut profiler = BOOT_PROFILER.lock();
    profiler.end_boot();
}
