//! Swappable Shell API for AthenaOS.
//!
//! The concept doc says: "Swappable shells — the default AthShell can be
//! replaced with a competing one. The OS doesn't care."
//!
//! This module defines:
//! - `ShellDescriptor` — metadata about a shell (name, binary, capabilities)
//! - `ShellRegistry` — tracks installed shells and the active shell
//! - `switch_shell()` — terminate current shell, launch replacement
//! - Shell ↔ kernel IPC protocol for window management, app launching,
//!   notifications, and input routing
//!
//! The default shell is AthShell. Alternative shells (GameOS couch UI,
//! tiling-only shells, accessibility shells) register via AthStore or
//! sideload. Each shell must create at least one compositor surface for
//! the desktop and handle keyboard/mouse/controller input.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
// Shell Descriptor
// ═══════════════════════════════════════════════════════════════════════════════

/// Unique identifier for a registered shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShellId(pub u64);

impl ShellId {
    pub const RAEENSHELL: ShellId = ShellId(1);
    pub const GAMEOS: ShellId = ShellId(2);
}

/// Capabilities a shell can request from the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellCapabilities {
    pub compositor_access: bool,
    pub input_exclusive: bool,
    pub window_management: bool,
    pub notification_display: bool,
    pub app_launching: bool,
    pub system_tray: bool,
    pub screen_capture: bool,
    pub settings_access: bool,
    pub controller_input: bool,
    pub audio_routing: bool,
}

impl ShellCapabilities {
    pub const fn full() -> Self {
        Self {
            compositor_access: true,
            input_exclusive: true,
            window_management: true,
            notification_display: true,
            app_launching: true,
            system_tray: true,
            screen_capture: true,
            settings_access: true,
            controller_input: true,
            audio_routing: true,
        }
    }

    pub const fn minimal() -> Self {
        Self {
            compositor_access: true,
            input_exclusive: false,
            window_management: true,
            notification_display: false,
            app_launching: true,
            system_tray: false,
            screen_capture: false,
            settings_access: false,
            controller_input: false,
            audio_routing: false,
        }
    }
}

/// Compositor requirements for a shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositorMode {
    /// Full desktop compositing (glassmorphism, animations, etc.)
    FullDesktop,
    /// Simplified compositing (GameOS couch UI — lower overhead)
    GameOs,
    /// Minimal compositing (tiling WM, no effects)
    Minimal,
    /// Direct framebuffer access (kiosk mode, boot splash)
    DirectFb,
}

/// Input routing mode for a shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Keyboard + mouse (traditional desktop)
    KeyboardMouse,
    /// Controller-first (GameOS couch UI)
    ControllerFirst,
    /// Touch-first (future tablet mode)
    TouchFirst,
    /// Accessibility (screen reader, switch access)
    Accessibility,
    /// All input types simultaneously
    All,
}

/// Describes a shell that can be loaded and activated.
#[derive(Debug, Clone)]
pub struct ShellDescriptor {
    pub id: ShellId,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub binary_path: String,
    pub icon_path: Option<String>,
    pub capabilities: ShellCapabilities,
    pub compositor_mode: CompositorMode,
    pub input_mode: InputMode,
    pub is_builtin: bool,
    pub is_signed: bool,
    pub priority: u32,
}

impl ShellDescriptor {
    /// The default AthShell descriptor.
    pub fn raeenshell() -> Self {
        Self {
            id: ShellId::RAEENSHELL,
            name: String::from("AthShell"),
            version: String::from("0.1.0"),
            author: String::from("AthenaOS"),
            description: String::from(
                "Default desktop shell — taskbar, start menu, file manager, system tray",
            ),
            binary_path: String::from("/system/shells/raeshell"),
            icon_path: Some(String::from("/system/icons/raeshell.png")),
            capabilities: ShellCapabilities::full(),
            compositor_mode: CompositorMode::FullDesktop,
            input_mode: InputMode::KeyboardMouse,
            is_builtin: true,
            is_signed: true,
            priority: 0,
        }
    }

    /// The GameOS couch-UI shell.
    pub fn gameos_shell() -> Self {
        Self {
            id: ShellId::GAMEOS,
            name: String::from("GameOS"),
            version: String::from("0.1.0"),
            author: String::from("AthenaOS"),
            description: String::from("Couch UI — controller-driven, big-picture game launcher"),
            binary_path: String::from("/system/shells/gameos"),
            icon_path: Some(String::from("/system/icons/gameos.png")),
            capabilities: ShellCapabilities {
                compositor_access: true,
                input_exclusive: true,
                window_management: true,
                notification_display: true,
                app_launching: true,
                system_tray: false,
                screen_capture: true,
                settings_access: true,
                controller_input: true,
                audio_routing: true,
            },
            compositor_mode: CompositorMode::GameOs,
            input_mode: InputMode::ControllerFirst,
            is_builtin: true,
            is_signed: true,
            priority: 1,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shell State
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellState {
    Registered,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

struct ShellInstance {
    descriptor: ShellDescriptor,
    state: ShellState,
    task_id: Option<crate::task::TaskId>,
    surface_ids: Vec<u64>,
    start_time: u64,
    ipc_channel: Option<usize>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shell ↔ Kernel IPC Protocol
// ═══════════════════════════════════════════════════════════════════════════════

/// Messages from the shell to the kernel.
#[derive(Debug, Clone)]
pub enum ShellToKernel {
    /// Shell is ready and has created its compositor surface(s).
    Ready { surface_ids: Vec<u64> },
    /// Request to launch an application.
    LaunchApp { path: String, args: Vec<String> },
    /// Request to switch to a different shell.
    SwitchShell { target: ShellId },
    /// Shell wants to manage a window (move, resize, close, minimize).
    WindowAction {
        surface_id: u64,
        action: WindowAction,
    },
    /// Shell is posting a notification.
    PostNotification {
        title: String,
        body: String,
        urgency: NotificationUrgency,
    },
    /// Shell requests shutdown / reboot / sleep.
    PowerAction { action: PowerAction },
    /// Shell requests system info (battery, network, volume, etc.).
    QuerySystemStatus,
    /// Shell is shutting down voluntarily.
    Goodbye,
}

/// Messages from the kernel to the shell.
#[derive(Debug, Clone)]
pub enum KernelToShell {
    /// A new window was created by an application.
    WindowCreated {
        surface_id: u64,
        app_name: String,
        title: String,
    },
    /// A window was destroyed.
    WindowDestroyed { surface_id: u64 },
    /// A window title changed.
    WindowTitleChanged { surface_id: u64, title: String },
    /// Keyboard event routed to the shell.
    KeyEvent {
        scancode: u8,
        pressed: bool,
        modifiers: u32,
    },
    /// Mouse event routed to the shell.
    MouseEvent {
        x: i32,
        y: i32,
        buttons: u32,
        event_type: MouseEventType,
    },
    /// Controller event routed to the shell.
    ControllerEvent {
        controller_id: u8,
        button: u16,
        value: i16,
    },
    /// Notification from a system service.
    SystemNotification { source: String, message: String },
    /// System status response.
    SystemStatus {
        battery_pct: u8,
        network_up: bool,
        volume_pct: u8,
    },
    /// Shell is being asked to shut down (another shell is taking over).
    ShutdownRequest { reason: ShutdownReason },
    /// Display configuration changed (resolution, refresh rate, etc.).
    DisplayChanged {
        width: u32,
        height: u32,
        refresh_hz: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    Move { x: i32, y: i32 },
    Resize { width: u32, height: u32 },
    Minimize,
    Maximize,
    Restore,
    Close,
    Focus,
    SetAlwaysOnTop(bool),
    SetFullscreen(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationUrgency {
    Low,
    Normal,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Shutdown,
    Reboot,
    Sleep,
    Hibernate,
    LogOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventType {
    Move,
    ButtonDown,
    ButtonUp,
    Scroll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    ShellSwitch,
    UserLogout,
    SystemShutdown,
    ShellCrash,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shell Registry
// ═══════════════════════════════════════════════════════════════════════════════

pub struct ShellRegistry {
    shells: BTreeMap<ShellId, ShellInstance>,
    active_shell: Option<ShellId>,
    default_shell: ShellId,
    next_id: AtomicU64,
    switch_in_progress: bool,
}

impl ShellRegistry {
    fn new() -> Self {
        let mut registry = Self {
            shells: BTreeMap::new(),
            active_shell: None,
            default_shell: ShellId::RAEENSHELL,
            next_id: AtomicU64::new(100),
            switch_in_progress: false,
        };

        // Register the built-in shells
        let raeshell = ShellDescriptor::raeenshell();
        let gameos = ShellDescriptor::gameos_shell();
        registry.register_internal(raeshell);
        registry.register_internal(gameos);
        registry
    }

    fn register_internal(&mut self, descriptor: ShellDescriptor) {
        let id = descriptor.id;
        self.shells.insert(
            id,
            ShellInstance {
                descriptor,
                state: ShellState::Registered,
                task_id: None,
                surface_ids: Vec::new(),
                start_time: 0,
                ipc_channel: None,
            },
        );
    }

    /// Register a new shell from a AthStore package or sideload.
    pub fn register(&mut self, mut descriptor: ShellDescriptor) -> ShellId {
        let id = ShellId(self.next_id.fetch_add(1, Ordering::Relaxed));
        descriptor.id = id;
        self.shells.insert(
            id,
            ShellInstance {
                descriptor,
                state: ShellState::Registered,
                task_id: None,
                surface_ids: Vec::new(),
                start_time: 0,
                ipc_channel: None,
            },
        );
        id
    }

    /// Unregister a non-builtin shell.
    pub fn unregister(&mut self, id: ShellId) -> Result<(), ShellError> {
        let instance = self.shells.get(&id).ok_or(ShellError::NotFound)?;
        if instance.descriptor.is_builtin {
            return Err(ShellError::CannotRemoveBuiltin);
        }
        if self.active_shell == Some(id) {
            return Err(ShellError::ShellIsActive);
        }
        self.shells.remove(&id);
        Ok(())
    }

    /// List all registered shells.
    pub fn list_shells(&self) -> Vec<&ShellDescriptor> {
        self.shells.values().map(|i| &i.descriptor).collect()
    }

    /// Get the active shell's descriptor.
    pub fn active_shell(&self) -> Option<&ShellDescriptor> {
        self.active_shell
            .and_then(|id| self.shells.get(&id).map(|i| &i.descriptor))
    }

    /// Get the active shell's ID.
    pub fn active_shell_id(&self) -> Option<ShellId> {
        self.active_shell
    }

    /// Get a shell descriptor by ID.
    pub fn get_shell(&self, id: ShellId) -> Option<&ShellDescriptor> {
        self.shells.get(&id).map(|i| &i.descriptor)
    }

    /// Start the specified shell. If another shell is running, it must
    /// be stopped first via `switch_shell()`.
    fn start_shell(&mut self, id: ShellId) -> Result<(), ShellError> {
        let instance = self.shells.get_mut(&id).ok_or(ShellError::NotFound)?;

        if instance.state == ShellState::Running {
            return Err(ShellError::AlreadyRunning);
        }

        // Verify the shell binary exists in VFS
        let binary_path = instance.descriptor.binary_path.clone();
        // The actual process spawn would happen here. For now we update state.
        instance.state = ShellState::Starting;

        // Create an IPC channel for shell ↔ kernel communication
        let chan_id = crate::ipc::IPC.lock().create_channel(false);
        instance.ipc_channel = Some(chan_id);

        // In a full implementation, we'd spawn the shell binary:
        //   let task = Task::new_elf(shell_binary, None)?;
        //   scheduler::spawn(task);
        // For now, mark as running immediately.
        instance.state = ShellState::Running;
        self.active_shell = Some(id);

        crate::serial_println!(
            "[shell] Started shell '{}' (id={}, binary={})",
            instance.descriptor.name,
            id.0,
            binary_path,
        );

        Ok(())
    }

    /// Stop the specified shell.
    fn stop_shell(&mut self, id: ShellId) -> Result<(), ShellError> {
        let instance = self.shells.get_mut(&id).ok_or(ShellError::NotFound)?;

        if instance.state != ShellState::Running && instance.state != ShellState::Starting {
            return Err(ShellError::NotRunning);
        }

        // Send shutdown request to the shell
        instance.state = ShellState::Stopping;

        // Close all surfaces owned by the shell
        for &surface_id in &instance.surface_ids {
            let _ = crate::compositor::close_surface(surface_id);
        }
        instance.surface_ids.clear();

        // Kill the shell process if it has one
        if let Some(task_id) = instance.task_id {
            let _ = crate::scheduler::kill_task(task_id);
            instance.task_id = None;
        }

        instance.state = ShellState::Stopped;

        if self.active_shell == Some(id) {
            self.active_shell = None;
        }

        crate::serial_println!(
            "[shell] Stopped shell '{}' (id={})",
            instance.descriptor.name,
            id.0,
        );

        Ok(())
    }

    /// Switch from the current shell to a different one.
    pub fn switch_shell(&mut self, target: ShellId) -> Result<(), ShellError> {
        if self.switch_in_progress {
            return Err(ShellError::SwitchInProgress);
        }
        if self.active_shell == Some(target) {
            return Err(ShellError::AlreadyRunning);
        }
        if !self.shells.contains_key(&target) {
            return Err(ShellError::NotFound);
        }

        self.switch_in_progress = true;

        // Stop the current shell
        if let Some(current) = self.active_shell {
            if let Err(e) = self.stop_shell(current) {
                crate::serial_println!("[shell] Warning: failed to stop current shell: {:?}", e);
            }
        }

        // Start the new shell
        let result = self.start_shell(target);

        self.switch_in_progress = false;
        result
    }

    /// Handle a message from the shell.
    pub fn handle_shell_message(
        &mut self,
        from: ShellId,
        msg: ShellToKernel,
    ) -> Option<KernelToShell> {
        let instance = self.shells.get_mut(&from)?;

        match msg {
            ShellToKernel::Ready { surface_ids } => {
                instance.surface_ids = surface_ids;
                instance.state = ShellState::Running;
                Some(KernelToShell::SystemStatus {
                    battery_pct: 100,
                    network_up: true,
                    volume_pct: 75,
                })
            }

            ShellToKernel::LaunchApp { path, args: _args } => {
                // Spawn the application
                if let Some(inode) = crate::vfs::open_path(&path) {
                    let mut data = Vec::new();
                    let mut buf = [0u8; 4096];
                    let mut offset = 0;
                    loop {
                        let n = inode.read_at(offset, &mut buf);
                        if n == 0 {
                            break;
                        }
                        data.extend_from_slice(&buf[..n]);
                        offset += n;
                    }

                    let parent = crate::scheduler::current_task_id();
                    match crate::scheduler::spawn_elf_task(&data, parent) {
                        Ok(id) => {
                            crate::serial_println!(
                                "[shell] Launched app '{}' as task {}",
                                path,
                                id.raw(),
                            );
                        }
                        Err(_) => {
                            crate::serial_println!("[shell] Failed to launch '{}'", path);
                        }
                    }
                }
                None
            }

            ShellToKernel::SwitchShell { target } => {
                let _ = self.switch_shell(target);
                None
            }

            ShellToKernel::WindowAction { surface_id, action } => {
                match action {
                    WindowAction::Focus => {
                        let _ = crate::compositor::focus_surface(surface_id);
                    }
                    WindowAction::Close => {
                        let _ = crate::compositor::close_surface(surface_id);
                    }
                    WindowAction::Move { x, y } => {
                        let _ = crate::compositor::present_surface(surface_id, x, y);
                    }
                    _ => {}
                }
                None
            }

            ShellToKernel::PostNotification {
                title,
                body,
                urgency,
            } => {
                // Render a real toast through the notification surface
                // (MasterChecklist Phase 14.1) — no longer log-only.
                let _ = crate::notify::post(&title, &body, urgency);
                None
            }

            ShellToKernel::PowerAction { action } => {
                crate::serial_println!("[shell] Power action requested: {:?}", action);
                None
            }

            ShellToKernel::QuerySystemStatus => Some(KernelToShell::SystemStatus {
                battery_pct: 100,
                network_up: true,
                volume_pct: 75,
            }),

            ShellToKernel::Goodbye => {
                instance.state = ShellState::Stopped;
                if self.active_shell == Some(from) {
                    self.active_shell = None;
                }
                None
            }
        }
    }

    /// Route an input event to the active shell.
    pub fn route_key_event(
        &self,
        scancode: u8,
        pressed: bool,
        modifiers: u32,
    ) -> Option<KernelToShell> {
        if self.active_shell.is_some() {
            Some(KernelToShell::KeyEvent {
                scancode,
                pressed,
                modifiers,
            })
        } else {
            None
        }
    }

    /// Route a mouse event to the active shell.
    pub fn route_mouse_event(
        &self,
        x: i32,
        y: i32,
        buttons: u32,
        event_type: MouseEventType,
    ) -> Option<KernelToShell> {
        if self.active_shell.is_some() {
            Some(KernelToShell::MouseEvent {
                x,
                y,
                buttons,
                event_type,
            })
        } else {
            None
        }
    }

    /// Notify the active shell that a new window was created.
    pub fn notify_window_created(
        &self,
        surface_id: u64,
        app_name: &str,
        title: &str,
    ) -> Option<KernelToShell> {
        if self.active_shell.is_some() {
            Some(KernelToShell::WindowCreated {
                surface_id,
                app_name: String::from(app_name),
                title: String::from(title),
            })
        } else {
            None
        }
    }

    /// Notify the active shell that a window was destroyed.
    pub fn notify_window_destroyed(&self, surface_id: u64) -> Option<KernelToShell> {
        if self.active_shell.is_some() {
            Some(KernelToShell::WindowDestroyed { surface_id })
        } else {
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shell Errors
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellError {
    NotFound,
    AlreadyRunning,
    NotRunning,
    SwitchInProgress,
    CannotRemoveBuiltin,
    ShellIsActive,
    BinaryNotFound,
    SpawnFailed,
    PermissionDenied,
    InvalidDescriptor,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shell Interface Contract
// ═══════════════════════════════════════════════════════════════════════════════

/// Trait that all shell implementations must satisfy. The shell binary
/// calls these kernel services through the syscall interface; this trait
/// documents the contract.
///
/// A conforming shell MUST:
/// 1. Create at least one compositor surface for the desktop background
/// 2. Handle keyboard and mouse input events
/// 3. Manage application windows (position, z-order, focus)
/// 4. Respond to ShutdownRequest within 5 seconds
///
/// A conforming shell MAY:
/// - Create additional surfaces for taskbar, system tray, etc.
/// - Implement a start menu / app launcher
/// - Display notifications
/// - Provide a file manager
/// - Support controller input (required for GameOS mode)
pub trait ShellContract {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn capabilities(&self) -> ShellCapabilities;
    fn compositor_mode(&self) -> CompositorMode;
    fn input_mode(&self) -> InputMode;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Global Shell Registry
// ═══════════════════════════════════════════════════════════════════════════════

pub static SHELL_REGISTRY: Mutex<Option<ShellRegistry>> = Mutex::new(None);

/// Initialize the shell subsystem and register built-in shells.
pub fn init() {
    *SHELL_REGISTRY.lock() = Some(ShellRegistry::new());
    crate::serial_println!("[ OK ] Shell API initialized (AthShell + GameOS registered)");
}

/// Register a new shell from a package.
pub fn register_shell(descriptor: ShellDescriptor) -> Result<ShellId, ShellError> {
    let mut guard = SHELL_REGISTRY.lock();
    let registry = guard.as_mut().ok_or(ShellError::NotFound)?;
    Ok(registry.register(descriptor))
}

/// Switch to a different shell.
pub fn switch_shell(target: ShellId) -> Result<(), ShellError> {
    let mut guard = SHELL_REGISTRY.lock();
    let registry = guard.as_mut().ok_or(ShellError::NotFound)?;
    registry.switch_shell(target)
}

/// Start the default shell (called during boot).
pub fn start_default_shell() -> Result<(), ShellError> {
    let mut guard = SHELL_REGISTRY.lock();
    let registry = guard.as_mut().ok_or(ShellError::NotFound)?;
    let default = registry.default_shell;
    registry.start_shell(default)
}

/// List all registered shells.
pub fn list_shells() -> Vec<ShellDescriptor> {
    let guard = SHELL_REGISTRY.lock();
    if let Some(ref registry) = *guard {
        registry.list_shells().into_iter().cloned().collect()
    } else {
        Vec::new()
    }
}

/// Get the active shell's descriptor.
pub fn active_shell() -> Option<ShellDescriptor> {
    let guard = SHELL_REGISTRY.lock();
    if let Some(ref registry) = *guard {
        registry.active_shell().cloned()
    } else {
        None
    }
}

/// Get a shell descriptor by ID.
pub fn get_shell(id: ShellId) -> Option<ShellDescriptor> {
    let guard = SHELL_REGISTRY.lock();
    if let Some(ref registry) = *guard {
        registry.get_shell(id).cloned()
    } else {
        None
    }
}

/// Unregister a (non-builtin, non-active) shell.
pub fn unregister_shell(id: ShellId) -> Result<(), ShellError> {
    let mut guard = SHELL_REGISTRY.lock();
    let registry = guard.as_mut().ok_or(ShellError::NotFound)?;
    registry.unregister(id)
}

/// R10 smoketest (Concept §"swappable shell — AthShell can be replaced"): prove the
/// swap registry — a third-party shell can be registered, retrieved, and removed;
/// the built-ins are protected; an unknown id is `NotFound`. It deliberately does
/// NOT execute a live `switch_shell` (that stops the running boot shell); the live
/// AthShell↔GameOS swap is exercised by GameOS mode.
pub fn run_boot_smoketest() {
    let before = list_shells().len();
    let builtins_ok = before >= 2
        && get_shell(ShellId::RAEENSHELL).is_some()
        && get_shell(ShellId::GAMEOS).is_some();

    // Register a third-party (non-builtin) shell — "AthShell can be replaced".
    let mut custom = ShellDescriptor::raeenshell();
    custom.name = String::from("Test Shell");
    custom.is_builtin = false;
    custom.is_signed = false;
    let id = match register_shell(custom) {
        Ok(i) => i,
        Err(_) => {
            crate::serial_println!("[shell-api] smoketest: register FAILED -> FAIL");
            return;
        }
    };
    let registered_ok = get_shell(id)
        .map(|d| d.name == "Test Shell")
        .unwrap_or(false)
        && list_shells().len() == before + 1;

    // A built-in cannot be removed; an unknown id is not found.
    let cant_remove_builtin = matches!(
        unregister_shell(ShellId::RAEENSHELL),
        Err(ShellError::CannotRemoveBuiltin)
    );
    let unknown_notfound = get_shell(ShellId(0xDEAD_BEEF)).is_none();

    // Remove the test shell — back to the starting registry.
    let removed = unregister_shell(id).is_ok();
    let cleanup_ok = removed && list_shells().len() == before;

    let pass =
        builtins_ok && registered_ok && cant_remove_builtin && unknown_notfound && cleanup_ok;
    crate::serial_println!(
        "[shell-api] smoketest: builtins={} register={} cant_remove_builtin={} unknown_notfound={} cleanup={} -> {}",
        builtins_ok,
        registered_ok,
        cant_remove_builtin,
        unknown_notfound,
        cleanup_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}
