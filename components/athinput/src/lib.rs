//! RaeInput — unified gamepad + input normalization daemon.
//!
//! Concept alignment (LEGACY_GAMING_CONCEPT.md §Gaming):
//!   "DualSense + Xbox + every controller with full feature parity (haptics,
//!    adaptive triggers, gyro)."
//!
//! Architecture:
//!   Kernel (`usb_hid.rs`) → raw HID packets via IPC → this daemon → gilrs
//!   normalization → `RaeGamepadEvent` → kernel `SCHED_BODY` dispatch queue.
//!
//! The kernel never touches gamepad quirks. Raw USB HID reports are relayed
//! to userspace where gilrs handles DualSense adaptive triggers, Xbox GIP
//! protocol, force-feedback generation, and generic mapping tables.
//!
//! MasterChecklist Phase 12.2: Universal Controller Support.

/// Normalized gamepad event emitted by the athinput daemon to the kernel.
/// Sent via SYS_GAMEPAD_EVENT_PUSH syscall into the SCHED_BODY input queue.
#[derive(Debug, Clone, Copy)]
pub struct RaeGamepadEvent {
    /// Which controller slot (0-indexed, up to 8 players).
    pub slot: u8,
    pub kind: GamepadEventKind,
}

#[derive(Debug, Clone, Copy)]
pub enum GamepadEventKind {
    ButtonDown(GamepadButton),
    ButtonUp(GamepadButton),
    AxisChanged { axis: GamepadAxis, value: f32 },
    Connected { vendor_id: u16, product_id: u16 },
    Disconnected,
    /// Force-feedback request: motor_id, intensity 0.0–1.0, duration_ms
    RumbleRequest { motor: u8, intensity: f32, duration_ms: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamepadButton {
    // Face buttons
    South, East, West, North,
    // Shoulder / trigger
    LeftShoulder, RightShoulder, LeftTrigger, RightTrigger,
    // Sticks
    LeftThumb, RightThumb,
    // D-pad
    DPadUp, DPadDown, DPadLeft, DPadRight,
    // System
    Start, Select, Mode, // Mode = PS button / Xbox guide
    // DualSense extras
    Touchpad, Mute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamepadAxis {
    LeftStickX, LeftStickY,
    RightStickX, RightStickY,
    LeftTrigger, RightTrigger,
    DPadX, DPadY,
}

/// Gilrs-to-RaeGamepadEvent bridge (only compiled with the `gamepad` feature).
#[cfg(feature = "gamepad")]
pub mod daemon {
    //! Userspace daemon loop. Polls gilrs for events, converts to RaeGamepadEvent,
    //! and forwards to the kernel via the SCHED_BODY IPC channel.
    //!
    //! Stub implementation — full daemon wiring is Phase 12.2.
    //! When gilrs is available: call daemon::run() from the athinput process main().

    pub fn run() -> ! {
        // Phase 12.2: Initialize gilrs, poll for events in a loop,
        // convert gilrs::EventType → RaeGamepadEvent, push via syscall.
        // See docs/OSS_RECOMMENDATIONS.md §gilrs for the full architecture.
        loop {
            // Placeholder: real implementation uses gilrs::Gilrs::new() + event_loop
            core::hint::spin_loop();
        }
    }
}
