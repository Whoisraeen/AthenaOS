//! Application lifecycle for AthKit apps.
//!
//! Every AthKit application implements the `RaeApp` trait and hands itself to
//! `AppRunner::run()`, which creates a compositor surface, enters the event
//! loop, and re-renders whenever state changes.

extern crate alloc;
use alloc::collections::VecDeque;
use alloc::string::String;

use crate::sys;
use crate::view::ViewNode;

// ── AppEvent ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AppEvent {
    KeyPress { scancode: u8 },
    KeyRelease { scancode: u8 },
    CharInput { ch: char },
    MouseMove { x: i32, y: i32, dx: i32, dy: i32 },
    MouseDown { x: i32, y: i32, button: MouseButton },
    MouseUp { x: i32, y: i32, button: MouseButton },
    MouseClick { x: i32, y: i32, button: MouseButton },
    MouseScroll { dx: i32, dy: i32 },
    WindowResize { width: u32, height: u32 },
    WindowFocus,
    WindowBlur,
    Timer { id: u32 },
    StateChanged { generation: u64 },
    Action { id: u32 },
    Custom { kind: u32, payload: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

// ── App lifecycle state ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Launching,
    Active,
    Background,
    Suspended,
    Terminating,
}

// ── RaeApp trait ─────────────────────────────────────────────────────────

/// The core trait for AthKit applications. Implement this and pass it to
/// `AppRunner::run()` to launch your app.
pub trait RaeApp {
    fn name(&self) -> &str;

    /// Called once at launch. Return the initial view tree.
    fn on_launch(&mut self) -> ViewNode;

    /// Called when an event arrives. Return `Some(view)` to trigger a
    /// re-render, or `None` to keep the current view.
    fn on_event(&mut self, event: &AppEvent) -> Option<ViewNode>;

    /// Called when the app transitions to background.
    fn on_background(&mut self) {}

    /// Called when the app returns to foreground.
    fn on_foreground(&mut self) {}

    /// Called just before termination. Clean up resources here.
    fn on_terminate(&mut self) {}
}

// ── AppRunner ────────────────────────────────────────────────────────────

pub struct AppRunner {
    lifecycle: LifecycleState,
    surface_id: Option<u64>,
    width: u32,
    height: u32,
    _state_generation: u64,
    event_queue: VecDeque<AppEvent>,
    frame_count: u64,
    target_fps: u32,
    title: String,
}

impl AppRunner {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            lifecycle: LifecycleState::Launching,
            surface_id: None,
            width,
            height,
            _state_generation: 0,
            event_queue: VecDeque::new(),
            frame_count: 0,
            target_fps: 60,
            title: String::new(),
        }
    }

    pub fn with_title(mut self, title: &str) -> Self {
        self.title = String::from(title);
        self
    }

    pub fn with_fps(mut self, fps: u32) -> Self {
        self.target_fps = fps;
        self
    }

    pub fn push_event(&mut self, event: AppEvent) {
        self.event_queue.push_back(event);
    }

    pub fn lifecycle(&self) -> LifecycleState {
        self.lifecycle
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    pub fn surface_id(&self) -> Option<u64> {
        self.surface_id
    }

    /// Main entry point. Creates a surface, runs the event loop, and
    /// never returns.
    pub fn run(mut self, app: &mut dyn RaeApp) -> ! {
        self.title = String::from(app.name());

        let _initial_view = app.on_launch();
        self.lifecycle = LifecycleState::Active;

        let buf_size = (self.width as u64) * (self.height as u64) * 4;
        let buf_pages = (buf_size + 4095) & !4095;
        static SURFACE_VIRT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0x0000_7000_0000_0000);
        let user_virt = SURFACE_VIRT.fetch_add(buf_pages, core::sync::atomic::Ordering::SeqCst);
        let sid = sys::surface_create(self.width as u64, self.height as u64, user_virt);
        self.surface_id = Some(sid);

        loop {
            // Poll keyboard
            let key = sys::read_key();
            if key != 0 {
                if key == 0xFF {
                    break;
                }
                self.event_queue.push_back(AppEvent::KeyPress {
                    scancode: key as u8,
                });
            }

            // Poll mouse via IPC
            if let Some(mouse) = crate::ipc::poll_mouse() {
                if mouse.dx != 0 || mouse.dy != 0 {
                    self.event_queue.push_back(AppEvent::MouseMove {
                        x: 0,
                        y: 0,
                        dx: mouse.dx as i32,
                        dy: mouse.dy as i32,
                    });
                }
                if mouse.buttons & 1 != 0 {
                    self.event_queue.push_back(AppEvent::MouseClick {
                        x: 0,
                        y: 0,
                        button: MouseButton::Left,
                    });
                }
            }

            // Drain event queue
            while let Some(event) = self.event_queue.pop_front() {
                let _new_view = app.on_event(&event);
            }

            self.frame_count += 1;
            sys::yield_now();
        }

        app.on_terminate();
        self.lifecycle = LifecycleState::Terminating;

        if let Some(sid) = self.surface_id {
            sys::surface_close(sid);
        }

        sys::exit(0);
    }
}

// ── AppDescriptor ────────────────────────────────────────────────────────

/// Static metadata about the application, used by the OS for the app
/// launcher, task switcher, and store listing.
pub struct AppDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub icon_asset: u64,
    pub min_width: u32,
    pub min_height: u32,
}

impl AppDescriptor {
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            version: "0.1.0",
            icon_asset: 0,
            min_width: 320,
            min_height: 240,
        }
    }
}
