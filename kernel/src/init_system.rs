//! AthenaOS Init System — a systemd-class service manager.
//!
//! Manages service lifecycle, socket activation, timer units,
//! dependency resolution with topological sort, and journal logging.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::process::{Pid, ResourceLimits};

// ═══════════════════════════════════════════════════════════════════════════════
// Error Types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitError {
    UnitNotFound(String),
    DependencyFailed(String),
    AlreadyActive(String),
    AlreadyInactive(String),
    StartTimeout(String),
    StopTimeout(String),
    ExecFailed(String),
    CyclicDependency,
    ResourceExhausted,
    PermissionDenied,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Unit Types & State
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitType {
    Service,
    Socket,
    Timer,
    Mount,
    Target,
    Device,
    Path,
    Scope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitState {
    Inactive,
    Activating,
    Active,
    Deactivating,
    Failed,
    Reloading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    No,
    Always,
    OnFailure,
    OnAbnormal,
    OnWatchdog,
    OnAbort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    Journal,
    Console,
    File(String),
    Null,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Service Unit
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct ServiceUnit {
    pub name: String,
    pub description: String,
    pub unit_type: UnitType,
    pub state: UnitState,
    pub pid: Option<Pid>,
    pub exec_start: String,
    pub exec_stop: Option<String>,
    pub working_directory: Option<String>,
    pub environment: BTreeMap<String, String>,
    pub dependencies: Vec<String>,
    pub wanted_by: Vec<String>,
    pub conflicts: Vec<String>,
    pub restart_policy: RestartPolicy,
    pub restart_delay_ms: u64,
    pub restart_count: u32,
    pub max_restarts: u32,
    pub start_timeout_ms: u64,
    pub stop_timeout_ms: u64,
    pub resource_limits: Option<ResourceLimits>,
    pub capabilities: Vec<String>,
    pub sandbox: bool,
    pub log_target: LogTarget,
    pub started_at: Option<u64>,
    pub stopped_at: Option<u64>,
    pub exit_code: Option<i32>,
}

impl ServiceUnit {
    pub fn new(name: &str, exec_start: &str) -> Self {
        Self {
            name: String::from(name),
            description: String::new(),
            unit_type: UnitType::Service,
            state: UnitState::Inactive,
            pid: None,
            exec_start: String::from(exec_start),
            exec_stop: None,
            working_directory: None,
            environment: BTreeMap::new(),
            dependencies: Vec::new(),
            wanted_by: Vec::new(),
            conflicts: Vec::new(),
            restart_policy: RestartPolicy::No,
            restart_delay_ms: 100,
            restart_count: 0,
            max_restarts: 5,
            start_timeout_ms: 30_000,
            stop_timeout_ms: 10_000,
            resource_limits: None,
            capabilities: Vec::new(),
            sandbox: false,
            log_target: LogTarget::Journal,
            started_at: None,
            stopped_at: None,
            exit_code: None,
        }
    }

    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = String::from(desc);
        self
    }

    pub fn with_dependencies(mut self, deps: &[&str]) -> Self {
        self.dependencies = deps.iter().map(|s| String::from(*s)).collect();
        self
    }

    pub fn with_wanted_by(mut self, targets: &[&str]) -> Self {
        self.wanted_by = targets.iter().map(|s| String::from(*s)).collect();
        self
    }

    pub fn with_restart(mut self, policy: RestartPolicy) -> Self {
        self.restart_policy = policy;
        self
    }

    pub fn with_sandbox(mut self, sandboxed: bool) -> Self {
        self.sandbox = sandboxed;
        self
    }

    pub fn uptime(&self, now: u64) -> Option<u64> {
        self.started_at.map(|start| now.saturating_sub(start))
    }

    pub fn is_active(&self) -> bool {
        self.state == UnitState::Active
    }

    pub fn is_failed(&self) -> bool {
        self.state == UnitState::Failed
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Socket Unit (socket activation)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SocketUnit {
    pub name: String,
    pub listen_stream: Option<String>,
    pub listen_datagram: Option<String>,
    pub listen_sequential_packet: Option<String>,
    pub accept: bool,
    pub service: String,
    pub backlog: u32,
    pub state: UnitState,
    pub fd: Option<u32>,
}

impl SocketUnit {
    pub fn new(name: &str, service: &str) -> Self {
        Self {
            name: String::from(name),
            listen_stream: None,
            listen_datagram: None,
            listen_sequential_packet: None,
            accept: false,
            service: String::from(service),
            backlog: 128,
            state: UnitState::Inactive,
            fd: None,
        }
    }

    pub fn with_stream(mut self, addr: &str) -> Self {
        self.listen_stream = Some(String::from(addr));
        self
    }

    pub fn with_datagram(mut self, addr: &str) -> Self {
        self.listen_datagram = Some(String::from(addr));
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Timer Unit
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct TimerUnit {
    pub name: String,
    pub on_boot_sec: Option<u64>,
    pub on_unit_active_sec: Option<u64>,
    pub on_calendar: Option<String>,
    pub persistent: bool,
    pub service: String,
    pub last_triggered: Option<u64>,
    pub state: UnitState,
    pub next_trigger: Option<u64>,
}

impl TimerUnit {
    pub fn new(name: &str, service: &str) -> Self {
        Self {
            name: String::from(name),
            on_boot_sec: None,
            on_unit_active_sec: None,
            on_calendar: None,
            persistent: false,
            service: String::from(service),
            last_triggered: None,
            state: UnitState::Inactive,
            next_trigger: None,
        }
    }

    pub fn with_on_boot(mut self, secs: u64) -> Self {
        self.on_boot_sec = Some(secs);
        self
    }

    pub fn with_interval(mut self, secs: u64) -> Self {
        self.on_unit_active_sec = Some(secs);
        self
    }

    pub fn with_calendar(mut self, spec: &str) -> Self {
        self.on_calendar = Some(String::from(spec));
        self
    }

    pub fn should_trigger(&self, now: u64, boot_time: u64) -> bool {
        if let Some(on_boot) = self.on_boot_sec {
            let trigger_at = boot_time + on_boot * 1_000_000;
            if now >= trigger_at && self.last_triggered.is_none() {
                return true;
            }
        }

        if let Some(interval) = self.on_unit_active_sec {
            if let Some(last) = self.last_triggered {
                let next = last + interval * 1_000_000;
                if now >= next {
                    return true;
                }
            }
        }

        false
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Target Unit (grouping)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct TargetUnit {
    pub name: String,
    pub description: String,
    pub requires: Vec<String>,
    pub wants: Vec<String>,
    pub after: Vec<String>,
    pub before: Vec<String>,
    pub state: UnitState,
}

impl TargetUnit {
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: String::from(name),
            description: String::from(description),
            requires: Vec::new(),
            wants: Vec::new(),
            after: Vec::new(),
            before: Vec::new(),
            state: UnitState::Inactive,
        }
    }

    pub fn with_requires(mut self, units: &[&str]) -> Self {
        self.requires = units.iter().map(|s| String::from(*s)).collect();
        self
    }

    pub fn with_wants(mut self, units: &[&str]) -> Self {
        self.wants = units.iter().map(|s| String::from(*s)).collect();
        self
    }

    pub fn with_after(mut self, units: &[&str]) -> Self {
        self.after = units.iter().map(|s| String::from(*s)).collect();
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Journal / Logging
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: u64,
    pub unit: String,
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    pub fn new(timestamp: u64, unit: &str, level: LogLevel, message: &str) -> Self {
        Self {
            timestamp,
            unit: String::from(unit),
            level,
            message: String::from(message),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Service Manager (Global)
// ═══════════════════════════════════════════════════════════════════════════════

pub static SERVICE_MANAGER: Mutex<Option<ServiceManager>> = Mutex::new(None);

pub struct ServiceManager {
    services: BTreeMap<String, ServiceUnit>,
    sockets: BTreeMap<String, SocketUnit>,
    timers: BTreeMap<String, TimerUnit>,
    targets: BTreeMap<String, TargetUnit>,
    default_target: String,
    boot_time: u64,
    log_buffer: Vec<LogEntry>,
    max_log_entries: usize,
    next_timestamp: u64,
}

impl ServiceManager {
    pub fn new() -> Self {
        Self {
            services: BTreeMap::new(),
            sockets: BTreeMap::new(),
            timers: BTreeMap::new(),
            targets: BTreeMap::new(),
            default_target: String::from("graphical.target"),
            boot_time: 0,
            log_buffer: Vec::new(),
            max_log_entries: 8192,
            next_timestamp: 0,
        }
    }

    pub fn register_service(&mut self, unit: ServiceUnit) {
        self.log(LogLevel::Info, &unit.name, "Registered service unit");
        self.services.insert(unit.name.clone(), unit);
    }

    pub fn register_socket(&mut self, unit: SocketUnit) {
        self.log(LogLevel::Info, &unit.name, "Registered socket unit");
        self.sockets.insert(unit.name.clone(), unit);
    }

    pub fn register_timer(&mut self, unit: TimerUnit) {
        self.log(LogLevel::Info, &unit.name, "Registered timer unit");
        self.timers.insert(unit.name.clone(), unit);
    }

    pub fn register_target(&mut self, unit: TargetUnit) {
        self.log(LogLevel::Info, &unit.name, "Registered target unit");
        self.targets.insert(unit.name.clone(), unit);
    }

    pub fn start_unit(&mut self, name: &str) -> Result<(), InitError> {
        // Check if it's a target
        if let Some(target) = self.targets.get_mut(name) {
            target.state = UnitState::Active;
            self.log(LogLevel::Info, name, "Reached target");
            return Ok(());
        }

        let unit = self
            .services
            .get(name)
            .ok_or_else(|| InitError::UnitNotFound(String::from(name)))?;

        if unit.state == UnitState::Active {
            return Err(InitError::AlreadyActive(String::from(name)));
        }

        // Check dependencies
        let deps = unit.dependencies.clone();
        for dep in &deps {
            if let Some(dep_unit) = self.services.get(dep.as_str()) {
                if dep_unit.state != UnitState::Active {
                    self.log(
                        LogLevel::Warning,
                        name,
                        &alloc::format!("Dependency {} not active, starting it first", dep),
                    );
                    self.start_unit(dep)?;
                }
            } else if self.targets.get(dep.as_str()).is_none() {
                return Err(InitError::DependencyFailed(dep.clone()));
            }
        }

        // Check conflicts
        let conflicts = self
            .services
            .get(name)
            .map(|u| u.conflicts.clone())
            .unwrap_or_default();
        for conflict in &conflicts {
            if let Some(c_unit) = self.services.get(conflict.as_str()) {
                if c_unit.state == UnitState::Active {
                    self.log(
                        LogLevel::Warning,
                        name,
                        &alloc::format!("Stopping conflicting unit {}", conflict),
                    );
                    let _ = self.stop_unit(conflict);
                }
            }
        }

        // Activate the unit
        let exec_start = self
            .services
            .get(name)
            .map(|u| u.exec_start.clone())
            .unwrap_or_default();

        if let Some(unit) = self.services.get_mut(name) {
            unit.state = UnitState::Activating;
            unit.started_at = Some(self.next_timestamp);
            unit.stopped_at = None;
            unit.exit_code = None;

            if let Some(pid) = try_spawn_exec(&exec_start) {
                unit.pid = Some(Pid(pid));
                unit.state = UnitState::Active;
                self.log(
                    LogLevel::Info,
                    name,
                    &alloc::format!("Spawned ELF pid={}", pid),
                );
            } else {
                // Kernel-owned or not-yet-packaged units (compositor, dbus, …)
                // activate without a process — same as a oneshot target.
                unit.state = UnitState::Active;
                self.log(LogLevel::Info, name, "Started (no ELF in initramfs)");
            }
        }

        Ok(())
    }

    pub fn stop_unit(&mut self, name: &str) -> Result<(), InitError> {
        if let Some(target) = self.targets.get_mut(name) {
            target.state = UnitState::Inactive;
            return Ok(());
        }

        let unit = self
            .services
            .get(name)
            .ok_or_else(|| InitError::UnitNotFound(String::from(name)))?;

        if unit.state == UnitState::Inactive {
            return Err(InitError::AlreadyInactive(String::from(name)));
        }

        if let Some(unit) = self.services.get_mut(name) {
            unit.state = UnitState::Deactivating;
            unit.state = UnitState::Inactive;
            unit.stopped_at = Some(self.next_timestamp);
            unit.pid = None;
        }
        self.log(LogLevel::Info, name, "Stopped");

        Ok(())
    }

    pub fn restart_unit(&mut self, name: &str) -> Result<(), InitError> {
        self.log(LogLevel::Info, name, "Restarting service...");
        let _ = self.stop_unit(name);
        self.start_unit(name)
    }

    pub fn reload_unit(&mut self, name: &str) -> Result<(), InitError> {
        let unit = self
            .services
            .get_mut(name)
            .ok_or_else(|| InitError::UnitNotFound(String::from(name)))?;

        if unit.state != UnitState::Active {
            return Err(InitError::AlreadyInactive(String::from(name)));
        }

        unit.state = UnitState::Reloading;
        self.log(LogLevel::Info, name, "Reloading configuration...");

        if let Some(unit) = self.services.get_mut(name) {
            unit.state = UnitState::Active;
        }
        self.log(LogLevel::Info, name, "Reloaded");
        Ok(())
    }

    pub fn enable_unit(&mut self, name: &str) -> Result<(), InitError> {
        let unit = self
            .services
            .get(name)
            .ok_or_else(|| InitError::UnitNotFound(String::from(name)))?;

        let wanted_by = unit.wanted_by.clone();
        for target_name in &wanted_by {
            if let Some(target) = self.targets.get_mut(target_name.as_str()) {
                if !target.wants.contains(&String::from(name)) {
                    target.wants.push(String::from(name));
                }
            }
        }

        self.log(LogLevel::Info, name, "Unit enabled");
        Ok(())
    }

    pub fn disable_unit(&mut self, name: &str) -> Result<(), InitError> {
        if !self.services.contains_key(name) {
            return Err(InitError::UnitNotFound(String::from(name)));
        }

        for target in self.targets.values_mut() {
            target.wants.retain(|w| w != name);
        }

        self.log(LogLevel::Info, name, "Unit disabled");
        Ok(())
    }

    pub fn unit_status(&self, name: &str) -> Option<&ServiceUnit> {
        self.services.get(name)
    }

    pub fn list_units(&self) -> Vec<&ServiceUnit> {
        self.services.values().collect()
    }

    pub fn list_failed(&self) -> Vec<&ServiceUnit> {
        self.services
            .values()
            .filter(|u| u.state == UnitState::Failed)
            .collect()
    }

    pub fn boot(&mut self) -> Result<(), InitError> {
        self.log(LogLevel::Notice, "init", "System boot initiated");
        self.boot_time = self.next_timestamp;

        let boot_order = self.topological_sort();

        self.log(
            LogLevel::Info,
            "init",
            &alloc::format!("Boot order: {} units to start", boot_order.len()),
        );

        for unit_name in &boot_order {
            if self.services.contains_key(unit_name.as_str()) {
                match self.start_unit(unit_name) {
                    Ok(()) => {}
                    Err(InitError::AlreadyActive(_)) => {}
                    Err(e) => {
                        self.log(
                            LogLevel::Error,
                            unit_name,
                            &alloc::format!("Failed to start: {:?}", e),
                        );
                    }
                }
            } else if self.targets.contains_key(unit_name.as_str()) {
                let _ = self.start_unit(unit_name);
            }
        }

        // Activate timers
        let timer_names: Vec<String> = self.timers.keys().cloned().collect();
        for name in &timer_names {
            if let Some(timer) = self.timers.get_mut(name) {
                timer.state = UnitState::Active;
                if let Some(on_boot) = timer.on_boot_sec {
                    timer.next_trigger = Some(self.boot_time + on_boot * 1_000_000);
                }
            }
        }

        // Activate sockets
        let socket_names: Vec<String> = self.sockets.keys().cloned().collect();
        for name in &socket_names {
            if let Some(socket) = self.sockets.get_mut(name) {
                socket.state = UnitState::Active;
                self.log(LogLevel::Info, &name.clone(), "Socket listening");
            }
        }

        self.log(
            LogLevel::Notice,
            "init",
            "System boot complete — reached default target",
        );
        Ok(())
    }

    fn resolve_dependencies(&self, name: &str) -> Vec<String> {
        let mut resolved = Vec::new();
        let mut visited = Vec::new();
        self.resolve_deps_recursive(name, &mut resolved, &mut visited);
        resolved
    }

    fn resolve_deps_recursive(
        &self,
        name: &str,
        resolved: &mut Vec<String>,
        visited: &mut Vec<String>,
    ) {
        if visited.contains(&String::from(name)) {
            return;
        }
        visited.push(String::from(name));

        if let Some(unit) = self.services.get(name) {
            for dep in &unit.dependencies {
                self.resolve_deps_recursive(dep, resolved, visited);
            }
        }

        if let Some(target) = self.targets.get(name) {
            for dep in &target.requires {
                self.resolve_deps_recursive(dep, resolved, visited);
            }
            for dep in &target.wants {
                self.resolve_deps_recursive(dep, resolved, visited);
            }
        }

        if !resolved.contains(&String::from(name)) {
            resolved.push(String::from(name));
        }
    }

    fn topological_sort(&self) -> Vec<String> {
        let default_target = self.default_target.clone();
        self.resolve_dependencies(&default_target)
    }

    pub fn check_timers(&mut self, now: u64) {
        self.next_timestamp = now;

        let triggered: Vec<(String, String)> = self
            .timers
            .iter()
            .filter(|(_, timer)| {
                timer.state == UnitState::Active && timer.should_trigger(now, self.boot_time)
            })
            .map(|(name, timer)| (name.clone(), timer.service.clone()))
            .collect();

        for (timer_name, service_name) in triggered {
            self.log(
                LogLevel::Info,
                &timer_name,
                &alloc::format!("Timer triggered, starting {}", service_name),
            );

            if let Some(timer) = self.timers.get_mut(&timer_name) {
                timer.last_triggered = Some(now);
                if let Some(interval) = timer.on_unit_active_sec {
                    timer.next_trigger = Some(now + interval * 1_000_000);
                }
            }

            let _ = self.start_unit(&service_name);
        }
    }

    pub fn handle_child_exit(&mut self, pid: Pid, exit_code: i32) {
        let service_name = self
            .services
            .iter()
            .find(|(_, unit)| unit.pid == Some(pid))
            .map(|(name, _)| name.clone());

        if let Some(name) = service_name {
            self.log(
                LogLevel::Info,
                &name,
                &alloc::format!("Process exited with code {}", exit_code),
            );

            let should_restart = if let Some(unit) = self.services.get_mut(&name) {
                unit.state = if exit_code == 0 {
                    UnitState::Inactive
                } else {
                    UnitState::Failed
                };
                unit.stopped_at = Some(self.next_timestamp);
                unit.exit_code = Some(exit_code);
                unit.pid = None;

                match unit.restart_policy {
                    RestartPolicy::Always => true,
                    RestartPolicy::OnFailure => exit_code != 0,
                    RestartPolicy::OnAbnormal => exit_code != 0 && exit_code != 1,
                    RestartPolicy::OnAbort => exit_code < 0,
                    RestartPolicy::No => false,
                    RestartPolicy::OnWatchdog => false,
                }
            } else {
                false
            };

            if should_restart {
                let can_restart = if let Some(unit) = self.services.get(&name) {
                    unit.restart_count < unit.max_restarts
                } else {
                    false
                };

                if can_restart {
                    if let Some(unit) = self.services.get_mut(&name) {
                        unit.restart_count += 1;
                    }
                    self.log(LogLevel::Info, &name, "Scheduling restart...");
                    let _ = self.start_unit(&name);
                } else {
                    self.log(
                        LogLevel::Error,
                        &name,
                        "Maximum restart attempts reached, giving up",
                    );
                    if let Some(unit) = self.services.get_mut(&name) {
                        unit.state = UnitState::Failed;
                    }
                }
            }
        }
    }

    pub fn log(&mut self, level: LogLevel, unit: &str, msg: &str) {
        let entry = LogEntry::new(self.next_timestamp, unit, level, msg);

        if self.log_buffer.len() >= self.max_log_entries {
            self.log_buffer.remove(0);
        }
        self.log_buffer.push(entry);
    }

    pub fn journal(&self, unit: Option<&str>, since: Option<u64>) -> Vec<&LogEntry> {
        self.log_buffer
            .iter()
            .filter(|entry| {
                let unit_match = unit.map_or(true, |u| entry.unit == u);
                let time_match = since.map_or(true, |t| entry.timestamp >= t);
                unit_match && time_match
            })
            .collect()
    }

    pub fn journal_by_level(&self, min_level: LogLevel) -> Vec<&LogEntry> {
        self.log_buffer
            .iter()
            .filter(|entry| entry.level <= min_level)
            .collect()
    }

    pub fn uptime(&self, now: u64) -> u64 {
        now.saturating_sub(self.boot_time)
    }

    pub fn active_count(&self) -> usize {
        self.services.values().filter(|u| u.is_active()).count()
    }

    pub fn failed_count(&self) -> usize {
        self.services.values().filter(|u| u.is_failed()).count()
    }

    pub fn total_units(&self) -> usize {
        self.services.len() + self.sockets.len() + self.timers.len() + self.targets.len()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Default Boot Services
// ═══════════════════════════════════════════════════════════════════════════════

/// If `exec_start` names an ELF present in the initramfs, spawn it and return its pid.
fn try_spawn_exec(exec_start: &str) -> Option<u64> {
    let token = exec_start.split_whitespace().next()?;
    let path = token.trim_start_matches('/');
    let data = crate::vfs::read_file(path)?;
    let id = crate::scheduler::spawn_elf_task(&data, None).ok()?;
    Some(id.raw())
}

fn populate_default_services(mgr: &mut ServiceManager) {
    // ── Targets ──────────────────────────────────────────────────────────────

    mgr.register_target(
        TargetUnit::new("basic.target", "Basic system initialization")
            .with_requires(&["journal.service", "udev.service"]),
    );

    mgr.register_target(
        TargetUnit::new("multi-user.target", "Multi-user system")
            .with_requires(&["basic.target"])
            .with_wants(&[
                "dbus.service",
                "network.service",
                "raeshield.service",
                "raesync.service",
                "login.service",
            ])
            .with_after(&["basic.target"]),
    );

    mgr.register_target(
        TargetUnit::new("graphical.target", "Graphical desktop environment")
            .with_requires(&["multi-user.target"])
            .with_wants(&[
                "compositor.service",
                "raeshell.service",
                "audio.service",
                "bluetooth.service",
                "raestore.service",
            ])
            .with_after(&["multi-user.target"]),
    );

    // ── Core System Services ─────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("journal.service", "/sbin/journald")
            .with_description("System Journal Daemon")
            .with_wanted_by(&["basic.target"])
            .with_restart(RestartPolicy::Always),
    );

    mgr.register_service(
        ServiceUnit::new("udev.service", "/sbin/udevd")
            .with_description("Device Manager")
            .with_wanted_by(&["basic.target"])
            .with_restart(RestartPolicy::Always),
    );

    mgr.register_service(
        ServiceUnit::new("dbus.service", "/usr/bin/dbus-daemon --system")
            .with_description("D-Bus System Message Bus")
            .with_dependencies(&["basic.target"])
            .with_wanted_by(&["multi-user.target"])
            .with_restart(RestartPolicy::Always),
    );

    // ── Network ──────────────────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("network.service", "/sbin/raenetd")
            .with_description("AthenaOS Network Manager")
            .with_dependencies(&["udev.service", "dbus.service"])
            .with_wanted_by(&["multi-user.target"])
            .with_restart(RestartPolicy::OnFailure),
    );

    // ── Security ─────────────────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("raeshield.service", "/usr/sbin/raeshieldd")
            .with_description("AthenaOS Security Framework")
            .with_dependencies(&["basic.target"])
            .with_wanted_by(&["multi-user.target"])
            .with_restart(RestartPolicy::Always)
            .with_sandbox(true),
    );

    // ── Sync ─────────────────────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("raesync.service", "/usr/sbin/raesyncd")
            .with_description("AthenaOS Sync Daemon")
            .with_dependencies(&["network.service"])
            .with_wanted_by(&["multi-user.target"])
            .with_restart(RestartPolicy::OnFailure),
    );

    // ── Session Management ───────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("login.service", "/sbin/logind")
            .with_description("Login and Session Manager")
            .with_dependencies(&["dbus.service"])
            .with_wanted_by(&["multi-user.target"])
            .with_restart(RestartPolicy::Always),
    );

    // ── Audio ────────────────────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("audio.service", "/usr/bin/raeaudiod")
            .with_description("AthenaOS Audio Server")
            .with_dependencies(&["udev.service"])
            .with_wanted_by(&["graphical.target"])
            .with_restart(RestartPolicy::OnFailure),
    );

    // ── Bluetooth ────────────────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("bluetooth.service", "/usr/sbin/bluetoothd")
            .with_description("Bluetooth Manager")
            .with_dependencies(&["udev.service", "dbus.service"])
            .with_wanted_by(&["graphical.target"])
            .with_restart(RestartPolicy::OnFailure),
    );

    // ── Compositor ───────────────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("compositor.service", "/usr/bin/rae-compositor")
            .with_description("AthenaOS Wayland Compositor")
            .with_dependencies(&["login.service", "udev.service"])
            .with_wanted_by(&["graphical.target"])
            .with_restart(RestartPolicy::Always),
    );

    // ── Desktop Shell ────────────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("raeshell.service", "/usr/bin/raeshell")
            .with_description("AthenaOS Desktop Shell")
            .with_dependencies(&["compositor.service"])
            .with_wanted_by(&["graphical.target"])
            .with_restart(RestartPolicy::Always),
    );

    // ── App Store Daemon ─────────────────────────────────────────────────────

    mgr.register_service(
        ServiceUnit::new("raestore.service", "/usr/sbin/raestored")
            .with_description("AthenaOS App Store Daemon")
            .with_dependencies(&["network.service"])
            .with_wanted_by(&["graphical.target"])
            .with_restart(RestartPolicy::OnFailure),
    );

    // ── Packaged initramfs ELFs (actually spawned when the unit starts) ─────

    mgr.register_service(
        ServiceUnit::new("driver-supervisor.service", "driver_supervisor")
            .with_description("Userspace driver supervisor")
            .with_dependencies(&["basic.target"])
            .with_wanted_by(&["multi-user.target"])
            .with_restart(RestartPolicy::OnFailure),
    );

    // ── Sockets ──────────────────────────────────────────────────────────────

    mgr.register_socket(
        SocketUnit::new("dbus.socket", "dbus.service").with_stream("/run/dbus/system_bus_socket"),
    );

    mgr.register_socket(
        SocketUnit::new("journal.socket", "journal.service")
            .with_datagram("/run/systemd/journal/socket"),
    );

    // ── Timers ───────────────────────────────────────────────────────────────

    mgr.register_timer(
        TimerUnit::new("raesync.timer", "raesync.service")
            .with_on_boot(60)
            .with_interval(300),
    );

    mgr.register_timer(
        TimerUnit::new("logrotate.timer", "logrotate.service")
            .with_on_boot(900)
            .with_interval(3600),
    );

    // logrotate service (triggered by timer)
    mgr.register_service(
        ServiceUnit::new(
            "logrotate.service",
            "/usr/sbin/logrotate /etc/logrotate.conf",
        )
        .with_description("Log Rotation"),
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════════════════════════════════════════

pub fn init() {
    let mut mgr = ServiceManager::new();
    populate_default_services(&mut mgr);
    *SERVICE_MANAGER.lock() = Some(mgr);
}

pub fn boot() {
    let mut mgr_lock = SERVICE_MANAGER.lock();
    if let Some(ref mut mgr) = *mgr_lock {
        let _ = mgr.boot();
    }
}
